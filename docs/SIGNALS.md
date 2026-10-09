# Signals

A signal is a value derived from the world and from other signals, which the engine keeps
true. It is how to write a rule once, as what it is, instead of as handlers that set and
clear a shared flag and one day disagree.

The example this was built around is a bug in Age of Empires 4. A team holding every sacred
site wins when its clock runs out, and the clock stops while an enemy stands on a site. Step
onto one site: the clock stops. Step onto a second, lose the units on the first: the clock
starts again, with an enemy still standing on a site. Somewhere a flag was cleared by the
units leaving one site that should have stayed set for the other.

The rule, said plainly: *the clock runs while red holds every site and no blue unit stands on
any site red holds.* As signals:

```rust
app.add_signal("red.holds_all", |sites: Query<&Site>| {
    sites.iter().all(|site| site.holder == Some(Team::Red))
})
.add_signal("blue.contesting", |units: Query<(&Team, &Transform)>, sites: Query<(&Site, &Transform)>| {
    units.iter().any(|(team, at)| {
        *team == Team::Blue && sites.iter().any(|(site, centre)| {
            site.holder == Some(Team::Red) && at.translation.distance(centre.translation) < 2.0
        })
    })
});

let signals = app.world.resource_mut::<Signals>();
signals.define("blue.calm", Op::Not, ["blue.contesting"]);
signals.define("red.clock.running", Op::And, ["red.holds_all", "blue.calm"]);
signals.define("red.lost_a_site", Op::Not, ["red.holds_all"]);
signals.define("red.clock", Op::Timer, ["red.clock.running", "red.lost_a_site"]);
signals.set("win_after", 600.0);
signals.define("red.wins", Op::Compare(Compare::GreaterOrEqual), ["red.clock", "win_after"]);
```

No code sets the clock or a paused flag, so there is nothing to get out of step: units can
walk on and off and die in any order. That game is a test in `src/signal.rs`.

## The graph

Signals are nodes with names, in the `Signals` resource.

