# A live program

In a debug build the game is something you change, question and repair while it runs. Hot
reload ([plugins](PLUGINS.md), [shaders, textures](HOT_RELOAD.md), [prefabs](SCENES.md#prefabs))
is one half of that. This page is the other half: what happens when something goes wrong, and
how to hold the game still and look at it.

Everything here goes through one resource, `Live`.

## A failure pauses the game

When a system panics in a debug build, the game does not end:

1. The panic is caught. Its message, where it happened and the stack down to the system are
   kept, and logged.
2. That system is suspended: left out of later frames. Every other system carries on, and the
   frame finishes.
3. The game pauses, with the world as it was.

```text
`chase::move_player` failed in Update on frame 812
  the player has no health
  at plugins/chase/src/lib.rs:41:26
   8: chase::move_player
             at ./plugins/chase/src/lib.rs:41:26
   ...
```

Now fix it. If the system belongs to a plugin, save the fix: the plugin reloads, the new
system takes the old one's place, and since nothing is suspended any more the game carries on
by itself from the frame it stopped on. Otherwise mend the state and call `live.resume()`,
which lets suspended systems run again.

```rust
fn show_failures(live: Res<Live>) {
    for failure in live.failures() {
        println!("{failure}"); // system, stage, frame, message, location, stack
    }
}
```

- `live.catch_failures` turns this on and off. It is on in debug builds and off in release
  builds, where a panic is a panic; `VOXL_LIVE=1` or `VOXL_LIVE=0` overrides either.
- `live.pause_on_failure = false` keeps the game running without the failed system.
- A system that fails does so once, not once a frame.

What a caught failure can't promise: the system stopped half way, so what it was changing may
be half changed, and its commands for that frame are lost.

## Time

```rust
live.pause();          // the simulation stops; drawing, input and hot reload don't
live.step_frames(1);   // one frame of simulation, `live.step` long (1/60 s), then pause again
live.resume();
time.set_scale(0.25);  // quarter speed
```

While paused, the fixed stages and `Stage::Update` don't run and no game time passes
(`Time::delta` is zero). Every other stage runs, so the window stays alive, a plugin you fix
reloads, a prefab you edit rebuilds, and transforms changed from outside are drawn.

## Looking at the systems

```rust
for system in app.systems(Stage::Update) {
    println!("{} ran {} times, last {:?}", system.name, system.stats.runs, system.stats.last);
}
```

Each entry has the system's name, its sets and ordering constraints
([SCHEDULING.md](SCHEDULING.md)), whether it is suspended, and how long it takes, in the order
the stage runs them.

## What isn't here yet

- A connection to a running game from outside (a local socket): list entities, read and
  change components by name, pause and step, read failures. The pieces it will call are the
  ones on this page and the [type registry](SCENES.md#editing-by-name).
- Failures inside plugins. A Haskell exception is caught by the bindings and logged, and a
  crash in C is a crash; neither pauses the game yet.
- Stepping back: snapshots of the world to rewind to.
