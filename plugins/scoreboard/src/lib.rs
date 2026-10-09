//! An example mira plugin in Rust that knows nothing about any game except the name of one
//! event: it shows the latest `chase.Collected` in the window title.
//!
//! The event is sent by `plugins/chase`, which is written in Haskell. Neither plugin refers
//! to the other; they agree only on the event's name and that it is one 32-bit integer.

use std::sync::OnceLock;

use mira_plugin::{App, Error, Event, Stage, System};

static COLLECTED: OnceLock<Event<i32>> = OnceLock::new();

fn load(app: &mut App) -> Result<(), Error> {
    let _ = COLLECTED.set(app.event::<i32>("chase.Collected")?);
    app.add_system("show", Stage::Update, &[], show)
}

fn show(system: &mut System) {
    let collected = *COLLECTED.get().expect("systems only run after `load`");
    let mut latest = None;
    while let Some(total) = system.next_event(collected) {
        latest = Some(total);
    }
    if let Some(total) = latest {
        system.set_window_title(&format!("mira: {total} collected"));
        mira_plugin::info!("{total} collected");
    }
}

mira_plugin::export_plugin!(load);
