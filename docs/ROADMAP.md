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
- [ ] Reach more of the engine from a plugin: assets loaded from files, hierarchy (`Parent`),
      joints and character controllers, voxel terrain, text and UI, sound
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

## Phase 1: Data layer (L)

The layer Unreal's editor, saves, networking and Blueprints all stand on.

- [ ] Proc-macro crate: `#[derive(Component)]`, `#[derive(Reflect)]`
- [ ] Type registry: look up a type by name, list and edit its fields at runtime
- [ ] Serialization of any reflected value; a text scene format; prefabs with overrides
- [ ] Hierarchy as a real feature: `Children`, recursive despawn, cached propagation
- [ ] Asset server: load by path on worker threads, reference-counted handles, dependencies,
      hot reload when a file changes
- [ ] Asset processing: textures to BC7/ASTC with mips, meshes to a binary format, cached by
      content hash
- [ ] Load skins and animations from glTF

**Exit test:** save the Paris scene to a file and load it back identical; edit a texture on disk
and see it change in the running scene.

## Phase 2: Parallel core (L)

- [ ] System ordering: before/after, system sets, run conditions, application states
- [ ] Parallel scheduler built on the existing access sets; `World` safe to share
- [ ] Table storage as an option beside sparse sets, chosen per component
- [ ] Hooks and observers: run code when a component is added or removed
- [ ] Relations: `(ChildOf, e)`-style pairs, replacing `Parent`
- [ ] One job system for systems, asset loading and voxel work

**Exit test:** the Napoleonic demo's update time drops in proportion to cores used, with Miri
and a thread sanitizer clean.

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

Built from the engine's own UI and reflection.

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

1. Image-diff tests around the demos, and the rest of the crate split (Phase 0). Two things
   seen in screenshots to fix alongside: the voxel demo's fog starts at the camera, so nearby
   terrain is hazed (fog needs a start distance); and objects that move every frame smear
   under temporal anti-aliasing (it has no motion vectors).
2. Start the data layer: a proc-macro crate with `#[derive(Component)]` and
   `#[derive(Reflect)]` (Phase 1).
3. Assets from files for plugins (models and textures by path), which needs the asset server
   from Phase 1.
