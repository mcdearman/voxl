use std::{any::type_name, marker::PhantomData};

use super::{
    access::{Access, FilteredAccess},
    change::Mut,
    entity::Entity,
    storage::{Component, ComponentSet, Tick},
    system::{SystemMeta, SystemParam},
    world::World,
};

#[derive(Clone, Copy, Debug)]
pub struct SystemTicks {
    /// The tick of the system's previous run (0 if it never ran).
    pub last_run: Tick,
    /// The tick of the current run. Writes are stamped with this.
    pub this_run: Tick,
}

/// Something a query can fetch per entity: `&T`, `&mut T`, `Entity`, `Option<D>`, or tuples of these.
///
/// # Safety
/// `access` must report every component that `get` touches.
pub unsafe trait QueryData {
    type Fetch<'w>;
    type Item<'q>;

    fn init(world: &mut World);
    fn access(access: &mut FilteredAccess);

    /// # Safety
    /// The caller must hold the access reported by `access`.
    unsafe fn fetch(world: &World) -> Self::Fetch<'_>;

    /// The entities this fetch requires, used to pick the smallest set to iterate.
    ///
    /// # Safety
    /// Same as `fetch`.
    unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]>;

    /// # Safety
    /// For mutable data, the caller must not create two live items for the same entity.
    unsafe fn get<'q>(
        fetch: &Self::Fetch<'_>,
        entity: Entity,
        ticks: SystemTicks,
    ) -> Option<Self::Item<'q>>;
}

/// Query data that only reads, so items may be handed out from `&Query`.
///
/// # Safety
/// Implementors must never produce mutable references.
pub unsafe trait ReadOnlyQueryData: QueryData {}

/// Narrows a query without fetching data: `With<T>`, `Without<T>`, `Added<T>`, `Changed<T>`, or tuples.
///
/// # Safety
/// `access` must report every component that `matches` reads.
pub unsafe trait QueryFilter {
    type Fetch<'w>;

    fn init(world: &mut World);
    fn access(access: &mut FilteredAccess);

    /// # Safety
    /// The caller must hold the access reported by `access`.
    unsafe fn fetch(world: &World) -> Self::Fetch<'_>;

    /// # Safety
    /// Same as `fetch`.
    unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]>;

    /// # Safety
    /// Same as `fetch`.
    unsafe fn matches(fetch: &Self::Fetch<'_>, entity: Entity, ticks: SystemTicks) -> bool;
}

fn storage<C: Component>(world: &World) -> &ComponentSet<C> {
    world
        .storage::<C>()
        .expect("component storage is registered during query init")
}

fn shortest<'a>(a: Option<&'a [Entity]>, b: Option<&'a [Entity]>) -> Option<&'a [Entity]> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.len() < a.len() { b } else { a }),
        (a, None) => a,
        (None, b) => b,
    }
}

// --- data ---

unsafe impl<T: Component> QueryData for &T {
    type Fetch<'w> = &'w ComponentSet<T>;
    type Item<'q> = &'q T;

    fn init(world: &mut World) {
        world.register_component::<T>();
    }

    fn access(access: &mut FilteredAccess) {
        access.read::<T>();
        access.with::<T>();
    }

    unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
        storage(world)
    }

    unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        Some(fetch.entities())
    }

    unsafe fn get<'q>(fetch: &Self::Fetch<'_>, entity: Entity, _: SystemTicks) -> Option<&'q T> {
        let dense = fetch.dense_index(entity)?;
        Some(fetch.value(dense))
    }
}

unsafe impl<T: Component> ReadOnlyQueryData for &T {}

unsafe impl<T: Component> QueryData for &mut T {
    type Fetch<'w> = &'w ComponentSet<T>;
    type Item<'q> = Mut<'q, T>;

    fn init(world: &mut World) {
        world.register_component::<T>();
    }

    fn access(access: &mut FilteredAccess) {
        access.write::<T>();
        access.with::<T>();
    }

    unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
        storage(world)
    }

    unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        Some(fetch.entities())
    }

    unsafe fn get<'q>(
        fetch: &Self::Fetch<'_>,
        entity: Entity,
        ticks: SystemTicks,
    ) -> Option<Mut<'q, T>> {
        let dense = fetch.dense_index(entity)?;
        Some(Mut {
            value: fetch.value_mut(dense),
            ticks: fetch.ticks_mut(dense),
            last_run: ticks.last_run,
            this_run: ticks.this_run,
        })
    }
}

