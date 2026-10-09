use std::ffi::c_void;

use voxl_plugin::sys::{self, VoxlSystemFn};

use super::events::PluginEvents;

use crate::{
    asset_server::AssetServer,
    assets::Assets,
    ecs::{
        Access, CommandQueue, ComponentKey, Entity, ErasedStorage, FilteredAccess, System, Tick,
        World,
    },
    input::{ButtonInput, KeyCode, Mouse, MouseButton},
    render::{Image, Mesh},
    time::{FixedTime, Time},
};

pub(crate) struct Term {
    pub key: ComponentKey,
    pub access: u32,
    pub name: String,
}

impl Term {
    fn yields_pointer(&self) -> bool {
        matches!(self.access, sys::VOXL_READ | sys::VOXL_WRITE)
    }
}

/// Describes `terms` to the access checker, exactly as a typed query would describe itself.
pub(crate) fn access_of(terms: &[Term]) -> FilteredAccess {
    let mut access = FilteredAccess::default();
    for term in terms {
        match term.access {
            sys::VOXL_READ => {
                access.read_key(term.key, &term.name);
                access.with_key(term.key, &term.name);
            }
            sys::VOXL_WRITE => {
                access.write_key(term.key, &term.name);
                access.with_key(term.key, &term.name);
            }
            sys::VOXL_WITH => access.with_key(term.key, &term.name),
            _ => access.without_key(term.key, &term.name),
        }
    }
    access
}

/// Checks that a system's queries can coexist, by the rules typed systems follow.
pub(crate) fn check_queries(system: &str, queries: &[Vec<Term>]) -> Result<(), String> {
    let mut access = Access::new(system);
    for (index, terms) in queries.iter().enumerate() {
        access.try_add_query(access_of(terms), &format!("query {index}"))?;
    }
    Ok(())
}

/// A system whose body is a function in a native plugin. It has one or more queries, each
/// described by its terms, and deferred commands.
pub(crate) struct DynamicSystem {
    name: String,
    /// In a fixed stage, where "delta" is the fixed timestep.
    fixed: bool,
    run: VoxlSystemFn,
    user: *mut c_void,
    /// Query 0 is the one the system was added with.
    queries: Vec<Vec<Term>>,
    queue: CommandQueue,
    /// The entities each query will consider this run; kept to reuse the allocations.
    entities: Vec<Vec<Entity>>,
    /// This system's reader in `PluginEvents`, found by name on its first run.
    reader: Option<u32>,
    access: Option<Access>,
}

impl DynamicSystem {
    pub(crate) fn new(
        name: String,
        fixed: bool,
        run: VoxlSystemFn,
        user: *mut c_void,
        terms: Vec<Term>,
    ) -> Self {
        Self {
            name,
            fixed,
            run,
            user,
            queries: vec![terms],
            queue: CommandQueue::default(),
            entities: Vec::new(),
            reader: None,
            access: None,
        }
    }

    /// Adds a query, returning its number, if it can coexist with the ones already there.
    pub(crate) fn add_query(&mut self, terms: Vec<Term>) -> Result<usize, String> {
        self.queries.push(terms);
        if let Err(conflict) = check_queries(&self.name, &self.queries) {
            self.queries.pop();
            return Err(conflict);
        }
        Ok(self.queries.len() - 1)
    }
}

impl System for DynamicSystem {
    fn name(&self) -> &str {
        &self.name
    }

    fn initialize(&mut self, _world: &mut World) {
        // The queries were checked against each other when the plugin registered them; this
        // records the same access a typed system would, so a conflict can never slip through.
        let mut access = Access::new(self.name.clone());
        for (index, terms) in self.queries.iter().enumerate() {
            access.add_query(access_of(terms), &format!("query {index}"));
        }
        access.read_resource::<Time>();
        access.read_resource::<FixedTime>();
        access.read_resource::<ButtonInput<KeyCode>>();
        access.read_resource::<ButtonInput<MouseButton>>();
        access.read_resource::<Mouse>();
        access.write_resource::<Assets<Mesh>>();
        access.write_resource::<AssetServer>();
        access.read_resource::<crate::signal::Signals>();
        access.write_resource::<Assets<Image>>();
        access.read_resource::<PluginEvents>();
        self.access = Some(access);
    }

