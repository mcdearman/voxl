# When systems run

A frame is a fixed list of stages (`Stage::First` … `Stage::Render`). Inside a stage, systems
run one at a time in the order they were added, and each one's commands are applied before
the next starts. This page is about saying more than that: this system after that one, these
only while the game is being played, those once when a level starts.

## Order

```rust
app.add_systems(Stage::Update, (
    follow_player.after(move_player),            // wherever `move_player` was added
    (read_input, move_player, collide).chain(),  // one after another, as written
    (gravity, drag).in_set("forces"),
    integrate.after("forces"),                   // after every system in the set
));
```

- `a.before(b)` and `a.after(b)` take a system (the function) or the name of a set. They hold
  within one stage; for a different stage, use the stage.
- `.in_set("name")` puts systems in a set, so that code elsewhere (a plugin, say) can order
  itself around a group without naming what is in it.
- `.chain()` on a tuple runs it in the order written.
- Naming a system or a set that isn't in the stage does nothing, so a plugin can ask to run
  after another plugin's system whether or not that plugin is loaded.
- Everything else keeps the order it was added in: a system waits only for the ones it must
  follow.
- Constraints that go round in a circle panic when the app starts, naming the systems.

A system loaded from a [native plugin](PLUGINS.md) is named `plugin::system` (the names it
registered with), and can be ordered against by that name as a set would be.

## Conditions

```rust
app.add_systems(Stage::Update, (
    spawn_waves.run_if(in_state(Game::Playing)),
    tick_round.run_if(resource_exists::<RoundTimer>),
    (ai, pathfinding).run_if(not(resource_exists::<Paused>)),
    autosave.run_if(|time: Res<Time>| time.elapsed_secs() > 60.0),
));
```

A condition is a function, or a closure, that takes system parameters and returns `bool`. The
system runs only on frames when every one of its conditions holds. Given to a tuple, each
system gets its own copy.

A system that was held back misses nothing: when it next runs, `Changed` and `Added` report
everything since the last time it actually ran.

## States

Which part of the game is running, with systems tied to it:

```rust
#[derive(Clone, Copy, PartialEq, Debug)]
enum Game { Menu, Playing, Paused }

app.init_state(Game::Menu)
    .on_enter(Game::Playing, spawn_level)
    .on_exit(Game::Playing, despawn_level)
    .add_systems(Stage::Update, move_player.run_if(in_state(Game::Playing)));

fn start(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Game>>) {
    if keys.just_pressed(KeyCode::Enter) {
        next.set(Game::Playing);
    }
}
```

- `State<Game>` is a resource holding the current state; read it with `.get()`.
- `NextState<Game>::set` asks for a change. It happens early in the next frame
  (`Stage::PreUpdate`): the old state's exit systems run, seeing the old state; then the
  state changes and the new one's enter systems run.
- The first state's enter systems run on the first frame.
- Setting the state the game is already in leaves and re-enters it, which restarts a level.
- An app can have several kinds of state at once (`Game` and `Network`, say); each is its
  own type.

## Hooks

Some things should happen exactly when a component arrives on an entity or leaves it, not a
frame later when a system gets round to noticing. A hook is code the world runs at that
moment:

```rust
app.on_add::<Burning>(|world, entity| {
    world.insert(entity, Smoke::default());
})
.on_remove::<Burning>(|world, entity| {
    world.remove::<Smoke>(entity);
});
```

- `on_add` runs when a component is put on an entity that didn't have one (replacing one is
  not an arrival), once everything inserted with it is in place.
- `on_remove` runs just before a component leaves, whether it is removed or its entity is
  despawned; the component is still there to be read.
- A hook has the whole world, and may insert, remove, spawn and despawn; hooks for what it
  changes run in turn. A hook is not told again about a departure it is already handling.
- Changes made through `Commands` reach hooks when the commands are applied.

Use a hook to keep two things in step; use a system with `Added<T>` or `Changed<T>` for work
that can wait until its stage.

`on_change` is the third hook, for a component that is written:

