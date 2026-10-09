//! The game as a live program: in debug builds a failure stops the game where it is instead
//! of ending it, and time can be paused, stepped and slowed.
//!
//! [`Live`] is a resource. While the game is paused the simulation stages (`FixedUpdate` and
//! its neighbours, and `Update`) don't run, and everything else does: input, hot reload,
//! transforms, rendering. So a paused game still draws, still reloads a plugin you fix, and
//! still shows a change made to its world from outside.

use std::{collections::VecDeque, time::Duration};

use crate::{
    app::Stage,
    ecs::{SystemFailure, World},
    reflect::{Scene, TypeRegistry},
    time::Time,
};

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
    /// game down. On in debug builds; `MIRA_LIVE=1` or `=0` overrides.
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
    /// A signal that ends stepping early when it becomes true.
    until: Option<String>,
    /// Whether the last `step_until` ended because its signal became true.
    reached: bool,
    /// Whether the frame now running is one in which the simulation is held still.
    pub(crate) holding: bool,
    failures: Vec<Failure>,
}

impl Default for Live {
    fn default() -> Self {
        let catch_failures = match std::env::var("MIRA_LIVE").as_deref() {
            Ok("0") => false,
            Ok(_) => true,
            Err(_) => cfg!(debug_assertions),
        };
        Self {
            catch_failures,
            pause_on_failure: true,
            step: Duration::from_secs_f64(1.0 / 60.0),
            // `MIRA_PAUSED=1` starts the game held at its first moment, to be stepped from
            // there: the same frames every time, for tests that compare pictures.
            paused: std::env::var("MIRA_PAUSED").is_ok_and(|paused| paused != "0"),
            paused_by_failure: false,
            steps: 0,
            resume: false,
            until: None,
            reached: false,
            holding: false,
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
        self.until = None;
        self.resume = true;
    }

    /// Runs this many frames of the simulation, each `step` long, and pauses again.
    pub fn step_frames(&mut self, frames: u32) {
        self.paused = true;
        self.steps += frames;
    }

    /// Runs the simulation until the named signal is true, or for `max_frames` frames if it
    /// doesn't come true, and pauses. `reached` says afterwards which it was.
    pub fn step_until(&mut self, signal: &str, max_frames: u32) {
        self.paused = true;
        self.steps = max_frames;
        self.until = Some(signal.to_owned());
        self.reached = false;
    }

    /// How many frames of stepping are still to run.
    pub fn steps_left(&self) -> u32 {
        self.steps
    }

    /// Whether the last [`Live::step_until`] ended because its signal came true.
    pub fn reached(&self) -> bool {
        self.reached
    }

    /// The signal a `step_until` is waiting for, while it is.
    pub(crate) fn until(&self) -> Option<&str> {
        self.until.as_deref()
    }

    /// Ends a `step_until`: early if its signal has come true, or because its frames ran out.
    pub(crate) fn check_until(&mut self, came_true: bool) {
        if self.until.is_none() {
            return;
        }
        if came_true {
            self.steps = 0;
            self.reached = true;
        }
        if self.steps == 0 {
            self.until = None;
        }
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
        let frame = if !self.paused {
            Frame::Run
        } else if self.steps > 0 {
            self.steps -= 1;
            Frame::Step(self.step)
        } else {
            Frame::Hold
        };
        self.holding = frame == Frame::Hold;
        frame
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

/// Where the time of recent frames went. A resource every app has; the engine fills it in.
#[derive(Debug, Default)]
pub struct FrameStats {
    /// How long each of the last frames took, oldest first: the engine's own work in
    /// `App::update`, not the wait for the display.
    frames: VecDeque<Duration>,
    /// How long each stage took in the last frame it ran, in the order they run.
    stages: Vec<(Stage, Duration)>,
}

impl FrameStats {
    /// How many frames are remembered.
    pub const KEPT: usize = 240;

    pub(crate) fn begin_frame(&mut self) {
        self.stages.clear();
    }

    pub(crate) fn stage(&mut self, stage: Stage, took: Duration) {
        // The fixed stages may run several times in a frame.
        match self.stages.iter_mut().find(|(known, _)| *known == stage) {
            Some((_, total)) => *total += took,
            None => self.stages.push((stage, took)),
        }
    }

    pub(crate) fn end_frame(&mut self, took: Duration) {
        if self.frames.len() == Self::KEPT {
            self.frames.pop_front();
        }
        self.frames.push_back(took);
    }

    /// The remembered frame times, oldest first.
    pub fn frames(&self) -> impl Iterator<Item = Duration> + '_ {
        self.frames.iter().copied()
    }

    /// What each stage took in the last frame.
    pub fn stages(&self) -> &[(Stage, Duration)] {
        &self.stages
    }

    /// The mean of the remembered frame times.
    pub fn mean(&self) -> Duration {
        match self.frames.len() {
            0 => Duration::ZERO,
            count => self.frames.iter().sum::<Duration>() / count as u32,
        }
    }

    /// The longest remembered frame.
    pub fn worst(&self) -> Duration {
        self.frames.iter().copied().max().unwrap_or_default()
    }
}

/// What a frame does about the simulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Frame {
    Run,
    Step(Duration),
    Hold,
}

/// A moment of the game that can be gone back to.
struct Snapshot {
    frame: u64,
    elapsed: Duration,
    scene: Scene,
}

/// When a snapshot was taken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub frame: u64,
    pub seconds: f64,
}