    fn access(&self) -> Option<&Access> {
        self.access.as_ref()
    }

    fn run(&mut self, world: &mut World) {
        let tick = world.increment_change_tick();
        let world_ref: &World = world;
        self.entities.resize_with(self.queries.len(), Vec::new);
        let mut cursors = Vec::with_capacity(self.queries.len());
        for (terms, entities) in self.queries.iter().zip(&mut self.entities) {
            let Some(storages) = terms
                .iter()
                .map(|term| world_ref.erased_storage(term.key))
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            // Visit the smallest set of entities that could match, as typed queries do.
            entities.clear();
            let driver = terms
                .iter()
                .zip(&storages)
                .filter(|(term, _)| term.access != sys::VOXL_WITHOUT)
                .map(|(_, storage)| storage.entities())
                .min_by_key(|entities| entities.len());
            match driver {
                Some(driver) => entities.extend_from_slice(driver),
                None => *entities = world_ref.entities_snapshot(),
            }
            cursors.push(Cursor {
                terms,
                storages,
                entities,
                position: 0,
            });
        }

        let events = world_ref.get_resource::<PluginEvents>();
        if self.reader.is_none() {
            self.reader = events.map(|events| events.reader(&self.name));
        }

        let time = world_ref.get_resource::<Time>();
        let delta = match world_ref.get_resource::<FixedTime>() {
            Some(fixed) if self.fixed => fixed.timestep_secs(),
            _ => time.map_or(0.0, Time::delta_secs),
        };
        let mut context = Context {
            world: world_ref,
            queries: cursors,
            queue: &mut self.queue,
            reader: self.reader,
            tick,
            delta,
            elapsed: time.map_or(0.0, |t| t.elapsed().as_secs_f64()),
            failure: None,
            spawned: Vec::new(),
        };
        // SAFETY: we hold `&mut World`, so nothing else can touch the components the callback
        // reaches through the context, and the context outlives the call.
        unsafe { (self.run)((&raw mut context).cast(), self.user) };
        if let Some((message, trace)) = context.failure.take() {
            // As when a typed system panics: what it had queued is lost, and so are the
            // entities it had made to put things on.
            let spawned = std::mem::take(&mut context.spawned);
            self.queue = CommandQueue::default();
            for entity in spawned {
                world.despawn(entity);
            }
            if crate::ecs::guard::active() {
                crate::ecs::guard::raise(message, trace);
            }
            log::error!(target: "plugin", "`{}` failed: {message}\n{trace}", self.name);
            return;
        }
        self.queue.apply(world);
    }
}

/// One query's progress through a run.
struct Cursor<'a> {
    terms: &'a [Term],
    storages: Vec<&'a dyn ErasedStorage>,
    entities: &'a [Entity],
    position: usize,
}

impl Cursor<'_> {
    /// Tests `entity` against the terms, and if it matches writes a pointer for each
    /// read/write term to `out`.
    ///
    /// # Safety
    /// `out` must have room for one pointer per read/write term.
    unsafe fn fetch(&self, entity: Entity, tick: Tick, out: *mut *mut c_void) -> bool {
        let matches = self
            .terms
            .iter()
            .zip(&self.storages)
            .all(|(term, storage)| storage.contains(entity) != (term.access == sys::VOXL_WITHOUT));
        if !matches {
            return false;
        }
        let mut slot = 0;
        for (term, storage) in self.terms.iter().zip(&self.storages) {
            if !term.yields_pointer() {
                continue;
            }
            // SAFETY: the system has exclusive use of the world while it runs, and its
            // queries were checked not to reach the same component of the same entity twice.
            let Some(value) = storage.value_ptr(entity) else {
                return false;
            };
            if term.access == sys::VOXL_WRITE {
                storage.mark_changed(entity, tick);
            }
            out.add(slot).write(value.cast());
            slot += 1;
        }
        true
    }
}