unsafe impl QueryData for Entity {
    type Fetch<'w> = ();
    type Item<'q> = Entity;

    fn init(_: &mut World) {}

    fn access(_: &mut FilteredAccess) {}

    unsafe fn fetch(_: &World) {}

    unsafe fn driver<'f>(_: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        None
    }

    unsafe fn get<'q>(
        _: &Self::Fetch<'_>,
        entity: Entity,
        _: SystemTicks,
    ) -> Option<Self::Item<'q>> {
        Some(entity)
    }
}

unsafe impl ReadOnlyQueryData for Entity {}

unsafe impl<D: QueryData> QueryData for Option<D> {
    type Fetch<'w> = D::Fetch<'w>;
    type Item<'q> = Option<D::Item<'q>>;

    fn init(world: &mut World) {
        D::init(world);
    }

    fn access(access: &mut FilteredAccess) {
        let mut inner = FilteredAccess::default();
        D::access(&mut inner);
        access.extend_data(inner);
    }

    unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
        D::fetch(world)
    }

    unsafe fn driver<'f>(_: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        None
    }

    unsafe fn get<'q>(
        fetch: &Self::Fetch<'_>,
        entity: Entity,
        ticks: SystemTicks,
    ) -> Option<Self::Item<'q>> {
        Some(D::get(fetch, entity, ticks))
    }
}

unsafe impl<D: ReadOnlyQueryData> ReadOnlyQueryData for Option<D> {}

macro_rules! impl_query_data_tuple {
    ($($D:ident),*) => {
        #[allow(non_snake_case, unused_variables, unused_mut, clippy::unused_unit)]
        unsafe impl<$($D: QueryData),*> QueryData for ($($D,)*) {
            type Fetch<'w> = ($($D::Fetch<'w>,)*);
            type Item<'q> = ($($D::Item<'q>,)*);

            fn init(world: &mut World) {
                $($D::init(world);)*
            }

            fn access(access: &mut FilteredAccess) {
                $($D::access(access);)*
            }

            unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
                ($($D::fetch(world),)*)
            }

            unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
                let ($($D,)*) = fetch;
                let mut best = None;
                $(best = shortest(best, $D::driver($D));)*
                best
            }

            unsafe fn get<'q>(
                fetch: &Self::Fetch<'_>,
                entity: Entity,
                ticks: SystemTicks,
            ) -> Option<Self::Item<'q>> {
                let ($($D,)*) = fetch;
                Some(($($D::get($D, entity, ticks)?,)*))
            }
        }

        unsafe impl<$($D: ReadOnlyQueryData),*> ReadOnlyQueryData for ($($D,)*) {}
    };
}

impl_query_data_tuple!();
impl_query_data_tuple!(D0);
impl_query_data_tuple!(D0, D1);
impl_query_data_tuple!(D0, D1, D2);
impl_query_data_tuple!(D0, D1, D2, D3);
impl_query_data_tuple!(D0, D1, D2, D3, D4);
impl_query_data_tuple!(D0, D1, D2, D3, D4, D5);
impl_query_data_tuple!(D0, D1, D2, D3, D4, D5, D6);
impl_query_data_tuple!(D0, D1, D2, D3, D4, D5, D6, D7);

// --- filters ---

/// Only entities that have `T`.
pub struct With<T>(PhantomData<T>);
/// Only entities that don't have `T`.
pub struct Without<T>(PhantomData<T>);
/// Only entities whose `T` was added since this system last ran.
pub struct Added<T>(PhantomData<T>);
/// Only entities whose `T` was added or mutated since this system last ran.
pub struct Changed<T>(PhantomData<T>);

unsafe impl<T: Component> QueryFilter for With<T> {
    type Fetch<'w> = &'w ComponentSet<T>;

    fn init(world: &mut World) {
        world.register_component::<T>();
    }

    fn access(access: &mut FilteredAccess) {
        access.with::<T>();
    }

    unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
        storage(world)
    }

    unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        Some(fetch.entities())
    }

    unsafe fn matches(fetch: &Self::Fetch<'_>, entity: Entity, _: SystemTicks) -> bool {
        fetch.contains(entity)
    }
}

unsafe impl<T: Component> QueryFilter for Without<T> {
    type Fetch<'w> = &'w ComponentSet<T>;

    fn init(world: &mut World) {
        world.register_component::<T>();
    }