/// Snapshots of the world taken as the game runs, to step back to. A resource every app has;
/// off until `recording` is set, because a snapshot is a whole scene.
///
/// A snapshot holds what a scene does: the registered components of every entity, and the
/// registered resources. Going back to one puts those back, and the clock; what isn't
/// registered (a plugin's undescribed components, the insides of the physics solver, what
/// signals have timed) stays as it is now.
pub struct History {
    /// Whether snapshots are being taken.
    pub recording: bool,
    /// Frames between snapshots.
    pub every: u64,
    /// How many snapshots are kept; the oldest go first.
    pub keep: usize,
    snapshots: VecDeque<Snapshot>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            recording: false,
            every: 30,
            keep: 240,
            snapshots: VecDeque::new(),
        }
    }
}

impl History {
    /// The moments that can be gone back to, oldest first.
    pub fn moments(&self) -> Vec<Moment> {
        self.snapshots
            .iter()
            .map(|snapshot| Moment {
                frame: snapshot.frame,
                seconds: snapshot.elapsed.as_secs_f64(),
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    /// Takes a snapshot now, whatever `recording` and `every` say.
    pub fn snapshot(world: &mut World) {
        if !world.contains_resource::<TypeRegistry>() {
            return;
        }
        let (frame, elapsed) = world
            .get_resource::<Time>()
            .map_or((0, Duration::ZERO), |time| {
                (time.frame_count(), time.elapsed())
            });
        let scene = world
            .resource_scope(|world, registry: &mut TypeRegistry| Scene::capture(world, registry));
        let history = world.resource_mut::<History>();
        // One moment, one snapshot: a second one of the same frame replaces the first.
        history.snapshots.retain(|snapshot| snapshot.frame != frame);
        history.snapshots.push_back(Snapshot {
            frame,
            elapsed,
            scene,
        });
        while history.snapshots.len() > history.keep.max(1) {
            history.snapshots.pop_front();
        }
    }

    /// Steps the game back to the last snapshot taken at or before `frame`, pauses it there,
    /// and forgets the snapshots after it: from here the game is played again, perhaps by
    /// different code. Returns the moment gone back to, or `None` if no snapshot is that old.
    pub fn rewind_to(world: &mut World, frame: u64) -> Option<Moment> {
        let history = world.get_resource_mut::<History>()?;
        let index = history
            .snapshots
            .iter()
            .rposition(|snapshot| snapshot.frame <= frame)?;
        history.snapshots.truncate(index + 1);
        let (frame, elapsed) = (
            history.snapshots[index].frame,
            history.snapshots[index].elapsed,
        );
        let scene = history.snapshots[index].scene.clone();
        let restored = world
            .resource_scope(|world, registry: &mut TypeRegistry| scene.restore(world, registry));
        for problem in &restored.skipped {
            log::warn!("stepping back: {problem}");
        }
        if let Some(time) = world.get_resource_mut::<Time>() {
            time.rewind_to(elapsed, frame);
        }
        if let Some(live) = world.get_resource_mut::<Live>() {
            live.pause();
        }
        log::info!("stepped back to frame {frame}");
        Some(Moment {
            frame,
            seconds: elapsed.as_secs_f64(),
        })
    }

    /// Steps back by about this many frames. See [`History::rewind_to`].
    pub fn rewind(world: &mut World, frames: u64) -> Option<Moment> {
        let now = world.get_resource::<Time>().map_or(0, Time::frame_count);
        Self::rewind_to(world, now.saturating_sub(frames))
    }
}

/// Takes a snapshot every so often while recording. Runs last in a frame, so a snapshot is
/// the world as that frame left it.
pub(crate) fn record_history(world: &mut World) {
    let Some(history) = world.get_resource::<History>() else {
        return;
    };
    if !history.recording {
        return;
    }
    let frame = world.get_resource::<Time>().map_or(0, Time::frame_count);
    let due = history
        .snapshots
        .back()
        .is_none_or(|last| frame >= last.frame + history.every.max(1));
    // A paused game is not a new moment.
    let held = world
        .get_resource::<Live>()
        .is_some_and(|live| live.holding);
    if due && !held {
        History::snapshot(world);
    }
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
        let started = std::time::Instant::now();
        time.tick();
        std::thread::sleep(Duration::from_millis(40));
        time.tick();
        // Measured around the ticks, so a slow machine can't make this fail.
        let real = started.elapsed();
        let delta = time.delta();
        assert!(
            delta >= Duration::from_millis(10) && delta <= real.mul_f32(0.26),
            "{delta:?} of {real:?}"
        );
        assert_eq!(time.elapsed(), delta);
    }
    #[test]
    fn the_game_can_be_stepped_back_and_played_again() {
        use crate::{
            ecs::{Component, Entity, Query},
            reflect::Reflect,
            transform::{Parent, Transform, TransformPlugin},
        };

        #[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
        #[reflect(name = "test.Speed")]
        struct Speed(f32);
        #[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
        #[reflect(name = "test.Tired")]
        struct Tired;

        let mut app = App::new();
        app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
        app.register_type::<Speed>().register_type::<Tired>();
        app.world
            .resource_mut::<Time>()
            .set_fixed_step(Some(Duration::from_millis(100)));
        app.add_systems(
            Stage::Update,
            |mut movers: Query<(&Speed, &mut Transform)>| {
                for (speed, mut at) in &mut movers {
                    at.translation.x += speed.0;
                }
            },
        );
        let history = app.world.resource_mut::<History>();
        history.recording = true;
        history.every = 2;

        let runner = app.world.spawn((Transform::IDENTITY, Speed(1.0)));
        let rider = app
            .world
            .spawn((Transform::from_xyz(0.0, 1.0, 0.0), Parent(runner)));
        let doomed = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 9.0), Speed(2.0)));
        let follower = app.world.spawn((Transform::IDENTITY, Parent(doomed)));
        for _ in 0..4 {
            app.update();
        }
        let x =
            |app: &App, entity: Entity| app.world.get::<Transform>(entity).unwrap().translation.x;
        assert_eq!((x(&app, runner), x(&app, doomed)), (4.0, 8.0));

