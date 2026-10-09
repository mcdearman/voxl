use std::collections::HashMap;

use crate::{
    ecs::{event_update_system, Component, Events, IntoSystems, Schedule, World},
    input::InputPlugin,
    live::{Frame, Live},
    plugin::{NativePlugins, PluginEvents},
    reflect::{Reflect, TypeRegistry},
    render::RenderPlugin,
    time::{FixedTime, Time, TimePlugin},
    transform::TransformPlugin,
    window::WindowPlugin,
};

/// The phases of a frame, in the order they run. `Startup` stages run once before the first frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    /// Engine setup that must happen before user startup systems (e.g. creating the GPU device).
    PreStartup,
    Startup,
    First,
    PreUpdate,
    /// The three fixed stages run together, zero or more times per frame, at the rate set by
    /// `FixedTime`. Simulation goes in `FixedUpdate`; the other two are for engine bookkeeping.
    FixedFirst,
    FixedUpdate,
    FixedLast,
    Update,
    PostUpdate,
    Last,
    /// Copies what the renderer needs out of the ECS.
    Extract,
    /// Uploads buffers and builds draw lists.
    Prepare,
    /// Records and submits the frame.
    Render,
}

const EARLY_STAGES: [Stage; 2] = [Stage::First, Stage::PreUpdate];
const FIXED_STAGES: [Stage; 3] = [Stage::FixedFirst, Stage::FixedUpdate, Stage::FixedLast];
const LATE_STAGES: [Stage; 6] = [
    Stage::Update,
    Stage::PostUpdate,
    Stage::Last,
    Stage::Extract,
    Stage::Prepare,
    Stage::Render,
];

pub trait Plugin {
    fn build(&self, app: &mut App);
}

impl<F: Fn(&mut App)> Plugin for F {
    fn build(&self, app: &mut App) {
        self(app)
    }
}

/// Send this event to shut the app down at the end of the frame.
#[derive(Clone, Copy, Debug)]
pub struct AppExit;

