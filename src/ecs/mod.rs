//! A small sparse-set ECS with Bevy-style function systems.

mod access;
mod bundle;
mod change;
mod commands;
mod condition;
mod entity;
mod event;
pub mod guard;
mod query;
mod resource;
mod schedule;
pub(crate) mod storage;
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
pub(crate) use event::event_update_system;
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
pub(crate) use storage::ErasedStorage;
pub use storage::{Component, ComponentKey, ComponentTicks, DropFn, Tick};
pub use voxl_derive::Component;
pub use system::{BoxedSystem, IntoSystem, System, SystemMeta, SystemParam, SystemParamFunction};
pub use world::{NamedComponent, World};

pub mod prelude {
    pub use super::{
        Added, Bundle, Changed, Commands, Component, Entity, EventReader, EventWriter, Events,
        not, resource_exists, IntoSystems, Local, Mut, Query, Res, ResMut, Schedule, With,
        Without, World,
    };
}
