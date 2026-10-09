# Hot reload

Three kinds of thing can be changed while a voxl app is running. Each is watched on disk,
reloaded a moment after it is saved, and left as it was if the new version is broken.

| What | How to use it | When it is on |
|---|---|---|
| Gameplay code | Write it as a [native plugin](PLUGINS.md) | Always, unless `app.native_plugins().hot_reload = false` |
| Shaders | Name them with `shader!` instead of `include_str!` | Debug builds; `VOXL_HOT_SHADERS=1` or `=0` overrides |
| Textures | Load them through the `AssetServer` | Debug builds; `AssetServer::hot_reload` overrides |

## Shaders

```rust
let module = voxl::shader!("water.wgsl").module(&gpu.device);
// or, to prepend the engine's lighting code:
let source = format!("{}\n{}", gpu.pbr_wgsl(), voxl::shader!("water.wgsl").source());
```

`shader!` names a WGSL file beside the current source file, like `include_str!`. The file is
still compiled into the binary, so a release build needs nothing on disk. In a debug build the
file on disk is read instead and watched.

When a watched shader changes, every pipeline is rebuilt from the current sources. All of the
engine's own shaders reload this way (meshes, voxels, sky, tone mapping, bloom, anti-aliasing,
light shafts, skinning). If a shader fails to compile, or no longer fits its pipeline, the
error is logged and every old pipeline stays in use.

A renderer with its own pipelines joins in by pushing a function onto the `ShaderReload`
resource. The function builds new pipelines without touching the world and returns how to put
them in place, which only happens if every rebuild succeeded:

```rust
world.resource_mut::<ShaderReload>().0.push(|world| {
    let fresh = WaterRenderer::new(world.resource::<Gpu>());
    Some(Box::new(move |world: &mut World| world.insert_resource(fresh)))
});
```

Not covered yet: the ray-tracing and light-probe shaders (which only run on Vulkan), and the
shaders of the Napoleonic and Paris demos, which still use `include_str!`.

## Textures

```rust
fn setup(mut server: ResMut<AssetServer>, mut images: ResMut<Assets<Image>>) {
    let bricks = server.load_image(&mut images, "textures/bricks.png");
    // use `bricks` in a Material as usual
}
```

Images loaded through the `AssetServer` are watched. When the file is saved again, the image is
decoded on a worker thread and uploaded, and every material using it is rebuilt. A file that
can't be decoded is reported and the old image stays. See [ASSETS.md](ASSETS.md).
