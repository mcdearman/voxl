use std::time::{Duration, Instant};

use crate::app::{App, Plugin};

/// Frame timing. Updated at the very start of every frame.
pub struct Time {
    last: Option<Instant>,
    delta: Duration,
    elapsed: Duration,
    frame: u64,
    /// When set, every frame lasts exactly this long, whatever the clock says.
    step: Option<Duration>,
    /// How fast game time runs against the clock: 1 is real time.
    scale: f32,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            last: None,
            delta: Duration::ZERO,
            elapsed: Duration::ZERO,
            frame: 0,
            step: None,
            scale: 1.0,
        }
    }
}

impl Time {
    /// Makes every frame last exactly `step` instead of following the clock (or `None` to
    /// follow it again): for tests that must not depend on how fast they run, and for
    /// rendering offline at a fixed rate.
    pub fn set_fixed_step(&mut self, step: Option<Duration>) {
        self.step = step;
        self.last = None;
    }

    pub(crate) fn tick(&mut self) {
        if let Some(step) = self.step {
            self.advance_by(step);
            return;
        }
        let now = Instant::now();
        // The first frame reports a zero delta so slow startup work doesn't cause a huge jump.
        self.delta = self
            .last
            .map_or(Duration::ZERO, |last| (now - last).mul_f32(self.scale));
        self.last = Some(now);
        self.elapsed += self.delta;
        self.frame += 1;
    }

    /// A frame in which no game time passes: the game is paused.
    pub(crate) fn hold(&mut self) {
        self.delta = Duration::ZERO;
        self.last = Some(Instant::now());
    }

    /// Slows game time down or speeds it up: 0.25 is quarter speed. Frames forced to a fixed
    /// step are not scaled.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.0);
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Steps time by a set amount instead of by the clock: for tests, and for rendering
    /// frames offline at a fixed rate.
    pub fn advance_by(&mut self, delta: Duration) {
        self.delta = delta;
        self.elapsed += delta;
        self.frame += 1;
    }

    pub fn delta(&self) -> Duration {
        self.delta
    }

    pub fn delta_secs(&self) -> f32 {
        self.delta.as_secs_f32()
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }

    pub fn frame_count(&self) -> u64 {
        self.frame
    }
}

/// Controls the `FixedUpdate` stage.
pub struct FixedTime {
    pub timestep: Duration,
    /// Upper bound on steps per frame, so a long hitch doesn't turn into a spiral of catch-up work.
    pub max_steps: u32,
    accumulator: Duration,
}

impl Default for FixedTime {
    fn default() -> Self {
        Self::from_hz(60.0)
    }
}

impl FixedTime {
    pub fn from_hz(hz: f64) -> Self {
        Self {
            timestep: Duration::from_secs_f64(1.0 / hz),
            max_steps: 8,
            accumulator: Duration::ZERO,
        }
    }

    pub fn timestep_secs(&self) -> f32 {
        self.timestep.as_secs_f32()
    }

    /// How far we are into the next step, in `0..1`. Useful for interpolating rendered positions.
    pub fn overstep_fraction(&self) -> f32 {
        self.accumulator.as_secs_f32() / self.timestep_secs()
    }

    pub(crate) fn accumulate(&mut self, delta: Duration) -> u32 {
        self.accumulator += delta;
        let mut steps = 0;
        while self.accumulator >= self.timestep {
            self.accumulator -= self.timestep;
            steps += 1;
            if steps == self.max_steps {
                self.accumulator = Duration::ZERO;
                break;
            }
        }
        steps
    }
}

pub struct TimePlugin;

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Time>().init_resource::<FixedTime>();
    }
}
