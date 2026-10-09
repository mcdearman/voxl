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
    /// The operation's name in saved rules and over the debug connection.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Constant(_) => "constant",
            Op::And => "and",
            Op::Or => "or",
            Op::Not => "not",
            Op::Count => "count",
            Op::Sum => "sum",
            Op::Select => "select",
            Op::Timer => "timer",
            Op::HeldFor(_) => "held_for",
            Op::Compare(Compare::Less) => "less",
            Op::Compare(Compare::LessOrEqual) => "less_or_equal",
            Op::Compare(Compare::Equal) => "equal",
            Op::Compare(Compare::GreaterOrEqual) => "greater_or_equal",
            Op::Compare(Compare::Greater) => "greater",
        }
    }

    /// The operation with this name. A constant needs its `value`, and `held_for` its
    /// `seconds`.
    pub fn named(name: &str, value: Option<Signal>, seconds: Option<f64>) -> Result<Op, String> {
        Ok(match name {
            "constant" => Op::Constant(value.ok_or("a constant needs a `value`")?),
            "and" => Op::And,
            "or" => Op::Or,
            "not" => Op::Not,
            "count" => Op::Count,
            "sum" => Op::Sum,
            "select" => Op::Select,
            "timer" => Op::Timer,
            "held_for" => Op::HeldFor(seconds.ok_or("`held_for` needs `seconds`")?),
            "less" => Op::Compare(Compare::Less),
            "less_or_equal" => Op::Compare(Compare::LessOrEqual),
            "equal" => Op::Compare(Compare::Equal),
            "greater_or_equal" => Op::Compare(Compare::GreaterOrEqual),
            "greater" => Op::Compare(Compare::Greater),
            other => return Err(format!("there is no operation `{other}`")),
        })
    }

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

    /// The operation of a node that is worked out from others; `None` for a source or a
    /// signal that doesn't exist.
    pub fn op(&self, name: &str) -> Option<Op> {
        match &self.nodes[*self.names.get(name)?].kind {
            Kind::Op(op) => Some(*op),
            _ => None,
        }
    }

    /// Changes how a node is worked out, keeping its inputs. Returns false for a source or
    /// a signal that doesn't exist.
    pub fn set_op(&mut self, name: &str, op: Op) -> bool {
        match self
            .names
            .get(name)
            .map(|&index| &mut self.nodes[index].kind)
        {
            Some(Kind::Op(current)) => {
                *current = op;
                true
            }
            _ => false,
        }
    }

    /// Gives a node a new list of inputs, keeping how it is worked out. Returns false for a
    /// source or a signal that doesn't exist.
    pub fn set_inputs<I: Into<String>>(
        &mut self,
        name: &str,
        inputs: impl IntoIterator<Item = I>,
    ) -> bool {
        let Some(&index) = self.names.get(name) else {
            return false;
        };
        if !matches!(self.nodes[index].kind, Kind::Op(_)) {
            return false;
        }
        self.nodes[index].inputs = inputs.into_iter().map(Into::into).collect();
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

    /// The rules as data to keep: every signal that is worked out from others or is a
    /// constant, with its operation and inputs. Sources are code, and are not in it.
    pub fn rules(&self) -> Value {
        let rules = self.nodes.iter().filter_map(|node| {
            let Kind::Op(op) = &node.kind else {
                return None;
            };
            let mut rule = vec![
                ("name".to_owned(), Value::Text(node.name.clone())),
                ("op".to_owned(), Value::Text(op.name().to_owned())),
            ];
            match op {
                Op::Constant(Signal::Bool(value)) => {
                    rule.push(("value".to_owned(), Value::Bool(*value)))
                }
                Op::Constant(Signal::Number(value)) => {
                    rule.push(("value".to_owned(), Value::Float(*value)))
                }
                Op::HeldFor(seconds) => rule.push(("seconds".to_owned(), Value::Float(*seconds))),
                _ => {}
            }
            if !node.inputs.is_empty() {
                let inputs = node.inputs.iter().cloned().map(Value::Text).collect();
                rule.push(("inputs".to_owned(), Value::List(inputs)));
            }
            Some(Value::Map(rule))
        });
        Value::Map(vec![
            ("version".to_owned(), Value::Int(1)),
            ("signals".to_owned(), Value::List(rules.collect())),
        ])
    }

    /// Defines every rule in `rules` (as [`Signals::rules`] gives them), replacing those of
    /// the same name and leaving everything else alone. Nothing is changed if any of it
    /// can't be read. Returns how many were defined.
    pub fn apply_rules(&mut self, rules: &Value) -> Result<usize, String> {
        if rules.field("version") != Some(&Value::Int(1)) {
            return Err("not a version 1 set of signal rules".to_owned());
        }
        let Some(Value::List(list)) = rules.field("signals") else {
            return Err("the rules have no `signals` list".to_owned());
        };
        let mut read = Vec::new();
        for rule in list {
            let Some(Value::Text(name)) = rule.field("name") else {
                return Err("a rule needs a `name`".to_owned());
            };
            let wrong = |why: String| format!("`{name}`: {why}");
            let Some(Value::Text(op)) = rule.field("op") else {
                return Err(wrong("it needs an `op`".to_owned()));
            };
            let value = match rule.field("value") {
                None => None,
                Some(Value::Bool(value)) => Some(Signal::Bool(*value)),
                Some(other) => Some(Signal::Number(other.as_f64().ok_or_else(|| {
                    wrong("its `value` is true, false or a number".to_owned())
                })?)),
            };
            let seconds = rule.field("seconds").and_then(Value::as_f64);
            let op = Op::named(op, value, seconds).map_err(wrong)?;
            let inputs = match rule.field("inputs") {
                None => Vec::new(),
                Some(Value::List(inputs)) => inputs
                    .iter()
                    .map(|input| match input {
                        Value::Text(input) => Ok(input.clone()),
                        _ => Err(wrong("its `inputs` are signal names".to_owned())),
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(wrong("its `inputs` are a list".to_owned())),
            };
            if matches!(
                self.names.get(name).map(|&index| &self.nodes[index].kind),
                Some(Kind::Truth(_) | Kind::Measure(_))
            ) {
                return Err(wrong("it is a source, defined in code".to_owned()));
            }
            read.push((name.clone(), op, inputs));
        }
        let count = read.len();
        for (name, op, inputs) in read {
            self.define(&name, op, inputs);
        }
        Ok(count)
    }

    /// Writes the rules to a JSON file.
    pub fn save_rules(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        let path = path.as_ref();
        std::fs::write(path, crate::reflect::json::to_string(&self.rules()))
            .map_err(|err| format!("can't write {}: {err}", path.display()))
    }

    /// Reads rules from a JSON file and defines them. See [`Signals::apply_rules`].
    pub fn load_rules(&mut self, path: impl AsRef<std::path::Path>) -> Result<usize, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("can't read {}: {err}", path.display()))?;
        let rules = crate::reflect::json::parse(&text)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        self.apply_rules(&rules)
            .map_err(|err| format!("{}: {err}", path.display()))
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
    #[test]
    fn a_rule_can_be_given_other_inputs_and_another_operation() {
        let mut world = World::new();
        let mut signals = Signals::default();
        signals.source("lit", |_: Query<&Transform>| true);
        signals.set("a", true);
        signals.set("b", false);
        signals.define("rule", Op::And, ["a", "b"]);
        signals.update(&mut world, 0.0);
        assert!(!signals.is_true("rule"));
        assert_eq!(signals.op("rule"), Some(Op::And));

        assert!(signals.set_op("rule", Op::Or));
        signals.update(&mut world, 0.0);
        assert!(signals.is_true("rule"));
        assert!(signals.set_inputs("rule", ["b"]));
        signals.update(&mut world, 0.0);
        assert!(!signals.is_true("rule"));
        assert!(signals.set_inputs("rule", ["b", "a", "lit"]) && signals.set_op("rule", Op::Count));
        signals.update(&mut world, 0.0);
        assert_eq!(signals.number("rule"), 2.0);

        // A source reads the world, not other signals: it has no operation to change.
        assert_eq!(signals.op("lit"), None);
        assert!(!signals.set_op("lit", Op::Not) && !signals.set_inputs("lit", ["a"]));
        assert!(!signals.set_op("nothing", Op::Not) && !signals.set_inputs("nothing", ["a"]));
    }
    #[test]
    fn rules_can_be_saved_and_brought_back() {
        let mut world = World::new();
        let mut signals = Signals::default();
        signals.source("lit", |_: Query<&Transform>| true);
        signals.set("limit", 30.0);
        signals.set("open", true);
        signals.define("ready", Op::And, ["lit", "open"]);
        signals.define("steady", Op::HeldFor(2.5), ["ready"]);
        signals.define("clock", Op::Timer, ["ready"]);
        signals.define(
            "done",
            Op::Compare(Compare::GreaterOrEqual),
            ["clock", "limit"],
        );
        let saved = signals.rules();
        let text = crate::reflect::json::to_string(&saved);
        assert!(
            !text.contains("\"lit\"") || text.contains("\"inputs\""),
            "the source is only named as an input"
        );
        assert_eq!(
            saved.get_path("signals.0.name"),
            Some(&Value::Text("limit".into()))
        );

        // Another run of the same game: its sources come from code, its rules from the file.
        let mut later = Signals::default();
        later.source("lit", |_: Query<&Transform>| true);
        later.set("limit", 99.0);
        let parsed = crate::reflect::json::parse(&text).unwrap();
        assert_eq!(later.apply_rules(&parsed), Ok(6));
        assert_eq!(
            later.rules(),
            saved,
            "and saving again gives the same rules"
        );
        later.update(&mut world, 1.0);
        assert!(later.is_true("ready") && !later.is_true("steady"));
        assert_eq!(later.number("limit"), 30.0);

        // What can't be read changes nothing, and says which rule it was.
        let before = later.rules();
        for (bad, why) in [
            (r#"{"version": 2, "signals": []}"#, "version 1"),
            (r#"{"version": 1}"#, "no `signals`"),
            (
                r#"{"version": 1, "signals": [{"name": "x", "op": "and"}, {"name": "y", "op": "xor"}]}"#,
                "`y`: there is no operation",
            ),
            (
                r#"{"version": 1, "signals": [{"name": "x", "op": "held_for"}]}"#,
                "`x`: `held_for` needs",
            ),
            (
                r#"{"version": 1, "signals": [{"name": "lit", "op": "not", "inputs": ["open"]}]}"#,
                "`lit`: it is a source",
            ),
            (
                r#"{"version": 1, "signals": [{"name": "x", "op": "and", "inputs": [3]}]}"#,
                "signal names",
            ),
        ] {
            let err = later
                .apply_rules(&crate::reflect::json::parse(bad).unwrap())
                .unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
            assert_eq!(later.rules(), before);
        }
        assert!(later.get("x").is_none());

        // And through a file.
        let path = std::env::temp_dir().join(format!("mira-rules-{}.json", std::process::id()));
        signals.save_rules(&path).unwrap();
        assert_eq!(Signals::default().load_rules(&path), Ok(6));
        let _ = std::fs::remove_file(&path);
        assert!(Signals::default()
            .load_rules(&path)
            .unwrap_err()
            .contains("can't read"));
    }
}
