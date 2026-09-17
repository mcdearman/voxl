use super::{entity::Entity, storage::Component, world::World};

/// A set of components that can be inserted together. Implemented for every component and for
/// tuples of bundles, so `(Transform, Velocity, (Mesh3d, Material))` works.
pub trait Bundle: 'static {
    fn insert_into(self, world: &mut World, entity: Entity);
}

impl<C: Component> Bundle for C {
    fn insert_into(self, world: &mut World, entity: Entity) {
        let tick = world.change_tick();
        world.storage_or_insert::<C>().insert(entity, self, tick);
    }
}

macro_rules! impl_bundle_tuple {
    ($($B:ident),*) => {
        #[allow(non_snake_case, unused_variables)]
        impl<$($B: Bundle),*> Bundle for ($($B,)*) {
            fn insert_into(self, world: &mut World, entity: Entity) {
                let ($($B,)*) = self;
                $($B.insert_into(world, entity);)*
            }
        }
    };
}

impl_bundle_tuple!();
impl_bundle_tuple!(B0);
impl_bundle_tuple!(B0, B1);
impl_bundle_tuple!(B0, B1, B2);
impl_bundle_tuple!(B0, B1, B2, B3);
impl_bundle_tuple!(B0, B1, B2, B3, B4);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6, B7);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6, B7, B8);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9, B10);
impl_bundle_tuple!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9, B10, B11);
