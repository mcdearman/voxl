use std::{
    alloc::Layout,
    any::{type_name, Any, TypeId},
    cell::UnsafeCell,
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
};

use super::{
    access::FilteredAccess,
    bundle::Bundle,
    entity::{Entities, Entity},
    query::{Query, QueryData, QueryFilter, SystemTicks},
    storage::{
        BlobSet, Component, ComponentKey, ComponentSet, ComponentTicks, DropFn, ErasedStorage, Tick,
    },
};

pub(crate) struct ResourceCell {
    type_name: &'static str,
    value: UnsafeCell<Box<dyn Any>>,
    ticks: UnsafeCell<ComponentTicks>,
}

impl ResourceCell {
    /// # Safety
    /// No mutable reference to this resource may be alive.
    pub(crate) unsafe fn get<'a, T: 'static>(&self) -> (&'a T, ComponentTicks) {
        let value = (*self.value.get()).downcast_ref::<T>().unwrap();
        (value, *self.ticks.get())
    }

    /// # Safety
    /// No other reference to this resource may be alive.
    pub(crate) unsafe fn get_mut<'a, T: 'static>(&self) -> (&'a mut T, &'a mut ComponentTicks) {
        let value = (*self.value.get()).downcast_mut::<T>().unwrap();
        (value, &mut *self.ticks.get())
    }
}

/// A component that code without access to its Rust type (a native plugin) can find by name.
#[derive(Clone, Debug)]
pub struct NamedComponent {
    pub name: String,
    pub key: ComponentKey,
    pub layout: Layout,
}

