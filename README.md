# mira

A game engine in Rust, built from scratch on winit and wgpu around a Bevy-style ECS, with
voxel terrain. The engine is an app: a window, built with
[Neo](https://github.com/mcdearman/neo), in which a game is shown, looked into and changed
while it runs, with an AI agent beside it that can do the same.

```
cargo run                         # the app, on a small scene to start from
cargo run -- chase scoreboard     # the app, on a game made of plugins
```

The game is in one panel; around it are its entities as a tree, an inspector that edits the
chosen one, its signals, and a conversation with an agent. See [docs/EDITOR.md](docs/EDITOR.md).
Everything else here is an example, run by name:

```
cargo run --release --example voxel_world
```

In the voxel world, click to capture the mouse, then: WASD / Space / Shift to fly (Ctrl to go fast), left click to
break a block, right click to place one (1-4 picks which), F to throw a ball, F2 for a
screenshot, Esc to release the mouse and again to quit.

## Demos

```
cargo run --release --example napoleonic
```

France, summer 1813, as photographically as the engine can manage: a foot battery at gun
practice, a line battalion firing volleys, a column marching over the bridge and through the
village, cuirassiers in reserve, the Emperor and his staff on a knoll, and a camp by the river.
Click to capture the mouse; WASD walks, Shift runs, Tab switches to flying.

It uses a photographed sky and photoscanned materials (Poly Haven, CC0), soldiers built from a
scanned figure (Microsoft Rocketbox, MIT) and a CC0 horse, and generated trees (EZ-Tree, MIT);
see `res/napoleonic/CREDITS.md`. The figures, trees and their levels of detail are made by the
Blender scripts in `examples/napoleonic/tools`.

```
cargo run --release --example paris
```

The Place du Châtelet in about 1810, after Étienne Bouhot's painting: the Fontaine du Palmier
and its gilded Victory, a square full of townsfolk who stroll about, stop to talk and carry water,
Guard grenadiers at their post, carts and a coach, animals that walk on their own four (or two) feet, the fountain's jets simulated as water, and across the Pont au Change the clock tower of the Palais and
the towers of the Conciergerie. With hardware ray tracing it has traced sun shadows, true
reflections and light bounced about the square by baked light probes. The same controls as
above. Assets are listed in `res/paris/CREDITS.md`; the people are dressed and posed by the
Blender scripts in `examples/paris/tools`.

## Plugins and hot reload

Gameplay can live in native plugins: shared libraries in any language that can export C
functions, reloaded while the app runs with the world's data intact. The contract is one
header, `include/mira.h`. See [docs/PLUGINS.md](docs/PLUGINS.md).

```
plugins/swirl/build.sh && cargo build -p wave && plugins/pulse/build.sh
cargo run --example plugins
```

Then edit `plugins/swirl/Swirl.hs` (Haskell, the default plugin language), `plugins/wave/src/lib.rs`
(Rust) or `plugins/pulse/pulse.c` (C) and rebuild it. Each plugin is optional.

A plugin can be the whole game. `plugins/chase` is a small one in Haskell (WASD to steer,
collect the spheres, push the crates), with its own camera, lighting and physics. A separate
Rust plugin shows the score from an event the game sends. The host only opens a window:

```
plugins/chase/build.sh && cargo build -p scoreboard
cargo run --example host -- chase scoreboard
```

Shaders and textures reload while the app runs too; see [docs/HOT_RELOAD.md](docs/HOT_RELOAD.md).

Components can be saved to and loaded from JSON scene files, and inspected by name; see
[docs/SCENES.md](docs/SCENES.md).

Edits to voxel terrain are kept when their chunk streams out, and can be saved to a file; see
[docs/VOXELS.md](docs/VOXELS.md).

Ordering systems, run conditions and game states: [docs/SCHEDULING.md](docs/SCHEDULING.md).
Relations between entities: [docs/RELATIONS.md](docs/RELATIONS.md).

Failures that pause the game instead of ending it, and stepping time:
[docs/LIVE.md](docs/LIVE.md). Game rules as signals: [docs/SIGNALS.md](docs/SIGNALS.md).

AI agents driving a running game over MCP: [docs/MCP.md](docs/MCP.md).

Interface inside the game, on Armature: [docs/UI.md](docs/UI.md).

Where the engine is headed: [docs/ROADMAP.md](docs/ROADMAP.md).

## Layout

| Module | What it does |
|---|---|
| `ecs` | Sparse-set ECS: function systems, queries and filters, change detection, commands, events, startup access checking |
| `app` | `App`, plugins and the frame's stages, including a fixed-timestep group |
| `transform` | `Transform` / `GlobalTransform`, `Parent` hierarchies, `Interpolate` for smooth fixed-step motion |
| `render` | wgpu setup; HDR rendering with MSAA, AgX tone mapping and bloom; physically based materials with textures; image-based sky lighting and aerial haze, with a clear sky worked out from the sun's place when no picture of one is given; cascaded sun shadows; hardware ray tracing (traced shadows, contact shadows, reflections, and light probes for bounced light); temporal anti-aliasing with motion vectors for what moves; light shafts through the shadow maps; decals; colour grading and film grain; more than one view a frame (`ViewTarget`), view modes (unlit, lighting only, normals, wireframe) and debug lines; frustum culling and levels of detail; glTF loading with skeletal animation (clips cross-faded and masked, meshes skinned on the GPU), two-bone inverse kinematics, procedural walking (`Gait`: planted feet and swinging steps for any number of legs, walk, trot and strut patterns, foot locking under clips) and reaching (`Reach`: hands onto moving targets); screenshots. Plugins add pipelines through `DrawFunctions`, `TransparentDrawFunctions` and `ShadowDrawFunctions`, sharing the lighting in `PBR_WGSL` |
| `physics` | Rigid bodies (dynamic, kinematic, and animated ones the game moves) with sphere, box, capsule, plane, triangle-mesh and compound colliders; a sequential-impulse solver with friction, restitution, warm starting and sleeping; ball, hinge (with limits), distance and fixed joints; raycasts, overlap tests and a `CharacterController` that climbs steps and slides along walls. Fluids: `WaterSurface`, a wave-equation height field that floats bodies and ripples where things move or fall in, and `ParticleFluid`, position-based fluid particles with surface tension for jets, pours and spray that join the surfaces they land in. Add `PhysicsPlugin` |
| `voxel` | Chunk storage, background terrain generation and meshing with ambient occlusion, streaming around a `ChunkViewer`, raycasts, frustum culling |
| `plugin` | Loads native plugins over the C interface and hot-reloads them |
| `reflect` | Describes, saves and loads values by name: derive macros, the type registry, JSON scenes |
| `tasks` | Worker thread pool |
| `asset_server`, `assets` | Assets by name: images, model files and shapes, loaded once, reloaded when saved |
| `input`, `window`, `time` | The usual |

## Checks

```
cargo test
cargo +nightly miri test -p mira_ecs        # the ECS's unsafe code
scripts/check.sh --tsan-only                # the ECS's threads, under ThreadSanitizer
MIRA_SCREENSHOT=frame.png cargo run --release --example voxel_world   # render a frame to a file and quit
MIRA_FRAME_TESTS=1 cargo test --test frames     # draw fixed scenes and compare with stored frames
```

Debug builds carry only the debug information a failure's stack needs (see the profile in
`Cargo.toml`), which keeps the build folder to a few gigabytes; `cargo clean --profile dev`
empties it.

The last needs a graphics card, so it runs only when asked. It starts a scene hidden and
paused (`MIRA_PAUSED=1`), steps it to a set frame, and compares the picture, made small, with
the one in `tests/frames`; a frame that differs leaves `<name>.new.png` and `<name>.diff.png`
there. After a change that was meant to change the picture, store new frames with
`MIRA_UPDATE_FRAMES=1`. The comparison is `mira::render::frame_diff`, for a game's own tests.
