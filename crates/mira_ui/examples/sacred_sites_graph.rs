//! The sacred-site game with its signal graph shown inside it.
//!
//! ```sh
//! cargo run --manifest-path crates/mira_ui/Cargo.toml --example sacred_sites_graph
//! ```
//!
//! The panel at the top left is the game's rules, live: a lamp for each signal, lit while it
//! is true, what feeds what drawn as a tree. F1 hides it. It can still be watched and changed
//! from outside as well (`cargo run --bin mira-debug -- watch`).

#[path = "../../../examples/sacred_sites/game.rs"]
mod game;

fn main() -> anyhow::Result<()> {
    let mut app = game::build(false)?;
    app.add_plugins(mira_ui::signal_graph::plugin());
    app.run()
}
