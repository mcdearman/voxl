use std::{
    alloc::{self, Layout},
    any::{Any, TypeId},
    cell::UnsafeCell,
    ptr::{self, NonNull},
};

use super::entity::Entity;

/// A monotonically increasing counter used for change detection. Every system run gets its own tick.
pub type Tick = u64;

/// Identifies a component type: either a Rust type, or one defined at runtime (by a native
/// plugin, say) that Rust has no type for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKey {
    Type(TypeId),
    Dynamic(u32),
}

impl ComponentKey {
    pub fn of<T: 'static>() -> Self {
        Self::Type(TypeId::of::<T>())
    }
}

/// Marker trait for types that can be attached to entities.
pub trait Component: Send + Sync + 'static {}

#[derive(Clone, Copy, Debug)]
pub struct ComponentTicks {
    pub added: Tick,
    pub changed: Tick,
}

impl ComponentTicks {
    pub fn new(tick: Tick) -> Self {
        Self {
            added: tick,
            changed: tick,
        }
    }

    pub fn is_added(&self, last_run: Tick) -> bool {
        self.added > last_run
    }

    pub fn is_changed(&self, last_run: Tick) -> bool {
        self.changed > last_run
    }
}

const EMPTY: u32 = u32::MAX;

/// Sparse-set storage for a single component type.
///
/// `sparse` maps an entity index to a slot in the dense arrays, which stay tightly packed for
/// fast iteration. Values and ticks live in `UnsafeCell`s so a query can hand out `&mut T` to
/// different entities while the set itself is only shared-borrowed. The scheduler's access
/// checks are what make that sound.
pub struct ComponentSet<T> {
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    data: Vec<UnsafeCell<T>>,
    ticks: Vec<UnsafeCell<ComponentTicks>>,
}

impl<T> Default for ComponentSet<T> {
    fn default() -> Self {
        Self {
            sparse: Vec::new(),
            entities: Vec::new(),
            data: Vec::new(),
            ticks: Vec::new(),
        }
    }
}

impl<T> ComponentSet<T> {
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn dense_index(&self, entity: Entity) -> Option<usize> {
        let dense = *self.sparse.get(entity.index() as usize)?;
        (dense != EMPTY && self.entities[dense as usize] == entity).then_some(dense as usize)
    }

    /// The slot of `entity`, looking first at `guess`. A query walking one component's
    /// entities in order asks for each of them here, and in every other component it reads,
    /// with the place it has reached: right for the component it walks, and for the others
    /// when they were filled in the same order, as components spawned together are. A right
    /// guess is one load and a compare, where finding the slot from the entity is two loads
    /// in two arrays; a wrong one costs the compare and falls back.
    #[inline]
    pub fn dense_index_near(&self, entity: Entity, guess: usize) -> Option<usize> {
        if self.entities.get(guess) == Some(&entity) {
            return Some(guess);
        }
        self.dense_index(entity)
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.dense_index(entity).is_some()
    }

    pub fn get(&self, entity: Entity) -> Option<&T> {
        let dense = self.dense_index(entity)?;
        // SAFETY: `&self` access outside the scheduler never coexists with a `&mut T`,
        // since mutable access through `&World` only happens inside checked systems.
        Some(unsafe { &*self.data[dense].get() })
    }

    pub fn get_mut(&mut self, entity: Entity, tick: Tick) -> Option<&mut T> {
        let dense = self.dense_index(entity)?;
        self.ticks[dense].get_mut().changed = tick;
        Some(self.data[dense].get_mut())
    }

    pub fn ticks(&self, entity: Entity) -> Option<ComponentTicks> {
        let dense = self.dense_index(entity)?;
        // SAFETY: plain copy of the ticks; see `get`.
        Some(unsafe { *self.ticks[dense].get() })
    }

