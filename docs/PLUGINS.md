# Native plugins and hot reload

A voxl plugin is a shared library that adds components and systems to a running app. It can
be written in any language that can build a shared library exporting C functions, because the
whole contract is one C header: [`include/voxl.h`](../include/voxl.h).

The engine watches each plugin's file. Rebuild a plugin while the app is running and the new
code takes over within a fraction of a second, with the world exactly as it was.

## Try it

```
plugins/swirl/build.sh && cargo build -p wave && plugins/pulse/build.sh
cargo run --example plugins
```

A grid of cubes appears. The host program only draws them. Three plugins in three languages
move them: `plugins/swirl` (Haskell) turns the grid, `plugins/wave` (Rust) moves the cubes up
and down, and `plugins/pulse` (C) changes their size. Now edit a constant at the top of any of
them and rebuild just that plugin in another terminal:

```
plugins/swirl/build.sh       # after editing plugins/swirl/Swirl.hs   (needs GHC)
cargo build -p wave          # after editing plugins/wave/src/lib.rs
plugins/pulse/build.sh       # after editing plugins/pulse/pulse.c
```

A plugin that isn't built is skipped, so the example runs with whichever toolchains you have.

A plugin can also be the whole game. `plugins/chase` is one, in Haskell: steer a cube with
WASD or the arrow keys and collect the spheres. The host program only opens a window and sets
up a camera and a sun:

```
plugins/chase/build.sh
cargo run --example host -- chase
```

## What a plugin is

Three exported functions:

| Function | When it is called |
|---|---|
| `voxl_plugin_abi_version()` | First. Must return `VOXL_ABI_VERSION`, or the plugin is refused. |
| `voxl_plugin_load(api, app)` | On first load and after every reload. Register everything here. Return 0 for success. |
| `voxl_plugin_unload()` | Optional. Before the library is closed. |
| `voxl_plugin_flags()` | Optional. `VOXL_PLUGIN_KEEP_LOADED` asks the engine never to unmap the library, for languages whose runtime can't be unloaded (Haskell, Go). |

`api` is a table of the engine's functions. In `voxl_plugin_load` a plugin can:

- **Define a component** with `component_register`: a name, a size and an alignment. Values
  are plain bytes.
- **Find a component** someone else defined with `component_lookup`, such as the engine's
  `voxl.Transform`. It reports the size and alignment so the plugin can check its own
  definition matches.
- **Get persistent state** with `state`: a zeroed block of memory that survives reloads.
- **Add a system** with `system_add`: a function, the stage it runs in, and its *terms*.

A system's terms say which entities it visits and what it does with each component:
`VOXL_READ` and `VOXL_WRITE` give a pointer to the value, `VOXL_WITH` and `VOXL_WITHOUT` only
filter. Inside the system, `query_next` steps through the matching entities. `spawn`,
`despawn`, `insert` and `remove` queue changes that apply when the system returns.

A system can have more queries (`system_add_query`), for rules that relate two kinds of
entity: the player and the things it can collect, say. Two queries of one system may reach the
same component only if neither writes it, or if `WITH`/`WITHOUT` terms guarantee they never
match the same entity. The engine checks this when the plugin loads and refuses the query
otherwise.

Inside a system a plugin can also:

- **Read input:** `key_down`, `key_pressed`, `key_released`, `mouse_down`, `mouse_pressed`,
  `mouse_motion`. Keys are named by position, so WASD is the same four keys on any layout.
- **Create meshes:** `mesh_shape` for a cube, sphere or plane, and `mesh_create` from
  vertices and triangles. Make each mesh once, in a `STARTUP` system, and keep the handle in
  a `state` block.
- **Make an entity visible:** `set_mesh` and `set_material`, together with a
  `voxl.Transform`.
- **Read time:** `delta_seconds` and `elapsed_seconds`.

## What survives a reload

| Survives | Does not |
|---|---|
| Every component value, including those of components the plugin defined | The plugin's global and static variables |
| Blocks from `state` | Memory the plugin allocated that only its globals pointed to |
| The order systems run in (a system replaces its namesake) | `Startup` systems do not run again |

So: keep anything that must persist in components or in a `state` block, and fetch component
handles again in every `voxl_plugin_load`.

If a component is registered again with a different size or alignment, its old values are
discarded (with a warning), since they can no longer be interpreted. A plugin that changes a
component's meaning without changing its size must migrate the values itself.

If a rebuilt library can't be opened, or was built against a different interface version, the
old version keeps running and the failure is logged. The file is tried again when it next
changes.

## Rules

- Call functions that take a `VoxlApp*` only inside `voxl_plugin_load`, and functions that
  take a `VoxlSystem*` only inside that system. Don't keep either pointer.
- Everything happens on the thread that called you.
- Component pointers from a query are valid until the system returns.
- Strings are UTF-8 with an explicit length.
- Build the library somewhere else and move it into place (as Cargo and `build.sh` do), so the
  engine never loads a half-written file.

A plugin is native code inside the engine's process. It can crash the app or corrupt memory,
and the engine cannot prevent that. Only load plugins you trust.

## In Haskell (the default)

Haskell is the language voxl's plugin examples and documentation lead with: plugin code is
type-checked against the components it uses, and the bindings give it no pointers to misuse.
(The long-term intent is for Meadow to take this place once it is ready.)

