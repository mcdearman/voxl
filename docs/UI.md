# User interface in the game

mira's in-game interface is drawn with [Armature](https://github.com/mcdearman/armature), a GUI
framework with no look of its own: it lays out and draws a tree of widgets, and what they look
like is up to whoever uses it. The crate `mira_ui` is the thin layer between the two.

```rust
app.add_plugins(UiPlugin::new(Hud::default).sync(|world, hud| {
    hud.health = world.resource::<Player>().health;   // game state into the interface
}));
```

An interface is an Armature `App`: a value holding what it shows, a `view` that turns it into
widgets, and an `update` for the messages its widgets send. `mira_ui` does the rest:

- **Input.** The frame's window events are handed to the interface first. What it acts on (a
  click on a panel, a key it has a use for) is taken away from the game's input, so a click
  on a button doesn't also fire the gun behind it.
- **Drawing.** The interface is drawn over the finished frame, after tone mapping, in display
  colours. It is part of what a screenshot shows.
- **State.** `sync` runs every frame with the world and the interface, to copy in what it
  shows. The interface itself lives in the world as `UiHost<A>` and stays on the main thread.

## The signal graph, in the game

```sh
cargo run --manifest-path crates/mira_ui/Cargo.toml --example sacred_sites_graph
```

`mira_ui::signal_graph::plugin()` adds a panel showing the game's [signals](SIGNALS.md) live:
a lamp for each, lit while it is true, with what feeds what drawn as a tree; a signal that
changed this frame is underlined, one being forced has a ring. F1 hides it. Change the graph
from outside (`mira-debug signal_force …`) and the panel follows.

## Building it

`mira_ui` is not a member of mira's workspace, and mira's CI doesn't build it: Armature is a
private repository, expected beside mira's (`../armature`). Build it by its own manifest, as
above. Everything it needs from the engine is in the engine and usable by any other
interface layer:

- `Overlays`: functions that draw over the finished frame, given the world and the frame's
  view;
- `WindowEvents`: the frame's window events as the windowing library gave them;
- `ButtonInput::consume`: take a press away from the game.

## What isn't here yet

- A look for games: `mira_ui` has one hand-drawn panel. Buttons, text fields and the rest come
  from Armature's controls, which need a style.
- Interface in the world (a health bar over a unit), gamepad focus, and laying out for
  different screen sizes.
- Editing the signal graph from the panel; it shows, and the changing is done from outside.
- Armature's glass (backdrop blur) can't see the game behind it, so it is left off.