pub struct App {
    pub world: World,
    schedules: HashMap<Stage, Schedule>,
    started: bool,
    pub(crate) debug: Option<crate::remote::DebugServer>,
    // Last, so it is dropped last: the world's values may have destructors in plugin code.
    pub(crate) native: NativePlugins,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            world: World::new(),
            schedules: HashMap::new(),
            started: false,
            debug: None,
            native: NativePlugins::default(),
        };
        app.add_event::<AppExit>();
        app.init_resource::<PluginEvents>();
        app.init_resource::<TypeRegistry>();
        app.init_resource::<Live>();
        app.init_resource::<crate::live::History>();
        app.add_systems(Stage::Last, crate::live::record_history);
        app.add_plugins(crate::signal::SignalPlugin);
        app.add_systems(Stage::First, |world: &mut World| {
            world.resource_mut::<PluginEvents>().update();
        });
        app
    }

    pub fn add_plugins(&mut self, plugin: impl Plugin) -> &mut Self {
        plugin.build(self);
        self
    }

    pub fn add_systems<M>(&mut self, stage: Stage, systems: impl IntoSystems<M>) -> &mut Self {
        self.schedules
            .entry(stage)
            .or_default()
            .add_systems(systems);
        self
    }

    pub fn insert_resource<R: 'static>(&mut self, resource: R) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    pub fn init_resource<R: Default + 'static>(&mut self) -> &mut Self {
        self.world.init_resource::<R>();
        self
    }

    /// Makes a component type reachable by name, so scenes can save and load it and tools
    /// can inspect it.
    /// Makes a resource reachable by name, and saved with scenes.
    pub fn register_resource_type<R: Reflect>(&mut self) -> &mut Self {
        self.world.init_resource::<TypeRegistry>();
        self.world
            .resource_mut::<TypeRegistry>()
            .register_resource::<R>();
        self
    }

    pub fn register_type<C: Component + Reflect>(&mut self) -> &mut Self {
        self.world.init_resource::<TypeRegistry>();
        self.world.resource_mut::<TypeRegistry>().register::<C>();
        self
    }

    pub fn add_event<E: 'static>(&mut self) -> &mut Self {
        if !self.world.contains_resource::<Events<E>>() {
            self.world.init_resource::<Events<E>>();
            self.add_systems(Stage::First, event_update_system::<E>);
        }
        self
    }

    /// Runs the startup stages once and validates every system's access up front, so conflicts
    /// panic at launch instead of the first time a system happens to run.
    pub fn startup(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        for schedule in self.schedules.values_mut() {
            schedule.initialize(&mut self.world);
        }
        for stage in [Stage::PreStartup, Stage::Startup] {
            self.run_stage(stage);
        }
    }

    /// Runs one frame.
    pub fn update(&mut self) {
        self.startup();
        self.check_native_plugins();
        self.serve_debuggers();
        let frame = self.begin_live_frame();
        let mut fixed_steps = 0;
        if let Some(time) = self.world.get_resource_mut::<Time>() {
            match frame {
                Frame::Run => time.tick(),
                Frame::Step(step) => time.advance_by(step),
                Frame::Hold => time.hold(),
            }
            let delta = time.delta();
            if let Some(fixed) = self.world.get_resource_mut::<FixedTime>() {
                if frame != Frame::Hold {
                    fixed_steps = fixed.accumulate(delta);
                }
            }
        }
        for stage in EARLY_STAGES {
            self.run_stage(stage);
        }
        for _ in 0..fixed_steps {
            for stage in FIXED_STAGES {
                self.run_stage(stage);
            }
        }
        for stage in LATE_STAGES {
            // A paused game doesn't simulate, and does everything else.
            if stage == Stage::Update && frame == Frame::Hold {
                continue;
            }
            self.run_stage(stage);
        }
    }

    /// Settles what this frame does about the simulation: runs, steps or holds.
    fn begin_live_frame(&mut self) -> Frame {
        let Some(live) = self.world.get_resource_mut::<Live>() else {
            return Frame::Run;
        };
        if live.take_resume() {
            for schedule in self.schedules.values_mut() {
                schedule.resume();
            }
        } else if live.is_paused_by_failure()
            && self.schedules.values().all(|schedule| schedule.suspended() == 0)
        {
            // Every system that failed has been replaced by new code.
            live.failures_repaired();
        }
        live.begin_frame()
    }

    /// Every system in a stage, in the order they run, with how long each last took.
    pub fn systems(&self, stage: Stage) -> Vec<crate::ecs::SystemInfo> {
        self.schedules
            .get(&stage)
            .map_or(Vec::new(), |schedule| schedule.systems())
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub(crate) fn schedule_mut(&mut self, stage: Stage) -> &mut Schedule {
        self.schedules.entry(stage).or_default()
    }

    pub fn should_exit(&self) -> bool {
        self.world
            .get_resource::<Events<AppExit>>()
            .is_some_and(|events| !events.is_empty())
    }

    fn run_stage(&mut self, stage: Stage) {
        let Some(schedule) = self.schedules.get_mut(&stage) else {
            return;
        };
        let guarded = self
            .world
            .get_resource::<Live>()
            .is_some_and(|live| live.catch_failures);
        schedule.set_guarded(guarded);
        schedule.run(&mut self.world);
        let failures = schedule.take_failures();
        if !failures.is_empty() {
            let frame = self
                .world
                .get_resource::<Time>()
                .map_or(0, Time::frame_count);
            self.world.resource_mut::<Live>().record(stage, frame, failures);
        }
    }

    /// Opens a window and runs until the window closes or `AppExit` is sent.
    pub fn run(&mut self) -> anyhow::Result<()> {
        crate::window::run(std::mem::take(self))
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        for stage in [Stage::PreStartup, Stage::Startup]
            .into_iter()
            .chain(EARLY_STAGES)
            .chain(FIXED_STAGES)
            .chain(LATE_STAGES)
        {
            if let Some(schedule) = self.schedules.get(&stage) {
                out += &format!("{stage:?}\n");
                for name in schedule.system_names() {
                    out += &format!("  {name}\n");
                }
            }
        }
        out
    }
}

/// The standard set of engine plugins.
pub struct DefaultPlugins;

impl Plugin for DefaultPlugins {
    fn build(&self, app: &mut App) {
        let _ = env_logger::Builder::from_env(env_logger::Env::default())
            .filter_level(log::LevelFilter::Info)
            .parse_default_env()
            .try_init();
        app.add_plugins(TimePlugin)
            .add_plugins(WindowPlugin)
            .add_plugins(InputPlugin)
            .add_plugins(TransformPlugin)
            .add_plugins(crate::prefab::PrefabPlugin)
            .add_plugins(RenderPlugin);
        app.listen_for_debugger_from_env();
    }
}
