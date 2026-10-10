use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use super::{
    access::{Access, AccessSummary},
    condition::{BoxedCondition, IntoCondition},
    guard,
    pool::Pool,
    system::{BoxedSystem, IntoSystem},
    world::World,
};

/// Who added a system. Systems from a native plugin carry its id so they can be swapped or
/// removed when the plugin reloads; everything else is `ENGINE`.
pub type SystemOwner = u32;
pub const ENGINE: SystemOwner = 0;

/// A system with what was said about when it runs.
pub struct SystemConfig {
    system: BoxedSystem,
    /// Names of systems or sets this one runs before.
    before: Vec<String>,
    after: Vec<String>,
    /// The sets it belongs to, which others can order themselves around.
    sets: Vec<String>,
    /// It runs only when all of these say so.
    conditions: Vec<BoxedCondition>,
    /// It panicked, and is left out until it is resumed or replaced.
    suspended: bool,
    /// What it queues need not be seen by the systems that follow it in its stage.
    commands_wait: bool,
    stats: SystemStats,
}

/// How much a system has run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemStats {
    /// How many times it has run (not counting frames its conditions held it back).
    pub runs: u64,
    /// How long its last run took, conditions included.
    pub last: Duration,
    pub total: Duration,
}

/// A system as seen from outside: for a debugger, a profiler, an editor.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemInfo {
    pub name: String,
    pub sets: Vec<String>,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub conditions: usize,
    /// It panicked and is waiting to be resumed or replaced.
    pub suspended: bool,
    pub stats: SystemStats,
    /// What it reads and writes; `None` for a system that takes the whole world, or one that
    /// hasn't been initialized yet.
    pub access: Option<AccessSummary>,
    /// Which batch of the stage it is in: systems that share a batch touch nothing in common
    /// and could run at the same moment. `None` until the schedule has been initialized.
    pub batch: Option<usize>,
    /// Whether it has to run on the main thread: it takes the whole world, comes from a
    /// plugin, or uses a resource pinned there (the window). `None` until initialized.
    pub main_thread: Option<bool>,
}

/// A system that panicked while the schedule was guarded.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemFailure {
    pub system: String,
    pub message: String,
    pub location: String,
    pub stack: String,
}

impl SystemConfig {
    fn new(system: BoxedSystem) -> Self {
        Self {
            system,
            before: Vec::new(),
            after: Vec::new(),
            sets: Vec::new(),
            conditions: Vec::new(),
            suspended: false,
            commands_wait: false,
            stats: SystemStats::default(),
        }
    }

    fn answers_to(&self, label: &str) -> bool {
        self.system.name() == label || self.sets.iter().any(|set| set == label)
    }
}

/// A list of systems. They run one at a time, and each system's commands are applied before
/// the next one starts. The order is the order they were added in, except that a system told
/// to run after another (or that another was told to run before) waits for it.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(SystemOwner, SystemConfig)>,
    /// Indices into `systems`, in the order they run. Rebuilt when `systems` changes.
    order: Vec<usize>,
    /// For each place in `order`, which batch that system is in: systems of one batch touch
    /// nothing in common and could run at the same moment. Empty until worked out.
    batches: Vec<usize>,
    /// For each place in `order`, whether that system has to run on the main thread.
    pinned: Vec<bool>,
    /// For each system (by its place in `systems`), whether it can run beside others.
    shareable: Vec<bool>,
    /// Whether batches run on several threads. On unless told otherwise.
    sequential: bool,
    sorted: bool,
    /// Whether a panicking system is caught and suspended instead of unwinding out of `run`.
    guarded: bool,
    failures: Vec<SystemFailure>,
}

impl Schedule {
    pub fn add_systems<M>(&mut self, systems: impl IntoSystems<M>) {
        self.systems
            .extend(systems.into_configs().into_iter().map(|s| (ENGINE, s)));
        self.sorted = false;
        self.batches.clear();
    }

