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
scene reads the file again.

A model file with a skeleton comes with an `Animator` on its entity, playing the file's
clips, and its skinned parts follow it. Put a `Playing` beside the `Model` to say which
clip, as data a scene can hold:

```rust
commands.spawn((at, Model::new("models/citizen.glb"), Playing::new("Walk").with_speed(1.2)));
```

Change `playing.clip` and the animator cross-fades to it over `playing.fade` seconds.
`playing.time` follows the animator, so a scene saved while a figure walks comes back with
the figure in the same stride. `server.request_gltf(name)` starts the reading without an entity,
and `server.model(name)` is the model once it has arrived.

Image files and model files are watched and reloaded when they change. A model file that is
saved again is read on a worker again, and every `Model` showing it gets its new parts; see
[HOT_RELOAD.md](HOT_RELOAD.md).

## Processing

A picture file is not what a graphics card wants. As it is loaded, on a worker, a picture is
decoded, filtered down into its smaller levels, and compressed into BC7 blocks, which the
card reads directly: a quarter of the memory of plain pixels, and nothing left to do on the
frame it is first drawn. The result is kept in `.mira/cache` under the asset root, named by
a hash of the file's bytes and of how it was processed, so the work is done once: a file
that hasn't changed is read straight from the cache, and one that has is processed again.

- It happens where the graphics card can hold BC textures (desktop cards do), and not in a
  program with no renderer. `MIRA_COMPRESS=0` turns it off.
- A picture loaded this way keeps no pixels in memory (`image.has_pixels()` is false). A
  game that reads the pixels of pictures it loads by name sets `server.keep_pixels = true`.
- A picture whose sides are not multiples of four is left as plain pixels.
- The cache is bounded: over `server.cache_limit` bytes (2 GB), what was used longest ago
  goes. `server.cache = false` keeps nothing on disk. The folder can be deleted at any time.

## What needs what

The server keeps, by name, what each asset needs. A model file needs its meshes and
textures, and each of those the file; a prefab needs the pictures and shapes its entities
use, the model files they show, and the prefabs inside it. These are recorded as things are
loaded. A game with a kind of asset of its own says what it needs with `depends_on`.

```rust
server.dependencies("prefabs/street.json");      // what it needs itself
server.all_dependencies("prefabs/street.json");  // and what those need, all the way down
server.dependents("textures/bricks.png");        // what needs this
server.state("models/house.glb");                // Unknown, Loading, Loaded or Failed
server.is_ready("prefabs/street.json");          // it and everything it needs is loaded
```

`is_ready` is the moment a level can be shown with nothing still to pop in. From outside,
`mira-debug assets` lists every asset with its state and what it needs.

## Unloading

Handles are plain ids, not reference counts, so nothing is freed behind your back. Free what
is no longer used when it suits the game, after changing level for instance:

```rust
let unloaded = AssetServer::unload_unused(&mut app.world);
```

A named image or mesh is in use if a registered component on some entity refers to it (a
`Mesh3d`, a `Material`'s textures, a `Lods` level), or if something in use needs it: a model
stays or goes whole, and what a prefab in the scene needs stays with it. Everything else is removed, from the
GPU too. Unloading loses nothing for good, because asking for the name loads it again, which
is what spawning a scene or a prefab does.

A handle held where the engine can't see it, in a resource of yours or a component that isn't
registered, goes stale when its asset is unloaded. Protect those with `server.keep(handle)`
(and `release` later). Assets without a name are never unloaded.

## What isn't here yet

- Unloading is something the game asks for; nothing unloads on its own under memory pressure.
- A model's parts come all at once, and a reloaded model is rebuilt whole.
- Textures a model file refers to in other files are read with it, not as assets of their
  own with their own names.
- Only picture files are processed. Textures inside a model file are uploaded as plain
  pixels, meshes are not yet turned into a binary form, and there is no ASTC for the cards
  that want it in place of BC.
- The Napoleonic and Paris demos still load their assets directly.
