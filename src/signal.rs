//! Signals: values derived from the world and from each other, which the engine keeps true.
//!
//! A rule such as "the win timer is paused while anyone on your team stands on any sacred
//! site the other team holds" can be written as handlers that set and clear a flag when units
//! step on and off, and then it is wrong the first time two things happen in an order nobody
//! thought of. Or it can be written once, as what it is: a function of the world.
//!
//! ```ignore
//! app.add_signal("blue.contesting", |units: Query<(&Team, &Transform)>, sites: Query<&Site>| {
//!     units.iter().any(|(team, at)| *team == Team::Blue && sites.iter().any(|s| s.held_by(Team::Red) && s.contains(at)))
//! });
//! let signals = app.world.resource_mut::<Signals>();
//! signals.define("red.clock.running", Op::And, ["red.holds_sites", "blue.calm"]);
//! signals.define("blue.calm", Op::Not, ["blue.contesting"]);
//! signals.define("red.clock", Op::Timer, ["red.clock.running"]);
//! ```
//!
//! Signals form a graph of named nodes. *Sources* read the world and are written in code;
//! everything else is data: an [`Op`] and the names of its inputs. So the graph can be listed
//! with every node's current value ([`Signals::graph`]), and changed while the game runs: a
//! constant set, an input connected elsewhere, a node's output forced. Nothing holds state
//! that could go stale, except the two nodes whose meaning is time (`HeldFor`, `Timer`).

use std::collections::HashMap;

use crate::{
    app::{App, Plugin, Stage},
    ecs::{BoxedCondition, IntoCondition, IntoSystems, Res, World},
    reflect::Value,
    time::Time,
};

/// What a signal carries: a truth or a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Signal {
    Bool(bool),
    Number(f64),
}

impl Signal {
    /// A number is true when it isn't zero.
    pub fn is_true(self) -> bool {
        match self {
            Signal::Bool(b) => b,
            Signal::Number(n) => n != 0.0,
        }
    }

    /// True is 1 and false is 0.
    pub fn number(self) -> f64 {
        match self {
            Signal::Bool(b) => b as u8 as f64,
            Signal::Number(n) => n,
        }
    }
}

impl From<bool> for Signal {
    fn from(value: bool) -> Self {
        Signal::Bool(value)
    }
}

impl From<f64> for Signal {
    fn from(value: f64) -> Self {
        Signal::Number(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Less,
    LessOrEqual,
    Equal,
    GreaterOrEqual,
    Greater,
}

/// How a node that isn't a source gets its value from its inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// A fixed value, with no inputs: a setting, or a stand-in while developing.
    Constant(Signal),
    /// True when every input is. With no inputs, true.
    And,
    /// True when any input is.
    Or,
    /// True when its one input is false.
    Not,
    /// How many inputs are true.
    Count,
    /// The inputs added up.
    Sum,
    /// Its first input compared with its second.
    Compare(Compare),
    /// Its second input when its first is true, otherwise its third.
    Select,
    /// True once its input has been true for this many seconds without a break.
    HeldFor(f64),
    /// Seconds of game time for which its first input has been true: a clock that runs only
    /// while something holds. A true second input, if connected, sets it back to zero.
    Timer,
}

impl Op {
    fn label(&self) -> String {
        match self {
            Op::Constant(_) => "constant".to_owned(),
            Op::Compare(compare) => format!("compare {compare:?}"),
            Op::HeldFor(seconds) => format!("held for {seconds}"),
            other => format!("{other:?}").to_lowercase(),
        }
    }
}

/// A numeric source: reads the world and gives a number.
type Measure = Box<dyn FnMut(&mut World) -> f64 + Send>;

enum Kind {
    Truth(Exclusive<BoxedCondition>),
    Measure(Exclusive<Measure>),
    Op(Op),
}

/// A value only ever reached through `&mut`, which makes sharing a reference to it between
/// threads harmless: nothing can be done with one. Lets `Signals` be a resource any system
/// may read while its sources are only `Send`.
struct Exclusive<T>(T);

// SAFETY: a `&Exclusive<T>` gives no access to the `T`; the only way in is `get_mut`, which
// needs `&mut`.
unsafe impl<T: Send> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    fn get_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

struct Node {
    name: String,
    kind: Kind,
    inputs: Vec<String>,
    value: Signal,
    /// What a timing node has accumulated.
    seconds: f64,
    /// When set, the node's output is this whatever it would work out.
    forced: Option<Signal>,
    changed: bool,
    problem: Option<String>,
}

/// A node as seen from outside: for a graph viewer, a debugger, a log.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeInfo {
    pub name: String,
    /// `source`, or the operation: `and`, `not`, `timer`, `constant`, …
    pub kind: String,
    pub inputs: Vec<String>,
    pub value: Signal,
    /// Whether the value changed on the last update.
    pub changed: bool,
    pub forced: bool,
    /// Why the node can't be worked out properly, if it can't: an input that doesn't exist,
    /// a circle.
    pub problem: Option<String>,
}