    /// Makes `systems` the complete set belonging to `owner`. A new system takes the place of
    /// the owner's old system of the same name, so the order systems run in survives a
    /// reload; other new systems go at the end, and old ones without a successor are removed.
    pub fn replace_owned(&mut self, owner: SystemOwner, systems: Vec<BoxedSystem>) {
        let mut kept = vec![false; self.systems.len()];
        let mut added = Vec::new();
        for system in systems {
            let slot = self.systems.iter().enumerate().position(|(i, (o, old))| {
                *o == owner && !kept[i] && old.system.name() == system.name()
            });
            match slot {
                Some(i) => {
                    // New code: whatever made the old one fail may be gone.
                    self.systems[i].1.system = system;
                    self.systems[i].1.suspended = false;
                    kept[i] = true;
                }
                None => added.push((owner, SystemConfig::new(system))),
            }
        }
        let mut index = 0;
        self.systems.retain(|(o, _)| {
            let keep = *o != owner || kept[index];
            index += 1;
            keep
        });
        self.systems.extend(added);
        self.sorted = false;
        self.batches.clear();
    }

    /// Sets what the system called `name` runs before and after, in place of whatever it
    /// was told before. For systems that arrive without constraints of their own (a
    /// plugin's). Returns whether there is such a system.
    pub fn set_order(&mut self, name: &str, before: Vec<String>, after: Vec<String>) -> bool {
        let Some((_, config)) = self
            .systems
            .iter_mut()
            .find(|(_, config)| config.system.name() == name)
        else {
            return false;
        };
        config.before = before;
        config.after = after;
        self.sorted = false;
        self.batches.clear();
        true
    }

    /// Works out the order the systems run in: at each step, the earliest-added system that
    /// isn't waiting for another.
    ///
    /// # Panics
    /// If the constraints go round in a circle.
    fn sort(&mut self) {
        let count = self.systems.len();
        // `follows[a]` lists the systems that must come after `a`.
        let mut follows: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut waiting_on = vec![0usize; count];
        let mut edge = |first: usize, then: usize| {
            if first != then && !follows[first].contains(&then) {
                follows[first].push(then);
                waiting_on[then] += 1;
            }
        };
        for (index, (_, config)) in self.systems.iter().enumerate() {
            // A name nothing answers to is not an error: the other system may belong to a
            // plugin that isn't there.
            for (other, (_, candidate)) in self.systems.iter().enumerate() {
                if config
                    .before
                    .iter()
                    .any(|label| candidate.answers_to(label))
                {
                    edge(index, other);
                }
                if config.after.iter().any(|label| candidate.answers_to(label)) {
                    edge(other, index);
                }
            }
        }
        self.order.clear();
        let mut placed = vec![false; count];
        while self.order.len() < count {
            // The earliest-added system that is waiting on nothing.
            let Some(next) = (0..count).find(|&i| !placed[i] && waiting_on[i] == 0) else {
                let stuck: Vec<&str> = (0..count)
                    .filter(|&i| !placed[i])
                    .map(|i| self.systems[i].1.system.name())
                    .collect();
                panic!(
                    "these systems are ordered in a circle, each before another: {}",
                    stuck.join(", ")
                );
            };
            placed[next] = true;
            self.order.push(next);
            for &then in &follows[next] {
                waiting_on[then] -= 1;
            }
        }
        self.sorted = true;
    }

    pub fn initialize(&mut self, world: &mut World) {
        for (_, config) in &mut self.systems {
            config.system.initialize(world);
            for condition in &mut config.conditions {
                condition.initialize(world);
            }
        }
        if !self.sorted {
            self.sort();
        }
        self.plan(world);
    }

    /// Whether the system at `index` could run at the same moment as the one at `other`:
    /// both say what they touch, conditions included, and none of it clashes.
    fn independent(&self, index: usize, other: usize) -> bool {
        let parts = |index: usize| {
            let config = &self.systems[index].1;
            let mut parts = vec![config.system.access()];
            parts.extend(config.conditions.iter().map(|condition| condition.access()));
            parts.into_iter().collect::<Option<Vec<&Access>>>()
        };
        match (parts(index), parts(other)) {
            (Some(mine), Some(theirs)) => mine
                .iter()
                .all(|a| theirs.iter().all(|b| !a.conflicts_with(b))),
            // One of them takes the whole world, or hasn't said what it takes.
            _ => false,
        }
    }

