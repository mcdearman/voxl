//! The signal graph, shown and edited inside the running game, as a circuit: a box for each
//! signal, its inputs on the left and its output on the right, and a wire for every
//! connection, lit while what it carries is true. See mira's `docs/SIGNALS.md`.
//!
//! ```ignore
//! app.add_plugins(mira_ui::signal_graph::plugin());
//! ```
//!
//! - Drag from an input (the dot on a box's left edge) to another box to rewire it; drop the
//!   wire on nothing to take that input away.
//! - Drag from an output (the dot on the right edge) to another box to give it one more input.
//! - Right-click a box to change its operation (and, or, not, count, sum); right-click the
//!   panel's background for a new constant. Backspace over a box removes it.
//! - Click a box's lamp to force the signal true, again for false, again to let it go.
//! - Click a constant's value to flip it; scroll over a number to change it.
//! - Click a box's name to type a new one. Click its lower line, or press Enter over it, to
//!   type what it is: a value (`true`, `42`) makes it a constant, and an operation (`and`,
//!   `timer`, `held_for 5`, `>=`) makes it that. Enter takes what was typed, Escape drops it.
//! - Drag a box by its body to move it; drag the title to move the whole panel.
//! - F1 hides and shows the panel.
//!
//! Every edit is made to the game's `Signals` at once, so the game answers as you wire.

use std::collections::HashMap;

use armature::{
    App, Color, Cx, DrawCx, Element, Event, EventCx, FontFamily, Key, KeyEvent, Limits, Point,
    PointerButton, Rect, Size, Status, TextLayout, TextStyle, Widget,
};
use mira::{
    app::{App as Game, Plugin, Stage},
    ecs::World,
    signal::{NodeInfo, Op, Signal, Signals},
};

use crate::{kit::field, UiHost, UiPlugin};

/// A change to the graph made in the panel, waiting to be made to the game.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Connect input `input` of `node` to the signal `to`.
    Connect {
        node: String,
        input: usize,
        to: String,
    },
    /// Hold a signal's output at a value, or let it go.
    Force {
        node: String,
        value: Option<Signal>,
    },
    /// Give a constant a new value, or make a new constant.
    Set {
        node: String,
        value: Signal,
    },
    /// Give a node a whole new list of inputs: one more, or one fewer.
    Inputs {
        node: String,
        inputs: Vec<String>,
    },
    /// Change how a node is worked out.
    Op {
        node: String,
        op: Op,
    },
    Remove {
        node: String,
    },
    /// Give a rule another name; what reads it follows.
    Rename {
        node: String,
        to: String,
    },
    /// Record where a box was put, so the graph is drawn the same way next time.
    Place {
        node: String,
        at: [f32; 2],
    },
}

/// What the pointer is doing.
#[derive(Clone, Debug, Default, PartialEq)]
enum Held {
    #[default]
    Nothing,
    /// Moving a box; the offset is from its corner to the pointer.
    Node { name: String, grip: Point },
    /// Pulling a wire out of an input.
    Wire { node: String, input: usize },
    /// Pulling a new wire out of an output.
    Output { node: String },
    /// Moving the panel.
    Panel { grip: Point },
}

/// Which line of a box is being typed over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Line {
    /// The signal's name.
    Name,
    /// What it is: a constant's value, or the operation that works it out.
    Meaning,
}

/// Text being typed over a box.
#[derive(Clone, Debug, PartialEq)]
struct Typing {
    node: String,
    line: Line,
    text: String,
}

/// What typed text says a signal is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Meant {
    Value(Signal),
    Op(Op),
}

/// Reads what was typed on a box's lower line: `true`, `false` or a number is a constant;
/// otherwise an operation by its name or sign, with its seconds after it if it has any.
fn meant(text: &str) -> Option<Meant> {
    let text = text.trim().to_lowercase().replace("held for", "held_for");
    match text.as_str() {
        "true" => return Some(Meant::Value(Signal::Bool(true))),
        "false" => return Some(Meant::Value(Signal::Bool(false))),
        _ => {}
    }
    if let Ok(number) = text.parse::<f64>() {
        return number
            .is_finite()
            .then_some(Meant::Value(Signal::Number(number)));
    }
    let mut words = text.split_whitespace();
    let name = match words.next()? {
        "<" => "less",
        "<=" | "≤" => "less_or_equal",
        "=" | "==" => "equal",
        ">=" | "≥" => "greater_or_equal",
        ">" => "greater",
        name => name,
    };
    let seconds = words.next().map(str::parse::<f64>);
    if name == "constant" || matches!(seconds, Some(Err(_))) || words.next().is_some() {
        return None;
    }
    let seconds = seconds.and_then(Result::ok);
    Op::named(name, None, seconds).ok().map(Meant::Op)
}

/// What a box's lower line says, written the way [`meant`] reads it.
fn meaning_text(node: &NodeInfo) -> String {
    match node.kind.as_str() {
        "constant" => value_text(node.value),
        "compare Less" => "<".to_owned(),
        "compare LessOrEqual" => "<=".to_owned(),
        "compare Equal" => "==".to_owned(),
        "compare GreaterOrEqual" => ">=".to_owned(),
        "compare Greater" => ">".to_owned(),
        kind => kind.replace("held for", "held_for"),
    }
}

/// The panel's state.
pub struct SignalGraph {
    nodes: Vec<NodeInfo>,
    /// Where boxes have been dragged to, relative to the panel; the rest are laid out.
    placed: HashMap<String, Point>,
    origin: Point,
    held: Held,
    pointer: Point,
    hidden: bool,
    /// Where the pointer was when it last took hold of something, and whether it has gone
    /// anywhere since: a box let go where it was picked up was clicked, not moved.
    pressed: Point,
    moved: bool,
    typing: Option<Typing>,
    edits: Vec<Edit>,
}

impl Default for SignalGraph {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            placed: HashMap::new(),
            origin: Point::new(14.0, 14.0),
            held: Held::Nothing,
            pointer: Point::new(0.0, 0.0),
            hidden: false,
            pressed: Point::new(0.0, 0.0),
            moved: false,
            typing: None,
            edits: Vec::new(),
        }
    }
}

impl SignalGraph {
    /// Hides or shows the panel, as F1 does.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Starts typing over a line of a box, with what the line says now. A source's lines
    /// are the game's to say.
    fn type_over(&mut self, node: &str, line: Line) {
        let Some(known) = self.nodes.iter().find(|known| known.name == node) else {
            return;
        };
        if known.kind == "source" {
            return;
        }
        self.typing = Some(Typing {
            node: node.to_owned(),
            line,
            text: match line {
                Line::Name => node.to_owned(),
                Line::Meaning => meaning_text(known),
            },
        });
    }

