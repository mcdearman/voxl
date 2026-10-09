# The engine app

The editor and engine app (roadmap Phase 5) is built on [Neo](https://github.com/mcdearman/neo),
with the game shown inside a Neo window. This page records what has been agreed with the Neo
project and what exists on each side.

## What there is

`crates/mira_editor`: a Neo window with the game in a viewport, under a bar to pause, resume
and step it. Beside it: the game's entities as a tree (children under their `Parent`; drag
one onto another to make it its child, or beside one to share its parent), an inspector
in which the chosen entity's parts are fields to change (numbers and vectors drag or take a
typed value, true-or-false is a switch, a colour opens a picker; text, entity and asset
references are shown but not
yet changed there), each change written straight to the running game,
and the game's signals with their values. All are read from the game a few times a second.
They are panels of a dock:
drag a tab to move a panel, onto another to stack them or to an edge to split; drag the bar
between two to resize. The arrangement is kept in `.mira/editor.layout` in the folder the app
is run from. The game is any mira `App`, built as it would be to run alone:

```rust
mira_editor::run(my_game::build()?)?;
```

```sh
cargo run -p mira_editor --example sacred_sites_editor
```

What is done in the viewport is the game's: pointer, wheel and keys, in the game's own
terms. "Mouselook" holds and hides the pointer for games that turn with the mouse (Escape
lets go). The bar follows the game, so a game paused from outside, by an agent say, shows as
paused. Everything else on this page is still to come.

### The agent

The Agent panel is a conversation with an AI agent that is working on the game in the
window: write to it, Enter sends, and its answer appears as it is written, with a line for
each tool it uses. Stop stops it.

The agent is a program of its own that the window runs, behind a small interface
(`mira_editor::agent::Agent`: ask, stop), so another can be put there with
`Editor::with_agent`. The one the app starts with is Claude Code (`agent::ClaudeCode`), run
once for each thing asked and resuming the same conversation. It is found at `MIRA_AGENT`,
else `~/.local/bin/claude`, else `claude` on the path. It is given mira's own tools
(`mira_*`, served by `mira-mcp`: found at `MIRA_MCP`, else beside the app) attached to the
game in the window, so it sees the frame, reads every entity and signal, and can pause,
step and change the game, the same as an agent outside ([MCP.md](MCP.md)). It may also read
the project's files.

What it can't do yet: anything that needs a yes from you, such as changing a file or
running a command, is refused, because the window has nowhere yet to ask. Its answers are
shown as plain text. Both wait on the next Neo pieces.

The reading of Claude Code's output was written from its documentation and tested against
a stand-in program that prints the same lines; it has not yet been run against Claude Code
itself.

The app is tested as a person works it: `crates/mira_editor/tests/window.rs` opens it in
Neo's test window, which is drawn but never shown, and clicks its buttons and tree rows.
It needs a graphics card, so it runs when asked:

```sh
MIRA_FRAME_TESTS=1 cargo test -p mira_editor --test window
MIRA_FRAME_TESTS=1 MIRA_EDITOR_SHOT=window.png cargo test -p mira_editor --test window   # and a picture
```

## The game inside another program

A game normally opens its own window and runs its own loop (`App::run`). Inside an editor
neither is its own: the host opens the graphics device, owns the loop, and shows the game's
frames in a part of its interface. That much exists in mira now:

```rust
app.host(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, width, height);  // once
// then, each frame the host wants:
app.update();
let frame = mira::render::frame_texture(&app.world);   // to show
// and when the space given to the game changes:
app.host_resized(width, height);
```

- The frame is tone-mapped and opaque, and can be sampled. It is a new texture after a
  resize, so ask for it each frame.
- It is drawn in the sRGB format asked for. A host whose canvas holds sRGB-encoded bytes,
  as Armature's does, makes its view with `format.remove_srgb_suffix()` to get the bytes as
  stored; the texture allows that view.
- Input is the host's to pass on, through the input resources or `InjectedInput`.
- Ray tracing is off under a host: it depends on features the device was opened with.
- `examples/hosted_game` is a host with no window at all: it opens a device, runs the
  sacred-site game on it, changes its size and saves a frame.

## What Neo is to provide

Agreed in outline with the Neo session (October 2026); built there, not here.

1. **The viewport** (done, Neo `caaacc0`, Armature `9cd6e9d`). The shell hands the app its device and queue (`App::graphics`), and
   asks beforehand which optional features the app wants; an image made from a texture
   (`Image::from_texture`); a `viewport` widget that reports its bounds and scale, takes
   focus, forwards pointer and key input, captures the pointer for mouselook, and asks for
   a redraw every frame while playing; and a call once per presented frame, before drawing,
   in which mira steps and renders, so the picture shown is never a frame behind.
2. **Docking** (done, Neo `25c92b0`): a split tree, tabbed groups, the layout saved and restored.
3. **A tree** and **fields** (done, Neo `4548bd4`; expand, select, rename, drag to reparent) and **fields** for a property
   view: numbers that drag, vectors, entity references, a colour picker. The view itself is
   generated on mira's side from reflection.
4. **Dragging between panels.**
5. **A transcript** for the agent window (the plain one and its prompt are done, Neo
   `d6c11d1`; Markdown, folding rows, pictures and approve/refuse are next there): streamed Markdown, rows that fold for what the
   agent did, pictures inline, an input that grows and sends on Enter.

mira's signal graph stays the Armature widget it is; such a widget goes into a Neo app
unchanged. Undo is the app's own to keep.

## Limits to expect at first

One window. Frames paced by the display. No HDR output. The device opened with defaults
unless the app asks for more, so buffers are limited to 256 MB until mira asks for the
adapter's limit as it does in its own window.
