use std::{any::Any, cell::UnsafeCell};

use super::entity::Entity;

/// A monotonically increasing counter used for change detection. Every system run gets its own tick.
pub type Tick = u64;

/// Marker trait for types that can be attached to entities.
pub trait Component: 'static {}

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

/// Type-erased view of a `ComponentSet<T>` so the world can store all of them in one map.
pub(crate) trait ErasedStorage {
    fn remove_entity(&mut self, entity: Entity);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> ErasedStorage for ComponentSet<T> {
    fn remove_entity(&mut self, entity: Entity) {
        self.remove(entity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
