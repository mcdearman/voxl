use std::collections::HashMap;

use super::value::{Reflect, ReflectError, Schema, Value};
use crate::ecs::{Component, Entity, World};

/// What can be done with a component type knowing only its name: the handle an editor, a
/// scene file or a network message uses in place of the Rust type.
pub struct ComponentType {
    pub name: &'static str,
    pub schema: fn() -> Schema,
    /// The component's value on an entity, if it has one.
    pub get: fn(&World, Entity) -> Option<Value>,
    /// Builds the component from a value and puts it on the entity, replacing any it had.
    pub insert: fn(&mut World, Entity, &Value) -> Result<(), ReflectError>,
    pub remove: fn(&mut World, Entity),
    /// Every entity that has the component.
    pub entities: fn(&World) -> Vec<Entity>,
}

/// The component types that can be reached by name. A resource; add to it with
/// `App::register_type`.
#[derive(Default)]
pub struct TypeRegistry {
    types: Vec<ComponentType>,
    names: HashMap<&'static str, usize>,
}

impl TypeRegistry {
    pub fn register<C: Component + Reflect>(&mut self) {
        let name = C::type_name();
        if self.names.contains_key(name) {
            return;
        }
        self.names.insert(name, self.types.len());
        self.types.push(ComponentType {
            name,
            schema: C::schema,
            get: |world, entity| world.get::<C>(entity).map(C::to_value),
            insert: |world, entity, value| {
                let component = C::from_value(value)?;
                world.insert(entity, component);
                Ok(())
            },
            remove: |world, entity| {
                world.remove::<C>(entity);
            },
            entities: |world| {
                world
                    .storage::<C>()
                    .map_or(Vec::new(), |storage| storage.entities().to_vec())
            },
        });
    }

    pub fn get(&self, name: &str) -> Option<&ComponentType> {
        self.names.get(name).map(|&index| &self.types[index])
    }

    /// Every registered type, in the order they were registered.
    pub fn iter(&self) -> impl Iterator<Item = &ComponentType> {
        self.types.iter()
    }
}
