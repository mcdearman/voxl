# voxl

A small game engine in Rust, built from scratch on winit and wgpu around a Bevy-style ECS,
with voxel terrain.

```
cargo run --release
```

Click to capture the mouse, then: WASD / Space / Shift to fly (Ctrl to go fast), left click to
break a block, right click to place one (1-4 picks which), F to throw a ball, F2 for a
screenshot, Esc to release the mouse and again to quit.

## Layout

| Module | What it does |
|---|---|
| `ecs` | Sparse-set ECS: function systems, queries and filters, change detection, commands, events, startup access checking |
| `app` | `App`, plugins and the frame's stages, including a fixed-timestep group |
| `transform` | `Transform` / `GlobalTransform`, `Parent` hierarchies, `Interpolate` for smooth fixed-step motion |
| `render` | wgpu setup, instanced mesh pipeline, shared view uniform, fog, texture arrays, screenshots. Plugins add pipelines through `DrawFunctions` |
| `voxel` | Chunk storage, background terrain generation and meshing with ambient occlusion, streaming around a `ChunkViewer`, raycasts, frustum culling |
| `tasks` | Worker thread pool |
| `input`, `window`, `time`, `assets` | The usual |

## Checks

```
cargo test
cargo +nightly miri test --lib -- ecs::     # the ECS's unsafe code
VOXL_SCREENSHOT=frame.png cargo run --release   # render a frame to a file and quit
```