        // Then things happen: something is made, something dies, something changes shape.
        let late = app.world.spawn((Transform::IDENTITY, Speed(100.0)));
        app.world.spawn((Transform::IDENTITY, Parent(late)));
        app.world.despawn(doomed);
        app.world.insert(runner, Tired);
        app.world.remove::<Parent>(rider);
        app.world.get_mut::<Speed>(runner).unwrap().0 = 50.0;
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(x(&app, runner), 254.0);
        let moments = app.world.resource::<History>().moments();
        assert_eq!(
            moments.iter().map(|m| m.frame).collect::<Vec<_>>(),
            [1, 3, 5, 7, 9]
        );

        // Step back to before any of it. The nearest snapshot at or before frame 4 is 3.
        let moment = History::rewind_to(&mut app.world, 4).unwrap();
        assert_eq!((moment.frame, moment.seconds), (3, 0.3));
        assert!(app.world.resource::<Live>().is_paused());
        assert_eq!(app.world.resource::<Time>().frame_count(), 3);
        // Who was alive is where they were, under the same ids.
        assert_eq!(x(&app, runner), 3.0);
        assert_eq!(app.world.get::<Speed>(runner), Some(&Speed(1.0)));
        assert!(
            !app.world.has::<Tired>(runner),
            "what it has gained since is gone"
        );
        assert_eq!(app.world.get::<Parent>(rider), Some(&Parent(runner)));
        // Who was made since is gone, with what hung off it.
        assert!(!app.world.contains_entity(late));
        // Who had died is back, under a new id, and what pointed at it points at it again.
        assert!(!app.world.contains_entity(doomed));
        let Parent(risen) = *app.world.get::<Parent>(follower).unwrap();
        assert_ne!(risen, doomed);
        assert_eq!(x(&app, risen), 6.0);
        assert_eq!(app.world.entity_count(), 4);
        assert_eq!(
            app.world.resource::<History>().moments().len(),
            2,
            "the future is forgotten"
        );

        // Held still until told otherwise; then it plays forward again, differently.
        app.update();
        assert_eq!(x(&app, runner), 3.0);
        app.world.resource_mut::<Live>().resume();
        for _ in 0..2 {
            app.update();
        }
        assert_eq!((x(&app, runner), x(&app, risen)), (5.0, 10.0));
        assert_eq!(app.world.resource::<Time>().frame_count(), 5);
        assert!(
            History::rewind_to(&mut app.world, 0).is_none(),
            "nothing is that old"
        );
        assert_eq!(History::rewind(&mut app.world, 1).unwrap().frame, 3);

        // Not recording, nothing is kept.
        let mut quiet = App::new();
        quiet.add_plugins(TimePlugin);
        quiet.update();
        assert!(quiet.world.resource::<History>().moments().is_empty());
    }
}