    /// Whether systems after this one have to wait for it to finish entirely: it changes
    /// the world when it is done (commands), or isn't a system that can run beside others.
    fn ends_its_batch(&self, index: usize) -> bool {
        let config = &self.systems[index].1;
        // What it queues is applied at the end of its batch in any case; unless it has said
        // that will do, the batch ends with it so that the next system sees it done.
        let queues = |access: &Access| access.defers() && !config.commands_wait;
        !self.shareable[index] || config.system.access().is_none_or(queues)
    }

    /// Groups the systems, in running order, into batches of systems that could run at the
    /// same moment: a system joins the batch before it unless it clashes with, or was told to
    /// run after, something in it. Systems still run one at a time; this is the plan a
    /// parallel executor will follow, and it is what `systems()` reports.
    fn plan(&mut self, world: &mut World) {
        for (_, config) in &mut self.systems {
            config.system.initialize(world);
            for condition in &mut config.conditions {
                condition.initialize(world);
            }
        }
        self.shareable = self
            .systems
            .iter_mut()
            .map(|(_, config)| config.system.shared().is_some())
            .collect();
        let world = &*world;
        self.batches.clear();
        self.pinned = self
            .order
            .iter()
            .map(|&index| {
                let config = &self.systems[index].1;
                let touches_pinned = |access: Option<&Access>| match access {
                    Some(access) => access
                        .resources()
                        .any(|resource| world.is_pinned_to_main_thread(resource)),
                    // It takes the whole world, pinned resources and all.
                    None => true,
                };
                config.system.main_thread_only()
                    || touches_pinned(config.system.access())
                    || config
                        .conditions
                        .iter()
                        .any(|condition| touches_pinned(condition.access()))
            })
            .collect();
        let mut current: Vec<usize> = Vec::new();
        let mut batch = 0;
        for &index in &self.order {
            let config = &self.systems[index].1;
            let waits_for = |other: usize| {
                let earlier = &self.systems[other].1;
                config.after.iter().any(|label| earlier.answers_to(label))
                    || earlier.before.iter().any(|label| config.answers_to(label))
            };
            let fits = self.shareable[index]
                && current.iter().all(|&other| {
                    // What an earlier system queues, a later one is meant to see.
                    !self.ends_its_batch(other)
                        && self.independent(index, other)
                        && !waits_for(other)
                });
            if !fits && !current.is_empty() {
                batch += 1;
                current.clear();
            }
            current.push(index);
            self.batches.push(batch);
        }
    }

    pub fn run(&mut self, world: &mut World) {
        if !self.sorted {
            self.sort();
        }
        if self.batches.len() != self.order.len() {
            self.plan(world);
        }
        let mut start = 0;
        while start < self.order.len() {
            let batch = self.batches[start];
            let end = (start..self.order.len())
                .find(|&place| self.batches[place] != batch)
                .unwrap_or(self.order.len());
            if end - start > 1 && !self.sequential && Pool::global().workers() > 0 {
                self.run_together(world, start, end);
            } else {
                for place in start..end {
                    self.run_alone(world, place);
                }
            }
            start = end;
        }
        // What these systems wrote is told to whatever watches for it, before the next
        // stage runs.
        world.tell_of_changes();
    }

    /// Asks the conditions of the system at `place`, catching a panic if guarded. Returns
    /// whether the system should run.
    fn wanted(&mut self, world: &mut World, place: usize) -> bool {
        let config = &mut self.systems[self.order[place]].1;
        if config.suspended {
            return false;
        }
        let mut ask = || {
            // Every condition is asked, so each one sees every frame.
            let mut wanted = true;
            for condition in &mut config.conditions {
                wanted &= condition.check(world);
            }
            wanted
        };
        if !self.guarded {
            return ask();
        }
        match guard::catch(&mut ask) {
            Ok(wanted) => wanted,
            Err(caught) => {
                config.suspended = true;
                self.failures.push(failure(config.system.name(), caught));
                false
            }
        }
    }