A plugin is one module that exports `voxl_hs_main`. See
[`plugins/swirl`](../plugins/swirl/Swirl.hs).

```haskell
module Rise where

import Foreign (Ptr)
import Foreign.C.Types (CInt (..))
import Voxl

foreign export ccall voxl_hs_main :: Ptr () -> IO CInt

voxl_hs_main :: Ptr () -> IO CInt
voxl_hs_main = plugin $ \app -> do
  transform <- lookupComponent app "voxl.Transform"
  addSystem app "rise" Update (write transform) $ \sys _entity place -> do
    dt <- deltaSeconds sys
    modifyRef place $ \t -> t {translation = translation t + V3 0 dt 0}
```

A system's query is a value built from `readC`, `write`, `with` and `without` and combined
applicatively, for example `(,) <$> write transform <*> readC velocity <* without frozen`. The
query decides both which entities are visited and what the system's function receives for
each, so a system cannot read a component it did not ask for, and there is nothing to cast.
A component is any type with a `Storable` instance.

`addSystem` calls its function once per matching entity. `addSystem1`, `addSystem2` and
`addSystem3` run once per frame and hand over one, two or three queries to walk with
`forEach` or look entities up in with `fetch`:

```haskell
addSystem2 app "collect" Update
  (readC transform <* with player)
  (readC transform <* with pickup)
  $ \sys players pickups ->
    forEach players $ \_ here ->
      forEach pickups $ \entity there ->
        when (distance here there < 1) (despawn sys entity)
```

[`plugins/chase`](../plugins/chase/Chase.hs) is a complete small game written this way.

Build with the script, which links the module with the bindings
([`bindings/haskell/Voxl.hs`](../bindings/haskell/Voxl.hs)) and a small piece of C that starts
the GHC runtime:

```
bindings/haskell/build-plugin.sh <name> <Module.hs> <output directory>
```

Things to know:

- **It needs GHC** (tested with 9.12) with dynamic libraries, which the standard installers
  provide. A rebuild takes a few seconds.
- **No long collection pauses.** Plugins run on GHC's threaded runtime with the non-moving
  collector for the old generation, which marks concurrently on its own thread. A frame only
  ever waits for a young-generation collection. Because the runtime is threaded, a thread
  started with `forkIO` keeps running between frames; it must not touch the engine, which
  may only be called from inside a system.
- **Top-level `IORef`s do not survive a reload**, like any plugin global. Use components or
  `statePtr`.
- **Old versions stay in memory.** GHC's runtime can't unload code, so each reload leaves the
  previous version mapped (but never called). This costs memory during development only.
- **Top-level values are never garbage collected** in a plugin. That is what makes reloading
  safe, and it means a large constant computed once stays allocated.
- **An exception in a system is logged**, and the system simply ends for that frame.

## In Rust

Depend on the `voxl_plugin` crate (not on the engine, so a rebuild takes a second or two) and
set `crate-type = ["cdylib"]`. See [`plugins/wave`](../plugins/wave/src/lib.rs).

```rust
use voxl_plugin::{App, Error, Stage, System, Transform};

fn load(app: &mut App) -> Result<(), Error> {
    let transform = app.lookup::<Transform>("voxl.Transform")?;
    app.add_system("rise", Stage::Update, &[transform.write()], rise)
}

fn rise(system: &mut System) {
    let dt = system.delta();
    while let Some((_entity, [transform])) = system.next() {
        let transform = unsafe { &mut *transform.cast::<Transform>() };
        transform.translation[1] += dt;
    }
}

voxl_plugin::export_plugin!(load);
```

## In C or C++

Include `voxl.h` and build a shared library. See [`plugins/pulse`](../plugins/pulse/pulse.c).

```
cc -shared -fPIC -I include -o libmine.dylib mine.c
```

## In other languages

Anything that can produce a shared library with C-callable exports works directly against the
header: Zig (`@cImport`), C++, Odin, Nim, D, Swift, Pascal, Go and so on. (A Go plugin must
return `VOXL_PLUGIN_KEEP_LOADED` from `voxl_plugin_flags`, as the Haskell glue does.) Bindings are a
translation of `voxl.h`, which is about 150 lines.

Languages that run in a virtual machine or interpreter (Python, Lua, C#, Java, JavaScript)
can't be loaded as a shared library by themselves. They need a small native plugin that
embeds the runtime and forwards the three functions and the system callbacks to it. Nothing in
the engine needs to change for that, but no such shim has been written yet.

Haskell, Rust and C plugins are tested today, including hot reload for each.

## Loading plugins from a host

```rust
let mut app = App::new();
app.add_plugins(DefaultPlugins);
app.world.export_component::<Cell>("demo.Cell");   // make a #[repr(C)] component findable
app.load_native_plugin("path/to/libmine.dylib")?;
app.run()
```

`App::reload_native_plugins()` reloads changed plugins immediately, and
`app.native_plugins().hot_reload = false` turns the file watching off for a shipped game.

## What the interface does not cover yet

Plugins cannot yet send or receive events, use engine resources, load assets from files
(models, textures), control the camera or lights, use physics, draw text or UI, play sound,
or see any engine component other than `voxl.Transform`. Queries have no optional terms or
change filters. A plugin cannot be removed while the app runs. The table of functions is
versioned and carries its own size, so these can be added without breaking existing plugins.
