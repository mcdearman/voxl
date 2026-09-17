use std::{
    any::type_name,
    ops::{Deref, DerefMut},
};

use super::{
    access::Access,
    change::Mut,
    storage::{ComponentTicks, Tick},
    system::{SystemMeta, SystemParam},
    world::World,
};

/// Shared access to a resource.
pub struct Res<'w, T: 'static> {
    value: &'w T,
    ticks: ComponentTicks,
    last_run: Tick,
}

impl<T> Res<'_, T> {
    pub fn is_added(&self) -> bool {
        self.ticks.is_added(self.last_run)
    }

    pub fn is_changed(&self) -> bool {
        self.ticks.is_changed(self.last_run)
    }
}

impl<T> Deref for Res<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

/// Exclusive access to a resource.
pub struct ResMut<'w, T: 'static>(Mut<'w, T>);

impl<'w, T> ResMut<'w, T> {
    pub fn is_added(&self) -> bool {
        self.0.is_added()
    }

    pub fn is_changed(&self) -> bool {
        self.0.is_changed()
    }

    pub fn bypass_change_detection(&mut self) -> &mut T {
        self.0.bypass_change_detection()
    }
}

impl<T> Deref for ResMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

fn missing<T>(meta: &SystemMeta) -> ! {
    panic!(
        "system `{}` requested resource `{}`, which does not exist",
        meta.name,
        type_name::<T>()
    )
}

/// # Safety
/// The caller must hold the access declared by `Res<T>`.
unsafe fn fetch_res<'w, T: 'static>(world: &'w World, meta: &SystemMeta) -> Option<Res<'w, T>> {
    let (value, ticks) = world.resource_cell::<T>()?.get::<T>();
    Some(Res {
        value,
        ticks,
        last_run: meta.ticks.last_run,
    })
}

/// # Safety
/// The caller must hold the access declared by `ResMut<T>`.
unsafe fn fetch_res_mut<'w, T: 'static>(
    world: &'w World,
    meta: &SystemMeta,
) -> Option<ResMut<'w, T>> {
    let (value, ticks) = world.resource_cell::<T>()?.get_mut::<T>();
    Some(ResMut(Mut {
        value,
        ticks,
        last_run: meta.ticks.last_run,
        this_run: meta.ticks.this_run,
    }))
}

impl<T: 'static> SystemParam for Res<'_, T> {
    type State = ();
    type Item<'w, 's> = Res<'w, T>;

    fn init_state(_world: &mut World, access: &mut Access) {
        access.read_resource::<T>();
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> Res<'w, T> {
        fetch_res(world, meta).unwrap_or_else(|| missing::<T>(meta))
    }
}

impl<T: 'static> SystemParam for Option<Res<'_, T>> {
    type State = ();
    type Item<'w, 's> = Option<Res<'w, T>>;

    fn init_state(_world: &mut World, access: &mut Access) {
        access.read_resource::<T>();
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> Option<Res<'w, T>> {
        fetch_res(world, meta)
    }
}

impl<T: 'static> SystemParam for ResMut<'_, T> {
    type State = ();
    type Item<'w, 's> = ResMut<'w, T>;

    fn init_state(_world: &mut World, access: &mut Access) {
        access.write_resource::<T>();
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> ResMut<'w, T> {
        fetch_res_mut(world, meta).unwrap_or_else(|| missing::<T>(meta))
    }
}

impl<T: 'static> SystemParam for Option<ResMut<'_, T>> {
    type State = ();
    type Item<'w, 's> = Option<ResMut<'w, T>>;

    fn init_state(_world: &mut World, access: &mut Access) {
        access.write_resource::<T>();
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> Option<ResMut<'w, T>> {
        fetch_res_mut(world, meta)
    }
}

/// State private to a single system, kept between runs.
pub struct Local<'s, T>(&'s mut T);

impl<T> Deref for Local<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0
    }
}

impl<T> DerefMut for Local<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0
    }
}

impl<T: Default + 'static> SystemParam for Local<'_, T> {
    type State = T;
    type Item<'w, 's> = Local<'s, T>;

    fn init_state(_world: &mut World, _access: &mut Access) -> T {
        T::default()
    }

    unsafe fn fetch<'s>(state: &'s mut T, _world: &World, _meta: &SystemMeta) -> Local<'s, T> {
        Local(state)
    }
}
