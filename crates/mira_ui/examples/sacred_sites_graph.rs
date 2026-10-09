//! The sacred-site game with an interface in it: a small panel of controls made from the kit,
//! and the game's rules shown as a circuit that can be rewired while it runs.
//!
//! ```sh
//! cargo run -p mira_ui --example sacred_sites_graph
//! ```
//!
//! Bottom right, the round: the clock as a bar, a slider for how long red must hold out, and
//! buttons to pause and to start again. Top left, the signal graph (F1, or the switch, hides
//! it): drag its ports to rewire the rules. Both are ordinary Armature interfaces; see
//! `docs/UI.md`.

use mira::{live::Live, prelude::*};
use mira_ui::{
    armature::{App as Interface, Element, Scheme, Style},
    kit::{
        anchored, bar, button, caption, column, heading, panel, row, slider, toggle, Anchor, Theme,
    },
    signal_graph::SignalGraph,
    UiHost, UiPlugin,
};

#[path = "../../../examples/sacred_sites/game.rs"]
mod game;

/// The round's controls. What it shows is copied in from the game every frame; what the
/// player does with it is kept as requests for the game to carry out.
#[derive(Default)]
struct Round {
    clock: f32,
    limit: f32,
    contested: bool,
    won: bool,
    paused: bool,
    rules_shown: bool,
    asked: Vec<Ask>,
}

#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Limit(f32),
    Pause(bool),
    Rules(bool),
    Restart,
}

impl Interface for Round {
    type Message = Ask;

    fn update(&mut self, ask: Ask) {
        self.asked.push(ask);
    }

    fn style(&self, _: Scheme) -> Style {
        Theme::default().style()
    }

    fn view(&self) -> Element<Ask> {
        let state = if self.won {
            "Red has won"
        } else if self.contested {
            "Clock stopped: a scout is on a site"
        } else {
            "Clock running"
        };
        anchored(
            Anchor::BottomRight,
            panel(
                column()
                    .spacing(10.0)
                    .push(heading("Sacred sites"))
                    .push(caption(state))
                    .push(bar(self.clock / self.limit.max(1.0)))
                    .push(caption(format!(
                        "{:.0} of {:.0} seconds",
                        self.clock, self.limit
                    )))
                    .push(slider(10.0..=120.0, self.limit, Ask::Limit))
                    .push(toggle("Paused", self.paused, Ask::Pause))
                    .push(toggle("Show the rules", self.rules_shown, Ask::Rules))
                    .push(row().spacing(8.0).push(button("New round", Ask::Restart))),
            ),
        )
    }
}

/// Game state into the panel.
fn show(world: &World, round: &mut Round) {
    let signals = world.resource::<Signals>();
    round.clock = signals.number("red.clock") as f32;
    round.limit = signals.number("win_after") as f32;
    round.contested = signals.is_true("blue.contesting");
    round.won = signals.is_true("red.wins");
    round.paused = world.resource::<Live>().is_paused();
}

/// What the player asked for, done to the game.
fn carry_out(world: &mut World) {
    let Some(host) = world.get_resource_mut::<UiHost<Round>>() else {
        return;
    };
    let asked = std::mem::take(&mut host.app().asked);
    // "New round" holds the clock at nothing for one frame.
    world.resource_mut::<Signals>().set("restart", false);
    for ask in asked {
        match ask {
            Ask::Limit(seconds) => world
                .resource_mut::<Signals>()
                .set("win_after", seconds.round() as f64),
            Ask::Pause(true) => world.resource_mut::<Live>().pause(),
            Ask::Pause(false) => world.resource_mut::<Live>().resume(),
            Ask::Restart => world.resource_mut::<Signals>().set("restart", true),
            Ask::Rules(shown) => {
                if let Some(graph) = world.get_resource_mut::<UiHost<SignalGraph>>() {
                    graph.app().set_hidden(!shown);
                }
            }
        }
    }
    // The switch follows the panel, which F1 also hides.
    let shown = world
        .get_resource_mut::<UiHost<SignalGraph>>()
        .is_some_and(|graph| !graph.app().is_hidden());
    if let Some(host) = world.get_resource_mut::<UiHost<Round>>() {
        host.app().rules_shown = shown;
    }
}

fn main() -> anyhow::Result<()> {
    let mut app = game::build(false)?;
    app.add_plugins(mira_ui::signal_graph::plugin())
        .add_plugins(UiPlugin::new(Round::default).sync(show))
        .add_systems(Stage::First, carry_out);
    app.run()
}
