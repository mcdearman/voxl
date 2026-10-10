# mira roadmap

Where mira is going: a general-purpose engine in the class of Unreal, with voxel worlds as the
thing it does better than anyone. This is a long road. The plan is ordered so that every phase
leaves the engine more usable than before, and so that later phases stand on earlier ones
instead of being rewritten.

Phases are sized, not dated: **S** is days, **M** is weeks, **L** is months, **XL** is many
months. Sizes assume one person working with agents and are rough.

Each phase has an **exit test**: something concrete that works at the end and did not before.
A phase is not done until its exit test passes. Tick items off here as they land.

## Where we are (October 2026)

| Area | Have | Missing |
|---|---|---|
| Core | Hand-written sparse-set ECS, function systems, change detection, commands, events, startup access checks, fixed timestep with interpolation | Parallel scheduler, system ordering constraints, run conditions, states, reflection, derive macros |
| Data | glTF and image loading, typed handles | Serialization, scenes, prefabs, async loading, hot reload, processed assets, handle reference counts |
| Rendering | Forward HDR + MSAA + TAA, PBR, image-based lighting, cascaded sun shadows, bloom, light shafts, decals, GPU skinning, hand-authored LODs | GPU-driven drawing, occlusion culling, local lights, material system, particles as an engine feature, upscaling, compressed and streamed textures |
| Lighting | Ray-traced shadows/AO/reflections (Vulkan only), light probes baked once | Dynamic global illumination, anything ray-traced on Metal |
| Voxels | Chunked storage, threaded generation and meshing with ambient occlusion, streaming, raycasts, saving edits | Greedy meshing, chunk LOD, transparent blocks, voxel lighting |
| Simulation | Rigid bodies with sleeping, joints, character controller, fluids; shapes: sphere, box, capsule, plane, static triangle mesh | Convex hulls, continuous collision, ragdolls, vehicles, cloth, destruction |
| Animation | Clips, cross-fades, masks, two-bone IK, procedural gait | State machines, blend trees, retargeting, skins loaded from glTF, compression |
| Runtime | Keyboard and mouse | UI, text, audio, gamepad, input mapping, navigation, AI, networking, scripting, save games |
| Plugins | Native plugins over a C interface (`include/mira.h`) in any language, hot-reloaded with their data intact; Rust bindings | Most of the engine is not reachable from a plugin yet (see Phase 1A) |
| Tools | Screenshot capture, 62 tests, CI on Linux and macOS with clippy and Miri | Editor, profiler, image-diff tests |

## Principles

1. **Foundations before features.** Reflection comes before serialization, which comes before
   the editor. GPU-driven drawing comes before virtualized geometry.
2. **One renderer.** New techniques replace old paths rather than sitting beside them. The
   forward path is retired when the GPU-driven one matches it.
3. **Everything runs on Metal.** Hardware ray tracing stays an optional upgrade. Any feature
   that needs rays gets a software path first.
4. **Voxels are first-class.** Each rendering phase names what it does for voxel terrain, not
   only for meshes.
5. **Sound by construction.** The ECS's unsafe code stays behind declared access, and stays
   clean under Miri.

## Phase 0: Footing (M)

Make the project safe to change quickly.

- [x] The ECS as a crate of its own (`crates/mira_ecs`), which the engine re-exports as
      `mira::ecs`; it depended on nothing else, and Miri now builds only it
- [ ] The rest of the split: `mira_core`, `mira_render`, `mira_voxel`, `mira_physics`. These
      lean on each other (the app, reflection, assets and rendering all meet), so each needs
      its seams found first
- [x] CI on macOS and Linux: build, test, clippy, Miri on the ECS (written; not yet seen to
      pass on GitHub)
- [x] Image-diff tests: a fixed scene is stepped to a set frame, drawn, and compared with a
      stored frame (`tests/frames.rs`, `render::frame_diff`); run where there is a graphics
      card with `MIRA_FRAME_TESTS=1`
