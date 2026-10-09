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

- A system running on a worker thread beside others is caught the same way; the rest of its
  batch finishes first.
- `live.catch_failures` turns this on and off. It is on in debug builds and off in release
  builds, where a panic is a panic; `MIRA_LIVE=1` or `MIRA_LIVE=0` overrides either.
- `live.pause_on_failure = false` keeps the game running without the failed system.
- `MIRA_PAUSED=1` starts a game paused at its first moment, to be stepped from there: the
  same frames every time.
- A system that fails does so once, not once a frame.

What a caught failure can't promise: the system stopped half way, so what it was changing may
be half changed, and its commands for that frame are lost.

Plugins are part of this. An exception in a Haskell system, a panic in a Rust plugin's system,
or a C system calling `system_fail`, pauses the game the same way, and saving the fixed plugin
resumes it; see [PLUGINS.md](PLUGINS.md#failures).

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

## Stepping back

```rust
app.world.resource_mut::<History>().recording = true;   // a snapshot every 30 frames
// … later, when something has gone wrong …
History::rewind(&mut app.world, 300);                   // about 300 frames back; pauses there
```

While `History` is recording it keeps snapshots of the world (`every` frames apart, the last
`keep` of them). Going back to one puts the world as it was and pauses the game there, so you
can look, change the code or the world, and play forward again from that moment.

- Entities that are still alive keep their ids and get their components back. Entities made
  since are despawned. Entities that had died come back under new ids, and references to them
  follow.
- A snapshot is what a [scene](SCENES.md) is: registered components and registered resources.
  What isn't registered stays as it is now: a plugin's components it hasn't described, the
  physics solver's own memory of contacts, what signals have timed.
- Snapshots after the one gone back to are forgotten: that future didn't happen.
- Recording is off by default, because each snapshot is the whole scene.

From outside: `mira-debug record on=true`, `mira-debug history`, `mira-debug rewind frames=300`.

## Looking at the systems

```rust
for system in app.systems(Stage::Update) {
    println!("{} ran {} times, last {:?}", system.name, system.stats.runs, system.stats.last);
}
```

Each entry has the system's name, its sets and ordering constraints
([SCHEDULING.md](SCHEDULING.md)), whether it is suspended, how long it takes, and what it
reads and writes (`access`: components and resources by name), in the order the stage runs
them.

## The debug connection

A running game can be questioned and changed from outside, over a local socket:

```sh
MIRA_DEBUG=127.0.0.1:7878 cargo run --example host -- chase   # the game
cargo run --bin mira-debug -- status                           # from another terminal
cargo run --bin mira-debug -- entities with=mira.Camera
cargo run --bin mira-debug -- get entity=4294967297
cargo run --bin mira-debug -- set entity=4294967297 component=mira.Transform path=translation.1 value=3.5
cargo run --bin mira-debug -- pause
cargo run --bin mira-debug -- step frames=10
cargo run --bin mira-debug -- signals
cargo run --bin mira-debug -- signal_force name=blue.contesting value=false
```

Any app with `DefaultPlugins` listens where `MIRA_DEBUG` says, if it is set; any app at all
can call `app.listen_for_debugger("127.0.0.1:7878")`. Listen only on the machine itself:
whoever can connect can change the game.

The protocol is one JSON object per line each way, so anything can speak it (`nc 127.0.0.1
7878` will do). A request is `{"cmd": …}` with the command's arguments, and optionally an
`id` that the answer repeats; the answer is `{"ok": …}` or `{"error": "why"}`. Requests are
answered at the start of a frame, whether or not the game is paused.

| Command | Arguments | Answer |
| --- | --- | --- |
| `status` | | frame, seconds, time scale, paused, frames of stepping left, failures, entities |
| `quit` | | asks the game to stop, as closing its window would |
| `pause`, `resume` | | |
| `step` | `frames` (1) | runs that many frames, then pauses |
| `run_until` | `signal`, `max_frames` (600) | steps until the signal is true or the frames run out; `status` then says `reached` |
| `input` | `key` or `mouse_button` with `action` (tap, press, release) and `frames`; `mouse_motion`, `mouse_position`, `mouse_scroll`; `text` to type into the interface | plays input at the start of the next simulated frame |
| `time_scale` | `scale` | |
| `record` | `on`, `every` (frames), `keep` | what recording is set to |
| `history` | | the moments that can be gone back to |
| `rewind` | `frames` (60) or `to_frame` | steps back, pauses, and says where it landed |
| `profile` | `systems` (10) | frame times, what each stage took, and the costliest systems |
| `failures` | | every caught failure, with its stack |
| `watch` | `on` (true) | from then on, events are sent to this connection as they happen |
| `systems` | `stage` (all) | each stage's systems in order, with constraints and timings |
| `types` | | the names of registered components and resources |
| `schema` | `name` | the shape of a registered type |
| `unregistered` | | components and resources the world holds that are not registered, and so can't be reached by name |
| `describe` | `limit` (40) | the scene in words: the camera, then each placed entity with where it is and where on screen it shows |
| `entities` | `with` (a component), `limit` (200) | entities and what each has |
| `get` | `entity`, `component` (all) | the component's value, or every component's |
| `set` | `entity`, `component`, `value`, `path` (the whole component) | |
| `remove` | `entity`, `component` | |
| `spawn` | `components`: a map of values by name | the new entity |
| `despawn` | `entity` | how many went: it and everything below it |
| `resource` | `name`, `value` (to set it) | the resource's value |
| `save_scene` | `path` | how many entities were saved |
| `reload_plugins` | | how many reloaded |
| `signals` | | the [signal graph](SIGNALS.md): every node, its inputs and value |
| `signal_set` | `name`, `value` | defines or sets a constant |
| `signal_force` | `name`, `value` (none lets it go) | |
| `signal_connect` | `name`, `input`, `to` | |
| `signal_define` | `name`, `op`, `inputs`, and `value` or `seconds` where the op has one | |
| `signal_rename` | `name`, `to` | a rule's name; what reads it follows |
| `signal_remove` | `name` | |
| `signals_save`, `signals_load` | `path` | the rules (everything but sources) to and from a JSON file |

A client can also ask to be told what happens instead of asking over and over. After
`{"cmd": "watch"}` the game sends that connection a line whenever a system fails (with its
stack), a signal changes value, or the game pauses or resumes; requests on the same connection
are still answered, in among the news. `{"cmd": "watch", "on": false}` stops it.
`mira-debug events` prints them as they come:

```text
{"event": "signal", "name": "blue.contesting", "value": true}
{"event": "failure", "failure": {"system": "chase::move_player", "message": "…", "stack": […]}}
{"event": "paused", "frame": 812}
```

Entities are their numbers as `entities` lists them. Values have the shape they have in a
[scene file](SCENES.md). The operations are `constant`, `and`, `or`, `not`, `count`, `sum`,
`select`, `timer`, `held_for`, `less`, `less_or_equal`, `equal`, `greater_or_equal`,
`greater`.

## Where the time goes

```sh
cargo run --bin mira-debug -- profile
```

answers with how long recent frames took (mean and worst of the last 240, and the frame rate
that would allow), what each stage took in the last frame, and the systems that cost the
most. The frame time is the engine's own work in a frame, not the wait for the display. From
code it is the `FrameStats` resource and `App::systems`.

## What isn't here yet

- A timeline of a frame (which system ran on which thread, when), and marks inside a system.
- Tools on top of the connection: the signal graph viewer, an inspector, the editor.
- A crash in a plugin's native code (a null pointer in C) is still a crash. Failures a plugin
  can report are caught: see below.
- Snapshots that hold only what changed, so recording can stay on in a big world; scrubbing
  back and forth instead of only back.