    /// The edits made since this was last called.
    pub fn take_edits(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.edits)
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Toggle,
    Pointer(Point),
    Hold(HeldAt),
    Release,
    Edit(Edit),
    /// Start typing over a box.
    Type {
        node: String,
        line: Line,
    },
    Typed(String),
    /// Take what was typed.
    Enter,
    /// Drop what was typed.
    Leave,
}

/// What a press landed on.
#[derive(Clone, Debug)]
pub enum HeldAt {
    Node { name: String, grip: Point },
    Wire { node: String, input: usize },
    Output { node: String },
    Panel { grip: Point },
}

impl App for SignalGraph {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Toggle => self.hidden = !self.hidden,
            Message::Pointer(at) => {
                self.pointer = at;
                match &self.held {
                    Held::Node { name, grip } => {
                        self.moved |= !near(at, self.pressed, 3.0);
                        if !self.moved {
                            return;
                        }
                        let at = Point::new(
                            at.x - grip.x - self.origin.x,
                            at.y - grip.y - self.origin.y,
                        );
                        self.placed
                            .insert(name.clone(), Point::new(at.x.max(0.0), at.y.max(0.0)));
                    }
                    Held::Panel { grip } => self.origin = Point::new(at.x - grip.x, at.y - grip.y),
                    Held::Wire { .. } | Held::Output { .. } | Held::Nothing => {}
                }
            }
            Message::Hold(at) => {
                self.pressed = self.pointer;
                self.moved = false;
                self.typing = None;
                self.held = match at {
                    HeldAt::Node { name, grip } => Held::Node { name, grip },
                    HeldAt::Wire { node, input } => Held::Wire { node, input },
                    HeldAt::Output { node } => Held::Output { node },
                    HeldAt::Panel { grip } => Held::Panel { grip },
                }
            }
            Message::Release => {
                // A box that was moved is where it is from now on. One let go where it was
                // picked up was clicked: on its name or its lower line, to type there.
                if let Held::Node { name, grip } = std::mem::take(&mut self.held) {
                    if !self.moved {
                        let line = if grip.y - TITLE - PAD < BOX.h * 0.5 {
                            Line::Name
                        } else {
                            Line::Meaning
                        };
                        self.type_over(&name, line);
                    } else if let Some(at) = self.placed.get(&name) {
                        self.edits.push(Edit::Place {
                            node: name,
                            at: [at.x, at.y],
                        });
                    }
                }
                self.held = Held::Nothing
            }
            Message::Type { node, line } => self.type_over(&node, line),
            Message::Typed(text) => {
                if let Some(typing) = &mut self.typing {
                    typing.text = text;
                }
            }
            Message::Leave => self.typing = None,
            Message::Enter => {
                let Some(typing) = self.typing.take() else {
                    return;
                };
                let node = typing.node.clone();
                match typing.line {
                    Line::Name => {
                        let to = typing.text.trim().to_owned();
                        let taken = self.nodes.iter().any(|known| known.name == to);
                        if to == node {
                        } else if to.is_empty() || to.contains(char::is_whitespace) || taken {
                            // Not a name it can have: the field stays, to be put right.
                            self.typing = Some(typing);
                        } else {
                            if let Some(at) = self.placed.remove(&node) {
                                self.placed.insert(to.clone(), at);
                            }
                            self.edits.push(Edit::Rename { node, to });
                        }
                    }
                    Line::Meaning => match meant(&typing.text) {
                        Some(Meant::Value(value)) => self.edits.push(Edit::Set { node, value }),
                        Some(Meant::Op(op)) => self.edits.push(Edit::Op { node, op }),
                        None => self.typing = Some(typing),
                    },
                }
            }
            Message::Edit(edit) => {
                // A constant made here appears where it was asked for.
                if let Edit::Set { node, .. } = &edit {
                    if !self.nodes.iter().any(|known| known.name == *node) {
                        let at = Point::new(
                            (self.pointer.x - self.origin.x - PAD).max(0.0),
                            (self.pointer.y - self.origin.y - TITLE - PAD).max(0.0),
                        );
                        self.placed.insert(node.clone(), at);
                    }
                }
                if let Edit::Remove { node } = &edit {
                    self.placed.remove(node);
                }
                self.typing = None;
                self.edits.push(edit)
            }
        }
    }

    fn view(&self) -> Element<Message> {
        if self.hidden {
            return Element::new(Circuit::default());
        }
        let plan = Plan::of(&self.nodes, &self.placed, self.origin);
        // The field sits over the line it replaces.
        let mut over = None;
        let typed = self.typing.as_ref().and_then(|typing| {
            let index = self
                .nodes
                .iter()
                .position(|node| node.name == typing.node)?;
            over = Some((index, typing.line));
            let b = plan.boxes[index];
            let (at, width) = match typing.line {
                Line::Name => (Point::new(b.x + 26.0, b.y - 2.0), b.w - 30.0),
                Line::Meaning => (Point::new(b.x + 4.0, b.y + b.h * 0.5 - 3.0), b.w - 8.0),
            };
            let field = field("", &typing.text, Message::Typed as fn(String) -> Message)
                .on_submit(Message::Enter)
                .on_cancel(Message::Leave)
                .width(width)
                .autofocus();
            Some((at, Element::from(field)))
        });
        let circuit = Element::new(Circuit {
            plan,
            nodes: self.nodes.clone(),
            held: self.held.clone(),
            pointer: self.pointer,
            typed: over,
            ..Circuit::default()
        });
        match typed {
            Some((at, field)) => Element::new(Typed {
                at,
                size: Size::new(0.0, 0.0),
                layers: [circuit, field],
            }),
            None => circuit,
        }
    }

    fn on_key(&self, key: &KeyEvent) -> Option<Message> {
        // While something is being typed, the keys are the field's.
        if !key.pressed || key.repeat || self.typing.is_some() {
            return None;
        }
        match key.key {
            Key::F(1) => Some(Message::Toggle),
            // Over a box that isn't a source, Enter is for typing what it is.
            Key::Enter if !self.hidden => {
                let plan = Plan::of(&self.nodes, &self.placed, self.origin);
                let over = (0..self.nodes.len())
                    .find(|&index| plan.boxes[index].contains(self.pointer))?;
                let node = &self.nodes[over];
                (node.kind != "source").then(|| Message::Type {
                    node: node.name.clone(),
                    line: Line::Meaning,
                })
            }
            // Over a box that isn't a source, these take it out of the graph.
            Key::Backspace | Key::Delete if !self.hidden => {
                let plan = Plan::of(&self.nodes, &self.placed, self.origin);
                let over = (0..self.nodes.len())
                    .find(|&index| plan.boxes[index].contains(self.pointer))?;
                let node = &self.nodes[over];
                (node.kind != "source").then(|| {
                    Message::Edit(Edit::Remove {
                        node: node.name.clone(),
                    })
                })
            }
            _ => None,
        }
    }
}

