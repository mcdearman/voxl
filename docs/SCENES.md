# Reflection and scenes

Reflection lets the engine describe, save and load values without knowing their Rust type.
Scene files, the editor's inspector, save games and networking all build on it.

## Making a type reflectable

```rust
use voxl::prelude::*;

#[derive(Component, Reflect, Default)]
#[reflect(name = "game.Health", default)]
struct Health {
    current: f32,
    max: f32,
    #[reflect(skip)]
    last_hit_frame: u32,
}

app.register_type::<Health>();
```

`#[derive(Component)]` replaces writing `impl Component for Health {}` by hand.

`#[derive(Reflect)]` works on structs and enums whose fields are themselves reflectable:
numbers, `bool`, `String`, `Vec`, `Option`, arrays, `Entity`, the vector and quaternion types,
asset handles, and other derived types. Its attributes:

| Attribute | On | Meaning |
|---|---|---|
| `name = "game.Health"` | the type | The name it has in files. Defaults to the Rust name. Choose one and keep it: renaming the Rust type then doesn't break saved scenes. |
| `default` | the type | Fields missing from a file are filled from `Default::default()`, so adding a field doesn't break old files. |
| `default` | a field | Just this field is, from its own type's default. |
| `skip` | a field | Never saved; made with `Default::default()` when loading. For caches and per-frame state. |

## What a reflected value looks like

A value is taken apart into a `Value`: nested plain data (numbers, text, lists, maps, entity
references). Structs become maps, wrappers become what they wrap, and enum variants are
written by name. As JSON, a `Transform` is:

```json
{
  "translation": [1.0, 2.0, 3.0],
  "rotation": [0.0, 0.0, 0.0, 1.0],
  "scale": [1.0, 1.0, 1.0]
}
```

`Reflect::schema()` describes a type's shape (its fields and their types), which is what an
inspector uses to lay out its controls.

## Scenes

A scene is a set of entities with their registered components.

```rust
let registry = app.world.resource::<TypeRegistry>();
Scene::capture(&app.world, registry).save("level.json")?;

let scene = Scene::load("level.json")?;
let spawned = app.world.resource_scope(|world, registry: &mut TypeRegistry| {
    scene.spawn(world, registry)
});
for problem in &spawned.skipped {
    log::warn!("{problem}");
}
```

- **References survive.** A component that points at another entity (`Parent`) points at the
  right new entity after loading, whatever ids the entities get.
- **One bad value doesn't lose a level.** A component of an unknown type, with a value that
  no longer fits, or pointing at an entity outside the scene, is skipped and reported in
  `Spawned::skipped`; everything else loads.
- **Assets come back.** A mesh or texture loaded through the [`AssetServer`](ASSETS.md) is
  saved by name (`{"$asset": "image", "name": "textures/bricks.png"}`) and loaded again when
  the scene is, so a scene file works in a later run.
- **Files are plain JSON**, indented, with fields in a stable order, so they diff cleanly and
  any tool can read them.

## Editing by name

The `TypeRegistry` reaches components by name, which is what an editor does:

```rust
let transform = registry.get("voxl.Transform").unwrap();
let mut value = (transform.get)(&world, entity).unwrap();
value.set_path("translation.1", Value::Float(4.0));
(transform.insert)(&mut world, entity, &value)?;
```

## What isn't here yet

- Asset handles are saved by name when the [`AssetServer`](ASSETS.md) knows the asset, and a
  scene loaded later asks for those names again. An asset without a name is saved as its id,
  which only holds within a run.
- Registered so far: `Transform`, `Parent`, `Camera`, `DirectionalLight`, `Mesh3d` and
  `Material`. Physics, animation and voxel components are not.
- Resources and plugin-defined components are not captured.
- There are no prefabs (a scene used as a template, with overrides) yet.
