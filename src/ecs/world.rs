use std::{
    any::{type_name, Any, TypeId},
    cell::{Cell, RefCell, UnsafeCell},
    collections::{hash_map::Entry, HashMap},
};

use super::{
    access::FilteredAccess,
    bundle::Bundle,
    entity::{Entities, Entity},
    query::{Query, QueryData, QueryFilter, SystemTicks},
    storage::{Component, ComponentSet, ComponentTicks, ErasedStorage, Tick},
};

pub(crate) struct ResourceCell {
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

/// Holds all entities, components and resources.
///
/// Shared (`&self`) methods only hand out shared references, and mutation needs `&mut self`.
/// The one exception is system execution: a system borrows the whole world mutably, then its
/// parameters reach into it through `&World` under the access rules checked at initialization.
pub struct World {
    entities: RefCell<Entities>,
    storages: HashMap<TypeId, Box<dyn ErasedStorage>>,
    resources: HashMap<TypeId, ResourceCell>,
    change_tick: Cell<Tick>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: RefCell::default(),
            storages: HashMap::new(),
            resources: HashMap::new(),
            change_tick: Cell::new(1),
        }
    }

    // --- ticks ---

    pub fn change_tick(&self) -> Tick {
        self.change_tick.get()
    }

    /// Returns the current tick and advances the counter. Called once per system run.
    pub(crate) fn increment_change_tick(&self) -> Tick {
        let tick = self.change_tick.get();
        self.change_tick.set(tick + 1);
        tick
    }

    // --- entities ---

    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.entities.get_mut().alloc();
        bundle.insert_into(self, entity);
        entity
    }

    pub fn spawn_empty(&mut self) -> Entity {
        self.entities.get_mut().alloc()
    }

    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.get_mut().free(entity) {
            return false;
        }
        for storage in self.storages.values_mut() {
            storage.remove_entity(entity);
        }
        true
    }

    pub fn contains_entity(&self, entity: Entity) -> bool {
        self.entities.borrow().contains(entity)
    }

    pub fn entity_count(&self) -> usize {
        self.entities.borrow().len()
    }

    pub(crate) fn entities(&self) -> &RefCell<Entities> {
        &self.entities
    }

    pub(crate) fn entities_snapshot(&self) -> Vec<Entity> {
        self.entities.borrow().iter().collect()
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
            .get(&TypeId::of::<C>())
            .map(|s| s.as_any().downcast_ref().unwrap())
    }

    pub(crate) fn storage_mut<C: Component>(&mut self) -> Option<&mut ComponentSet<C>> {
        self.storages
            .get_mut(&TypeId::of::<C>())
            .map(|s| s.as_any_mut().downcast_mut().unwrap())
    }

    pub(crate) fn storage_or_insert<C: Component>(&mut self) -> &mut ComponentSet<C> {
        self.storages
            .entry(TypeId::of::<C>())
            .or_insert_with(|| Box::new(ComponentSet::<C>::default()))
            .as_any_mut()
            .downcast_mut()
            .unwrap()
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