/// Reads the graph out of the world into the panel.
pub fn sync(world: &World, panel: &mut SignalGraph) {
    let Some(signals) = world.get_resource::<Signals>() else {
        panel.nodes.clear();
        return;
    };
    panel.nodes = signals.graph();
    // Where boxes were put is kept with the rules; a box in hand is where the hand has it.
    for node in &panel.nodes {
        let held = matches!(&panel.held, Held::Node { name, .. } if *name == node.name);
        if let (Some([x, y]), false) = (signals.place_of(&node.name), held) {
            panel.placed.insert(node.name.clone(), Point::new(x, y));
        }
    }
}

/// Makes the panel's edits to the game's signals.
fn apply_edits(world: &mut World) {
    let Some(host) = world.get_resource_mut::<UiHost<SignalGraph>>() else {
        return;
    };
    let edits = host.app().take_edits();
    let Some(signals) = world.get_resource_mut::<Signals>() else {
        return;
    };
    for edit in edits {
        match edit {
            Edit::Connect { node, input, to } => {
                signals.connect(&node, input, &to);
            }
            Edit::Force { node, value } => {
                signals.force(&node, value);
            }
            Edit::Set { node, value } => signals.set(&node, value),
            Edit::Inputs { node, inputs } => {
                signals.set_inputs(&node, inputs);
            }
            Edit::Op { node, op } => {
                signals.set_op(&node, op);
            }
            Edit::Remove { node } => {
                signals.remove(&node);
            }
            Edit::Rename { node, to } => {
                // A name that can't be had is refused there; the box keeps its old one.
                let _ = signals.rename(&node, &to);
            }
            Edit::Place { node, at } => signals.place(&node, at),
        }
    }
}

/// Shows the signal graph in the game, and lets it be rewired there.
pub struct SignalGraphPlugin;

impl Plugin for SignalGraphPlugin {
    fn build(&self, app: &mut Game) {
        app.add_plugins(UiPlugin::new(SignalGraph::default).sync(sync))
            // Before the signals are next worked out, so an edit shows the same frame.
            .add_systems(Stage::First, apply_edits);
    }
}

/// The plugin that shows the signal graph in the game.
pub fn plugin() -> SignalGraphPlugin {
    SignalGraphPlugin
}

// --- where everything is ---

const BOX: Size = Size { w: 176.0, h: 50.0 };
const COLUMN: f32 = 236.0;
const ROW: f32 = 68.0;
const PAD: f32 = 16.0;
const TITLE: f32 = 30.0;
const PORT: f32 = 5.0;
const LAMP: f32 = 10.0;

/// Where each box, port and the panel itself is, in window coordinates: worked out once from
/// the panel's state and used alike to draw and to tell what the pointer is on.
#[derive(Clone, Debug, Default)]
struct Plan {
    panel: Rect,
    boxes: Vec<Rect>,
}

impl Plan {
    fn of(nodes: &[NodeInfo], placed: &HashMap<String, Point>, origin: Point) -> Self {
        // A signal's column is one past the furthest of its inputs: sources on the left,
        // what is made from them to the right, as a circuit is read.
        fn column(
            index: usize,
            nodes: &[NodeInfo],
            seen: &mut Vec<usize>,
            known: &mut [Option<usize>],
        ) -> usize {
            if let Some(column) = known[index] {
                return column;
            }
            // Round a circle: stop where it closes.
            if seen.contains(&index) {
                return 0;
            }
            seen.push(index);
            let column = nodes[index]
                .inputs
                .iter()
                .filter_map(|input| nodes.iter().position(|node| node.name == *input))
                .map(|input| column(input, nodes, seen, known) + 1)
                .max()
                .unwrap_or(0);
            seen.pop();
            known[index] = Some(column);
            column
        }
        let mut known = vec![None; nodes.len()];
        let columns: Vec<usize> = (0..nodes.len())
            .map(|index| column(index, nodes, &mut Vec::new(), &mut known))
            .collect();
        let mut filled: HashMap<usize, usize> = HashMap::new();
        let mut boxes = Vec::new();
        let (mut right, mut bottom) = (0.0f32, 0.0f32);
        for (node, column) in nodes.iter().zip(columns) {
            let row = filled.entry(column).or_insert(0);
            let laid = Point::new(column as f32 * COLUMN, *row as f32 * ROW);
            *row += 1;
            let at = placed.get(&node.name).copied().unwrap_or(laid);
            right = right.max(at.x + BOX.w);
            bottom = bottom.max(at.y + BOX.h);
            boxes.push(Rect::new(
                origin.x + PAD + at.x,
                origin.y + TITLE + PAD + at.y,
                BOX.w,
                BOX.h,
            ));
        }
        let panel = if nodes.is_empty() {
            Rect::new(0.0, 0.0, 0.0, 0.0)
        } else {
            Rect::new(
                origin.x,
                origin.y,
                right + PAD * 2.0,
                bottom + TITLE + PAD * 2.0,
            )
        };
        Self { panel, boxes }
    }

    fn output(&self, index: usize) -> Point {
        let b = self.boxes[index];
        Point::new(b.x + b.w, b.y + b.h * 0.5)
    }

    fn input(&self, index: usize, input: usize, of: usize) -> Point {
        let b = self.boxes[index];
        Point::new(b.x, b.y + b.h * (input as f32 + 1.0) / (of as f32 + 1.0))
    }

    fn lamp(&self, index: usize) -> Rect {
        let b = self.boxes[index];
        Rect::new(b.x + 10.0, b.y + 9.0, LAMP, LAMP)
    }

    /// The lower half of a box, where its value is written.
    fn value(&self, index: usize) -> Rect {
        let b = self.boxes[index];
        Rect::new(b.x + 8.0, b.y + b.h * 0.5, b.w - 16.0, b.h * 0.5)
    }
}