    /// Inserts or replaces the value. Replacing counts as a change, not an addition.
    pub fn insert(&mut self, entity: Entity, value: T, tick: Tick) -> Option<T> {
        if let Some(dense) = self.dense_index(entity) {
            self.ticks[dense].get_mut().changed = tick;
            return Some(std::mem::replace(self.data[dense].get_mut(), value));
        }
        let index = entity.index() as usize;
        if index >= self.sparse.len() {
            self.sparse.resize(index + 1, EMPTY);
        }
        self.sparse[index] = self.entities.len() as u32;
        self.entities.push(entity);
        self.data.push(UnsafeCell::new(value));
        self.ticks.push(UnsafeCell::new(ComponentTicks::new(tick)));
        None
    }

    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let dense = self.dense_index(entity)?;
        self.sparse[entity.index() as usize] = EMPTY;
        let last = self.entities.len() - 1;
        if dense != last {
            let moved = self.entities[last];
            self.sparse[moved.index() as usize] = dense as u32;
        }
        self.entities.swap_remove(dense);
        self.ticks.swap_remove(dense);
        Some(self.data.swap_remove(dense).into_inner())
    }

    /// # Safety
    /// The caller must guarantee no other reference to this slot's value is alive.
    pub(crate) unsafe fn value_mut<'a>(&self, dense: usize) -> &'a mut T {
        &mut *self.data[dense].get()
    }

    /// # Safety
    /// The caller must guarantee no mutable reference to this slot's value is alive.
    pub(crate) unsafe fn value<'a>(&self, dense: usize) -> &'a T {
        &*self.data[dense].get()
    }

    /// # Safety
    /// The caller must guarantee no other reference to this slot's ticks is alive.
    pub(crate) unsafe fn ticks_mut<'a>(&self, dense: usize) -> &'a mut ComponentTicks {
        &mut *self.ticks[dense].get()
    }

    /// # Safety
    /// The caller must guarantee no mutable reference to this slot's ticks is alive.
    pub(crate) unsafe fn ticks_at(&self, dense: usize) -> ComponentTicks {
        *self.ticks[dense].get()
    }
}

/// Type-erased view of a component's storage, so the world can keep every kind in one map
/// and code that only knows a component's size (native plugins) can still reach its values.
pub(crate) trait ErasedStorage {
    /// The Rust type stored, or nothing for a component defined at runtime.
    fn type_name(&self) -> &'static str {
        ""
    }
    fn remove_entity(&mut self, entity: Entity);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn entities(&self) -> &[Entity];
    fn contains(&self, entity: Entity) -> bool;

    /// # Safety
    /// The caller must hold access to this component, as for a typed query.
    unsafe fn value_ptr(&self, entity: Entity) -> Option<*mut u8>;

    /// # Safety
    /// The caller must hold write access to this component.
    unsafe fn mark_changed(&self, entity: Entity, tick: Tick);

    /// Moves a value in from raw bytes, replacing any existing one. `src` may be unaligned.
    ///
    /// # Safety
    /// `src` must point to a valid value of the stored type, which the caller gives up.
    unsafe fn insert_raw(&mut self, entity: Entity, src: *const u8, tick: Tick);
}

