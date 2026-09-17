//! A small sparse-set ECS with Bevy-style function systems.

mod access;
mod bundle;
mod change;
mod commands;
mod entity;
mod event;
mod query;
mod resource;
mod schedule;
mod storage;
mod system;
mod world;

#[cfg(test)]
mod tests;

pub use access::{Access, FilteredAccess};
pub use bundle::Bundle;
pub use change::Mut;
pub use commands::{CommandQueue, Commands, EntityCommands};
pub use entity::Entity;
pub(crate) use event::event_update_system;
pub use event::{EventReader, EventWriter, Events};
pub use query::{
    Added, Changed, Query, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData, SystemTicks, With,
    Without,
};
pub use resource::{Local, Res, ResMut};
pub use schedule::{IntoSystems, Schedule};
pub use storage::{Component, ComponentTicks, Tick};
pub use system::{BoxedSystem, IntoSystem, System, SystemMeta, SystemParam, SystemParamFunction};
pub use world::World;

pub mod prelude {
    pub use super::{
        Added, Bundle, Changed, Commands, Component, Entity, EventReader, EventWriter, Events,
        Local, Mut, Query, Res, ResMut, Schedule, With, Without, World,
    };
}