```rust
app.on_change::<Health>(|world, entity| {
    let health = world.get::<Health>(entity).map_or(0.0, |health| health.0);
    world.insert(entity, HealthBar::showing(health));
});
```

Writing a component is only a borrow, so there is no moment to run code at; the hooks are
told when the stage whose systems did the writing has run, before the next stage. Each
entity is told of once however often it was written, an arrival counts as a writing, and
what the hooks themselves write to that component is not told again.

## Observers

An event can be aimed at one entity, and heard at once by whatever observes that kind of
event:

```rust
struct Hit { damage: f32 }

app.observe::<Hit>(|world, entity, hit| {
    if let Some(health) = world.get_mut::<Health>(entity) {
        health.0 -= hit.damage;
    }
});

// From a system:
commands.entity(target).trigger(Hit { damage: 4.0 });
// With the world in hand:
world.trigger(target, Hit { damage: 4.0 });
```

`world.observe_entity::<Hit>(door, …)` listens on one entity only, after those that listen
anywhere, and is forgotten when the entity is despawned. Observers run in the order they
were added, with the whole world, and may trigger further events. Where `Events<T>` are
read by systems in their stage, a frame at a time, a triggered event is heard there and
then, by code that is told which entity it is about.

## Running at the same moment

Systems that touch nothing in common run at the same moment, on several threads. Nothing has
to be asked for: the engine knows what every system reads and writes from its parameters, and
groups each stage into batches.

A system joins the batch before it unless

- one of them writes something the other reads or writes (a component on entities both could
  reach, or a resource); queries made disjoint with `With`/`Without` don't clash;
- it was told to run after something in the batch;
- something in the batch queues commands, since a later system is meant to see what an
  earlier one spawned or inserted. So a system with `Commands` is the last of its batch, and
  its commands are applied before the next batch starts, exactly as when taking turns;
- it takes the whole world (`&mut World`) or is a plugin's system: those run alone.

The results are the same as running one at a time, in the order written; only the time
differs. What your code has to do is be thread-safe in the ways the compiler already asks: a
resource read through `Res` or `ResMut` must be `Send + Sync`, and what a system keeps between
runs (`Local`, queued commands) must be `Send`.

Some things must stay on the main thread: a window, for one. `world.pin_to_main_thread::<T>()`
keeps every system that uses `T` there, while the rest of its batch runs elsewhere. The window
is pinned already.

To see the plan for a running game, `mira-debug systems` shows each system's `batch` and
whether it is `main_thread`. `MIRA_THREADS=1` runs everything on one thread, which is the way
to find out whether a bug is about threads; `Schedule::set_parallel(false)` does the same for
one schedule. `cargo run --release --example parallel_bench` measures the difference on your
machine: eight systems over 100,000 entities each ran 5.2 times faster on a ten-core M2 Pro.

A panic on a worker thread is caught like any other in a debug build ([LIVE.md](LIVE.md)): the
system is suspended, the rest of its batch finishes, and the game pauses.

## One system on many threads

A system whose query covers many entities, each one's work independent of the rest, can
share that work between the same threads:

```rust
fn steer(mut boids: Query<(&Transform, &mut Velocity)>, flock: Res<Flock>) {
    boids.par_for_each_mut(|(at, mut velocity)| velocity.0 = flock.pull(at));
}
```

`par_for_each` is the same for a query that only reads. The closure runs on several threads
at once, so what it takes from outside must be shareable (a `Res`, an atomic, a lock), and
the order entities are met in is not kept. Fewer than a few hundred entities are done where
they stand: handing them out would cost more than doing them. Systems in one batch may each
do this at the same moment; a thread waiting for its own shares does other queued work
meanwhile.

## What isn't here yet

- Batches are made in the order systems are written, so two systems that could share a
  batch but have a clashing one between them don't. Queued commands end a batch; there is no
  way yet to say that a later system needn't see them.
- Hooks and observers are for Rust types; a plugin can't yet hook its own components or
  observe events.
- A condition on a tuple is asked once per system, not once for the group.
- Plugins can order their systems ([PLUGINS.md](PLUGINS.md#order)) but can't yet give them
  run conditions.