/// What a `VoxlSystem*` points to while a plugin system runs.
pub(crate) struct Context<'a> {
    world: &'a World,
    queries: Vec<Cursor<'a>>,
    pub queue: &'a mut CommandQueue,
    reader: Option<u32>,
    tick: Tick,
    pub delta: f32,
    pub elapsed: f64,
    /// Set when the plugin says this run failed: its message and whatever trace it has.
    pub failure: Option<(String, String)>,
    /// The entities this run has made, in case it fails.
    spawned: Vec<Entity>,
}

impl Context<'_> {
    /// # Safety
    /// `out` must have room for one pointer per read/write term of the query.
    pub(crate) unsafe fn next(&mut self, query: usize, out: *mut *mut c_void) -> Option<Entity> {
        let tick = self.tick;
        let cursor = self.queries.get_mut(query)?;
        while let Some(&entity) = cursor.entities.get(cursor.position) {
            cursor.position += 1;
            if cursor.fetch(entity, tick, out) {
                return Some(entity);
            }
        }
        None
    }

    /// # Safety
    /// As `next`.
    pub(crate) unsafe fn get(&self, query: usize, entity: Entity, out: *mut *mut c_void) -> bool {
        self.queries.get(query).is_some_and(|cursor| {
            self.world.contains_entity(entity) && cursor.fetch(entity, self.tick, out)
        })
    }

    pub(crate) fn rewind(&mut self, query: usize) {
        if let Some(cursor) = self.queries.get_mut(query) {
            cursor.position = 0;
        }
    }

    /// The next event on a channel that this system hasn't read, if any.
    pub(crate) fn next_event(&mut self, event: u32) -> Option<&[u8]> {
        self.world
            .get_resource::<PluginEvents>()?
            .next(self.reader?, event)
    }

    pub(crate) fn spawn(&mut self) -> Entity {
        let entity = self.world.entities().borrow_mut().alloc();
        self.spawned.push(entity);
        entity
    }

    pub(crate) fn world(&self) -> &World {
        self.world
    }

    /// A named shape (`shape:cube:1`) from the asset server, made once however often it is
    /// asked for, so a reloaded plugin gets the mesh it had and scenes can save it by name.
    pub(crate) fn shape_mesh(&mut self, name: &str) -> Option<u32> {
        let meshes = self.world.resource_cell::<Assets<Mesh>>()?;
        let server = self.world.resource_cell::<AssetServer>()?;
        // SAFETY: the running system has exclusive use of the world, declared write access to
        // both resources, and holds no other reference to them.
        let ((meshes, ticks), (server, _)) = unsafe {
            (
                meshes.get_mut::<Assets<Mesh>>(),
                server.get_mut::<AssetServer>(),
            )
        };
        ticks.changed = self.tick;
        match server.shape(meshes, name) {
            Ok(handle) => Some(handle.id()),
            Err(err) => {
                log::error!(target: "plugin", "{err:#}");
                None
            }
        }
    }

    /// An image by name from the asset server, loading it if this is the first time.
    pub(crate) fn load_image(&mut self, name: &str) -> Option<u32> {
        let images = self.world.resource_cell::<Assets<Image>>()?;
        let server = self.world.resource_cell::<AssetServer>()?;
        // SAFETY: as in `shape_mesh`.
        let ((images, ticks), (server, _)) = unsafe {
            (
                images.get_mut::<Assets<Image>>(),
                server.get_mut::<AssetServer>(),
            )
        };
        ticks.changed = self.tick;
        Some(server.load_image(images, name).id())
    }

    /// Adds a mesh to the app's assets.
    pub(crate) fn add_mesh(&mut self, mesh: Mesh) -> Option<u32> {
        let cell = self.world.resource_cell::<Assets<Mesh>>()?;
        // SAFETY: the running system has exclusive use of the world, declared write access to
        // the mesh assets, and holds no other reference to them.
        let (meshes, ticks) = unsafe { cell.get_mut::<Assets<Mesh>>() };
        ticks.changed = self.tick;
        Some(meshes.add(mesh).id())
    }
}