- [ ] Image-diff tests of the bigger demos (voxel world, Paris, Napoleonic), which load in
      the background and so are not yet at the same moment every run; run in CI (the runners
      have no graphics card set up)
- [x] CPU frame timings: per frame, per stage and per system, from the running game
      (`FrameStats`, `mira-debug profile`)
- [ ] A timeline of a frame across threads; GPU timings (timestamp queries per pass); shown
      on screen
- [ ] Benchmarks for ECS iteration, chunk meshing and a standard frame

**Exit test:** a change that breaks rendering or halves ECS speed fails CI.

## Phase 1A: Plugins and hot reload (L)

Pulled forward from Phase 9 at Chris's request (October 2026): hot reload and plugins are a
priority, and plugins may be written in any language that can speak a C interface. The
contract is `include/mira.h`; see [PLUGINS.md](PLUGINS.md).

- [x] Components defined at runtime by name, size and alignment, checked by the same access
      rules as Rust components
- [x] The C interface, version 1: define and find components, persistent state, systems with
      one query each, deferred spawn/despawn/insert/remove, time, logging
- [x] Loading shared libraries, with interface-version checking
- [x] Hot reload: a rebuilt plugin replaces itself in the running app, keeping component data,
      state and system order; a broken rebuild leaves the old version running
- [x] Rust bindings (`crates/mira_plugin`) and example plugins in Rust and C
- [x] Haskell as the default plugin language: typed bindings (`bindings/haskell`), an example
      plugin, and hot reload that survives garbage collection. Meadow is meant to take over
      this role when it is ready.
- [x] Input (keys, mouse), meshes (shapes and custom), mesh and material on an entity
- [x] More than one query per system, checked against each other like typed queries
- [x] A generic host (`examples/host`) and a whole small game as a Haskell plugin
      (`plugins/chase`)
- [x] Events between plugins and from the engine, with each reader's place kept across
      reloads; cameras, lights and the window title; colliders, bodies, impulses, raycasts and
      contact events. The example game now needs nothing from its host.
- [x] Plugins describe their components' fields, so scenes and inspectors handle them; plugins
      load images and spawn models by name
- [x] Hierarchy from plugins: `set_parent` and `despawn_tree`
- [ ] Reach more of the engine from a plugin: animation, joints and character controllers,
      voxel terrain, text and UI, sound
- [x] Ordering a plugin's systems (`system_order`); orthographic cameras
      (`set_camera_orthographic`)
- [ ] Optional terms and change filters in queries; hooks from plugins
- [ ] Removing a plugin at runtime, with its components and systems
- [ ] A generic script host for languages that can't build a shared library themselves
- [ ] Meadow bindings, once Meadow can export C functions
- [ ] Bindings generated from the header for at least one more compiled language (Zig)
- [x] Hot reload for shaders and textures (see [HOT_RELOAD.md](HOT_RELOAD.md))
- [ ] Hot reload for meshes, models and scenes (shared with Phase 1), and for the shaders that
      only run on Vulkan
- [ ] Hot reload for the engine's own Rust gameplay code through the same mechanism

**Exit test:** the Phase 4 sample game's rules live entirely in plugins, in two languages, and
can be rewritten while the game is being played.

## Phase 1B: A live program (L)

Added at Chris's request (October 2026), after Bret Victor's talks and Jack Rusher's "Stop
Writing Dead Programs": in debug builds the game is a program you change, question and repair
while it runs, not one you restart. Hot reload (Phase 1A) is half of this; the other half is
being able to see inside.

- [x] A failure stops the game where it is instead of ending it: a system that panics is
      caught, its stack and message kept, and the game paused with the world intact; fix the
      code (a plugin reloads itself), resume; see [LIVE.md](LIVE.md)
