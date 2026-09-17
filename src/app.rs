use std::collections::HashMap;

use crate::{
    ecs::{event_update_system, Events, IntoSystems, Schedule, World},
    input::InputPlugin,
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
    /// Runs zero or more times per frame at the rate set by `FixedTime`.
    FixedUpdate,
    Update,
    PostUpdate,
    Last,
    Render,
}

const FRAME_STAGES: [Stage; 3] = [Stage::First, Stage::PreUpdate, Stage::FixedUpdate];
const LATE_STAGES: [Stage; 4] = [Stage::Update, Stage::PostUpdate, Stage::Last, Stage::Render];

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
        };
        app.add_event::<AppExit>();
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
        let mut fixed_steps = 0;
        if let Some(time) = self.world.get_resource_mut::<Time>() {
            time.tick();
            let delta = time.delta();
            if let Some(fixed) = self.world.get_resource_mut::<FixedTime>() {
                fixed_steps = fixed.accumulate(delta);
            }
        }
        for stage in FRAME_STAGES {
            if stage == Stage::FixedUpdate {
                for _ in 0..fixed_steps {
                    self.run_stage(stage);
                }
            } else {
                self.run_stage(stage);
            }
        }
        for stage in LATE_STAGES {
            self.run_stage(stage);
        }
    }

    pub fn should_exit(&self) -> bool {
        self.world
            .get_resource::<Events<AppExit>>()
            .is_some_and(|events| !events.is_empty())
    }

    fn run_stage(&mut self, stage: Stage) {
        if let Some(schedule) = self.schedules.get_mut(&stage) {
            schedule.run(&mut self.world);
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
            .chain(FRAME_STAGES)
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
            .add_plugins(RenderPlugin);
    }
}
