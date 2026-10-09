//! The signal graph, shown and edited inside the running game, as a circuit: a box for each
//! signal, its inputs on the left and its output on the right, and a wire for every
//! connection, lit while what it carries is true. See mira's `docs/SIGNALS.md`.
//!
//! ```ignore
//! app.add_plugins(mira_ui::signal_graph::plugin());
//! ```
//!
//! - Drag from an input (the dot on a box's left edge) to another box to rewire it.
//! - Click a box's lamp to force the signal true, again for false, again to let it go.
//! - Click a constant's value to flip it; scroll over a number to change it.
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
    signal::{NodeInfo, Signal, Signals},
};

use crate::{UiHost, UiPlugin};

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
    Force { node: String, value: Option<Signal> },
    /// Give a constant a new value.
    Set { node: String, value: Signal },
}

/// What the pointer is doing.
#[derive(Clone, Debug, Default, PartialEq)]
enum Held {
    #[default]
    Nothing,
    /// Moving a box; the offset is from its corner to the pointer.
    Node {
        name: String,
        grip: Point,
    },
    /// Pulling a wire out of an input.
    Wire {
        node: String,
        input: usize,
    },
    /// Moving the panel.
    Panel {
        grip: Point,
    },
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
            edits: Vec::new(),
        }
    }
}

impl SignalGraph {
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
}

/// What a press landed on.
#[derive(Clone, Debug)]
pub enum HeldAt {
    Node { name: String, grip: Point },
    Wire { node: String, input: usize },
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
                        let at = Point::new(
                            at.x - grip.x - self.origin.x,
                            at.y - grip.y - self.origin.y,
                        );
                        self.placed
                            .insert(name.clone(), Point::new(at.x.max(0.0), at.y.max(0.0)));
                    }
                    Held::Panel { grip } => self.origin = Point::new(at.x - grip.x, at.y - grip.y),
                    Held::Wire { .. } | Held::Nothing => {}
                }
            }
            Message::Hold(at) => {
                self.held = match at {
                    HeldAt::Node { name, grip } => Held::Node { name, grip },
                    HeldAt::Wire { node, input } => Held::Wire { node, input },
                    HeldAt::Panel { grip } => Held::Panel { grip },
                }
            }
            Message::Release => self.held = Held::Nothing,
            Message::Edit(edit) => self.edits.push(edit),
        }
    }

    fn view(&self) -> Element<Message> {
        if self.hidden {
            return Element::new(Circuit::default());
        }
        Element::new(Circuit {
            plan: Plan::of(&self.nodes, &self.placed, self.origin),
            nodes: self.nodes.clone(),
            held: self.held.clone(),
            pointer: self.pointer,
            ..Circuit::default()
        })
    }

    fn on_key(&self, key: &KeyEvent) -> Option<Message> {
        (key.pressed && !key.repeat && key.key == Key::F(1)).then_some(Message::Toggle)
    }
}

/// Reads the graph out of the world into the panel.
pub fn sync(world: &World, panel: &mut SignalGraph) {
    panel.nodes = world
        .get_resource::<Signals>()
        .map_or(Vec::new(), Signals::graph);
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

// --- the widget ---

#[derive(Default)]
struct Circuit {
    plan: Plan,
    nodes: Vec<NodeInfo>,
    held: Held,
    pointer: Point,
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
                "Signals      drag an input to rewire  ·  click a lamp to force  ·  F1 hides",
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
            let aimed_at = matches!(self.held, Held::Wire { .. }) && b.contains(self.pointer);
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
            cx.scene.text(
                &self.names[index],
                Point::new(b.x + 28.0, b.y + 5.0),
                Color::WHITE.with_alpha(if on { 1.0 } else { 0.75 }),
            );
            let detail = if node.problem.is_some() {
                WARN
            } else {
                Color::WHITE.with_alpha(0.5)
            };
            cx.scene.text(
                &self.details[index],
                Point::new(b.x + 10.0, b.y + b.h * 0.5 + 3.0),
                detail,
            );

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
            Event::PointerReleased {
                pos,
                button: PointerButton::Primary,
            } => {
                if self.held == Held::Nothing {
                    return Status::Ignored;
                }
                // A wire let go over a box is connected to it.
                if let Held::Wire { node, input } = &self.held {
                    let over =
                        (0..self.nodes.len()).find(|&index| self.plan.boxes[index].contains(*pos));
                    if let Some(over) = over.filter(|&over| self.nodes[over].name != *node) {
                        cx.emit(Message::Edit(Edit::Connect {
                            node: node.clone(),
                            input: *input,
                            to: self.nodes[over].name.clone(),
                        }));
                    }
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
}
