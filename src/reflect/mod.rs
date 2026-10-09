//! Reflection: describing, saving and loading values without knowing their Rust type.
//!
//! This is the layer scenes, an editor's inspector, save games and network replication all
//! stand on. A [`Reflect`] type can be taken apart into a [`Value`] (plain nested data) and
//! put back together, and can describe its shape as a [`Schema`]. The [`TypeRegistry`] makes
//! component types reachable by name, and a [`Scene`] is a set of entities captured that way,
//! written as JSON.
//!
//! ```ignore
//! #[derive(Component, Reflect, Default)]
//! #[reflect(name = "game.Health", default)]
//! struct Health { current: f32, max: f32 }
//!
//! app.register_type::<Health>();
//! let saved = Scene::capture(&app.world, app.world.resource::<TypeRegistry>()).to_json();
//! ```

pub mod json;
mod registry;
mod scene;
mod value;

#[cfg(test)]
mod tests;

pub use registry::{ComponentType, TypeRegistry};
pub use scene::{Scene, SceneEntity, Spawned};
pub use value::{Reflect, ReflectError, Schema, Value};
pub use voxl_derive::Reflect;