fn near(a: Point, b: Point, within: f32) -> bool {
    (a.x - b.x).abs() <= within && (a.y - b.y).abs() <= within
}

fn value_text(value: Signal) -> String {
    match value {
        Signal::Bool(value) => value.to_string(),
        Signal::Number(value) if value.fract() == 0.0 && value.abs() < 1e12 => {
            format!("{value:.0}")
        }
        Signal::Number(value) => format!("{value:.2}"),
    }
}

/// A node's kind as its box says it: comparisons by their sign.
fn kind_text(kind: &str) -> &str {
    match kind {
        "compare Less" => "<",
        "compare LessOrEqual" => "≤",
        "compare Equal" => "==",
        "compare GreaterOrEqual" => "≥",
        "compare Greater" => ">",
        other => other,
    }
}

// --- the widgets ---

/// The circuit with a field over it, where something is being typed.
struct Typed {
    at: Point,
    size: Size,
    layers: [Element<Message>; 2],
}

impl Widget<Message> for Typed {
    fn children_mut(&mut self) -> &mut [Element<Message>] {
        &mut self.layers
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let whole = self.layers[0].layout(cx, limits);
        self.layers[0].set_position(Point::new(0.0, 0.0));
        self.size = self.layers[1].layout(cx, Limits::loose(limits.max));
        self.layers[1].set_position(self.at);
        whole
    }

    fn draw(&self, cx: &mut DrawCx) {
        self.layers[0].draw(cx);
        self.layers[1].draw(cx);
    }

    fn event(&mut self, cx: &mut EventCx<Message>, event: &Event) -> Status {
        // A press anywhere else is the end of typing.
        if let Event::PointerPressed { pos, .. } = event {
            let field = Rect::new(self.at.x, self.at.y, self.size.w, self.size.h);
            if !field.contains(*pos) {
                cx.emit(Message::Leave);
            }
        }
        match self.layers[1].event(cx, event) {
            Status::Captured => Status::Captured,
            Status::Ignored => self.layers[0].event(cx, event),
        }
    }
}

#[derive(Default)]
struct Circuit {
    plan: Plan,
    nodes: Vec<NodeInfo>,
    held: Held,
    pointer: Point,
    /// The box and line a field is over, if one is.
    typed: Option<(usize, Line)>,
    names: Vec<TextLayout>,
    details: Vec<TextLayout>,
    title: Option<TextLayout>,
}

const LIT: Color = Color::hex(0x7ee08a);
const DARK: Color = Color::hex(0x6b7280);
const WARN: Color = Color::hex(0xff6b6b);
const GOLD: Color = Color::hex(0xffc857);

impl Circuit {
    fn index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.name == name)
    }

    fn wire_colour(&self, from: Option<usize>) -> Color {
        match from {
            Some(from) if self.nodes[from].value.is_true() => LIT,
            Some(_) => DARK.with_alpha(0.7),
            None => WARN,
        }
    }

    /// A wire from an output to an input: out to the right, in from the left, with an easy
    /// curve between, as wires are drawn on a circuit.
    fn wire(
        &self,
        scene: &mut armature::Scene,
        from: Point,
        to: Point,
        colour: Color,
        thickness: f32,
    ) {
        let reach = ((to.x - from.x).abs() * 0.5).clamp(24.0, 90.0);
        let (c1, c2) = (
            Point::new(from.x + reach, from.y),
            Point::new(to.x - reach, to.y),
        );
        let points: Vec<Point> = (0..=24)
            .map(|step| {
                let t = step as f32 / 24.0;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                Point::new(
                    a * from.x + b * c1.x + c * c2.x + d * to.x,
                    a * from.y + b * c1.y + c * c2.y + d * to.y,
                )
            })
            .collect();
        scene.polyline(&points, thickness, colour);
    }

    /// What a right-click at `at` does: on a box, the next operation; on the panel, a new
    /// constant.
    fn alter(&self, at: Point) -> Option<Message> {
        for (index, node) in self.nodes.iter().enumerate() {
            if !self.plan.boxes[index].contains(at) {
                continue;
            }
            let op = match node.kind.as_str() {
                "and" => Op::Or,
                "or" => Op::Not,
                "not" => Op::Count,
                "count" => Op::Sum,
                "sum" => Op::And,
                // A constant, a source, a timer: not something to turn into something else.
                _ => return Some(Message::Pointer(at)),
            };
            return Some(Message::Edit(Edit::Op {
                node: node.name.clone(),
                op,
            }));
        }
        if !self.plan.panel.contains(at) {
            return None;
        }
        let name = (1..)
            .map(|n| format!("new.{n}"))
            .find(|name| self.index(name).is_none())
            .expect("there is always another number");
        Some(Message::Edit(Edit::Set {
            node: name,
            value: Signal::Bool(false),
        }))
    }

    /// What a press at `at` is on, most particular thing first.
    fn press(&self, at: Point) -> Option<Message> {
        for (index, node) in self.nodes.iter().enumerate() {
            for input in 0..node.inputs.len() {
                if near(
                    at,
                    self.plan.input(index, input, node.inputs.len()),
                    PORT + 5.0,
                ) {
                    return Some(Message::Hold(HeldAt::Wire {
                        node: node.name.clone(),
                        input,
                    }));
                }
            }
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if near(at, self.plan.output(index), PORT + 5.0) {
                return Some(Message::Hold(HeldAt::Output {
                    node: node.name.clone(),
                }));
            }
        }
        for (index, node) in self.nodes.iter().enumerate() {
            let b = self.plan.boxes[index];
            if !b.contains(at) {
                continue;
            }
            let lamp = self.plan.lamp(index);
            if near(
                at,
                Point::new(lamp.x + LAMP * 0.5, lamp.y + LAMP * 0.5),
                LAMP,
            ) {
                // Round the three: let be, held true, held false.
                let value = match (node.forced, node.value.is_true()) {
                    (false, _) => Some(Signal::Bool(true)),
                    (true, true) => Some(Signal::Bool(false)),
                    (true, false) => None,
                };
                return Some(Message::Edit(Edit::Force {
                    node: node.name.clone(),
                    value,
                }));
            }
            if node.kind == "constant" && self.plan.value(index).contains(at) {
                if let Signal::Bool(value) = node.value {
                    return Some(Message::Edit(Edit::Set {
                        node: node.name.clone(),
                        value: Signal::Bool(!value),
                    }));
                }
            }
            return Some(Message::Hold(HeldAt::Node {
                name: node.name.clone(),
                grip: Point::new(at.x - b.x + PAD, at.y - b.y + TITLE + PAD),
            }));
        }
        self.plan.panel.contains(at).then(|| {
            Message::Hold(HeldAt::Panel {
                grip: Point::new(at.x - self.plan.panel.x, at.y - self.plan.panel.y),
            })
        })
    }
}