/// A signal's value changed. Sent as an event when the app has `Events<SignalChanged>`.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalChanged {
    pub name: String,
    pub value: Signal,
}

/// Every signal. A resource.
#[derive(Default)]
pub struct Signals {
    nodes: Vec<Node>,
    names: HashMap<String, usize>,
    /// The order nodes are worked out in: inputs before what reads them. Empty when stale.
    order: Vec<usize>,
    changes: Vec<SignalChanged>,
}

impl Signals {
    fn put(&mut self, name: &str, kind: Kind, inputs: Vec<String>) {
        match self.names.get(name) {
            // Redefined: its value and what it has timed carry over, so that changing a
            // rule while the game runs doesn't restart its clocks.
            Some(&index) => {
                // The order only depends on who reads whom.
                if self.nodes[index].inputs != inputs {
                    self.order.clear();
                }
                self.nodes[index].kind = kind;
                self.nodes[index].inputs = inputs;
            }
            None => {
                self.order.clear();
                self.names.insert(name.to_owned(), self.nodes.len());
                self.nodes.push(Node {
                    name: name.to_owned(),
                    kind,
                    inputs,
                    value: Signal::Bool(false),
                    seconds: 0.0,
                    forced: None,
                    changed: false,
                    problem: None,
                });
            }
        }
    }

    /// Defines a true-or-false source: a function with system parameters that returns
    /// `bool`, asked once every update.
    pub fn source<M>(&mut self, name: &str, read: impl IntoCondition<M>) {
        self.put(
            name,
            Kind::Truth(Exclusive(read.into_condition())),
            Vec::new(),
        );
    }

    /// Defines a numeric source, read from the world once every update.
    pub fn measure(&mut self, name: &str, read: impl FnMut(&mut World) -> f64 + Send + 'static) {
        self.put(name, Kind::Measure(Exclusive(Box::new(read))), Vec::new());
    }

    /// Defines a node worked out from other signals, or replaces the definition of one.
    /// Inputs are names, and may be defined later.
    pub fn define<I: Into<String>>(
        &mut self,
        name: &str,
        op: Op,
        inputs: impl IntoIterator<Item = I>,
    ) {
        self.put(
            name,
            Kind::Op(op),
            inputs.into_iter().map(Into::into).collect(),
        );
    }

    /// Defines a constant, or sets one that exists.
    pub fn set(&mut self, name: &str, value: impl Into<Signal>) {
        self.define::<String>(name, Op::Constant(value.into()), []);
    }

    /// The value of a constant, if the named signal is one.
    pub fn constant(&self, name: &str) -> Option<Signal> {
        match &self.nodes[*self.names.get(name)?].kind {
            Kind::Op(Op::Constant(value)) => Some(*value),
            _ => None,
        }
    }

    /// Whether a node is defined exactly this way already.
    pub fn is_defined_as(&self, name: &str, op: Op, inputs: &[String]) -> bool {
        self.names.get(name).is_some_and(|&index| {
            let node = &self.nodes[index];
            matches!(&node.kind, Kind::Op(existing) if *existing == op) && node.inputs == inputs
        })
    }

    /// Connects one input of a node to a different signal. Returns whether there was such an
    /// input.
    pub fn connect(&mut self, name: &str, input: usize, to: &str) -> bool {
        let Some(slot) = self
            .names
            .get(name)
            .and_then(|&index| self.nodes[index].inputs.get_mut(input))
        else {
            return false;
        };
        *slot = to.to_owned();
        self.order.clear();
        true
    }