    /// Records how a run of the system at `place` went.
    fn record(&mut self, place: usize, outcome: Result<Duration, guard::Caught>) {
        let config = &mut self.systems[self.order[place]].1;
        match outcome {
            Ok(took) => {
                config.stats.runs += 1;
                config.stats.last = took;
                config.stats.total += took;
            }
            Err(caught) => {
                config.suspended = true;
                self.failures.push(failure(config.system.name(), caught));
            }
        }
    }

    /// Runs one system by itself, with the whole world.
    fn run_alone(&mut self, world: &mut World, place: usize) {
        let started = Instant::now();
        if !self.wanted(world, place) {
            return;
        }
        let system = &mut self.systems[self.order[place]].1.system;
        let outcome = if self.guarded {
            guard::catch(|| system.run(world)).map(|()| started.elapsed())
        } else {
            system.run(world);
            Ok(started.elapsed())
        };
        self.record(place, outcome);
    }

    /// Runs the systems at places `start..end`, one batch, at the same moment: those that
    /// must stay on this thread here, the rest on the worker threads.
    fn run_together(&mut self, world: &mut World, start: usize, end: usize) {
        // Conditions first, here, one at a time: none of them reads what a system of the
        // batch writes, so asking early changes no answer.
        let wanted: Vec<usize> = (start..end)
            .filter(|&place| self.wanted(world, place))
            .collect();
        let guarded = self.guarded;
        let mut outcomes: Vec<(usize, Result<Duration, guard::Caught>)> = Vec::new();
        {
            /// A pointer to the world that may cross to another thread.
            struct Shared(*const World);
            // SAFETY: only used to run systems of one batch, which the plan has checked
            // touch nothing in common except to read; the world's own bookkeeping that a
            // shared run reaches (the change tick, the entity allocator) is atomic or locked.
            unsafe impl Send for Shared {}
            unsafe impl Sync for Shared {}
            let shared = Shared(world);
            let shared = &shared;

            // Each system of the batch, taken apart from the others: they are different
            // elements of `systems`, so the borrows don't overlap.
            let mut systems: Vec<Option<&mut SystemConfig>> = self
                .systems
                .iter_mut()
                .map(|(_, config)| Some(config))
                .collect();
            let results: Mutex<Vec<(usize, Result<Duration, guard::Caught>)>> = Mutex::default();
            let results = &results;
            let (mut here, mut there): (Vec<_>, Vec<_>) = (Vec::new(), Vec::new());
            for &place in &wanted {
                let config = systems[self.order[place]]
                    .take()
                    .expect("each system is in one place");
                let system = config
                    .system
                    .shared()
                    .expect("only shareable systems share a batch");
                let job = move || {
                    let started = Instant::now();
                    // SAFETY: see `Shared`; the system was initialized when the batch was
                    // planned.
                    let mut run = || unsafe { system.run_shared(&*shared.0) };
                    let outcome = if guarded {
                        guard::catch(&mut run).map(|()| started.elapsed())
                    } else {
                        run();
                        Ok(started.elapsed())
                    };
                    results
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push((place, outcome));
                };
                if self.pinned[place] {
                    here.push(job);
                } else {
                    there.push(job);
                }
            }
            // This thread works too: with nothing pinned to it, it takes one of the others.
            if here.is_empty() {
                here.extend(there.pop());
            }
            let there: Vec<Box<dyn FnOnce() + Send + '_>> = there
                .into_iter()
                .map(|job| Box::new(job) as Box<dyn FnOnce() + Send + '_>)
                .collect();
            Pool::global().run(there, || here.into_iter().for_each(|mut job| job()));
            outcomes.extend(
                results
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .drain(..),
            );
        }
        // Back to one thread: what the systems deferred is applied in the order they are
        // written, and the bookkeeping done.
        outcomes.sort_by_key(|(place, _)| *place);
        for (place, outcome) in outcomes {
            if outcome.is_ok() {
                let system = &mut self.systems[self.order[place]].1.system;
                if let Some(system) = system.shared() {
                    system.apply_deferred(world);
                }
            }
            self.record(place, outcome);
        }
    }

    /// Sets whether batches of systems run on several threads (the default) or every system
    /// runs by itself on the calling thread. The results are the same either way; this is
    /// for measuring, and for finding out whether a bug is about threads.
    pub fn set_parallel(&mut self, parallel: bool) {
        self.sequential = !parallel;
    }

    /// Sets whether a system that panics is caught: it is then suspended (left out of later
    /// runs) and reported by `take_failures`, and the rest of the schedule carries on.
    pub fn set_guarded(&mut self, guarded: bool) {
        self.guarded = guarded;
    }

    /// The systems that have panicked since this was last called.
    pub fn take_failures(&mut self) -> Vec<SystemFailure> {
        std::mem::take(&mut self.failures)
    }

    /// Lets suspended systems run again. Returns how many there were.
    pub fn resume(&mut self) -> usize {
        let mut resumed = 0;
        for (_, config) in &mut self.systems {
            resumed += std::mem::take(&mut config.suspended) as usize;
        }
        resumed
    }

    pub fn suspended(&self) -> usize {
        self.systems
            .iter()
            .filter(|(_, config)| config.suspended)
            .count()
    }

    /// Every system, in the order they run (see `system_names`).
    pub fn systems(&self) -> Vec<SystemInfo> {
        let sorted = self.sorted.then_some(&self.order);
        (0..self.systems.len())
            .map(|i| {
                let config = &self.systems[sorted.map_or(i, |order| order[i])].1;
                SystemInfo {
                    name: config.system.name().to_owned(),
                    sets: config.sets.clone(),
                    before: config.before.clone(),
                    after: config.after.clone(),
                    conditions: config.conditions.len(),
                    suspended: config.suspended,
                    stats: config.stats,
                    access: config.system.access().map(Access::summary),
                    batch: self.batches.get(i).copied(),
                    main_thread: self.pinned.get(i).copied(),
                }
            })
            .collect()
    }

    /// The systems' names, in the order they run (the order they were added, until the
    /// schedule has been initialized or run).
    pub fn system_names(&self) -> impl Iterator<Item = &str> + '_ {
        let sorted = self.sorted.then_some(&self.order);
        (0..self.systems.len()).map(move |i| {
            let index = sorted.map_or(i, |order| order[i]);
            self.systems[index].1.system.name()
        })
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

fn failure(system: &str, caught: guard::Caught) -> SystemFailure {
    SystemFailure {
        system: system.to_owned(),
        message: caught.message,
        location: caught.location,
        stack: caught.stack,
    }
}

/// What a system can be ordered against: another system (give the function) or a set (give
/// its name).
pub trait IntoLabel<Marker> {
    fn into_label(self) -> String;
}

impl IntoLabel<()> for &str {
    fn into_label(self) -> String {
        self.to_owned()
    }
}

#[doc(hidden)]
pub struct SystemLabel;

impl<M, S: IntoSystem<M>> IntoLabel<(SystemLabel, M)> for S {
    fn into_label(self) -> String {
        use super::system::System;
        self.into_system().name().to_owned()
    }
}

/// Systems that have been told something about when they run. What `before`, `after`,
/// `in_set`, `run_if` and `chain` return, so they can be combined.
pub struct SystemConfigs(Vec<SystemConfig>);

impl IntoSystems<()> for SystemConfigs {
    fn into_configs(self) -> Vec<SystemConfig> {
        self.0
    }
}

/// A single system or a tuple of them, e.g. `app.add_systems(Stage::Update, (move_player, spin))`.
///
/// Systems run in the order they are added unless told otherwise:
///
/// ```ignore
/// app.add_systems(Stage::Update, (
///     follow_player.after(move_player),          // wherever `move_player` was added
///     (read_input, move_player, collide).chain(), // one after another
///     spawn_enemies.run_if(in_state(Game::Playing)),
///     (gravity, drag).in_set("forces"),
///     integrate.after("forces"),
/// ));
/// ```
pub trait IntoSystems<Marker>: Sized {
    fn into_configs(self) -> Vec<SystemConfig>;

    /// Runs these before a system (give the function) or every system in a set (give its
    /// name), in the same stage. Naming something that isn't in the stage does nothing.
    fn before<M>(self, other: impl IntoLabel<M>) -> SystemConfigs {
        let label = other.into_label();
        let mut configs = self.into_configs();
        for config in &mut configs {
            config.before.push(label.clone());
        }
        SystemConfigs(configs)
    }

    /// Runs these after a system or every system in a set, in the same stage.
    fn after<M>(self, other: impl IntoLabel<M>) -> SystemConfigs {
        let label = other.into_label();
        let mut configs = self.into_configs();
        for config in &mut configs {
            config.after.push(label.clone());
        }
        SystemConfigs(configs)
    }

    /// Puts these in a named set, which other systems can run `before` or `after`.
    fn in_set(self, set: &str) -> SystemConfigs {
        let mut configs = self.into_configs();
        for config in &mut configs {
            config.sets.push(set.to_owned());
        }
        SystemConfigs(configs)
    }

    /// Runs these only when the condition holds. A condition is a function with system
    /// parameters that returns `bool`; each system asks its own copy just before it would
    /// run.
    fn run_if<M>(self, condition: impl IntoCondition<M> + Clone) -> SystemConfigs {
        let mut configs = self.into_configs();
        for config in &mut configs {
            config.conditions.push(condition.clone().into_condition());
        }
        SystemConfigs(configs)
    }

    /// Says that what these queue with `Commands` need not be seen by the systems after
    /// them in the stage: it is applied when their batch has run, as ever, but the batch no
    /// longer ends with them, so systems that touch nothing in common with them can run at
    /// the same moment. For a system whose commands are for a later stage or the next
    /// frame: spawning effects, despawning what has expired.
    fn commands_can_wait(self) -> SystemConfigs {
        let mut configs = self.into_configs();
        for config in &mut configs {
            config.commands_wait = true;
        }
        SystemConfigs(configs)
    }

    /// Runs these one after another, in the order written.
    fn chain(self) -> SystemConfigs {
        let mut configs = self.into_configs();
        for index in 1..configs.len() {
            let previous = configs[index - 1].system.name().to_owned();
            configs[index].after.push(previous);
        }
        SystemConfigs(configs)
    }
}

#[doc(hidden)]
pub struct SingleSystem;

impl<M, S: IntoSystem<M>> IntoSystems<(SingleSystem, M)> for S {
    fn into_configs(self) -> Vec<SystemConfig> {
        vec![SystemConfig::new(Box::new(self.into_system()))]
    }
}

#[doc(hidden)]
pub struct SystemTuple;

macro_rules! impl_into_systems_tuple {
    ($(($S:ident, $M:ident)),*) => {
        #[allow(non_snake_case)]
        impl<$($M, $S: IntoSystems<$M>),*> IntoSystems<(SystemTuple, $($M,)*)> for ($($S,)*) {
            fn into_configs(self) -> Vec<SystemConfig> {
                let ($($S,)*) = self;
                let mut systems = Vec::new();
                $(systems.extend($S.into_configs());)*
                systems
            }
        }
    };
}

impl_into_systems_tuple!((S0, M0));
impl_into_systems_tuple!((S0, M0), (S1, M1));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3), (S4, M4));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3), (S4, M4), (S5, M5));
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9),
    (S10, M10)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9),
    (S10, M10),
    (S11, M11)
);
