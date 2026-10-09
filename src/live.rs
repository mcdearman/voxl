//! The game as a live program: in debug builds a failure stops the game where it is instead
//! of ending it, and time can be paused, stepped and slowed.
//!
//! [`Live`] is a resource. While the game is paused the simulation stages (`FixedUpdate` and
//! its neighbours, and `Update`) don't run, and everything else does: input, hot reload,
//! transforms, rendering. So a paused game still draws, still reloads a plugin you fix, and
//! still shows a change made to its world from outside.

use std::time::Duration;

use crate::{app::Stage, ecs::SystemFailure};

/// A system that failed, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub stage: Stage,
    /// The frame it failed on (`Time::frame_count`).
    pub frame: u64,
    pub system: String,
    pub message: String,
    /// `file:line:column` of the panic.
    pub location: String,
    /// The stack from the panic down to the system, one frame per line.
    pub stack: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "`{}` failed in {:?} on frame {}",
            self.system, self.stage, self.frame
        )?;
        writeln!(f, "  {}", self.message)?;
        if !self.location.is_empty() {
            writeln!(f, "  at {}", self.location)?;
        }
        write!(f, "{}", self.stack)
    }
}

/// Control of a running game, for debugging. A resource every app has.
#[derive(Debug)]
pub struct Live {
    /// Whether a system that panics is caught, suspended and reported, instead of taking the
    /// game down. On in debug builds; `VOXL_LIVE=1` or `=0` overrides.
    pub catch_failures: bool,
    /// Whether a caught failure also pauses the game. The game resumes by itself when the
    /// code that failed has been replaced (a plugin reloaded), or when told to.
    pub pause_on_failure: bool,
    /// How much game time one step is, when stepping a paused game.
    pub step: Duration,
    paused: bool,
    paused_by_failure: bool,
    steps: u32,
    resume: bool,
    failures: Vec<Failure>,
}

impl Default for Live {
    fn default() -> Self {
        let catch_failures = match std::env::var("VOXL_LIVE").as_deref() {
            Ok("0") => false,
            Ok(_) => true,
            Err(_) => cfg!(debug_assertions),
        };
        Self {
            catch_failures,
            pause_on_failure: true,
            step: Duration::from_secs_f64(1.0 / 60.0),
            paused: false,
            paused_by_failure: false,
            steps: 0,
            resume: false,
            failures: Vec::new(),
        }
    }
}

impl Live {
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Stops the simulation. Everything else carries on.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Carries on, and lets systems that failed run again.
    pub fn resume(&mut self) {
        self.paused = false;
        self.paused_by_failure = false;
        self.steps = 0;
        self.resume = true;
    }

    /// Runs this many frames of the simulation, each `step` long, and pauses again.
    pub fn step_frames(&mut self, frames: u32) {
        self.paused = true;
        self.steps += frames;
    }

    /// Every failure since the game started, oldest first.
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    pub fn clear_failures(&mut self) {
        self.failures.clear();
    }

    pub(crate) fn record(&mut self, stage: Stage, frame: u64, failures: Vec<SystemFailure>) {
        for failure in failures {
            let failure = Failure {
                stage,
                frame,
                system: failure.system,
                message: failure.message,
                location: failure.location,
                stack: failure.stack,
            };
            log::error!("{failure}");
            if self.pause_on_failure {
                log::error!(
                    "the game is paused; `{}` is left out until its code is replaced or the game is resumed",
                    failure.system
                );
                self.paused = true;
                self.paused_by_failure = true;
            }
            self.failures.push(failure);
        }
    }

    /// Whether the simulation runs this frame, and for how long if that is fixed.
    pub(crate) fn begin_frame(&mut self) -> Frame {
        if !self.paused {
            Frame::Run
        } else if self.steps > 0 {
            self.steps -= 1;
            Frame::Step(self.step)
        } else {
            Frame::Hold
        }
    }

    pub(crate) fn take_resume(&mut self) -> bool {
        std::mem::take(&mut self.resume)
    }

    /// Called when no system is suspended any more: a pause that a failure caused is over.
    pub(crate) fn failures_repaired(&mut self) {
        if self.paused_by_failure {
            self.paused_by_failure = false;
            self.paused = false;
            log::info!("the code that failed has been replaced; carrying on");
        }
    }

    pub(crate) fn is_paused_by_failure(&self) -> bool {
        self.paused_by_failure
    }
}

