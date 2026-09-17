use std::any::type_name;

use super::{
    access::Access,
    system::{SystemMeta, SystemParam},
    world::World,
};

/// A double-buffered event queue. Events live for two updates, so every system gets to see an
/// event regardless of whether it runs before or after the sender.
pub struct Events<E> {
    old: Vec<E>,
    new: Vec<E>,
    /// Id of `old[0]`. Ids let each reader track what it has already seen.
    old_start: usize,
}

impl<E> Default for Events<E> {
    fn default() -> Self {
        Self {
            old: Vec::new(),
            new: Vec::new(),
            old_start: 0,
        }
    }
}

impl<E> Events<E> {
    pub fn send(&mut self, event: E) {
        self.new.push(event);
    }

    /// Drops the oldest buffer. Called once per frame.
    pub fn update(&mut self) {
        self.old_start += self.old.len();
        std::mem::swap(&mut self.old, &mut self.new);
        self.new.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }

    pub fn len(&self) -> usize {
        self.old.len() + self.new.len()
    }

    pub fn clear(&mut self) {
        self.old_start = self.next_id();
        self.old.clear();
        self.new.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &E> {
        self.old.iter().chain(&self.new)
    }

    fn next_id(&self) -> usize {
        self.old_start + self.len()
    }

    fn read_from(&self, id: usize) -> impl Iterator<Item = &E> {
        self.iter().skip(id.saturating_sub(self.old_start))
    }
}

pub(crate) fn event_update_system<E: 'static>(world: &mut World) {
    if let Some(events) = world.get_resource_mut::<Events<E>>() {
        events.update();
    }
}

fn events<'w, E: 'static>(world: &'w World, meta: &SystemMeta) -> &'w World {
    if !world.contains_resource::<Events<E>>() {
        panic!(
            "system `{}` uses event `{}`, which was never registered. Call `app.add_event::<{0}>()`.",
            meta.name,
            type_name::<E>()
        );
    }
    world
}

/// Reads events of type `E` that this system hasn't seen yet.
pub struct EventReader<'w, 's, E: 'static> {
    events: &'w Events<E>,
    cursor: &'s mut usize,
}

impl<'w, E> EventReader<'w, '_, E> {
    pub fn read(&mut self) -> impl Iterator<Item = &'w E> + 'w {
        let start = *self.cursor;
        *self.cursor = self.events.next_id();
        self.events.read_from(start)
    }

    pub fn is_empty(&self) -> bool {
        self.events.next_id() <= *self.cursor
    }

    /// Marks all pending events as read.
    pub fn clear(&mut self) {
        *self.cursor = self.events.next_id();
    }
}

impl<E: 'static> SystemParam for EventReader<'_, '_, E> {
    type State = usize;
    type Item<'w, 's> = EventReader<'w, 's, E>;

    fn init_state(_world: &mut World, access: &mut Access) -> usize {
        access.read_resource::<Events<E>>();
        0
    }

    unsafe fn fetch<'w, 's>(
        cursor: &'s mut usize,
        world: &'w World,
        meta: &SystemMeta,
    ) -> EventReader<'w, 's, E> {
        let cell = events::<E>(world, meta)
            .resource_cell::<Events<E>>()
            .unwrap();
        EventReader {
            events: cell.get::<Events<E>>().0,
            cursor,
        }
    }
}

/// Sends events of type `E`.
pub struct EventWriter<'w, E: 'static> {
    events: &'w mut Events<E>,
}

impl<E> EventWriter<'_, E> {
    pub fn send(&mut self, event: E) {
        self.events.send(event);
    }
}

impl<E: 'static> SystemParam for EventWriter<'_, E> {
    type State = ();
    type Item<'w, 's> = EventWriter<'w, E>;

    fn init_state(_world: &mut World, access: &mut Access) {
        access.write_resource::<Events<E>>();
    }

    unsafe fn fetch<'w>(_: &mut (), world: &'w World, meta: &SystemMeta) -> EventWriter<'w, E> {
        let cell = events::<E>(world, meta)
            .resource_cell::<Events<E>>()
            .unwrap();
        let (events, ticks) = cell.get_mut::<Events<E>>();
        ticks.changed = meta.ticks.this_run;
        EventWriter { events }
    }
}