    /// Makes a node's output a fixed value whatever it would work out, or lets it go again
    /// with `None`: for trying something out while the game runs.
    pub fn force(&mut self, name: &str, value: Option<Signal>) -> bool {
        match self.names.get(name) {
            Some(&index) => {
                self.nodes[index].forced = value;
                true
            }
            None => false,
        }
    }

    /// Removes a node. Nodes that read it report a problem and read false.
    pub fn remove(&mut self, name: &str) -> bool {
        let Some(index) = self.names.remove(name) else {
            return false;
        };
        self.nodes.remove(index);
        for other in self.names.values_mut() {
            *other -= (*other > index) as usize;
        }
        self.order.clear();
        true
    }

    /// A signal's value as of the last update.
    pub fn get(&self, name: &str) -> Option<Signal> {
        self.names.get(name).map(|&index| self.nodes[index].value)
    }

    /// Whether a signal is true. One that doesn't exist is not.
    pub fn is_true(&self, name: &str) -> bool {
        self.get(name).is_some_and(Signal::is_true)
    }

    /// A signal as a number; 0 if it doesn't exist.
    pub fn number(&self, name: &str) -> f64 {
        self.get(name).map_or(0.0, Signal::number)
    }

    /// Whether a signal's value changed on the last update.
    pub fn changed(&self, name: &str) -> bool {
        self.names
            .get(name)
            .is_some_and(|&index| self.nodes[index].changed)
    }

    /// Whether a signal went from false to true on the last update.
    pub fn became_true(&self, name: &str) -> bool {
        self.changed(name) && self.is_true(name)
    }

    /// Whether a signal went from true to false on the last update.
    pub fn became_false(&self, name: &str) -> bool {
        self.changed(name) && !self.is_true(name)
    }

    /// The whole graph, in the order nodes were defined.
    pub fn graph(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .map(|node| NodeInfo {
                name: node.name.clone(),
                kind: match &node.kind {
                    Kind::Truth(_) | Kind::Measure(_) => "source".to_owned(),
                    Kind::Op(op) => op.label(),
                },
                inputs: node.inputs.clone(),
                value: node.value,
                changed: node.changed,
                forced: node.forced.is_some(),
                problem: node.problem.clone(),
            })
            .collect()
    }

    /// The graph as plain data, for a viewer outside the game.
    pub fn to_value(&self) -> Value {
        let nodes = self.graph().into_iter().map(|node| {
            Value::Map(vec![
                ("name".to_owned(), Value::Text(node.name)),
                ("kind".to_owned(), Value::Text(node.kind)),
                (
                    "inputs".to_owned(),
                    Value::List(node.inputs.into_iter().map(Value::Text).collect()),
                ),
                (
                    "value".to_owned(),
                    match node.value {
                        Signal::Bool(b) => Value::Bool(b),
                        Signal::Number(n) => Value::Float(n),
                    },
                ),
                ("changed".to_owned(), Value::Bool(node.changed)),
                ("forced".to_owned(), Value::Bool(node.forced)),
                (
                    "problem".to_owned(),
                    node.problem.map_or(Value::Null, Value::Text),
                ),
            ])
        });
        Value::List(nodes.collect())
    }

    /// Puts the nodes in an order where each comes after its inputs. A node on a circle is
    /// placed where the circle was noticed, and reads last update's value of the input that
    /// closes it.
    fn sort(&mut self) {
        fn visit(
            index: usize,
            signals: &Signals,
            state: &mut [u8],
            order: &mut Vec<usize>,
            circles: &mut Vec<usize>,
        ) {
            match state[index] {
                2 => return,
                1 => return circles.push(index),
                _ => {}
            }
            state[index] = 1;
            for input in &signals.nodes[index].inputs {
                if let Some(&input) = signals.names.get(input) {
                    visit(input, signals, state, order, circles);
                }
            }
            state[index] = 2;
            order.push(index);
        }
        let mut state = vec![0u8; self.nodes.len()];
        let (mut order, mut circles) = (Vec::new(), Vec::new());
        for index in 0..self.nodes.len() {
            visit(index, self, &mut state, &mut order, &mut circles);
        }
        for node in &mut self.nodes {
            node.problem = None;
        }
        for index in circles {
            self.nodes[index].problem =
                Some("is part of a circle; one input is a frame late".to_owned());
        }
        self.order = order;
    }

