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
cargo run -p mira_ui --example sacred_sites_graph
```

`mira_ui::signal_graph::plugin()` adds a panel that shows the game's [signals](SIGNALS.md) as
a circuit and lets you rewire it while the game runs. Each signal is a box, with its inputs
down the left edge and its output on the right; sources are on the left of the panel and
what is made from them to the right. A wire joins every input to the signal it reads, and is
lit while that signal is true.

- **Rewire:** drag from an input onto another box, and the input reads that signal instead.
- **Force:** click a box's lamp to hold the signal true; again for false; again to let it go.
  A forced signal's lamp has a gold ring.
- **Settings:** click a true-or-false constant's value to flip it; scroll over a number to
  turn it up or down.
- **Arrange:** drag a box by its body, or the panel by its background. F1 hides the panel.

Every edit is made to the running game at once: force `red.wins` and the game announces it.
A signal that changed this frame has a gold outline; an input wired to a signal that doesn't
exist is drawn red. Changes made from outside (`mira-debug signal_connect …`, an agent's
`mira_signal_define`) show in the panel as they happen.

## What the engine provides

`mira_ui` is a crate of mira's workspace that depends on Armature by its repository and
revision. Everything it needs from the engine is in the engine and usable by any other
interface layer:

- `Overlays`: functions that draw over the finished frame, given the world and the frame's
  view;
- `WindowEvents`: the frame's window events as the windowing library gave them;
- `ButtonInput::consume`: take a press away from the game.

Input played from outside (`mira-debug input`, an agent's `mira_input`) reaches the interface
too: a pointer position and a button press are put among the window's events, so a click an
agent makes lands on a panel as a person's would.

## What isn't here yet

- A look for games: `mira_ui` has one hand-drawn panel. Buttons, text fields and the rest come
  from Armature's controls, which need a style.
- Interface in the world (a health bar over a unit), gamepad focus, and laying out for
  different screen sizes.
- In the signal panel: adding and removing signals, changing a box's operation, typing a
  number, and remembering where boxes were put. The layout is by column only, so wires cross.
- Armature's glass (backdrop blur) can't see the game behind it, so it is left off.