    fn access(access: &mut FilteredAccess) {
        access.without::<T>();
    }

    unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
        storage(world)
    }

    unsafe fn driver<'f>(_: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
        None
    }

    unsafe fn matches(fetch: &Self::Fetch<'_>, entity: Entity, _: SystemTicks) -> bool {
        !fetch.contains(entity)
    }
}

macro_rules! impl_tick_filter {
    ($name:ident, $check:ident) => {
        unsafe impl<T: Component> QueryFilter for $name<T> {
            type Fetch<'w> = &'w ComponentSet<T>;

            fn init(world: &mut World) {
                world.register_component::<T>();
            }

            fn access(access: &mut FilteredAccess) {
                access.filter_read::<T>();
                access.with::<T>();
            }

            unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
                storage(world)
            }

            unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
                Some(fetch.entities())
            }

            unsafe fn matches(fetch: &Self::Fetch<'_>, entity: Entity, ticks: SystemTicks) -> bool {
                fetch
                    .dense_index(entity)
                    .is_some_and(|dense| fetch.ticks_at(dense).$check(ticks.last_run))
            }
        }
    };
}

impl_tick_filter!(Added, is_added);
impl_tick_filter!(Changed, is_changed);

macro_rules! impl_query_filter_tuple {
    ($($F:ident),*) => {
        #[allow(non_snake_case, unused_variables, unused_mut, clippy::unused_unit)]
        unsafe impl<$($F: QueryFilter),*> QueryFilter for ($($F,)*) {
            type Fetch<'w> = ($($F::Fetch<'w>,)*);

            fn init(world: &mut World) {
                $($F::init(world);)*
            }

            fn access(access: &mut FilteredAccess) {
                $($F::access(access);)*
            }

            unsafe fn fetch(world: &World) -> Self::Fetch<'_> {
                ($($F::fetch(world),)*)
            }

            unsafe fn driver<'f>(fetch: &'f Self::Fetch<'_>) -> Option<&'f [Entity]> {
                let ($($F,)*) = fetch;
                let mut best = None;
                $(best = shortest(best, $F::driver($F));)*
                best
            }

            unsafe fn matches(fetch: &Self::Fetch<'_>, entity: Entity, ticks: SystemTicks) -> bool {
                let ($($F,)*) = fetch;
                true $(&& $F::matches($F, entity, ticks))*
            }
        }
    };
}

impl_query_filter_tuple!();
impl_query_filter_tuple!(F0);
impl_query_filter_tuple!(F0, F1);
impl_query_filter_tuple!(F0, F1, F2);
impl_query_filter_tuple!(F0, F1, F2, F3);

// --- query ---

/// A view over every entity matching `D` and `F`.
pub struct Query<'w, D: QueryData, F: QueryFilter = ()> {
    world: &'w World,
    data: D::Fetch<'w>,
    filter: F::Fetch<'w>,
    ticks: SystemTicks,
}

enum EntityList<'a> {
    Borrowed(&'a [Entity]),
    Owned(Vec<Entity>),
}

impl EntityList<'_> {
    fn get(&self, i: usize) -> Option<Entity> {
        match self {
            EntityList::Borrowed(s) => s.get(i).copied(),
            EntityList::Owned(v) => v.get(i).copied(),
        }
    }
}

impl<'w, D: QueryData, F: QueryFilter> Query<'w, D, F> {
    /// # Safety
    /// The caller must hold the access reported by `D` and `F`.
    pub(crate) unsafe fn new(world: &'w World, ticks: SystemTicks) -> Self {
        Self {
            world,
            data: D::fetch(world),
            filter: F::fetch(world),
            ticks,
        }
    }

