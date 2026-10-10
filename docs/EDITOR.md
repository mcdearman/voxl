# The engine app

The editor and engine app (roadmap Phase 5) is built on [Neo](https://github.com/mcdearman/neo),
with the game shown inside a Neo window. This page records what has been agreed with the Neo
project and what exists on each side.

## What there is

`cargo run` opens it. It is the engine's own program (`src/main.rs`, on `mira::editor`,
which is `src/editor`): a Neo window with the game in a viewport, under a bar to pause, resume
and step it. Beside it: the game's entities as a tree (children under their `Parent`; drag
one onto another to make it its child, or beside one to share its parent), an inspector
in which the chosen entity's parts are fields to change (numbers and vectors drag or take a
typed value, true-or-false is a switch, a colour opens a picker, and a field that names an
entity takes one dragged onto it from the tree (a click on it goes to the entity it names);
text is typed; asset references are shown but not
yet changed there), each change written straight to the running game,
and the game's signals with their values. All are read from the game a few times a second.
They are panels of a dock:
drag a tab to move a panel, onto another to stack them or to an edge to split; drag the bar
between two to resize. The arrangement is kept in `.mira/editor.layout` in the folder the app
is run from. The game is any mira `App`, built as it would be to run alone:

```rust
mira::editor::run(my_game::build()?)?;
```

```sh
cargo run                                  # a small scene to start from
cargo run -- chase scoreboard              # a game made of plugins (built into target/, or paths)
cargo run --example sacred_sites_editor    # a game written in Rust, opened in the app
```

What is done in the viewport is the game's: pointer, wheel and keys, in the game's own
terms. See "Working in the picture" below for what the pointer does there. The bar follows the game, so a game paused from outside, by an agent say, shows as
paused. Everything else on this page is still to come.

### How it looks

The app is a Neo app and looks like one: it follows the appearance set for the Neo desktop
(light or dark, the accent, corners), and its window is glass, translucent with what is
behind it blurred. Glass is the app's default; **Settings** (the gear at the right of the
tool bar, or Cmd/Ctrl+comma) has the switch to make the window solid, and the choice is
kept. The tool bar is the one part that is mira's own and not Neo's standard buttons: flat
icons set side by side with no room between them, a little in from the window's edge, a
thin line between one group
and the next (run or pause, step; undo, redo, save; what the pointer does), the frame count and the
settings gear at the right, and one line under the bar to part it from the panels. The window opens at 1440 by 900.

### Working in the picture

Three buttons in the tool bar say what the pointer does in the picture of the game:

- **Move** (the arrow, the one it starts with): a press chooses the entity under the
  pointer, the same as choosing it in the tree, and frames it. Dragging slides it over
  level ground at the height it was taken hold of, keeping its own height; a child goes
  where the pointer is whatever its parent is doing. One drag is one change to take back.
  The game hears none of this.
- **Play** (the gamepad): the picture is the game's, as in a window of its own: clicks and
  keys go to it.
- **Look** (the mouse): the game's, with the pointer held in the picture and hidden, for
  games that turn with the mouse. Escape lets it go.

What is under the pointer is found from each entity's mesh, as a box round it; an entity
with no mesh (a light) is a small box where it stands. The camera the game is seen through
can't be picked.

The chosen entity also has three handles, drawn from its middle: red runs east, green up,
blue south. Dragging one by its outer part moves the entity that way and no other, which is
how a thing is lifted or lowered. They stay the same length in the picture however far off
the entity is. Turning and sizing in the picture are not there yet: use the inspector's
fields.

### Changing things, and keeping them

- Every change made from the app (a field, a parent, a name) can be taken back and made
  again: the Undo and Redo buttons, or Cmd/Ctrl+Z and Shift+Cmd/Ctrl+Z. One drag on a
  field is one change, however far it went, and so is one run of typing.
- **Save** (or Cmd/Ctrl+S) writes the scene: every entity with its registered components,
  and the registered settings. It goes to the file the app was opened on, or `scene.json`.
  `cargo run -- level.json` opens the app on a saved scene; if the file isn't there yet, it
  is where Save will write.
- Double-click a row of the tree (or Enter, or F2) to name the entity. A name is a
  component, `Name`, like any other; an empty one takes it away.

What is not taken back by Undo: what the game itself did meanwhile, and what an agent
changed through its own tools.

### The agent

The Agent panel is a conversation with an AI agent that is working on the game in the
window: write to it, Enter sends, and its answer appears as it is written, as Markdown.
Each use of a tool is a row of its own that opens to show what the tool was given and what
came back, with the picture if it was a screenshot. Stop stops it.

The agent is a program of its own that the window runs, behind a small interface
(`mira::editor::agent::Agent`: ask, stop), so another can be put there with
`Editor::with_agent`. The one the app starts with is Claude Code (`agent::ClaudeCode`), run
once for each thing asked and resuming the same conversation. It is found at `MIRA_AGENT`,
else `~/.local/bin/claude`, else `claude` on the path. It is given mira's own tools
(`mira_*`, served by `mira-mcp`: found at `MIRA_MCP`, else beside the app) attached to the
game in the window, so it sees the frame, reads every entity and signal, and can pause,
step and change the game, the same as an agent outside ([MCP.md](MCP.md)). It may also read
the project's files.