- [x] Time under control: pause, step frames, slow motion
- [x] Every system's name, stage, order, constraints and run times, from the running app
- [x] What each system reads and writes, from the running app
- [x] A debug connection (a local socket speaking JSON) to a running game: list and search
      entities, read and change any registered component or resource by name, spawn and
      despawn, save the scene, pause and step, see failures with their stacks, reload
      plugins, read and rewire the signal graph
- [x] A command-line client for it (`mira-debug`)
- [ ] The same protocol under the editor (Phase 5)
- [ ] Hot reload of the host's own Rust systems, not only plugins (the engine as a library the
      game reloads)
- [x] Rewind: snapshots of the world every few frames (reflection makes them); step back to
      one, change code or state, play forward again
- [ ] Snapshots of only what changed; scrubbing both ways; restoring what signals have timed
- [x] Failures across the plugin boundary: a plugin system that throws (Haskell), panics
      (Rust) or calls `system_fail` (C) pauses the game with its message, and reloading the
      fixed plugin resumes it
- [ ] Haskell call stacks with the exception; catching crashes in native plugin code

**Exit test:** make a plugin system divide by zero while the sample game runs; the game
freezes on that frame and shows the stack; fix the line, save, and play carries on from the
same frame without the window ever closing.

## Phase 1C: Agents at the controls (M)

Added at Chris's request (October 2026): an AI agent should be able to drive the engine for
development as fully as a person at an editor: see the scene, know everything about the
game's state, change it, and control time and code. The means is the Model Context Protocol
(MCP), on top of the debug connection of Phase 1B ([LIVE.md](LIVE.md)).

- [x] An MCP server (`mira-mcp`, JSON-RPC over stdio) that connects to a running game by its
      debug address, with a tool for every command of the debug connection: entities,
      components and resources by name, spawn and despawn, systems, failures with stacks,
      pause, step, rewind, time scale, signals, plugin reload, scene save; see
      [MCP.md](MCP.md)
- [x] Seeing the scene: a screenshot tool that returns the next frame as an image, at a
      chosen size
- [ ] Screenshots from a chosen camera or a free viewpoint, with debug overlays (entity ids,
      bounds, colliders, the signal graph); loading a scene
- [x] The scene in words: where the camera is and what there is, on screen first and nearest
      first (`describe`), for when an image is more than is needed
- [ ] What changed since last asked; sizes and what hides what
- [x] The schema of every registered type, so an agent knows what it may read and write and
      in what shape
- [x] A report of state that is not reflected, and so invisible (`unregistered`)
- [ ] Total knowledge: registering what that report lists in the engine itself (the
      renderer's and physics' resources, `GlobalTransform`, animation)
- [x] Events pushed, not polled, over the debug connection: failures, signal changes, pauses
      (`watch`, `mira-debug events`)
- [ ] The same events through MCP; log lines and plugin reloads as events
- [x] Running to a condition: step frames and wait for them, or run until a signal is true
- [x] Input from the agent: keys and mouse injected as if played, so an agent can play-test
      what it built
- [x] Launching and owning a game from MCP: start it by its command line, read its log, shut
      it down
- [x] Running with the window hidden (`MIRA_HIDDEN=1`), rendering off screen; a game an
      agent launches is hidden by default
- [ ] Rendering on a machine with no display; seeding; gamepad and text input
- [ ] Editing through the same door: write a prefab or a scene, define signals, build and
      reload a plugin, and see the result, without leaving the conversation
- [ ] The same tools from inside the editor (Phase 5), so a person and an agent can work on
      one running game

**Exit test:** an agent with only the MCP tools is asked to "make the red team win faster in
the sacred-site game and show me": it finds the rule in the signal graph, changes it, steps
the game until `red.wins`, and returns a screenshot of the moment.

## Phase 1: Data layer (L)

The layer Unreal's editor, saves, networking and Blueprints all stand on.

- [x] Proc-macro crate: `#[derive(Component)]`, `#[derive(Reflect)]`
- [x] Type registry: look up a type by name, list and edit its fields at runtime
- [x] Serialization of any reflected value; a text scene format (JSON); see
      [SCENES.md](SCENES.md)
