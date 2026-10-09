//! The signal graph, shown live inside the game: which signals are true, what just changed,
//! what feeds what. See mira's `docs/SIGNALS.md`.
//!
//! ```ignore
//! app.add_plugins(mira_ui::signal_graph::plugin());
//! ```
//!
//! F1 hides and shows it. A click on the panel is the panel's, not the game's.

use armature::{
    App, Color, Cx, DrawCx, Element, Event, EventCx, FontFamily, Key, KeyEvent, Limits, Point,
    Rect, Size, Status, TextLayout, TextStyle, Widget,
};
use mira::{ecs::World, signal::Signals};

use crate::UiPlugin;

/// One signal as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    /// The line of the tree: its branches, the signal's name, its value and its kind.
    text: String,
    /// True, or not zero.
    on: bool,
    changed: bool,
    forced: bool,
    problem: bool,
}

/// The panel's state: the graph as of this frame.
#[derive(Default)]
pub struct SignalGraph {
    rows: Vec<Row>,
    hidden: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Toggle,
}

impl App for SignalGraph {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Toggle => self.hidden = !self.hidden,
        }
    }

    fn view(&self) -> Element<Message> {
        Element::new(Panel {
            rows: if self.hidden {
                Vec::new()
            } else {
                self.rows.clone()
            },
            lines: Vec::new(),
            heading: None,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
        })
    }

    fn on_key(&self, key: &KeyEvent) -> Option<Message> {
        (key.pressed && !key.repeat && key.key == Key::F(1)).then_some(Message::Toggle)
    }
}

/// Reads the graph out of the world into the panel.
pub fn sync(world: &World, panel: &mut SignalGraph) {
    let Some(signals) = world.get_resource::<Signals>() else {
        panel.rows.clear();
        return;
    };
    // The same drawing the terminal viewer makes, a line a signal, with its marks taken off
    // the front and the end and turned into colour.
    let drawn = mira::remote::signals_text(&signals.to_value());
    panel.rows = drawn
        .lines()
        .map(|line| {
            let on = line.contains('●');
            let problem = line.trim_end().ends_with(')')
                && line.contains("  (")
                && line.matches("  (").count() > 1;
            let (line, forced) = match line.strip_suffix(" !") {
                Some(rest) => (rest, true),
                None => (line, false),
            };
            let (line, changed) = match line.strip_suffix(" *") {
                Some(rest) => (rest, true),
                None => (line, false),
            };
            Row {
                text: line.replace("● ", "").replace("○ ", ""),
                on,
                changed,
                forced,
                problem: problem || line.starts_with('?'),
            }
        })
        .collect();
}

/// The plugin that shows the signal graph in the game.
pub fn plugin() -> UiPlugin<SignalGraph> {
    UiPlugin::new(SignalGraph::default).sync(sync)
}

const MARGIN: f32 = 14.0;
const PADDING: f32 = 12.0;
const DOT: f32 = 8.0;

struct Panel {
    rows: Vec<Row>,
    lines: Vec<TextLayout>,
    heading: Option<TextLayout>,
    bounds: Rect,
}

impl Panel {
    fn style() -> TextStyle {
        TextStyle {
            size: 12.5,
            weight: 500,
            family: FontFamily::Mono,
            line_height: 1.45,
            letter_spacing: 0.0,
        }
    }
}

impl Widget<Message> for Panel {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = Self::style();
        self.lines = self
            .rows
            .iter()
            .map(|row| cx.text().layout(&row.text, &style, None))
            .collect();
        let heading = TextStyle {
            weight: 700,
            family: FontFamily::Sans,
            ..style
        };
        self.heading =
            (!self.rows.is_empty()).then(|| cx.text().layout("Signals   F1 hides", &heading, None));
        let widest = self
            .lines
            .iter()
            .chain(&self.heading)
            .map(|line| line.size().w)
            .fold(0.0, f32::max);
        let tall: f32 = self
            .lines
            .iter()
            .chain(&self.heading)
            .map(|line| line.size().h)
            .sum();
        self.bounds = Rect::new(
            MARGIN,
            MARGIN,
            widest + DOT + 8.0 + PADDING * 2.0,
            tall + PADDING * 2.0 + 6.0,
        );
        // The widget is the whole screen; only the panel is drawn, so only it is "there".
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let Some(heading) = &self.heading else {
            return;
        };
        cx.scene.fill(
            self.bounds,
            10.0,
            Color::hex(0x14161a).with_alpha(0.86),
            Some((1.0, Color::WHITE.with_alpha(0.12))),
        );
        let mut y = self.bounds.y + PADDING;
        let x = self.bounds.x + PADDING;
        cx.scene
            .text(heading, Point::new(x, y), Color::WHITE.with_alpha(0.55));
        y += heading.size().h + 6.0;
        for (row, line) in self.rows.iter().zip(&self.lines) {
            let height = line.size().h;
            let colour = if row.problem {
                Color::hex(0xff6b6b)
            } else if row.on {
                Color::hex(0x7ee08a)
            } else {
                Color::WHITE.with_alpha(0.45)
            };
            // A lamp for each signal: lit while it is true, ringed when it is being forced.
            let lamp = Rect::new(x, y + (height - DOT) * 0.5, DOT, DOT);
            let ring = row.forced.then_some((1.5, Color::hex(0xffc857)));
            cx.scene.fill(
                lamp,
                DOT * 0.5,
                if row.on {
                    colour
                } else {
                    colour.with_alpha(0.25)
                },
                ring,
            );
            if row.changed {
                // What changed this frame is underlined for the frame.
                let under = Rect::new(x + DOT + 8.0, y + height - 2.0, line.size().w, 1.0);
                cx.scene
                    .fill(under, 0.0, Color::hex(0xffc857).with_alpha(0.8), None);
            }
            cx.scene.text(
                line,
                Point::new(x + DOT + 8.0, y),
                if row.on { Color::WHITE } else { colour },
            );
            y += height;
        }
    }

    fn event(&mut self, _cx: &mut EventCx<Message>, _event: &Event) -> Status {
        Status::Ignored
    }
}
