use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use super::value::{Reflect, ReflectError, Schema, Value};
use crate::ecs::{Component, Entity, World};

/// Makes a name live for the rest of the program, once however often it is asked for. Type and
/// field names are `&'static str` throughout reflection; this is how names that arrive at
/// runtime (a plugin's components and their fields) join them.
pub fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// What can be done with a component type knowing only its name: the handle an editor, a
/// scene file or a network message uses in place of the Rust type.
pub struct ComponentType {
    pub name: &'static str,
    pub schema: Box<dyn Fn() -> Schema>,
    /// The component's value on an entity, if it has one.
    pub get: GetFn,
    /// Builds the component from a value and puts it on the entity, replacing any it had.
    pub insert: InsertFn,
    pub remove: RemoveFn,
    /// Every entity that has the component.
    pub entities: EntitiesFn,
}

pub type GetFn = Box<dyn Fn(&World, Entity) -> Option<Value>>;
pub type InsertFn = Box<dyn Fn(&mut World, Entity, &Value) -> Result<(), ReflectError>>;
pub type RemoveFn = Box<dyn Fn(&mut World, Entity)>;
pub type EntitiesFn = Box<dyn Fn(&World) -> Vec<Entity>>;
pub type GetResourceFn = Box<dyn Fn(&World) -> Option<Value>>;
pub type InsertResourceFn = Box<dyn Fn(&mut World, &Value) -> Result<(), ReflectError>>;

/// What can be done with a resource type knowing only its name.
pub struct ResourceType {
    pub name: &'static str,
    pub schema: Box<dyn Fn() -> Schema>,
    /// The resource's value, if the world has it.
    pub get: GetResourceFn,
    /// Builds the resource from a value and puts it in the world, replacing any it had.
    pub insert: InsertResourceFn,
}

/// The component and resource types that can be reached by name. A resource; add to it with
/// `App::register_type` and `App::register_resource_type`.
#[derive(Default)]
pub struct TypeRegistry {
    types: Vec<ComponentType>,
    names: HashMap<&'static str, usize>,
    resources: Vec<ResourceType>,
    /// The Rust types behind what is registered, to tell what in a world is not.
    rust_types: HashSet<std::any::TypeId>,
}

impl TypeRegistry {
    pub fn register<C: Component + Reflect>(&mut self) {
        self.rust_types.insert(std::any::TypeId::of::<C>());
        if self.names.contains_key(C::type_name()) {
            return;
        }
        self.insert(ComponentType {
            name: C::type_name(),
            schema: Box::new(C::schema),
            get: Box::new(|world, entity| world.get::<C>(entity).map(C::to_value)),
            insert: Box::new(|world, entity, value| {
                let component = C::from_value(value)?;
                world.insert(entity, component);
                Ok(())
            }),
            remove: Box::new(|world, entity| {
                world.remove::<C>(entity);
            }),
            entities: Box::new(|world| {
                world
                    .storage::<C>()
                    .map_or(Vec::new(), |storage| storage.entities().to_vec())
            }),
        });
    }

    /// Adds a type described at runtime, or replaces the one of the same name (keeping its
    /// place in the order): a plugin describes its components again each time it is loaded.
    pub fn insert(&mut self, component: ComponentType) {
        match self.names.get(component.name) {
            Some(&index) => self.types[index] = component,
            None => {
                self.names.insert(component.name, self.types.len());
                self.types.push(component);
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&ComponentType> {
        self.names.get(name).map(|&index| &self.types[index])
    }

    /// Every registered type, in the order they were registered.
    pub fn iter(&self) -> impl Iterator<Item = &ComponentType> {
        self.types.iter()
    }

    /// Makes a resource reachable by name, and part of captured scenes.
    pub fn register_resource<R: Reflect>(&mut self) {
        self.rust_types.insert(std::any::TypeId::of::<R>());
        if self.resource(R::type_name()).is_some() {
            return;
        }
        self.resources.push(ResourceType {
            name: R::type_name(),
            schema: Box::new(R::schema),
            get: Box::new(|world| world.get_resource::<R>().map(R::to_value)),
            insert: Box::new(|world, value| {
                let resource = R::from_value(value)?;
                world.insert_resource(resource);
                Ok(())
            }),
        });
    }

    /// Whether a component or resource of this Rust type has been registered.
    pub fn knows_type(&self, rust_type: std::any::TypeId) -> bool {
        self.rust_types.contains(&rust_type)
    }

    pub fn resource(&self, name: &str) -> Option<&ResourceType> {
        self.resources.iter().find(|resource| resource.name == name)
    }

    /// Every registered resource type, in the order they were registered.
    pub fn resources(&self) -> impl Iterator<Item = &ResourceType> {
        self.resources.iter()
    }
}