- [x] Components defined by plugins are reflected once the plugin describes their fields
- [x] Prefabs: a scene by name, instanced under an entity, rebuilt when its file changes,
      with per-instance overrides; plugins can spawn them
- [x] Reflecting the physics components (bodies, colliders with every shape, joints,
      character controllers), `Lods`, `Interpolate`, `ChunkViewer`
- [x] Scenes capture registered resources (a level's fog and ambient light)
- [x] Saving edited voxel chunks: an edited chunk keeps its blocks when it streams out, and
      the edited chunks of a world save to one file and load back. [VOXELS.md](VOXELS.md)
- [ ] Saving skeletal animation state
- [x] Hierarchy as a real feature: `Children` kept from `Parent`, recursive despawn, and
      propagation that walks down from the roots (one multiply per entity at any depth)
- [x] Asset server: assets by name (files, parts of model files, shapes), images decoded on
      worker threads and reloaded when the file changes, scenes saving assets by name; see
      [ASSETS.md](ASSETS.md)
- [x] Unloading assets nothing refers to, by tracing reflected components (handles stay
      plain ids): `AssetServer::unload_unused`
- [ ] Model files loaded off the main thread and watched, dependencies between assets
- [ ] Asset processing: textures to BC7/ASTC with mips, meshes to a binary format, cached by
      content hash
- [ ] Load skins and animations from glTF

**Exit test:** save the Paris scene to a file and load it back identical; edit a texture on disk
and see it change in the running scene.

## Phase 2: Parallel core (L)

- [x] System ordering: before/after, system sets, chains, run conditions, application states
      with enter and exit systems; see [SCHEDULING.md](SCHEDULING.md)
- [x] Groundwork for running systems in parallel: components must be `Send + Sync` (they all
      were), systems and conditions report what they touch, and each stage is planned into
      batches of systems that could run at the same moment (shown by the debug connection)
- [x] Thread safety settled in the types: resources read through `Res`/`ResMut` must be
      `Send + Sync`, system state and commands `Send`; a resource can be pinned to the main
      thread (the window is), and each system is marked if it must run there (pinned
      resource, whole world, or a plugin's code)
- [x] The parallel executor: a pool of worker threads runs each batch, pinned systems stay on
      the main thread, commands are applied between batches in order; results identical to
      running in turn; clean under Miri with worker threads
- [ ] Splitting one query's work across threads; relaxing "commands end a batch" where a
      later system needn't see them; a ThreadSanitizer run in CI
- [x] A faster walk through the storage there is: a query guesses that an entity sits at the
      same place in each component it reads as in the one it walks, which is so for
      components spawned together. Measured with `examples/query_bench` (200,000 entities,
      one thread): one component 3.8 to 1.6 ns an entity; three components spawned together
      6.4 to 5.0; three added in different orders unchanged at 7.4
- [ ] Table storage as an option beside sparse sets, chosen per component. By the same
      measurement it would win back the gap between "spawned together" and "added in
      different orders" (about a third) and little else, for a rewrite of storage, queries
      and everything that reaches components by name; worth doing when a game shows that gap
- [x] Hooks: run code when a component is added or removed (`on_add`, `on_remove`)
- [ ] Observers for other events (a component changing, custom events aimed at an entity);
      hooks from plugins
- [x] Relations: any component that names another entity, with the way back kept by the
      engine (`Relation`, `Related<R>`); `Parent` and `Children` are one of them. See
      [RELATIONS.md](RELATIONS.md)
- [ ] Relations with many targets of one kind; cleaning up when a target is despawned;
      relations from plugins
- [ ] One job system for systems, asset loading and voxel work

**Exit test:** the Napoleonic demo's update time drops in proportion to cores used, with Miri
and a thread sanitizer clean.

## Phase 2B: Signals (M)

Added at Chris's request (October 2026). Game rules written as handlers that set and clear
shared flags get out of step with each other. His example is a bug in Age of Empires 4:
standing on a sacred site pauses the win timer; step onto a second site and the units on the
first die and the timer starts again. The rule should be stated once, as a function of the
world: the timer is paused while anyone on your team stands on any sacred site the other
team holds. That is a signal, in the sense of functional reactive programming: a value
derived from other values, which the engine keeps true.

- [x] Signals: a named value computed from queries over the world and from other signals,
      read by systems through the `Signals` resource; see [SIGNALS.md](SIGNALS.md)
- [x] Signals as run conditions, and edges (became true, became false) as conditions and
      events, so "when the timer un-pauses" is written once
- [x] Signals over time: held for, and a timer that runs only while a signal is true (the
      sacred-site clock itself)
- [ ] Sources skipped when what they read hasn't changed
- [ ] Dependencies known to the engine: which signals read which, shown in the debug
      connection, with the current value of each (Phase 1B)
- [x] Signals from plugins, through the C interface (set, get, define), with Rust and Haskell
      bindings
- [x] A Haskell layer in which a rule reads as an expression (`defineRule`, `sig`, `.&&.`,
      `notS`, `timer`, …)
- [x] The sacred-site rule as a worked example and a test
- [x] Signals as data, not closures: a graph of named nodes (sources read from the world;
      operations such as and, or, not, count, compare, held-for) joined by connections, which
      can be listed with their values and rewired, forced and redefined while the game runs
- [x] Saving and loading a game's rules (`Signals::save_rules`, `load_rules`)
- [ ] Signals carrying entities
- [x] The signal graph over the debug connection: read every node and its value, set
      constants, connect inputs, force outputs, define and remove nodes, while the game runs
- [x] A signal graph viewer in the terminal (`mira-debug watch`): which signals are active as
      you play, what feeds what, what just changed; edited live from a second terminal
- [x] An orthographic camera (`Camera::orthographic(height)`), with shadows, sky and fog
      working under it
- [x] `examples/sacred_sites` as a 3D game under an orthographic camera, drawn from its
      signals (the graph is watched from a terminal)
- [x] The signal graph shown live inside that game (`crates/mira_ui`, example
      `sacred_sites_graph`); see [UI.md](UI.md)
- [x] The in-game view as a circuit: boxes, ports and wires lit when true; rewiring by
      dragging, forcing, and changing constants, on the running game
- [x] In the panel: adding and removing inputs by dragging wires, changing a box's
      operation, adding constants, removing signals
- [x] The panel's layout is kept with the rules and saved with them
- [x] Naming signals, timers and comparisons from the panel; typing numbers (click a box's
      name or its lower line; `Signals::rename`, `signal_rename`)
- [ ] The graphical editor of the graph (Chris, October 2026): signals and connections laid
      out and edited by hand while the game runs; in the game's panel and in the editor
      (Phase 5)

**Exit test:** the sacred-site game: two teams, several sites, units walking on and off and
dying in any order; the timer is right in every case because no code ever sets it.

## Phase 3: GPU-driven renderer (XL)

- [ ] Render graph: passes declare what they read and write; the graph orders them and
      allocates targets
- [ ] Material system: shader modules with imports, generated variants, material instances
- [ ] Scene on the GPU: shared vertex and index buffers, a persistent instance buffer updated
      only for changed transforms, bindless textures
- [ ] Culling in compute against the frustum and a depth pyramid; multi-draw indirect
- [ ] Visibility buffer or deferred shading, replacing forward + MSAA
- [ ] Point and spot lights, clustered, with a shadow atlas
- [ ] GPU particles, sorted or order-independent transparency, temporal upscaling
- [ ] Voxels: greedy meshing, transparent-block pass, chunk draws through the same indirect path

**Exit test:** 100,000 mesh instances and 1,000 shadow-less lights at 60 fps on the M2 Pro.

## Phase 4: Runtime for real games (XL)

- [x] Text and panels in the game: Armature underneath, a thin mira layer on top
      (`crates/mira_ui`: input shared with the game, drawn over the frame); see [UI.md](UI.md)
- [x] A look and a first set of controls for games (`mira_ui::kit`: theme, panel, button,
      toggle, slider, bar, anchoring)
- [x] A text field (`kit::field`), and typing played in from outside (`mira_input` `text`)
- [x] Tabs, a choice between options, lists and scrolling (`kit::tabs`, `choice`, `list`,
      `scroll`)
- [ ] Dropdowns and other pop-ups; interface in the world; gamepad focus
- [ ] Audio: mixing, 3D positioning, streaming music
- [ ] Input: named actions, rebinding, gamepads
- [ ] Animation graph: state machines, blend trees, retargeting, root motion
- [ ] Physics: convex hulls, continuous collision, ragdolls; decide here whether to keep the
      custom solver or adopt Rapier
- [ ] Navigation: navmesh for meshes, grid pathfinding for voxels; behaviour trees
- [ ] Save games, built on Phase 1
- [ ] Voxels: save edited chunks to region files; player collision against blocks

**Exit test:** ship a small complete game (menu, saves, sound, enemies that find their way)
using only the engine.

## Phase 5: Editor (XL)

Built with [Neo](https://github.com/mcdearman/neo), Chris's GUI toolkit, on top of the
engine's reflection. Before starting, work out with the Neo project what the
editor needs from it.

- [x] Agree with Neo on what it must provide: a wgpu viewport inside a Neo window, dockable
      panels, tree and property views, drag and drop, a transcript for the agent window; see
      [EDITOR.md](EDITOR.md). Neo is public, so the app can be built in this workspace
- [x] The game run by a host: on a device it is given, with no window or loop of its own,
      its frames a texture for the host to show (`App::host`, `render::frame_texture`)
- [x] The first app: a Neo window with the game in a viewport, and a bar to pause, resume
      and step it
- [x] The app is the engine's own program: bare `cargo run` opens it, on a starter scene or
      on the plugins named; the editor is `mira::editor`, on by default; the voxel world
      that used to be `cargo run` is `--example voxel_world`
- [x] Panels in a dock, arranged by dragging and kept between runs; the game's entities and
      signals listed live beside it

- [x] Entity tree (select, open and shut, drag to reparent) and a read-only inspector
      generated from reflection
- [x] Editing in the inspector: numbers, vectors and switches change the running game
- [x] Colours picked as colours, and entity references set by dragging from the tree
- [ ] In the inspector still: text and asset references, and
      one undo step per edit; renaming (mira has no name component yet); transform gizmos
- [ ] Asset browser, drag to place, prefab editing
- [x] Undo and redo of what is changed from the app; saving the scene and opening one;
      names for entities (`Name`), given in the tree
- [ ] Play in editor: the scene as edited kept apart from the game as it runs, so that
      stopping puts it back
- [ ] Material editor; voxel sculpting and painting tools
- [ ] Profiler and render-graph viewers
- [x] The agent window's first form: a conversation panel under the game, with Claude Code
      run behind it and given the game's own tools (see [EDITOR.md](EDITOR.md#the-agent))
- [ ] The agent window in full: a panel of the engine app in which to talk to an AI agent that is
      working on the game in front of you, so that no second app need be open. Asked for by
      Chris (October 2026), as the other half of Phase 1C: there the agent reaches the engine
      from outside; here it sits inside it.
  - The conversation, with what the agent does shown as it does it: each tool call and its
    answer, screenshots inline, the signal graph and entities it names as links into the
    other panels
  - The agent drives this very engine through the same tools as from outside (`mira_*`),
    served in-process, with no socket to set up
  - The agent itself is a program the window runs and talks to, not something built into
    the engine: any agent that speaks a common protocol can be put there. To be settled
    before building: which protocol (the Agent Client Protocol that editors use for this is
    the likely one) and how the agent is told about the engine's tools
  - Saying yes or no to what an agent asks to do; stopping it; and the game's own pause,
    step and rewind beside the conversation, since they are how its work is checked
  - What you have selected and are looking at (the entity, the signal, the camera's view)
    given to the agent as context without being typed

**Exit test:** rebuild the Paris square without writing Rust for placement.

## Phase 6: Dynamic global illumination (XL)

mira's answer to Lumen. The voxel grid is the advantage: rays can step through it on any GPU.

- [ ] Block occupancy as a 3D texture clipmap, with a distance field for fast stepping
- [ ] Meshes join the same scene through per-mesh distance fields
- [ ] Probe grid updated every frame on a budget, replacing the baked probes
- [ ] Cached surface lighting so bounces accumulate (per block face for voxels, cards for meshes)
- [ ] Screen-space probes for the final gather; traced reflections for smooth surfaces
- [ ] Hardware ray tracing as a quality tier of the same pipeline

**Exit test:** knock a hole in a cave roof and watch daylight spread and bounce inside within a
second, on Metal.

## Phase 7: Virtualized geometry and textures (XL)

mira's answer to Nanite.

- [ ] Meshlets and a cluster hierarchy built at import (meshoptimizer)
- [ ] Per-cluster LOD selection and two-pass occlusion culling on the GPU
- [ ] Compute rasterizer for pixel-sized triangles
- [ ] Cluster streaming; virtual shadow maps; virtual texturing
- [ ] Voxels: chunk LOD out to the horizon with seamless transitions

**Exit test:** a scene with a billion source triangles holds 60 fps, with frame time that
depends on resolution rather than scene size.

## Phase 8: World scale (L)

- [ ] Streaming for non-voxel content in cells; large coordinates without jitter
- [ ] Foliage, water, weather and time of day as engine features (lifted from the demos)
- [ ] Destruction: meshes that fracture, voxel structures that collapse

**Exit test:** fly 100 km in a straight line with no hitch and no precision artefacts.

## Phase 9: Networking and scripting (XL)

- [ ] Replication driven by reflection; client prediction and rollback from world snapshots
- [ ] A sandboxed modding layer for untrusted code (WebAssembly is the candidate). Trusted
      native plugins and hot reload are Phase 1A.

**Exit test:** a 16-player session of the Phase 4 game, with a mod loaded at runtime.

## Phase 10: Shipping (L, ongoing)

- [ ] Packaged builds for Windows, Linux and macOS; a web build on WebGPU
- [ ] Pipeline caching, crash reports, localization, accessibility settings
- [ ] Documentation, a book, and sample projects

**Exit test:** someone who has never seen mira builds and ships a game with it from the docs.

## What can run alongside

Phases 0 to 2 are strictly ordered, with 1A alongside them. After that, three tracks can
advance independently:

- **Rendering:** 3, then 6, then 7
- **Game runtime and tools:** 4, then 5, then 9
- **World:** the voxel items in each phase, then 8

## Not planned

Things Unreal has that only make sense with a team or licences: console support, a
MetaHuman-style character pipeline, film-grade cinematics tools, a marketplace. Revisit if the
project grows.

## Next three steps

1. The app's working loop (Phase 5): undo and redo; saving what was edited as a scene and
   opening one; names for entities; then the asset browser with drag to place, and gizmos.
2. The agent in the app: a first run against Claude Code itself, approvals so that it can
   change code, and what is selected passed to it. Then model files loaded off the main
   thread and watched (Phase 1), which the app needs before it can host the big demos.
3. Two things seen in screenshots: the built-in sky is dull next to the sun it comes with,
   and objects that move every frame smear under temporal anti-aliasing (it has no motion
   vectors). Then the rest of Phase 1C and of Phase 2, and on to Phase 3.