- A **source** reads the world: `app.add_signal(name, f)`, where `f` is a function with
  system parameters returning `bool` (the same shape as a
  [run condition](SCHEDULING.md#conditions)), or `signals.measure(name, f)` for a number.
- Every other node is data: an operation and the names of its inputs,
  `signals.define(name, op, inputs)`.

| `Op` | Value |
| --- | --- |
| `Constant(v)` | `v`; `signals.set(name, v)` is short for it |
| `And`, `Or`, `Not` | as they say |
| `Count` | how many inputs are true |
| `Sum` | the inputs added up |
| `Compare(how)` | the first input against the second |
| `Select` | the second input if the first is true, else the third |
| `HeldFor(seconds)` | true once its input has been true that long without a break |
| `Timer` | seconds its first input has been true; a true second input resets it |

A value is a truth or a number; where one is wanted and the other given, true is 1 and any
number but 0 is true. Inputs may be defined after the nodes that read them. Two nodes that
read each other are allowed: one of them reads the other a frame late, and says so.

Signals are worked out once a frame in `Stage::PreUpdate`, in the set `"signals"`, from the
world as it is then.

## Using them

```rust
fn hud(signals: Res<Signals>) {
    let left = 600.0 - signals.number("red.clock");
    let stopped = !signals.is_true("red.clock.running");
}

app.add_systems(Stage::Update, (
    announce_victory.run_if(signal_became_true("red.wins")),
    play_alarm.run_if(signal("blue.contesting")),
));
```

`signals.changed(name)`, `became_true` and `became_false` are about the last update, and each
change is also sent as a `SignalChanged` event.

## Watching and changing them live

Because every node but a source is data, the graph can be read and rewired while the game
runs, which is what a signal graph viewer will do:

```rust
for node in signals.graph() {
    // name, kind ("source", "and", "timer", …), inputs, value, changed, forced, problem
}
signals.set("win_after", 30.0);                        // change a setting
signals.connect("red.clock.running", 1, "always");     // connect an input elsewhere
signals.force("blue.contesting", Some(Signal::Bool(false))); // hold a node's output
signals.force("blue.contesting", None);                // let it go
signals.define("red.clock.running", Op::Or, ["red.holds_all", "cheat"]); // replace a rule
signals.set_op("blue.calm", Op::Count);                // the same inputs, another rule
signals.set_inputs("red.wins", ["red.clock"]);          // the same rule, other inputs
```

Redefining a node keeps its value and what it has timed, so changing a rule doesn't restart
its clocks. An input connected to a name that doesn't exist reads false and is reported in
the node's `problem`. `signals.to_value()` is the whole graph as plain data.

## The viewer

With the game listening for debuggers ([LIVE.md](LIVE.md#the-debug-connection)), this draws
the graph in a terminal and keeps it up to date as you play. The sacred-site game is an
example to try it on: two sites seen from above through an orthographic camera, scouts
wandering on and off them, and a bar that fills as red's clock runs, all drawn from the
signals (`--headless` runs the same game with no window):

```sh
cargo run --example sacred_sites        # the game
cargo run --bin mira-debug -- watch     # in another terminal
```

```text
frame 4821  80.4s  running  failures: 0

○ red.wins = false  (compare GreaterOrEqual)
├─ ● red.clock = 7  (timer)
│  ├─ ○ red.clock.running = false  (and) *
│  │  ├─ ● red.holds_all = true  (source)
│  │  └─ ○ blue.calm = false  (not) *
│  │     └─ ● blue.contesting = true  (source) *
│  └─ ○ red.lost_a_site = false  (not)
│     └─ ● red.holds_all = true  (source)
└─ ● win_after = 600  (constant)
```

Each signal nothing else reads is at the left, with what it reads below it. `●` is true (or
not zero), `*` changed on the last update, `!` is forced. Change the graph from another
terminal and watch it follow:

```sh
cargo run --bin mira-debug -- signal_set name=win_after value=30
cargo run --bin mira-debug -- signal_force name=blue.contesting value=false
cargo run --bin mira-debug -- signal_connect name=red.clock.running input=1 to=always
cargo run --bin mira-debug -- signal_define name=blue.calm op=held_for seconds=3 inputs='["blue.away"]'
```

## From a plugin

A plugin gives the graph its own facts by setting signals from a system, builds rules on them
as data, and reads any signal by name:

```haskell
addSystem_ app "rules" Update $ \sys -> do
  setSignal sys "blue.contesting" =<< anyBlueOnARedSite sys   -- a fact, from the plugin's own queries
  defineRule sys "red.clock" $
    timerReset (sig "red.holds_all" .&&. notS (sig "blue.contesting"))
               (notS (sig "red.holds_all"))
  defineRule sys "red.wins" (sig "red.clock" .>=. number 600)
  won <- signalIsTrue sys "red.wins"
  when won (announce sys)
```

In Haskell a rule is an expression: `sig` names a signal, `.&&.`, `.||.`, `notS` and the
comparisons (`.<.`, `.>=.`, …) combine them, `+` adds, and `timer`, `timerReset`, `heldFor`,
`countS` and `selectS` are the rest. `defineRule` turns the expression into nodes, naming its
parts after the rule (`red.clock#1`, …), so the whole of it shows in the graph and can be
rewired there. `defineSignal` defines one node at a time, as the other languages do.

In C these are `signal_set`, `signal_get` and `signal_define` in `include/mira.h`; in Rust,
`System::set_signal`, `signal`, `define_signal`. What a plugin sets in one frame the graph has
in the next. Defining the same rule again changes nothing, so a plugin can define its rules
every frame or once; either way they are there again after a reload, with their clocks intact.

## What isn't here yet

- A graphical viewer, with the graph laid out and edited by hand: a panel of the editor.
- Sources written in a plugin as functions the engine calls (a plugin sets its facts from a
  system instead).
- Signals that carry more than a truth or a number (an entity, a set of entities).
- Saving a graph to a file and loading it, as scenes are.
- Sources are asked every frame; they are not yet skipped when what they read hasn't changed.
