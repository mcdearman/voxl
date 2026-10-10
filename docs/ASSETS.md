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
- **Model files** load one of two ways. `load_gltf` reads the file on the calling thread and
  hands back the whole model, for a game that builds its scene in code. A `Model` component
  asks for the file to be read on a worker thread instead: the entity is there at once, and
  the model's parts appear under it as children when the file has been read.
- **Shapes** are made when first asked for.

```rust
commands.spawn((Transform::from_xyz(4.0, 0.0, 0.0), Model::new("models/house.glb")));
```

A `Model` is saved in a scene as the file's name, and its parts are not saved: loading the
scene reads the file again. `server.request_gltf(name)` starts the reading without an entity,
and `server.model(name)` is the model once it has arrived.

Image files and model files are watched and reloaded when they change. A model file that is
saved again is read on a worker again, and every `Model` showing it gets its new parts; see
[HOT_RELOAD.md](HOT_RELOAD.md).

## Unloading

Handles are plain ids, not reference counts, so nothing is freed behind your back. Free what
is no longer used when it suits the game, after changing level for instance:

```rust
let unloaded = AssetServer::unload_unused(&mut app.world);
```

A named image or mesh is in use if a registered component on some entity refers to it (a
`Mesh3d`, a `Material`'s textures, a `Lods` level), or if it is part of a model file another
part of which is in use: a model stays or goes whole. Everything else is removed, from the
GPU too. Unloading loses nothing for good, because asking for the name loads it again, which
is what spawning a scene or a prefab does.

A handle held where the engine can't see it, in a resource of yours or a component that isn't
registered, goes stale when its asset is unloaded. Protect those with `server.keep(handle)`
(and `release` later). Assets without a name are never unloaded.

## What isn't here yet

- Unloading is something the game asks for; nothing unloads on its own under memory pressure.
- A model's parts come all at once, and a reloaded model is rebuilt whole. Nothing knows yet
  which assets depend on which.
- There is no processing step: textures are not compressed for the GPU, and nothing is cached
  on disk.
- The Napoleonic and Paris demos still load their assets directly.
