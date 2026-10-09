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

## What isn't here yet

- Systems still run one at a time. The parallel scheduler will use the same constraints, and
  the access each system declares, to run systems that don't conflict at the same moment.
- A condition on a tuple is asked once per system, not once for the group.
- Plugins written against the C interface can't yet give constraints or conditions for their
  own systems; the host can order around them by name.