impl<T: Component> ErasedStorage for ComponentSet<T> {
    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn remove_entity(&mut self, entity: Entity) {
        self.remove(entity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn entities(&self) -> &[Entity] {
        &self.entities
    }

    fn contains(&self, entity: Entity) -> bool {
        self.dense_index(entity).is_some()
    }

    unsafe fn value_ptr(&self, entity: Entity) -> Option<*mut u8> {
        let dense = self.dense_index(entity)?;
        Some(self.data[dense].get().cast())
    }

    unsafe fn mark_changed(&self, entity: Entity, tick: Tick) {
        if let Some(dense) = self.dense_index(entity) {
            (*self.ticks[dense].get()).changed = tick;
        }
    }

    unsafe fn insert_raw(&mut self, entity: Entity, src: *const u8, tick: Tick) {
        self.insert(entity, ptr::read_unaligned(src.cast::<T>()), tick);
    }
}

/// Called to destroy a value in place before its bytes are discarded.
pub type DropFn = unsafe extern "C" fn(*mut u8);

/// Sparse-set storage for a component known only by its size and alignment: the same shape as
/// `ComponentSet`, with the values kept as raw bytes.
pub(crate) struct BlobSet {
    layout: Layout,
    /// Distance between values: the size rounded up to the alignment.
    stride: usize,
    drop: Option<DropFn>,
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    data: NonNull<u8>,
    capacity: usize,
    ticks: Vec<UnsafeCell<ComponentTicks>>,
}

impl BlobSet {
    pub(crate) fn new(layout: Layout, drop: Option<DropFn>) -> Self {
        let stride = layout.pad_to_align().size();
        Self {
            layout,
            stride,
            drop,
            sparse: Vec::new(),
            entities: Vec::new(),
            // Well aligned and never read until `grow` replaces it (or ever, for empty types).
            data: NonNull::new(ptr::without_provenance_mut(layout.align())).unwrap(),
            capacity: if stride == 0 { usize::MAX } else { 0 },
            ticks: Vec::new(),
        }
    }

    /// Replaces the destructor. `None` leaks values instead of destroying them, which is the
    /// safe choice while the code that owned the destructor is unloaded.
    pub(crate) fn set_drop(&mut self, drop: Option<DropFn>) {
        self.drop = drop;
    }

    fn dense_index(&self, entity: Entity) -> Option<usize> {
        let dense = *self.sparse.get(entity.index() as usize)?;
        (dense != EMPTY && self.entities[dense as usize] == entity).then_some(dense as usize)
    }

    fn slot(&self, dense: usize) -> *mut u8 {
        // SAFETY: callers pass `dense <= capacity`, so this stays inside (or one past) the
        // allocation; for empty types the offset is zero.
        unsafe { self.data.as_ptr().add(dense * self.stride) }
    }

    fn array_layout(&self, capacity: usize) -> Layout {
        Layout::from_size_align(self.stride * capacity, self.layout.align())
            .expect("component storage too large")
    }

    fn grow(&mut self) {
        let capacity = (self.capacity * 2).max(4);
        let new_layout = self.array_layout(capacity);
        // SAFETY: the layouts have non-zero size (stride is non-zero here), and the old
        // pointer came from this allocator with the old layout.
        let data = unsafe {
            if self.capacity == 0 {
                alloc::alloc(new_layout)
            } else {
                alloc::realloc(
                    self.data.as_ptr(),
                    self.array_layout(self.capacity),
                    new_layout.size(),
                )
            }
        };
        self.data = NonNull::new(data).unwrap_or_else(|| alloc::handle_alloc_error(new_layout));
        self.capacity = capacity;
    }

    fn drop_slot(&mut self, dense: usize) {
        if let Some(drop) = self.drop {
            // SAFETY: the slot holds a live value that is about to be overwritten or forgotten.
            unsafe { drop(self.slot(dense)) }
        }
    }
}

impl Drop for BlobSet {
    fn drop(&mut self) {
        for dense in 0..self.entities.len() {
            self.drop_slot(dense);
        }
        if self.stride != 0 && self.capacity != 0 {
            // SAFETY: allocated in `grow` with exactly this layout.
            unsafe { alloc::dealloc(self.data.as_ptr(), self.array_layout(self.capacity)) }
        }
    }
}

impl ErasedStorage for BlobSet {
    fn remove_entity(&mut self, entity: Entity) {
        let Some(dense) = self.dense_index(entity) else {
            return;
        };
        self.drop_slot(dense);
        self.sparse[entity.index() as usize] = EMPTY;
        let last = self.entities.len() - 1;
        if dense != last {
            let moved = self.entities[last];
            self.sparse[moved.index() as usize] = dense as u32;
            // SAFETY: distinct live slots of `stride` bytes each.
            unsafe { ptr::copy_nonoverlapping(self.slot(last), self.slot(dense), self.stride) }
        }
        self.entities.swap_remove(dense);
        self.ticks.swap_remove(dense);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn entities(&self) -> &[Entity] {
        &self.entities
    }

    fn contains(&self, entity: Entity) -> bool {
        self.dense_index(entity).is_some()
    }

    unsafe fn value_ptr(&self, entity: Entity) -> Option<*mut u8> {
        Some(self.slot(self.dense_index(entity)?))
    }

    unsafe fn mark_changed(&self, entity: Entity, tick: Tick) {
        if let Some(dense) = self.dense_index(entity) {
            (*self.ticks[dense].get()).changed = tick;
        }
    }

    unsafe fn insert_raw(&mut self, entity: Entity, src: *const u8, tick: Tick) {
        if let Some(dense) = self.dense_index(entity) {
            self.drop_slot(dense);
            ptr::copy_nonoverlapping(src, self.slot(dense), self.layout.size());
            self.ticks[dense].get_mut().changed = tick;
            return;
        }
        let dense = self.entities.len();
        if dense == self.capacity {
            self.grow();
        }
        ptr::copy_nonoverlapping(src, self.slot(dense), self.layout.size());
        let index = entity.index() as usize;
        if index >= self.sparse.len() {
            self.sparse.resize(index + 1, EMPTY);
        }
        self.sparse[index] = dense as u32;
        self.entities.push(entity);
        self.ticks.push(UnsafeCell::new(ComponentTicks::new(tick)));
    }
}
