# Assets

An asset is something loaded once and shared: a mesh, a texture, a model file. Assets live in
`Assets<T>` stores and are referred to by `Handle<T>`, a small copyable id.

The `AssetServer` gives assets **names**, and a name is how an asset is asked for:

| Name | What it is |
|---|---|
| `textures/bricks.png` | An image file, read as a colour texture |
| `textures/bricks_normal.png?linear` | The same kind of file read as data (normal and roughness maps) |
| `models/house.glb` | A glTF model file |
| `models/house.glb#mesh2` | The third mesh in that file |
| `models/house.glb#image0` | The first texture in that file |
| `shape:cube:1` | A cube with edges of 1, made in code. Also `shape:sphere:<radius>` and `shape:plane:<edge>` |

File names are relative to the server's root (`AssetServer::set_root`; the working directory
by default). Asking for the same name twice gives the same handle.

```rust
fn setup(
    mut commands: Commands,
    mut server: ResMut<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) -> anyhow::Result<()> {
    let cube = server.cube(&mut meshes, 1.0);
    let bricks = server.load_image(&mut images, "textures/bricks.png");
    let house = server.load_gltf("models/house.glb", &mut meshes, &mut images)?;
    // ...
}
```

## Why names matter

A handle is only a number, good while the app runs. A [scene](SCENES.md) saves the *name* of
each asset its entities use, and loading the scene asks the server for those names again. So
a saved scene means the same thing in a later run, whatever ids the assets get that time. An
asset with no name (a mesh you built by hand and added with `Assets::add`) is saved as its id,
which does not survive; give it one with `AssetServer::name` if it should.

## Loading

- **Images** are decoded on worker threads. `load_image` returns the handle at once, and the
  image appears under it shortly after; until then anything using it draws with a plain white
  texture. `AssetServer::loading()` says how many are outstanding.
- **Model files** are loaded when asked for, on the calling thread, once per file.
- **Shapes** are made when first asked for.

Image files are watched and reloaded when they change; see [HOT_RELOAD.md](HOT_RELOAD.md).

## What isn't here yet

- Assets are never unloaded. Handles are plain ids rather than reference counts; the plan is to
  free what no component refers to by tracing reflected components.
- Model files load on the calling thread, and are not watched for changes.
- There is no processing step: textures are not compressed for the GPU, and nothing is cached
  on disk.
- Plugins can make shapes but can't yet load files.
- The Napoleonic and Paris demos still load their assets directly.