/// Holds all entities, components and resources.
///
/// Shared (`&self`) methods only hand out shared references, and mutation needs `&mut self`.
/// The one exception is system execution: a system borrows the whole world mutably, then its
/// parameters reach into it through `&World` under the access rules checked at initialization.
pub struct World {
    /// Behind a lock, not for `&mut` users (who go straight in) but because systems running
    /// at the same moment may each be making entities through `Commands`.
    entities: Mutex<Entities>,
    storages: HashMap<ComponentKey, Box<dyn ErasedStorage>>,
    /// Components that can be found by name, in the order they were named.
    named: Vec<NamedComponent>,
    names: HashMap<String, u32>,
    next_dynamic: u32,
    resources: HashMap<TypeId, ResourceCell>,
    /// Resources that must only be used from the main thread, though their types could be
    /// shared: a window, say.
    main_thread: HashSet<TypeId>,
    change_tick: AtomicU64,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: Mutex::default(),
            storages: HashMap::new(),
            named: Vec::new(),
            names: HashMap::new(),
            next_dynamic: 0,
            resources: HashMap::new(),
            main_thread: HashSet::new(),
            change_tick: AtomicU64::new(1),
        }
    }

    // --- ticks ---

    pub fn change_tick(&self) -> Tick {
        self.change_tick.load(Ordering::Relaxed)
    }

    /// Returns the current tick and advances the counter. Called once per system run.
    pub(crate) fn increment_change_tick(&self) -> Tick {
        self.change_tick.fetch_add(1, Ordering::Relaxed)
    }

    // --- entities ---

    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.entities_mut().alloc();
        bundle.insert_into(self, entity);
        entity
    }

    pub fn spawn_empty(&mut self) -> Entity {
        self.entities_mut().alloc()
    }

    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities_mut().free(entity) {
            return false;
        }
        for storage in self.storages.values_mut() {
            storage.remove_entity(entity);
        }
        true
    }

    pub fn contains_entity(&self, entity: Entity) -> bool {
        self.entities().contains(entity)
    }

    pub fn entity_count(&self) -> usize {
        self.entities().len()
    }

    /// The entity allocator, locked: for making entities with only shared access.
    pub(crate) fn entities(&self) -> MutexGuard<'_, Entities> {
        self.entities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn entities_mut(&mut self) -> &mut Entities {
        self.entities
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn entities_snapshot(&self) -> Vec<Entity> {
        self.entities().iter().collect()
    }

    // --- components ---

    pub fn register_component<C: Component>(&mut self) {
        self.storage_or_insert::<C>();
    }

    /// Inserts `bundle` into `entity`. Returns false if the entity is dead.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) -> bool {
        if !self.contains_entity(entity) {
            log::warn!(
                "tried to insert `{}` into dead entity {entity:?}",
                type_name::<B>()
            );
            return false;
        }
        bundle.insert_into(self, entity);
        true
    }

    pub fn remove<C: Component>(&mut self, entity: Entity) -> Option<C> {
        self.storage_mut::<C>()?.remove(entity)
    }

    pub fn get<C: Component>(&self, entity: Entity) -> Option<&C> {
        self.storage::<C>()?.get(entity)
    }

    pub fn get_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C> {
        let tick = self.change_tick();
        self.storage_mut::<C>()?.get_mut(entity, tick)
    }

    pub fn has<C: Component>(&self, entity: Entity) -> bool {
        self.storage::<C>().is_some_and(|s| s.contains(entity))
    }

    pub(crate) fn storage<C: Component>(&self) -> Option<&ComponentSet<C>> {
        self.storages
            .get(&ComponentKey::of::<C>())
            .map(|s| s.as_any().downcast_ref().unwrap())
    }

    pub(crate) fn storage_mut<C: Component>(&mut self) -> Option<&mut ComponentSet<C>> {
        self.storages
            .get_mut(&ComponentKey::of::<C>())
            .map(|s| s.as_any_mut().downcast_mut().unwrap())
    }

    pub(crate) fn storage_or_insert<C: Component>(&mut self) -> &mut ComponentSet<C> {
        self.storages
            .entry(ComponentKey::of::<C>())
            .or_insert_with(|| Box::new(ComponentSet::<C>::default()))
            .as_any_mut()
            .downcast_mut()
            .unwrap()
    }

    // --- components by name ---

    /// Makes a Rust component reachable by name, with its bytes as its interface. Only do this
    /// for `#[repr(C)]` types whose layout is part of a published contract.
    pub fn export_component<C: Component + Copy>(&mut self, name: &str) -> u32 {
        self.storage_or_insert::<C>();
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        self.name_component(name, ComponentKey::of::<C>(), Layout::new::<C>())
    }

    /// Defines a component by size and alignment alone, or finds the one already defined under
    /// `name`. A component redefined with the same layout keeps its values (this is what lets
    /// data outlive a plugin reload); with a different layout the old values are destroyed.
    pub fn register_blob_component(
        &mut self,
        name: &str,
        layout: Layout,
        drop: Option<DropFn>,
    ) -> Result<u32, String> {
        let Some(&id) = self.names.get(name) else {
            let key = ComponentKey::Dynamic(self.next_dynamic);
            self.next_dynamic += 1;
            self.storages
                .insert(key, Box::new(BlobSet::new(layout, drop)));
            return Ok(self.name_component(name, key, layout));
        };
        let entry = &mut self.named[id as usize];
        if matches!(entry.key, ComponentKey::Type(_)) {
            return Err(format!("`{name}` is a built-in component"));
        }
        if entry.layout == layout {
            self.blob_mut(id).unwrap().set_drop(drop);
        } else {
            log::warn!(
                "component `{name}` changed layout ({:?} to {layout:?}); its values were discarded",
                entry.layout
            );
            entry.layout = layout;
            let key = entry.key;
            self.storages
                .insert(key, Box::new(BlobSet::new(layout, drop)));
        }
        Ok(id)
    }

    fn name_component(&mut self, name: &str, key: ComponentKey, layout: Layout) -> u32 {
        let id = self.named.len() as u32;
        self.named.push(NamedComponent {
            name: name.to_owned(),
            key,
            layout,
        });
        self.names.insert(name.to_owned(), id);
        id
    }

    pub fn named_component_id(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }

    pub fn named_component(&self, id: u32) -> Option<&NamedComponent> {
        self.named.get(id as usize)
    }

    fn blob_mut(&mut self, id: u32) -> Option<&mut BlobSet> {
        let key = self.named.get(id as usize)?.key;
        self.storages.get_mut(&key)?.as_any_mut().downcast_mut()
    }

    /// Stops destroying a runtime-defined component's values (they leak instead). Used while
    /// the code holding its destructor is unloaded.
    pub(crate) fn forget_blob_drop(&mut self, id: u32) {
        if let Some(blob) = self.blob_mut(id) {
            blob.set_drop(None);
        }
    }

    pub(crate) fn erased_storage(&self, key: ComponentKey) -> Option<&dyn ErasedStorage> {
        self.storages.get(&key).map(|s| &**s)
    }

    /// Inserts a component from raw bytes. Returns false if the entity is dead or the component
    /// unknown.
    ///
    /// # Safety
    /// `src` must point to a valid value of the component, which the caller gives up.
    pub(crate) unsafe fn insert_raw(
        &mut self,
        entity: Entity,
        key: ComponentKey,
        src: *const u8,
    ) -> bool {
        let tick = self.change_tick();
        if !self.contains_entity(entity) {
            return false;
        }
        let Some(storage) = self.storages.get_mut(&key) else {
            return false;
        };
        storage.insert_raw(entity, src, tick);
        true
    }

    pub(crate) fn remove_by_key(&mut self, entity: Entity, key: ComponentKey) {
        if let Some(storage) = self.storages.get_mut(&key) {
            storage.remove_entity(entity);
        }
    }

    /// Runs a one-off query outside of a system. Change filters treat everything as new.
    pub fn query<D: QueryData>(&mut self) -> Query<'_, D> {
        self.query_filtered::<D, ()>()
    }

    pub fn query_filtered<D: QueryData, F: QueryFilter>(&mut self) -> Query<'_, D, F> {
        D::init(self);
        F::init(self);
        let mut access = FilteredAccess::default();
        D::access(&mut access);
        F::access(&mut access);
        let ticks = SystemTicks {
            last_run: 0,
            this_run: self.increment_change_tick(),
        };
        let mut checked = super::access::Access::new("World::query");
        checked.add_query(access, type_name::<D>());
        // SAFETY: `&mut self` gives exclusive access, and the query's own access was validated.
        unsafe { Query::new(self, ticks) }
    }

    // --- resources ---

    /// Every kind of component the world holds: its key, its name (the Rust type, or the
    /// name a runtime-defined component was given) and how many entities have it.
    pub fn component_kinds(&self) -> Vec<(ComponentKey, String, usize)> {
        let mut kinds: Vec<_> = self
            .storages
            .iter()
            .map(|(key, storage)| {
                let name = match storage.type_name() {
                    "" => self
                        .named
                        .iter()
                        .find(|named| named.key == *key)
                        .map_or("(unnamed)".to_owned(), |named| named.name.clone()),
                    name => name.to_owned(),
                };
                (*key, name, storage.entities().len())
            })
            .collect();
        kinds.sort_by(|a, b| a.1.cmp(&b.1));
        kinds
    }

    /// Every resource the world holds, by type.
    pub fn resource_kinds(&self) -> Vec<(TypeId, &'static str)> {
        let mut kinds: Vec<_> = self
            .resources
            .iter()
            .map(|(id, cell)| (*id, cell.type_name))
            .collect();
        kinds.sort_by_key(|kind| kind.1);
        kinds
    }

    /// Says that systems using this resource must run on the main thread: for things the
    /// operating system ties to the thread that made them. Other systems are unaffected.
    pub fn pin_to_main_thread<R: 'static>(&mut self) {
        self.main_thread.insert(TypeId::of::<R>());
    }

    /// Whether a resource type was pinned with [`World::pin_to_main_thread`].
    pub fn is_pinned_to_main_thread(&self, resource: TypeId) -> bool {
        self.main_thread.contains(&resource)
    }

    pub fn insert_resource<R: 'static>(&mut self, value: R) {
        let tick = self.change_tick();
        match self.resources.entry(TypeId::of::<R>()) {
            Entry::Occupied(mut entry) => {
                let cell = entry.get_mut();
                *cell.value.get_mut() = Box::new(value);
                cell.ticks.get_mut().changed = tick;
            }
            Entry::Vacant(entry) => {
                entry.insert(ResourceCell {
                    type_name: type_name::<R>(),
                    value: UnsafeCell::new(Box::new(value)),
                    ticks: UnsafeCell::new(ComponentTicks::new(tick)),
                });
            }
        }
    }

    pub fn init_resource<R: Default + 'static>(&mut self) {
        if !self.contains_resource::<R>() {
            self.insert_resource(R::default());
        }
    }

    pub fn remove_resource<R: 'static>(&mut self) -> Option<R> {
        let cell = self.resources.remove(&TypeId::of::<R>())?;
        cell.value.into_inner().downcast().ok().map(|b| *b)
    }

    pub fn contains_resource<R: 'static>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    pub fn get_resource<R: 'static>(&self) -> Option<&R> {
        let cell = self.resource_cell::<R>()?;
        // SAFETY: `&self` can't coexist with the `&mut self` needed for mutable access.
        Some(unsafe { cell.get::<R>().0 })
    }

    pub fn get_resource_mut<R: 'static>(&mut self) -> Option<&mut R> {
        let tick = self.change_tick();
        let cell = self.resources.get_mut(&TypeId::of::<R>())?;
        cell.ticks.get_mut().changed = tick;
        cell.value.get_mut().downcast_mut()
    }

    pub fn resource<R: 'static>(&self) -> &R {
        self.get_resource()
            .unwrap_or_else(|| panic!("resource `{}` does not exist", type_name::<R>()))
    }

    pub fn resource_mut<R: 'static>(&mut self) -> &mut R {
        self.get_resource_mut()
            .unwrap_or_else(|| panic!("resource `{}` does not exist", type_name::<R>()))
    }

    /// Temporarily removes a resource so it can be used alongside `&mut World`.
    pub fn resource_scope<R: 'static, T>(&mut self, f: impl FnOnce(&mut World, &mut R) -> T) -> T {
        let mut resource = self
            .remove_resource::<R>()
            .unwrap_or_else(|| panic!("resource `{}` does not exist", type_name::<R>()));
        let out = f(self, &mut resource);
        self.insert_resource(resource);
        out
    }

    pub(crate) fn resource_cell<R: 'static>(&self) -> Option<&ResourceCell> {
        self.resources.get(&TypeId::of::<R>())
    }
}