    /// Works every signal out again from the world as it is now. `seconds` is how much game
    /// time has passed since the last update, for the nodes that time things.
    pub fn update(&mut self, world: &mut World, seconds: f64) {
        if self.order.len() != self.nodes.len() {
            self.sort();
        }
        for position in 0..self.order.len() {
            let index = self.order[position];
            let mut missing = None;
            let inputs: Vec<Signal> = self.nodes[index]
                .inputs
                .iter()
                .map(|name| {
                    self.get(name).unwrap_or_else(|| {
                        missing = Some(format!("there is no signal `{name}`"));
                        Signal::Bool(false)
                    })
                })
                .collect();
            let input = |at: usize| inputs.get(at).copied().unwrap_or(Signal::Bool(false));
            let node = &mut self.nodes[index];
            let worked_out = match &mut node.kind {
                Kind::Truth(read) => Signal::Bool(read.get_mut().check(world)),
                Kind::Measure(read) => Signal::Number(read.get_mut()(world)),
                Kind::Op(op) => match *op {
                    Op::Constant(value) => value,
                    Op::And => Signal::Bool(inputs.iter().all(|i| i.is_true())),
                    Op::Or => Signal::Bool(inputs.iter().any(|i| i.is_true())),
                    Op::Not => Signal::Bool(!input(0).is_true()),
                    Op::Count => {
                        Signal::Number(inputs.iter().filter(|i| i.is_true()).count() as f64)
                    }
                    Op::Sum => Signal::Number(inputs.iter().map(|i| i.number()).sum()),
                    Op::Compare(compare) => {
                        let (a, b) = (input(0).number(), input(1).number());
                        Signal::Bool(match compare {
                            Compare::Less => a < b,
                            Compare::LessOrEqual => a <= b,
                            Compare::Equal => a == b,
                            Compare::GreaterOrEqual => a >= b,
                            Compare::Greater => a > b,
                        })
                    }
                    Op::Select => {
                        if input(0).is_true() {
                            input(1)
                        } else {
                            input(2)
                        }
                    }
                    Op::HeldFor(needed) => {
                        node.seconds = if input(0).is_true() {
                            node.seconds + seconds
                        } else {
                            0.0
                        };
                        Signal::Bool(input(0).is_true() && node.seconds >= needed)
                    }
                    Op::Timer => {
                        if input(1).is_true() {
                            node.seconds = 0.0;
                        } else if input(0).is_true() {
                            node.seconds += seconds;
                        }
                        Signal::Number(node.seconds)
                    }
                },
            };
            if missing.is_some() && node.problem != missing {
                log::warn!(
                    "signal `{}`: {}",
                    node.name,
                    missing.as_deref().unwrap_or_default()
                );
            }
            if missing.is_some()
                || node
                    .problem
                    .as_deref()
                    .is_some_and(|p| p.starts_with("there is no"))
            {
                node.problem = missing;
            }
            let value = node.forced.unwrap_or(worked_out);
            node.changed = value != node.value;
            node.value = value;
            if node.changed {
                self.changes.push(SignalChanged {
                    name: node.name.clone(),
                    value,
                });
            }
        }
    }

    /// The changes since this was last called, in the order they happened.
    pub fn take_changes(&mut self) -> Vec<SignalChanged> {
        std::mem::take(&mut self.changes)
    }
}

/// Works the signals out once a frame, and sends [`SignalChanged`] events for the ones that
/// changed.
pub fn update_signals(world: &mut World) {
    if !world.contains_resource::<Signals>() {
        return;
    }
    let seconds = world
        .get_resource::<Time>()
        .map_or(0.0, |time| time.delta().as_secs_f64());
    let changes = world.resource_scope(|world, signals: &mut Signals| {
        signals.update(world, seconds);
        signals.take_changes()
    });
    if let Some(events) = world.get_resource_mut::<crate::ecs::Events<SignalChanged>>() {
        for change in changes {
            events.send(change);
        }
    }
}

/// A run condition: holds while the named signal is true.
pub fn signal(name: &str) -> impl Fn(Option<Res<Signals>>) -> bool + Clone {
    let name = name.to_owned();
    move |signals: Option<Res<Signals>>| signals.is_some_and(|signals| signals.is_true(&name))
}

