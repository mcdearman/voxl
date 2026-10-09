//! The sacred sites of Age of Empires 4, as signals: a small game seen from above through an
//! orthographic camera, to watch from outside while it runs.
//!
//! ```sh
//! cargo run --example sacred_sites              # the game; listens on 127.0.0.1:7878
//! cargo run --example sacred_sites -- --headless   # the same game with no window
//! cargo run --bin mira-debug -- watch           # in another terminal: the signal graph, live
//! cargo run --bin mira-debug -- signal_set name=win_after value=20
//! cargo run --bin mira-debug -- signal_force name=blue.contesting value=false
//! ```
//!
//! Red holds both sites, and wins when its clock reaches `win_after`. The clock stops while a
//! blue unit stands on any site red holds. Two blue scouts wander on and off the sites; no
//! code ever sets the clock or a paused flag. See `docs/SIGNALS.md`.
//!
//! On screen: the sites are slabs, red while red's clock runs and amber while a scout is on
//! one; the bar along the bottom is the clock, filling toward `win_after`. All of it is drawn
//! from the signals, so what you force or rewire from outside shows at once.

use std::time::Duration;

mod game;

fn main() -> anyhow::Result<()> {
    let headless = std::env::args().any(|arg| arg == "--headless");
    let mut app = game::build(headless)?;
    if !headless {
        return app.run();
    }
    // Until told to stop (`mira-debug quit`), since there is no window to close.
    while !app.should_exit() {
        app.update();
        std::thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}
