use std::cell::RefCell;

use super::{
    access::Access,
    bundle::Bundle,
    entity::{Entities, Entity},
    storage::Component,
    system::{SystemMeta, SystemParam},
    world::World,
};

type Command = Box<dyn FnOnce(&mut World)>;

#[derive(Default)]
pub struct CommandQueue {
    commands: Vec<Command>,
}

impl CommandQueue {
    pub fn push(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.commands.push(Box::new(command));
    }

    pub fn apply(&mut self, world: &mut World) {
        for command in self.commands.drain(..) {
            command(world);
        }
    }
}

/// Deferred structural changes. They are applied right after the system that queued them.
pub struct Commands<'w, 's> {
    queue: &'s mut CommandQueue,
    entities: &'w RefCell<Entities>,
}

impl Commands<'_, '_> {
    /// Spawns an entity. Its id is valid immediately; its components arrive when commands apply.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> EntityCommands<'_> {
        let entity = self.entities.borrow_mut().alloc();
        self.queue.push(move |world| {
            world.insert(entity, bundle);
        });
        EntityCommands {
            entity,
            queue: self.queue,
        }
    }

    pub fn entity(&mut self, entity: Entity) -> EntityCommands<'_> {
        EntityCommands {
            entity,
            queue: self.queue,
        }
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.queue.push(move |world| {
            world.despawn(entity);
        });
    }

    pub fn insert_resource<R: 'static>(&mut self, resource: R) {
        self.queue
            .push(move |world| world.insert_resource(resource));
    }

    pub fn remove_resource<R: 'static>(&mut self) {
        self.queue.push(|world| {
            world.remove_resource::<R>();
        });
    }

    /// Queues arbitrary work that needs `&mut World`.
    pub fn add(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.queue.push(command);
    }
}

pub struct EntityCommands<'a> {
    entity: Entity,
    queue: &'a mut CommandQueue,
}

impl EntityCommands<'_> {
    pub fn id(&self) -> Entity {
        self.entity
    }

    pub fn insert<B: Bundle>(&mut self, bundle: B) -> &mut Self {
        let entity = self.entity;
        self.queue.push(move |world| {
            world.insert(entity, bundle);
        });
        self
    }

    pub fn remove<C: Component>(&mut self) -> &mut Self {
        let entity = self.entity;
        self.queue.push(move |world| {
            world.remove::<C>(entity);
        });
        self
    }

    pub fn despawn(&mut self) {
        let entity = self.entity;
        self.queue.push(move |world| {
            world.despawn(entity);
        });
    }
}

impl SystemParam for Commands<'_, '_> {
    type State = CommandQueue;
    type Item<'w, 's> = Commands<'w, 's>;

    fn init_state(_world: &mut World, _access: &mut Access) -> CommandQueue {
        CommandQueue::default()
    }

    unsafe fn fetch<'w, 's>(
        state: &'s mut CommandQueue,
        world: &'w World,
        _meta: &SystemMeta,
    ) -> Commands<'w, 's> {
        Commands {
            queue: state,
            entities: world.entities(),
        }
    }

    fn apply(state: &mut CommandQueue, world: &mut World) {
        state.apply(world);
    }
}