/// What a frame does about the simulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Frame {
    Run,
    Step(Duration),
    Hold,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::App,
        ecs::{IntoSystem, Res, ResMut, Schedule},
        time::{FixedTime, Time, TimePlugin},
    };

    #[derive(Default)]
    struct Counts {
        update: u32,
        fixed: u32,
        post: u32,
        fragile: u32,
    }

    /// Set to make `fragile` fail.
    struct Broken;

    fn fragile(broken: Option<Res<Broken>>, mut counts: ResMut<Counts>) {
        let health: Option<u32> = broken.is_none().then_some(3);
        counts.fragile += health.expect("the player has no health");
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin).init_resource::<Counts>();
        app.world.resource_mut::<Live>().catch_failures = true;
        app.world
            .resource_mut::<Time>()
            .set_fixed_step(Some(Duration::from_millis(20)));
        app.world.insert_resource(FixedTime::from_hz(50.0));
        app.add_systems(
            Stage::Update,
            (fragile, |mut c: ResMut<Counts>| c.update += 1),
        )
        .add_systems(Stage::FixedUpdate, |mut c: ResMut<Counts>| c.fixed += 1)
        .add_systems(Stage::PostUpdate, |mut c: ResMut<Counts>| c.post += 1);
        app
    }

    fn counts(app: &App) -> (u32, u32, u32, u32) {
        let c = app.world.resource::<Counts>();
        (c.update, c.fixed, c.post, c.fragile)
    }

    #[test]
    fn a_failing_system_pauses_the_game_and_keeps_its_stack() {
        let mut app = app();
        app.update();
        app.update();
        assert_eq!(counts(&app), (2, 2, 2, 6));

        app.world.insert_resource(Broken);
        app.update();
        // The frame it failed on finished: the systems after it ran.
        assert_eq!(counts(&app), (3, 3, 3, 6));
        let live = app.world.resource::<Live>();
        assert!(live.is_paused());
        let [failure] = live.failures() else {
            panic!("one failure, not {:?}", live.failures());
        };
        assert_eq!(failure.stage, Stage::Update);
        assert_eq!(failure.frame, 3);
        assert!(failure.system.ends_with("fragile"), "{}", failure.system);
        assert_eq!(failure.message, "the player has no health");
        assert!(
            failure.location.starts_with("src/live.rs:"),
            "{}",
            failure.location
        );
        // The stack is there to be read: it names the function that failed, and stops
        // short of the scheduler's own frames.
        if !cfg!(miri) {
            assert!(
                failure.stack.contains("live::tests::fragile"),
                "{}",
                failure.stack
            );
            assert!(!failure.stack.contains("guard::catch"), "{}", failure.stack);
        }
        assert!(failure.to_string().contains("the player has no health"));

        // Paused: the simulation holds still, time doesn't pass, and the rest carries on.
        let elapsed = app.world.resource::<Time>().elapsed();
        app.update();
        app.update();
        assert_eq!(counts(&app), (3, 3, 5, 6));
        assert_eq!(app.world.resource::<Time>().elapsed(), elapsed);
        assert_eq!(
            app.world.resource::<Live>().failures().len(),
            1,
            "it fails once, not every frame"
        );

        // One frame at a time. The system that failed stays out of it.
        app.world.resource_mut::<Live>().step = Duration::from_millis(20);
        app.world.resource_mut::<Live>().step_frames(2);
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(counts(&app), (5, 5, 8, 6));
        assert!(app.world.resource::<Live>().is_paused());

        // Mend the world and carry on: the system runs again.
        app.world.remove_resource::<Broken>();
        app.world.resource_mut::<Live>().resume();
        app.update();
        assert_eq!(counts(&app), (6, 6, 9, 9));
        let info = app.systems(Stage::Update);
        assert_eq!(info.len(), 2);
        assert!(info[0].name.ends_with("fragile") && !info[0].suspended);
        assert_eq!(info[0].stats.runs, 3, "the run that failed is not counted");
    }

    #[test]
    fn replacing_the_code_that_failed_resumes_the_game() {
        // What a plugin reload does: the schedule swaps the owner's systems by name.
        fn tick() {
            panic!("divide by zero");
        }
        let mut world = crate::ecs::World::new();
        let mut schedule = Schedule::default();
        schedule.replace_owned(7, vec![Box::new(tick.into_system())]);
        schedule.set_guarded(true);
        schedule.run(&mut world);
        schedule.run(&mut world);
        assert_eq!(schedule.suspended(), 1);
        let failures = schedule.take_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].message, "divide by zero");

        schedule.replace_owned(7, vec![Box::new(tick.into_system())]);
        assert_eq!(schedule.suspended(), 0, "new code gets a new chance");
        schedule.run(&mut world);
        assert_eq!(schedule.take_failures().len(), 1);
    }

    #[test]
    fn without_catching_a_panic_is_a_panic() {
        let mut app = app();
        app.world.resource_mut::<Live>().catch_failures = false;
        app.world.insert_resource(Broken);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()));
        assert!(result.is_err());
    }

    #[test]
    fn time_can_be_slowed() {
        let mut time = Time::default();
        time.set_scale(0.25);
        time.tick();
        std::thread::sleep(Duration::from_millis(40));
        time.tick();
        let delta = time.delta();
        assert!(
            delta >= Duration::from_millis(10) && delta < Duration::from_millis(30),
            "{delta:?}"
        );
        assert_eq!(time.elapsed(), delta);
    }
}