/// A run condition: holds on the frame the named signal becomes true.
pub fn signal_became_true(name: &str) -> impl Fn(Option<Res<Signals>>) -> bool + Clone {
    let name = name.to_owned();
    move |signals: Option<Res<Signals>>| signals.is_some_and(|signals| signals.became_true(&name))
}

/// A run condition: holds on the frame the named signal becomes false.
pub fn signal_became_false(name: &str) -> impl Fn(Option<Res<Signals>>) -> bool + Clone {
    let name = name.to_owned();
    move |signals: Option<Res<Signals>>| signals.is_some_and(|signals| signals.became_false(&name))
}

impl App {
    /// Defines a true-or-false signal read from the world: a function with system parameters
    /// that returns `bool`. Other signals are defined on the [`Signals`] resource.
    pub fn add_signal<M>(&mut self, name: &str, read: impl IntoCondition<M>) -> &mut Self {
        self.world.resource_mut::<Signals>().source(name, read);
        self
    }
}

/// Signals are worked out in `Stage::PreUpdate`, in the set `"signals"`: order a system
/// `.after("signals")` there to read this frame's values, or read them in any later stage.
pub struct SignalPlugin;

impl Plugin for SignalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Signals>()
            .add_event::<SignalChanged>()
            .add_systems(Stage::PreUpdate, update_signals.in_set("signals"));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use glam::Vec3;

    use super::*;
    use crate::{
        ecs::{Component, Entity, EventReader, Query, ResMut},
        time::TimePlugin,
        transform::Transform,
    };

    // The sacred sites of Age of Empires 4: a team that holds every site wins when its
    // clock reaches the end, and the clock stops while an enemy stands on any of the sites.

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Team {
        Red,
        Blue,
    }
    impl Component for Team {}

    struct Site {
        holder: Option<Team>,
    }
    impl Component for Site {}

    const SITE_RADIUS: f32 = 2.0;
    const WIN_AFTER: f64 = 10.0;

    /// Whether any unit of `team` stands on any site held by `holder`.
    #[allow(clippy::type_complexity)]
    fn contests(
        team: Team,
        holder: Team,
    ) -> impl Fn(Query<(&Team, &Transform)>, Query<(&Site, &Transform)>) -> bool {
        move |units: Query<(&Team, &Transform)>, sites: Query<(&Site, &Transform)>| {
            units.iter().any(|(unit, at)| {
                *unit == team
                    && sites.iter().any(|(site, centre)| {
                        site.holder == Some(holder)
                            && at.translation.distance(centre.translation) < SITE_RADIUS
                    })
            })
        }
    }

    #[derive(Default)]
    struct Announcements(Vec<String>);

    struct Game {
        app: App,
        sites: [Entity; 2],
    }

    impl Game {
        fn new() -> Self {
            let mut app = App::new();
            app.add_plugins(TimePlugin).init_resource::<Announcements>();
            app.world
                .resource_mut::<Time>()
                .set_fixed_step(Some(Duration::from_secs(1)));
            let sites = [-50.0, 50.0].map(|x| {
                app.world.spawn((
                    Site {
                        holder: Some(Team::Red),
                    },
                    Transform::from_xyz(x, 0.0, 0.0),
                ))
            });

            // The rules. Nothing below ever sets the clock or a paused flag.
            app.add_signal("red.holds_all", |sites: Query<&Site>| {
                sites.iter().all(|site| site.holder == Some(Team::Red))
            })
            .add_signal("blue.contesting", contests(Team::Blue, Team::Red));
            let signals = app.world.resource_mut::<Signals>();
            signals.define("blue.calm", Op::Not, ["blue.contesting"]);
            signals.define("red.clock.running", Op::And, ["red.holds_all", "blue.calm"]);
            signals.define("red.lost_a_site", Op::Not, ["red.holds_all"]);
            signals.define(
                "red.clock",
                Op::Timer,
                ["red.clock.running", "red.lost_a_site"],
            );
            signals.set("win_after", WIN_AFTER);
            signals.define(
                "red.wins",
                Op::Compare(Compare::GreaterOrEqual),
                ["red.clock", "win_after"],
            );

            app.add_systems(
                Stage::Update,
                (
                    (|mut said: ResMut<Announcements>| said.0.push("the clock stopped".into()))
                        .run_if(signal_became_false("red.clock.running")),
                    (|mut said: ResMut<Announcements>| said.0.push("red wins".into()))
                        .run_if(signal_became_true("red.wins")),
                    |mut changes: EventReader<SignalChanged>, mut said: ResMut<Announcements>| {
                        for change in changes.read() {
                            if change.name == "blue.contesting" {
                                said.0
                                    .push(format!("contested: {}", change.value.is_true()));
                            }
                        }
                    },
                ),
            );
            Self { app, sites }
        }

        fn unit(&mut self, team: Team) -> Entity {
            self.app
                .world
                .spawn((team, Transform::from_xyz(0.0, 0.0, 0.0)))
        }

        fn walk_to(&mut self, unit: Entity, site: Option<usize>) {
            let to = match site {
                Some(site) => {
                    self.app
                        .world
                        .get::<Transform>(self.sites[site])
                        .unwrap()
                        .translation
                }
                None => Vec3::ZERO,
            };
            self.app
                .world
                .get_mut::<Transform>(unit)
                .unwrap()
                .translation = to;
        }

        /// Runs `seconds` frames of one second each, and returns the clock.
        fn play(&mut self, seconds: u32) -> f64 {
            for _ in 0..seconds {
                self.app.update();
            }
            self.signals().number("red.clock")
        }

        fn signals(&self) -> &Signals {
            self.app.world.resource::<Signals>()
        }
    }

    #[test]
    fn the_sacred_site_clock_is_right_whatever_order_things_happen_in() {
        let mut game = Game::new();
        let (first, second) = (game.unit(Team::Blue), game.unit(Team::Blue));
        let bystander = game.unit(Team::Red);
        assert_eq!(game.play(3), 3.0);

        // A blue unit steps onto a site: the clock stops.
        game.walk_to(first, Some(0));
        assert_eq!(game.play(2), 3.0);
        // Another steps onto the other site: still stopped.
        game.walk_to(second, Some(1));
        assert_eq!(game.play(2), 3.0);
        // Everyone on the first site dies. This is where the real game started the clock
        // again; here someone is still on a site, so it stays stopped.
        game.app.world.despawn(first);
        assert_eq!(game.play(2), 3.0);
        assert!(game.signals().is_true("blue.contesting"));
        // Red units on a site contest nothing.
        game.walk_to(bystander, Some(0));
        assert_eq!(game.play(1), 3.0);
        // The last one walks off: the clock runs.
        game.walk_to(second, None);
        assert_eq!(game.play(2), 5.0);
        // On, off, on, off within a few frames, in any order: nothing to get out of step.
        for site in [Some(0), None, Some(1), Some(0), None] {
            game.walk_to(second, site);
            game.play(1);
        }
        assert_eq!(game.signals().number("red.clock"), 7.0);

        // Red loses a site: the clock goes back to nothing, and starts over when retaken.
        let site = game.sites[1];
        game.app.world.get_mut::<Site>(site).unwrap().holder = Some(Team::Blue);
        assert_eq!(game.play(2), 0.0);
        game.app.world.get_mut::<Site>(site).unwrap().holder = Some(Team::Red);
        assert_eq!(game.play(9), 9.0);
        assert!(!game.signals().is_true("red.wins"));
        assert_eq!(game.play(1), 10.0);
        assert!(game.signals().became_true("red.wins"));

        let said = &game.app.world.resource::<Announcements>().0;
        assert_eq!(said.iter().filter(|line| *line == "red wins").count(), 1);
        assert_eq!(
            said.iter()
                .filter(|line| *line == "the clock stopped")
                .count(),
            4
        );
        assert_eq!(said[..2], ["the clock stopped", "contested: true"]);
    }

    #[test]
    fn the_graph_can_be_read_and_rewired_while_the_game_runs() {
        let mut game = Game::new();
        let unit = game.unit(Team::Blue);
        game.walk_to(unit, Some(0));
        game.play(2);

        // What a viewer shows: every node, what it reads, its value.
        let graph = game.signals().graph();
        let node = |name: &str| graph.iter().find(|node| node.name == name).unwrap().clone();
        assert_eq!(node("blue.contesting").kind, "source");
        assert_eq!(node("blue.contesting").value, Signal::Bool(true));
        assert_eq!(node("red.clock.running").kind, "and");
        assert_eq!(
            node("red.clock.running").inputs,
            ["red.holds_all", "blue.calm"]
        );
        assert_eq!(node("red.clock").value, Signal::Number(0.0));
        assert_eq!(node("red.wins").kind, "compare GreaterOrEqual");
        assert!(graph.iter().all(|node| node.problem.is_none()));
        let Value::List(nodes) = game.signals().to_value() else {
            panic!("a list of nodes");
        };
        assert_eq!(nodes.len(), graph.len());
        assert_eq!(nodes[1].field("value"), Some(&Value::Bool(true)));

        fn signals(game: &mut Game) -> &mut Signals {
            game.app.world.resource_mut::<Signals>()
        }
        // Try the game with contesting switched off: force the node, and the clock runs.
        assert!(signals(&mut game).force("blue.contesting", Some(Signal::Bool(false))));
        assert_eq!(game.play(2), 2.0);
        assert!(game.signals().graph().iter().any(|node| node.forced));
        signals(&mut game).force("blue.contesting", None);
        assert_eq!(game.play(1), 2.0);

        // Change a setting and a rule: win sooner, and let the clock ignore contesting, by
        // connecting its second input to a constant. The clock keeps what it had counted.
        signals(&mut game).set("win_after", 4.0);
        signals(&mut game).set("always", true);
        assert!(signals(&mut game).connect("red.clock.running", 1, "always"));
        assert!(!signals(&mut game).connect("red.clock.running", 5, "always"));
        assert_eq!(game.play(2), 4.0);
        assert!(game.signals().is_true("red.wins"));

        // A connection to nothing is reported, reads false, and mends when the name appears.
        signals(&mut game).connect("red.clock.running", 1, "blue.asleep");
        game.play(1);
        let broken = game
            .signals()
            .graph()
            .into_iter()
            .find(|n| n.name == "red.clock.running")
            .unwrap();
        assert_eq!(
            broken.problem.as_deref(),
            Some("there is no signal `blue.asleep`")
        );
        assert_eq!(broken.value, Signal::Bool(false));
        signals(&mut game).set("blue.asleep", true);
        assert_eq!(game.play(1), 5.0);
        assert!(game
            .signals()
            .graph()
            .iter()
            .all(|node| node.problem.is_none()));
        assert!(signals(&mut game).remove("always") && !signals(&mut game).remove("always"));
        assert_eq!(
            game.play(1),
            6.0,
            "removing a node keeps the others' names straight"
        );
    }

    #[test]
    fn operations_and_circles() {
        let mut world = World::new();
        let mut signals = Signals::default();
        signals.measure("three", |_| 3.0);
        signals.set("yes", true);
        signals.set("no", false);
        signals.define("count", Op::Count, ["yes", "no", "three"]);
        signals.define("sum", Op::Sum, ["three", "yes", "count"]);
        signals.define("either", Op::Or, ["no", "yes"]);
        signals.define("pick", Op::Select, ["no", "three", "sum"]);
        signals.define("steady", Op::HeldFor(1.0), ["yes"]);
        // Defined before what it reads, and a pair that read each other: a blinker.
        signals.define("late", Op::Not, ["later"]);
        signals.set("later", false);
        signals.define("tick", Op::Not, ["tock"]);
        signals.define("tock", Op::Or, ["tick"]);
        signals.update(&mut world, 0.5);
        assert_eq!(signals.number("count"), 2.0);
        assert_eq!(signals.number("sum"), 6.0);
        assert!(signals.is_true("either") && signals.is_true("late"));
        assert_eq!(signals.number("pick"), 6.0);
        assert!(!signals.is_true("steady"));
        signals.update(&mut world, 0.5);
        assert!(signals.became_true("steady"));
        assert!(!signals.changed("sum") && signals.get("nothing").is_none());
        let circle = signals
            .graph()
            .into_iter()
            .filter(|node| node.problem.is_some())
            .count();
        assert_eq!(circle, 1);
        let first = signals.is_true("tick");
        signals.update(&mut world, 0.5);
        assert_ne!(
            signals.is_true("tick"),
            first,
            "a circle is a frame late, so it blinks"
        );
        assert!(!signals.take_changes().is_empty());
    }
}
