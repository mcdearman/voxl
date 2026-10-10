//! The sacred-site game inside the engine app.
//!
//! ```sh
//! cargo run --example sacred_sites_editor
//! ```

#[path = "../sacred_sites/game.rs"]
mod game;

fn main() -> anyhow::Result<()> {
    let game = game::build(false)?;
    mira::editor::run(game).map_err(|err| anyhow::anyhow!("{err}"))
}
