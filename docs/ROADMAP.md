# voxl roadmap

Where voxl is going: a general-purpose engine in the class of Unreal, with voxel worlds as the
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
| Voxels | Chunked storage, threaded generation and meshing with ambient occlusion, streaming, raycasts | Greedy meshing, chunk LOD, transparent blocks, saving edits, voxel lighting |
| Simulation | Rigid bodies with sleeping, joints, character controller, fluids; shapes: sphere, box, capsule, plane, static triangle mesh | Convex hulls, continuous collision, ragdolls, vehicles, cloth, destruction |
| Animation | Clips, cross-fades, masks, two-bone IK, procedural gait | State machines, blend trees, retargeting, skins loaded from glTF, compression |
| Runtime | Keyboard and mouse | UI, text, audio, gamepad, input mapping, navigation, AI, networking, scripting, save games |
| Plugins | Native plugins over a C interface (`include/voxl.h`) in any language, hot-reloaded with their data intact; Rust bindings | Most of the engine is not reachable from a plugin yet (see Phase 1A) |
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

- [ ] Split into a workspace: `voxl_ecs`, `voxl_core`, `voxl_render`, `voxl_voxel`, `voxl_physics`
      (the workspace exists, with `voxl_plugin` and the example plugins as members; the engine
      itself is still one crate)
- [x] CI on macOS and Linux: build, test, clippy, Miri on the ECS (written; not yet seen to
      pass on GitHub)
- [ ] Image-diff tests: render fixed scenes with `VOXL_SCREENSHOT` and compare to stored frames
- [ ] CPU and GPU frame profiler (spans per system, timestamp queries per pass), on screen
- [ ] Benchmarks for ECS iteration, chunk meshing and a standard frame

**Exit test:** a change that breaks rendering or halves ECS speed fails CI.

## Phase 1A: Plugins and hot reload (L)

Pulled forward from Phase 9 at Chris's request (October 2026): hot reload and plugins are a
priority, and plugins may be written in any language that can speak a C interface. The
contract is `include/voxl.h`; see [PLUGINS.md](PLUGINS.md).

- [x] Components defined at runtime by name, size and alignment, checked by the same access
      rules as Rust components
- [x] The C interface, version 1: define and find components, persistent state, systems with
      one query each, deferred spawn/despawn/insert/remove, time, logging
- [x] Loading shared libraries, with interface-version checking
- [x] Hot reload: a rebuilt plugin replaces itself in the running app, keeping component data,
      state and system order; a broken rebuild leaves the old version running
- [x] Rust bindings (`crates/voxl_plugin`) and example plugins in Rust and C
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
- [ ] Optional terms and change filters in queries
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
- [ ] What each system reads and writes, from the running app
- [x] A debug connection (a local socket speaking JSON) to a running game: list and search
      entities, read and change any registered component or resource by name, spawn and
      despawn, save the scene, pause and step, see failures with their stacks, reload
      plugins, read and rewire the signal graph
- [x] A command-line client for it (`voxl-debug`)
- [ ] The same protocol under the editor (Phase 5); pushing changes to a client that is
      watching, instead of being asked
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
- [ ] Saving skeletal animation state and edited voxel chunks
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
- [ ] Parallel scheduler built on the existing access sets; `World` safe to share
- [ ] Table storage as an option beside sparse sets, chosen per component
- [ ] Hooks and observers: run code when a component is added or removed
- [ ] Relations: `(ChildOf, e)`-style pairs, replacing `Parent`
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
- [ ] A Haskell layer in the applicative style of the bindings' queries, so a rule reads as
      an expression
- [x] The sacred-site rule as a worked example and a test
- [x] Signals as data, not closures: a graph of named nodes (sources read from the world;
      operations such as and, or, not, count, compare, held-for) joined by connections, which
      can be listed with their values and rewired, forced and redefined while the game runs
- [ ] Saving and loading a signal graph; signals carrying entities
- [x] The signal graph over the debug connection: read every node and its value, set
      constants, connect inputs, force outputs, define and remove nodes, while the game runs
- [x] A signal graph viewer in the terminal (`voxl-debug watch`): which signals are active as
      you play, what feeds what, what just changed; edited live from a second terminal
- [ ] The graphical viewer (Chris, October 2026): the graph laid out, signals and connections
      edited by hand while the game runs; a panel of the editor (Phase 5)

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

- [ ] Text rendering and a UI layer for games
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

- [ ] Agree with Neo on what it must provide: a wgpu viewport inside a Neo window, dockable
      panels, tree and property views, drag and drop, undo

- [ ] Viewport, entity tree, inspector generated from reflection, transform gizmos
- [ ] Asset browser, drag to place, prefab editing
- [ ] Undo and redo as a command log; play in editor
- [ ] Material editor; voxel sculpting and painting tools
- [ ] Profiler and render-graph viewers

**Exit test:** rebuild the Paris square without writing Rust for placement.

## Phase 6: Dynamic global illumination (XL)

voxl's answer to Lumen. The voxel grid is the advantage: rays can step through it on any GPU.

- [ ] Block occupancy as a 3D texture clipmap, with a distance field for fast stepping
- [ ] Meshes join the same scene through per-mesh distance fields
- [ ] Probe grid updated every frame on a budget, replacing the baked probes
- [ ] Cached surface lighting so bounces accumulate (per block face for voxels, cards for meshes)
- [ ] Screen-space probes for the final gather; traced reflections for smooth surfaces
- [ ] Hardware ray tracing as a quality tier of the same pipeline

**Exit test:** knock a hole in a cave roof and watch daylight spread and bounce inside within a
second, on Metal.

## Phase 7: Virtualized geometry and textures (XL)

voxl's answer to Nanite.

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

**Exit test:** someone who has never seen voxl builds and ships a game with it from the docs.

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

1. Saving edited voxel chunks; model files loaded off the main thread and watched (Phase 1).
2. Image-diff tests around the demos, and the rest of the crate split (Phase 0). Two things
   seen in screenshots to fix alongside: the built-in sky is dull next to the sun it comes
   with; and objects that move every frame smear under temporal anti-aliasing (it has no
   motion vectors).
3. The parallel scheduler, on the ordering constraints and access sets that are now there
   (Phase 2).