    fn entities(&self) -> EntityList<'_> {
        // SAFETY: the access for both fetches is held for the query's lifetime.
        match unsafe { shortest(D::driver(&self.data), F::driver(&self.filter)) } {
            Some(slice) => EntityList::Borrowed(slice),
            None => EntityList::Owned(self.world.entities_snapshot()),
        }
    }

    /// # Safety
    /// For mutable data, the caller must not create two live items for the same entity.
    unsafe fn fetch_item(&self, entity: Entity) -> Option<D::Item<'_>> {
        if F::matches(&self.filter, entity, self.ticks) {
            D::get(&self.data, entity, self.ticks)
        } else {
            None
        }
    }

    unsafe fn get_unchecked(&self, entity: Entity) -> Option<D::Item<'_>> {
        if !self.world.contains_entity(entity) {
            return None;
        }
        self.fetch_item(entity)
    }

    unsafe fn iter_unchecked(&self) -> QueryIter<'_, 'w, D, F> {
        QueryIter {
            query: self,
            entities: self.entities(),
            pos: 0,
        }
    }

    pub fn iter(&self) -> QueryIter<'_, 'w, D, F>
    where
        D: ReadOnlyQueryData,
    {
        // SAFETY: read-only items may alias.
        unsafe { self.iter_unchecked() }
    }

    pub fn iter_mut(&mut self) -> QueryIter<'_, 'w, D, F> {
        // SAFETY: `&mut self` prevents any other item from being alive, and the iterator visits
        // each entity once.
        unsafe { self.iter_unchecked() }
    }

    pub fn get(&self, entity: Entity) -> Option<D::Item<'_>>
    where
        D: ReadOnlyQueryData,
    {
        // SAFETY: read-only items may alias.
        unsafe { self.get_unchecked(entity) }
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<D::Item<'_>> {
        // SAFETY: `&mut self` prevents any other item from being alive.
        unsafe { self.get_unchecked(entity) }
    }

    pub fn contains(&self, entity: Entity) -> bool {
        // SAFETY: the item is dropped immediately, and `&self` can't coexist with items from
        // the `&mut self` methods.
        unsafe { self.get_unchecked(entity).is_some() }
    }

    pub fn is_empty(&self) -> bool {
        // SAFETY: see `contains`.
        unsafe { self.iter_unchecked().next().is_none() }
    }

    pub fn count(&self) -> usize {
        // SAFETY: see `contains`.
        unsafe { self.iter_unchecked().count() }
    }

    /// Returns the only matching item, or `None` if there are zero or several.
    pub fn get_single(&self) -> Option<D::Item<'_>>
    where
        D: ReadOnlyQueryData,
    {
        let mut iter = self.iter();
        let item = iter.next()?;
        iter.next().is_none().then_some(item)
    }

    pub fn get_single_mut(&mut self) -> Option<D::Item<'_>> {
        // SAFETY: at most one item escapes, and `&mut self` is held for its lifetime.
        unsafe {
            if self.count() != 1 {
                return None;
            }
            self.iter_unchecked().next()
        }
    }

    pub fn single(&self) -> D::Item<'_>
    where
        D: ReadOnlyQueryData,
    {
        self.get_single()
            .unwrap_or_else(|| panic!("expected exactly one match for `{}`", type_name::<D>()))
    }

    pub fn single_mut(&mut self) -> D::Item<'_> {
        self.get_single_mut()
            .unwrap_or_else(|| panic!("expected exactly one match for `{}`", type_name::<D>()))
    }
}

pub struct QueryIter<'q, 'w, D: QueryData, F: QueryFilter> {
    query: &'q Query<'w, D, F>,
    entities: EntityList<'q>,
    pos: usize,
}

impl<'q, D: QueryData, F: QueryFilter> Iterator for QueryIter<'q, '_, D, F> {
    type Item = D::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entity = self.entities.get(self.pos)?;
            self.pos += 1;
            // SAFETY: each entity is visited once; mutable iteration borrows the query mutably.
            if let Some(item) = unsafe { self.query.fetch_item(entity) } {
                return Some(item);
            }
        }
    }
}

impl<'q, 'w, D: ReadOnlyQueryData, F: QueryFilter> IntoIterator for &'q Query<'w, D, F> {
    type Item = D::Item<'q>;
    type IntoIter = QueryIter<'q, 'w, D, F>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'q, 'w, D: QueryData, F: QueryFilter> IntoIterator for &'q mut Query<'w, D, F> {
    type Item = D::Item<'q>;
    type IntoIter = QueryIter<'q, 'w, D, F>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<D: QueryData + 'static, F: QueryFilter + 'static> SystemParam for Query<'_, D, F> {
    type State = ();
    type Item<'w, 's> = Query<'w, D, F>;

    fn init_state(world: &mut World, access: &mut Access) {
        D::init(world);
        F::init(world);
        let mut query_access = FilteredAccess::default();
        D::access(&mut query_access);
        F::access(&mut query_access);
        access.add_query(query_access, type_name::<Self>());
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> Query<'w, D, F> {
        Query::new(world, meta.ticks)
    }
}