impl Widget<Message> for Circuit {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let name = TextStyle {
            size: 12.5,
            weight: 600,
            family: FontFamily::Sans,
            line_height: 1.3,
            letter_spacing: 0.0,
        };
        let detail = TextStyle {
            size: 11.5,
            weight: 500,
            family: FontFamily::Mono,
            ..name
        };
        self.names = self
            .nodes
            .iter()
            .map(|node| cx.text().layout(&node.name, &name, Some(BOX.w - 34.0)))
            .collect();
        self.details = self
            .nodes
            .iter()
            .map(|node| {
                let text = match &node.problem {
                    Some(problem) => problem.clone(),
                    None => format!("{}  =  {}", kind_text(&node.kind), value_text(node.value)),
                };
                cx.text().layout(&text, &detail, Some(BOX.w - 16.0))
            })
            .collect();
        self.title = (!self.nodes.is_empty()).then(|| {
            let title = TextStyle {
                size: 12.0,
                weight: 700,
                ..name
            };
            cx.text().layout(
                "Signals      drag ports to wire  ·  click a lamp to force  ·  click a line to type it  ·  right-click to change  ·  F1 hides",
                &title,
                None,
            )
        });
        // The widget is the whole screen; only the panel is drawn, so only it is "there".
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let Some(title) = &self.title else {
            return;
        };
        let panel = self.plan.panel;
        let panel = Rect::new(
            panel.x,
            panel.y,
            panel.w.max(title.size().w + PAD * 2.0),
            panel.h,
        );
        cx.scene.fill(
            panel,
            12.0,
            Color::hex(0x101216).with_alpha(0.9),
            Some((1.0, Color::WHITE.with_alpha(0.14))),
        );
        cx.scene.text(
            title,
            Point::new(panel.x + PAD, panel.y + 9.0),
            Color::WHITE.with_alpha(0.6),
        );

        // Wires first, so that boxes sit on top of them.
        for (index, node) in self.nodes.iter().enumerate() {
            for (input, name) in node.inputs.iter().enumerate() {
                if matches!(&self.held, Held::Wire { node: held, input: which } if *held == node.name && *which == input)
                {
                    continue;
                }
                let to = self.plan.input(index, input, node.inputs.len());
                match self.index(name) {
                    Some(from) => {
                        let colour = self.wire_colour(Some(from));
                        let thickness = if self.nodes[from].value.is_true() {
                            2.5
                        } else {
                            1.5
                        };
                        self.wire(cx.scene, self.plan.output(from), to, colour, thickness);
                    }
                    // A wire to nothing: a stub, in the colour of trouble.
                    None => cx.scene.line(Point::new(to.x - 22.0, to.y), to, 2.0, WARN),
                }
            }
        }
        if let Held::Output { node } = &self.held {
            if let Some(index) = self.index(node) {
                self.wire(cx.scene, self.plan.output(index), self.pointer, GOLD, 2.5);
            }
        }
        // The wire being pulled follows the pointer.
        if let Held::Wire { node, input } = &self.held {
            if let Some(index) = self.index(node) {
                let to = self
                    .plan
                    .input(index, *input, self.nodes[index].inputs.len());
                self.wire(cx.scene, self.pointer, to, GOLD, 2.5);
            }
        }

