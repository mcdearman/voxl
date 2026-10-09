//! A small sparse-set ECS with Bevy-style function systems.
//!
//! This is the part of mira that knows nothing of windows, rendering or assets: entities,
//! components, resources, queries, systems, and the schedule that orders them and runs them
//! in parallel. The engine re-exports it as `mira::ecs`.
//!
//! Some of what is public here is so for the engine's sake (the storage types, the raw
//! and by-key ways into the world): the engine's plugin interface and scene code reach
//! components whose types they don't know. A game has no need of them.

mod access;
mod bundle;
mod change;
mod commands;
mod condition;
mod entity;
mod event;
mod pool;
pub mod guard;
mod query;
mod resource;
mod schedule;
pub mod storage;
mod system;
mod world;

#[cfg(test)]
mod tests;

pub use access::{Access, AccessSummary, FilteredAccess};
pub use bundle::Bundle;
pub use change::Mut;
pub use commands::{CommandQueue, Commands, EntityCommands};
pub use condition::{not, resource_exists, BoxedCondition, Condition, IntoCondition};
pub use entity::Entity;
pub use event::event_update_system;
pub use event::{EventReader, EventWriter, Events};
pub use query::{
    Added, Changed, Query, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData, SystemTicks, With,
    Without,
};
pub use resource::{Local, Res, ResMut};
pub use schedule::{
    IntoLabel, IntoSystems, Schedule, SystemConfig, SystemConfigs, SystemFailure, SystemInfo,
    SystemOwner, SystemStats,
};
pub use storage::ErasedStorage;
pub use storage::{Component, ComponentKey, ComponentTicks, DropFn, Tick};
pub use mira_derive::Component;
pub use system::{BoxedSystem, IntoSystem, System, SystemMeta, SystemParam, SystemParamFunction};
pub use world::{NamedComponent, World};

pub mod prelude {
    pub use super::{
        Added, Bundle, Changed, Commands, Component, Entity, EventReader, EventWriter, Events,
        not, resource_exists, IntoSystems, Local, Mut, Query, Res, ResMut, Schedule, With,
        Without, World,
    };
}
