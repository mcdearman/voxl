use std::time::{Duration, Instant};

use crate::app::{App, Plugin};

/// Frame timing. Updated at the very start of every frame.
pub struct Time {
    startup: Instant,
    last: Option<Instant>,
    delta: Duration,
    elapsed: Duration,
    frame: u64,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            startup: Instant::now(),
            last: None,
            delta: Duration::ZERO,
            elapsed: Duration::ZERO,
            frame: 0,
        }
    }
}

impl Time {
    pub(crate) fn tick(&mut self) {
        let now = Instant::now();
        // The first frame reports a zero delta so slow startup work doesn't cause a huge jump.
        self.delta = self.last.map_or(Duration::ZERO, |last| now - last);
        self.last = Some(now);
        self.elapsed = now - self.startup;
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