        for (index, node) in self.nodes.iter().enumerate() {
            let b = self.plan.boxes[index];
            let on = node.value.is_true();
            let source = node.kind == "source";
            let aimed_at = matches!(self.held, Held::Wire { .. } | Held::Output { .. })
                && b.contains(self.pointer);
            let border = if node.problem.is_some() {
                (1.5, WARN)
            } else if aimed_at {
                (2.0, GOLD)
            } else if node.changed {
                (1.5, GOLD.with_alpha(0.9))
            } else if on {
                (1.0, LIT.with_alpha(0.55))
            } else {
                (1.0, Color::WHITE.with_alpha(0.16))
            };
            let fill = if source {
                Color::hex(0x1b2433)
            } else {
                Color::hex(0x1c1f26)
            };
            cx.scene.fill(b, 8.0, fill, Some(border));

            let lamp = self.plan.lamp(index);
            let ring = node.forced.then_some((2.0, GOLD));
            cx.scene.fill(
                lamp,
                LAMP * 0.5,
                if on { LIT } else { DARK.with_alpha(0.5) },
                ring,
            );
            // A line being typed over is the field's to show.
            let typed = |line| self.typed == Some((index, line));
            if !typed(Line::Name) {
                cx.scene.text(
                    &self.names[index],
                    Point::new(b.x + 28.0, b.y + 5.0),
                    Color::WHITE.with_alpha(if on { 1.0 } else { 0.75 }),
                );
            }
            let detail = if node.problem.is_some() {
                WARN
            } else {
                Color::WHITE.with_alpha(0.5)
            };
            if !typed(Line::Meaning) {
                cx.scene.text(
                    &self.details[index],
                    Point::new(b.x + 10.0, b.y + b.h * 0.5 + 3.0),
                    detail,
                );
            }

            // Ports: inputs down the left edge, the output on the right.
            for input in 0..node.inputs.len() {
                let at = self.plan.input(index, input, node.inputs.len());
                let from = self.index(&node.inputs[input]);
                let dot = Rect::new(at.x - PORT, at.y - PORT, PORT * 2.0, PORT * 2.0);
                cx.scene.fill(
                    dot,
                    PORT,
                    self.wire_colour(from),
                    Some((1.0, Color::hex(0x101216))),
                );
            }
            let out = self.plan.output(index);
            let dot = Rect::new(out.x - PORT, out.y - PORT, PORT * 2.0, PORT * 2.0);
            cx.scene.fill(
                dot,
                PORT,
                if on { LIT } else { DARK },
                Some((1.0, Color::hex(0x101216))),
            );
        }
    }

    fn event(&mut self, cx: &mut EventCx<Message>, event: &Event) -> Status {
        match event {
            Event::PointerMoved { pos } => {
                cx.emit(Message::Pointer(*pos));
                if self.held == Held::Nothing {
                    Status::Ignored
                } else {
                    Status::Captured
                }
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } => match self.press(*pos) {
                Some(message) => {
                    cx.emit(Message::Pointer(*pos));
                    cx.emit(message);
                    Status::Captured
                }
                None => Status::Ignored,
            },
            Event::PointerPressed {
                pos,
                button: PointerButton::Secondary,
            } => match self.alter(*pos) {
                Some(message) => {
                    cx.emit(Message::Pointer(*pos));
                    cx.emit(message);
                    Status::Captured
                }
                None => Status::Ignored,
            },
            Event::PointerReleased {
                pos,
                button: PointerButton::Primary,
            } => {
                if self.held == Held::Nothing {
                    return Status::Ignored;
                }
                let over =
                    (0..self.nodes.len()).find(|&index| self.plan.boxes[index].contains(*pos));
                match &self.held {
                    // A wire let go over a box is connected to it; let go over nothing, the
                    // input it came from is taken away.
                    Held::Wire { node, input } => match over {
                        Some(over) if self.nodes[over].name != *node => {
                            cx.emit(Message::Edit(Edit::Connect {
                                node: node.clone(),
                                input: *input,
                                to: self.nodes[over].name.clone(),
                            }))
                        }
                        Some(_) => {}
                        None => {
                            if let Some(index) = self.index(node) {
                                let mut inputs = self.nodes[index].inputs.clone();
                                if *input < inputs.len() {
                                    inputs.remove(*input);
                                }
                                cx.emit(Message::Edit(Edit::Inputs {
                                    node: node.clone(),
                                    inputs,
                                }));
                            }
                        }
                    },
                    // An output let go over a box that is worked out from others becomes
                    // one more of its inputs.
                    Held::Output { node } => {
                        let target = over.map(|over| &self.nodes[over]).filter(|target| {
                            target.name != *node
                                && target.kind != "source"
                                && target.kind != "constant"
                        });
                        if let Some(target) = target {
                            let mut inputs = target.inputs.clone();
                            inputs.push(node.clone());
                            cx.emit(Message::Edit(Edit::Inputs {
                                node: target.name.clone(),
                                inputs,
                            }));
                        }
                    }
                    _ => {}
                }
                cx.emit(Message::Release);
                Status::Captured
            }
            Event::Wheel { pos, delta } => {
                // Scrolling over a constant number turns it up and down.
                let over =
                    (0..self.nodes.len()).find(|&index| self.plan.boxes[index].contains(*pos));
                match over.map(|index| &self.nodes[index]) {
                    Some(node) if node.kind == "constant" => {
                        if let Signal::Number(value) = node.value {
                            let step = if delta.y > 0.0 { 1.0 } else { -1.0 };
                            cx.emit(Message::Edit(Edit::Set {
                                node: node.name.clone(),
                                value: Signal::Number(value + step),
                            }));
                        }
                        Status::Captured
                    }
                    _ if self.plan.panel.contains(*pos) => Status::Captured,
                    _ => Status::Ignored,
                }
            }
            _ => Status::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: &str, inputs: &[&str], value: Signal) -> NodeInfo {
        NodeInfo {
            name: name.to_owned(),
            kind: kind.to_owned(),
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
            value,
            changed: false,
            forced: false,
            problem: None,
        }
    }

    /// Two sources, a rule on them, a setting, and a pair that read each other.
    fn graph() -> Vec<NodeInfo> {
        vec![
            node("held", "source", &[], Signal::Bool(true)),
            node("contested", "source", &[], Signal::Bool(false)),
            node("calm", "not", &["contested"], Signal::Bool(true)),
            node("running", "and", &["held", "calm"], Signal::Bool(true)),
            node("limit", "constant", &[], Signal::Number(60.0)),
            node("open", "constant", &[], Signal::Bool(false)),
            node("tick", "not", &["tock"], Signal::Bool(true)),
            node("tock", "or", &["tick"], Signal::Bool(true)),
        ]
    }

    fn circuit(nodes: Vec<NodeInfo>, held: Held) -> Circuit {
        Circuit {
            plan: Plan::of(&nodes, &HashMap::new(), Point::new(0.0, 0.0)),
            nodes,
            held,
            ..Circuit::default()
        }
    }

    #[test]
    fn signals_are_laid_out_left_to_right_as_they_feed_each_other() {
        let nodes = graph();
        let plan = Plan::of(&nodes, &HashMap::new(), Point::new(10.0, 20.0));
        let column = |name: &str| {
            let index = nodes.iter().position(|node| node.name == name).unwrap();
            ((plan.boxes[index].x - 10.0 - PAD) / COLUMN).round() as usize
        };
        assert_eq!(
            [column("held"), column("contested"), column("limit")],
            [0, 0, 0]
        );
        assert_eq!([column("calm"), column("running")], [1, 2]);
        // A circle is cut where it closes rather than followed for ever.
        assert!(column("tick") <= 2 && column("tock") <= 2);
        // No two boxes share a place, and the panel holds them all.
        for (i, a) in plan.boxes.iter().enumerate() {
            assert!(
                plan.panel.contains(Point::new(a.x, a.y))
                    && plan.panel.contains(Point::new(a.x + a.w, a.y + a.h))
            );
            for b in &plan.boxes[i + 1..] {
                assert!(a.x != b.x || a.y != b.y);
            }
        }
        // A box that was dragged stays where it was put.
        let placed = HashMap::from([("calm".to_owned(), Point::new(500.0, 300.0))]);
        let moved = Plan::of(&nodes, &placed, Point::new(0.0, 0.0));
        assert_eq!(
            (moved.boxes[2].x, moved.boxes[2].y),
            (PAD + 500.0, TITLE + PAD + 300.0)
        );
        assert!(Plan::of(&[], &HashMap::new(), Point::new(0.0, 0.0)).panel.w == 0.0);
    }

    #[test]
    fn a_press_is_on_the_most_particular_thing_under_it() {
        let circuit = circuit(graph(), Held::Nothing);
        let index = |name: &str| circuit.index(name).unwrap();
        // An input port starts a wire.
        let port = circuit.plan.input(index("running"), 1, 2);
        assert!(matches!(
            circuit.press(port),
            Some(Message::Hold(HeldAt::Wire { node, input: 1 })) if node == "running"
        ));
        // A lamp forces: first true, then (once forced and true) false, then lets go.
        let lamp = circuit.plan.lamp(index("contested"));
        let lamp = Point::new(lamp.x + LAMP * 0.5, lamp.y + LAMP * 0.5);
        assert!(matches!(
            circuit.press(lamp),
            Some(Message::Edit(Edit::Force { node, value: Some(Signal::Bool(true)) })) if node == "contested"
        ));
        let mut forced = graph();
        forced[1].forced = true;
        forced[1].value = Signal::Bool(true);
        let again = self::circuit(forced.clone(), Held::Nothing);
        assert!(matches!(
            again.press(lamp),
            Some(Message::Edit(Edit::Force {
                value: Some(Signal::Bool(false)),
                ..
            }))
        ));
        forced[1].value = Signal::Bool(false);
        let third = self::circuit(forced, Held::Nothing);
        assert!(matches!(
            third.press(lamp),
            Some(Message::Edit(Edit::Force { value: None, .. }))
        ));
        // A true-or-false constant flips when its value is clicked; a number does not.
        let value = |name: &str| {
            let rect = circuit.plan.value(index(name));
            Point::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5)
        };
        assert!(matches!(
            circuit.press(value("open")),
            Some(Message::Edit(Edit::Set { node, value: Signal::Bool(true) })) if node == "open"
        ));
        assert!(matches!(
            circuit.press(value("limit")),
            Some(Message::Hold(HeldAt::Node { .. }))
        ));
        // The rest of a box moves it, the panel around the boxes moves the panel, and
        // outside the panel is the game's.
        assert!(
            matches!(circuit.press(value("calm")), Some(Message::Hold(HeldAt::Node { name, .. })) if name == "calm")
        );
        assert!(matches!(
            circuit.press(Point::new(4.0, 4.0)),
            Some(Message::Hold(HeldAt::Panel { .. }))
        ));
        let panel = circuit.plan.panel;
        assert!(circuit
            .press(Point::new(
                panel.x + panel.w + 50.0,
                panel.y + panel.h + 50.0
            ))
            .is_none());
    }

    #[test]
    fn the_panel_keeps_its_state_and_hands_on_its_edits() {
        let mut panel = SignalGraph {
            nodes: graph(),
            ..Default::default()
        };
        panel.update(Message::Hold(HeldAt::Node {
            name: "calm".into(),
            grip: Point::new(5.0, 5.0),
        }));
        panel.update(Message::Pointer(Point::new(219.0, 119.0)));
        assert_eq!(
            panel.placed.get("calm"),
            Some(&Point::new(200.0, 100.0)),
            "the pointer, less the grip and the panel's corner"
        );
        panel.update(Message::Release);
        assert_eq!(
            panel.take_edits(),
            [Edit::Place {
                node: "calm".into(),
                at: [200.0, 100.0]
            }],
            "and the game is told, so it is there next time"
        );
        panel.update(Message::Pointer(Point::new(900.0, 900.0)));
        assert_eq!(
            panel.placed.get("calm"),
            Some(&Point::new(200.0, 100.0)),
            "let go, it stays"
        );
        panel.update(Message::Hold(HeldAt::Panel {
            grip: Point::new(10.0, 10.0),
        }));
        panel.update(Message::Pointer(Point::new(110.0, 60.0)));
        assert_eq!((panel.origin.x, panel.origin.y), (100.0, 50.0));
        panel.update(Message::Release);

        let edit = Edit::Connect {
            node: "calm".into(),
            input: 0,
            to: "held".into(),
        };
        panel.update(Message::Edit(edit.clone()));
        assert_eq!(panel.take_edits(), [edit]);
        assert!(panel.take_edits().is_empty());
        panel.update(Message::Toggle);
        assert!(panel.hidden);
        assert_eq!(kind_text("compare GreaterOrEqual"), "≥");
        assert_eq!(value_text(Signal::Number(2.5)), "2.50");
    }

    #[test]
    fn edits_reach_the_games_signals() {
        let mut world = World::new();
        let mut signals = Signals::default();
        signals.set("limit", 60.0);
        signals.set("held", true);
        signals.set("contested", false);
        signals.define("calm", mira::signal::Op::Not, ["contested"]);
        world.insert_resource(signals);
        // What `apply_edits` does with what the panel hands it.
        for edit in [
            Edit::Connect {
                node: "calm".into(),
                input: 0,
                to: "held".into(),
            },
            Edit::Force {
                node: "held".into(),
                value: Some(Signal::Bool(false)),
            },
            Edit::Set {
                node: "limit".into(),
                value: Signal::Number(61.0),
            },
        ] {
            let signals = world.resource_mut::<Signals>();
            match edit {
                Edit::Connect { node, input, to } => assert!(signals.connect(&node, input, &to)),
                Edit::Force { node, value } => assert!(signals.force(&node, value)),
                Edit::Set { node, value } => signals.set(&node, value),
                Edit::Inputs { node, inputs } => assert!(signals.set_inputs(&node, inputs)),
                Edit::Op { node, op } => assert!(signals.set_op(&node, op)),
                Edit::Remove { node } => assert!(signals.remove(&node)),
                Edit::Rename { node, to } => assert!(signals.rename(&node, &to).is_ok()),
                Edit::Place { node, at } => signals.place(&node, at),
            }
        }
        world.resource_scope(|world, signals: &mut Signals| signals.update(world, 0.0));
        let signals = world.resource::<Signals>();
        assert!(
            signals.is_true("calm"),
            "not held, since held is forced false"
        );
        assert_eq!(signals.number("limit"), 61.0);
    }

    #[test]
    fn a_box_is_typed_over_to_name_it_and_say_what_it_is() {
        assert_eq!(meant(" True "), Some(Meant::Value(Signal::Bool(true))));
        assert_eq!(meant("42.5"), Some(Meant::Value(Signal::Number(42.5))));
        assert_eq!(meant("timer"), Some(Meant::Op(Op::Timer)));
        assert_eq!(meant("held for 5"), Some(Meant::Op(Op::HeldFor(5.0))));
        assert_eq!(meant(">="), meant("greater_or_equal"));
        assert!(matches!(meant("=="), Some(Meant::Op(Op::Compare(_)))));
        for nonsense in [
            "",
            "held_for",
            "held_for long",
            "constant",
            "and 1 2",
            "nan",
            "xor",
        ] {
            assert_eq!(meant(nonsense), None, "{nonsense}");
        }
        // What a line says is read back as what it is.
        for node in graph() {
            if node.kind != "source" {
                assert!(meant(&meaning_text(&node)).is_some(), "{}", node.kind);
            }
        }

        let mut panel = SignalGraph {
            nodes: graph(),
            ..Default::default()
        };
        let plan = Plan::of(&panel.nodes, &panel.placed, panel.origin);
        let names: Vec<String> = panel.nodes.iter().map(|node| node.name.clone()).collect();
        let corner = |name: &str| {
            let b = plan.boxes[names.iter().position(|known| known == name).unwrap()];
            Point::new(b.x, b.y)
        };
        let click = |panel: &mut SignalGraph, name: &str, down: f32| {
            let at = Point::new(corner(name).x + 60.0, corner(name).y + down);
            panel.update(Message::Pointer(at));
            panel.update(Message::Hold(HeldAt::Node {
                name: name.into(),
                grip: Point::new(60.0 + PAD, down + TITLE + PAD),
            }));
            panel.update(Message::Release);
        };

        // A click on the name types a new one; one that is taken, or no name, is not had.
        click(&mut panel, "calm", 12.0);
        assert_eq!(
            panel.typing,
            Some(Typing {
                node: "calm".into(),
                line: Line::Name,
                text: "calm".into()
            })
        );
        assert!(panel.take_edits().is_empty(), "a click moves nothing");
        for refused in ["limit", "two words", " "] {
            panel.update(Message::Typed(refused.into()));
            panel.update(Message::Enter);
            assert!(
                panel.typing.is_some() && panel.edits.is_empty(),
                "{refused}"
            );
        }
        panel.update(Message::Typed("quiet".into()));
        panel.update(Message::Enter);
        assert_eq!(panel.typing, None);
        assert_eq!(
            panel.take_edits(),
            [Edit::Rename {
                node: "calm".into(),
                to: "quiet".into()
            }]
        );

        // A click on the lower line types what the signal is.
        click(&mut panel, "limit", 40.0);
        assert_eq!(
            panel.typing.as_ref().map(|typing| typing.text.as_str()),
            Some("60")
        );
        panel.update(Message::Typed("ninety".into()));
        panel.update(Message::Enter);
        assert!(
            panel.typing.is_some(),
            "not a value or an operation: still typing"
        );
        panel.update(Message::Typed("90".into()));
        panel.update(Message::Enter);
        click(&mut panel, "running", 40.0);
        panel.update(Message::Typed("or".into()));
        panel.update(Message::Enter);
        assert_eq!(
            panel.take_edits(),
            [
                Edit::Set {
                    node: "limit".into(),
                    value: Signal::Number(90.0)
                },
                Edit::Op {
                    node: "running".into(),
                    op: Op::Or
                }
            ]
        );

        // A source is the game's to name; Escape drops what was typed; a drag is no click.
        click(&mut panel, "held", 12.0);
        assert_eq!(panel.typing, None);
        click(&mut panel, "limit", 12.0);
        panel.update(Message::Leave);
        assert_eq!(panel.typing, None);
        let at = Point::new(corner("limit").x + 60.0, corner("limit").y + 12.0);
        panel.update(Message::Pointer(at));
        panel.update(Message::Hold(HeldAt::Node {
            name: "limit".into(),
            grip: Point::new(60.0 + PAD, 12.0 + TITLE + PAD),
        }));
        panel.update(Message::Pointer(Point::new(at.x + 40.0, at.y)));
        panel.update(Message::Release);
        assert_eq!(panel.typing, None);
        assert!(matches!(&panel.take_edits()[..], [Edit::Place { node, .. }] if node == "limit"));
    }

    #[test]
    fn the_graph_itself_can_be_changed_from_the_panel() {
        let circuit = circuit(graph(), Held::Nothing);
        let index = |name: &str| circuit.index(name).unwrap();
        let middle = |name: &str| {
            let b = circuit.plan.boxes[index(name)];
            Point::new(b.x + b.w * 0.5, b.y + b.h * 0.75)
        };
        // An output port starts a new wire.
        assert!(matches!(
            circuit.press(circuit.plan.output(index("held"))),
            Some(Message::Hold(HeldAt::Output { node })) if node == "held"
        ));
        // A right-click on a box moves it on to the next operation; on something that has
        // no next (a source, a constant) it changes nothing; on the panel it makes a constant.
        assert!(matches!(
            circuit.alter(middle("running")),
            Some(Message::Edit(Edit::Op { node, op: Op::Or })) if node == "running"
        ));
        assert!(matches!(
            circuit.alter(middle("calm")),
            Some(Message::Edit(Edit::Op { op: Op::Count, .. }))
        ));
        assert!(matches!(
            circuit.alter(middle("held")),
            Some(Message::Pointer(_))
        ));
        let empty = Point::new(
            circuit.plan.panel.x + 4.0,
            circuit.plan.panel.y + circuit.plan.panel.h - 4.0,
        );
        assert!(matches!(
            circuit.alter(empty),
            Some(Message::Edit(Edit::Set { node, value: Signal::Bool(false) })) if node == "new.1"
        ));
        assert!(circuit.alter(Point::new(5000.0, 5000.0)).is_none());

        // The panel puts a new constant where it was asked for, and removes with Backspace
        // what the pointer is over, unless that is a source.
        let mut panel = SignalGraph {
            nodes: graph(),
            ..Default::default()
        };
        panel.update(Message::Pointer(Point::new(330.0, 260.0)));
        panel.update(Message::Edit(Edit::Set {
            node: "new.1".into(),
            value: Signal::Bool(false),
        }));
        assert_eq!(
            panel.placed.get("new.1"),
            Some(&Point::new(330.0 - 14.0 - PAD, 260.0 - 14.0 - TITLE - PAD))
        );
        let key = |key: Key| KeyEvent {
            key,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
            text: None,
        };
        let plan = Plan::of(&panel.nodes, &panel.placed, panel.origin);
        let over = |panel: &mut SignalGraph, name: &str| {
            let b = plan.boxes[panel
                .nodes
                .iter()
                .position(|node| node.name == name)
                .unwrap()];
            panel.update(Message::Pointer(Point::new(b.x + 20.0, b.y + 30.0)));
        };
        over(&mut panel, "calm");
        assert!(
            matches!(panel.on_key(&key(Key::Backspace)), Some(Message::Edit(Edit::Remove { node })) if node == "calm")
        );
        over(&mut panel, "held");
        assert!(
            panel.on_key(&key(Key::Backspace)).is_none(),
            "a source is the game's, not the panel's"
        );
        assert!(matches!(
            panel.on_key(&key(Key::F(1))),
            Some(Message::Toggle)
        ));
    }
}