What it can't do yet: anything that needs a yes from you, such as changing a file or
running a command, is refused: Neo now has the row for asking, but how Claude Code hands
such a question to the program that runs it is not written down for its command line, so
that part waits on finding out.

The reading of Claude Code's output was written from its documentation and tested against
a stand-in program that prints the same lines; it has not yet been run against Claude Code
itself.

The app is tested as a person works it: `tests/editor.rs` opens it in
Neo's test window, which is drawn but never shown, and clicks its buttons and tree rows.
It needs a graphics card, so it runs when asked:

```sh
MIRA_FRAME_TESTS=1 cargo test --test editor
MIRA_FRAME_TESTS=1 MIRA_EDITOR_SHOT=window.png cargo test --test editor   # and a picture
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

## Panels to come

What other engines' editors have (Unreal, Unity, Godot), sorted by how near mira is to
having each. "Ready" means the engine already holds the data and only the panel is missing.

**Ready: the engine has the data**

| Panel | In other engines | What it would show here |
|---|---|---|
| Log | Output Log, Console | The game's log lines, filtered by level and by text; plugin reloads and failures as they happen |
| Console | Cmd, Console | A line to type any debug command (`pause`, `entities with=…`, `signal_force …`) with its answer |
| Profiler | Unreal Insights, Unity Profiler | Frame time as a graph; time per stage and per system (`FrameStats`), sorted |
| Systems | (Unity's Systems window) | Every system by stage, in the order and batches it runs, with what it reads and writes |
| Signal graph | Blueprint-like graphs | mira's circuit of rules, already built for in-game use, as a dock panel: rewire, force, rename |
| World settings | World Settings, Lighting, Project Settings | The registered resources (post-processing, shadows, fog, ambient light, voxel settings) as an inspector |
| History | Godot's History dock, Undo History | The list of changes made, to step back and forward through |
| Time | (Rewind in some debuggers) | Record, a scrubber over the kept moments, rewind; time scale; run until a signal |
| Failures | Message Log | Systems that panicked: message, where, stack; resume |
| Plugins | Plugins browser | Loaded plugins, their systems and components, reload now |
| Statistics | Stat overlays, Statistics | Entity and component counts, draw counts, memory |

**Needs some engine work first**

| Panel | In other engines | What is missing |
|---|---|---|
| Assets | Content Browser, Project, FileSystem | Thumbnails; a listing of files on disk and not only what is loaded; dragging into the scene |
| Place | Place Actors, Create menu | A palette of shapes, lights, cameras and prefabs to drag in; needs spawn and delete from the app |
| Prefab editor | Blueprint/Prefab mode | Opening a prefab by itself, its overrides shown apart |
| Scene camera | Editor viewport camera, view modes | A camera of the app's own to fly about with, apart from the game's; wireframe, unlit, overdraw views |
| Game view | Unity's Game beside Scene | A second viewport: the game as the player sees it beside the scene as the editor does |
| Material editor | Material Editor, Shader Graph | A material system with graphs (Phase 3) |
| Voxel tools | Landscape, Foliage, Modeling modes; Tile Palette | Brushes for sculpting and painting blocks; a block palette |
| Agent tasks | (none) | What the agent is doing as a list of steps with approvals; several agents at once |

**Further off: whole systems to build**

| Panel | In other engines | Depends on |
|---|---|---|
| Sequencer / Timeline | Sequencer, Timeline | Animation of any property over time; cinematics |
| Animation | Persona, Animator, AnimationTree | Skeletal animation loaded from files, an animation graph (Phase 4) |
| Curve editor | Curve Editor | Keyed curves |
| Particles | Niagara, VFX Graph | GPU particles (Phase 3) |
| Audio mixer | Audio Mixer, Audio buses | Audio (Phase 4) |
| Navigation | Navigation, NavMesh | Navmesh and pathfinding (Phase 4) |
| Behaviour tree | Behavior Tree editor | AI (Phase 4) |
| Game UI designer | UMG, UI Builder | A visual layout for `mira_ui` interfaces |
| Physics debugger | Physics Debugger, Collision view | Drawing colliders, contacts and joints over the scene |
| Lighting | Lightmass, Light Mixer | Baked or dynamic global illumination (Phase 6) |
| Reference viewer, size map | Reference Viewer, Size Map | Dependencies between assets |
| Build | Project Launcher, Build Settings | Packaging a game for a platform |
| Source control | Revision Control | Git status and diffs for scenes and assets |
| Tests | Session Frontend, Test Runner | Running the game's tests and the frame comparisons from the app |

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
4. **Dragging between panels** (done, Neo `a0db824`).
5. **A transcript** for the agent window (done, Neo `a0db824`): streamed Markdown, rows that fold for what the
   agent did, pictures inline, an input that grows and sends on Enter.

mira's signal graph stays the Armature widget it is; such a widget goes into a Neo app
unchanged. Undo is the app's own to keep.

## Limits to expect at first

One window. Frames paced by the display. No HDR output. The device opened with defaults
unless the app asks for more, so buffers are limited to 256 MB until mira asks for the
adapter's limit as it does in its own window.
