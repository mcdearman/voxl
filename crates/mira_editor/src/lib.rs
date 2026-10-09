//! mira's engine app: a game shown and worked on inside a [Neo](https://github.com/mcdearman/neo)
//! window.
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     let game = my_game::build()?;      // a mira `App`, as it would be run
//!     mira_editor::run(game)?;
//!     Ok(())
//! }
//! ```
//!
//! The window is Neo's, and so are the loop and the graphics device. The game is hosted
//! (`App::host`): it draws each frame into a texture, which the window shows in a viewport
//! among its own controls.
//!
//! The window is a dock of panels that can be dragged about, split and stacked as tabs:
//! the game, its entities as a tree, what the chosen entity is made of, its signals, and a
//! conversation with an agent that is working on the game. How they are arranged is kept in `.mira/editor.layout`
//! in the folder the app is run from. See mira's `docs/EDITOR.md` for what is to come.

pub mod agent;

use std::{
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

use agent::{Agent, Heard, NoAgent};

use mira::{
    ecs::Entity,
    input::{ButtonInput, KeyCode, Mouse, MouseButton},
    live::Live,
    prelude::Vec2,
    reflect::{TypeRegistry, Value},
    render::frame_texture,
    signal::{Signal, Signals},
    time::Time,
    transform::Parent,
};
use neo::prelude::*;
use neo::{wgpu, Color, Graphics, Image, Key, KeyEvent, Point, PointerButton, Rect};

/// What happens in the window.
#[derive(Clone, Debug)]
pub enum Message {
    /// The viewport has this much room, at this many pixels to the point.
    Resized(Rect, f32),
    /// Something done in the viewport: the game's to hear.
    Input(ViewportEvent),
    /// Pause the game, or let it run again.
    Pause,
    /// Run a paused game one frame on.
    Step,
    /// Hold the pointer in the viewport, for games that turn with the mouse, or stop.
    Mouselook,
    /// The panels were rearranged.
    Arranged(Dock),
    /// An entity was chosen in the tree.
    Chosen(Entity),
    /// An entity's children were shown or hidden.
    Opened(Entity, bool),
    /// An entity was dragged onto, before or after another in the tree.
    Moved(Entity, Entity, Place),
    /// A field of the chosen entity was given a new value in the inspector: the component
    /// by its full name, the way down to the field, and the value.
    Edited(String, Vec<String>, Value),
    /// Something was typed in what is being written to the agent.
    Writing(Action),
    /// What was written is sent to the agent.
    Ask,
    /// The agent is told to stop.
    Stop,
}

/// The panels, by the names the layout knows them by.
const GAME: &str = "Game";
const ENTITIES: &str = "Entities";
const SIGNALS: &str = "Signals";
const INSPECTOR: &str = "Inspector";
const AGENT: &str = "Agent";

/// Where the arrangement of the panels is kept, in the folder the app is run from.
const LAYOUT_FILE: &str = ".mira/editor.layout";

/// The arrangement to start from: the game over the conversation with the agent, and down
/// their right side the entities (with the
/// signals behind them) over what the chosen one is made of.
fn first_layout() -> Dock {
    Dock::beside(
        Dock::above(Dock::tabs([GAME]), 0.66, Dock::tabs([AGENT])),
        0.7,
        Dock::above(
            Dock::tabs([ENTITIES, SIGNALS]),
            0.5,
            Dock::tabs([INSPECTOR]),
        ),
    )
}

/// The arrangement kept from last time, if it still has every panel and no others.
fn kept_layout(kept: &str) -> Option<Dock> {
    let layout = Dock::parse(kept.trim())?;
    let mut panels = layout.panels();
    panels.sort_unstable();
    (panels == [AGENT, ENTITIES, GAME, INSPECTOR, SIGNALS]).then_some(layout)
}

/// What the lists show of the game, read from it now and then.
#[derive(Clone, Default, PartialEq)]
struct Lists {
    /// Every entity with a component the game has registered, those components, and the
    /// entity it is a child of.
    entities: Vec<(Entity, Vec<String>, Option<Entity>)>,
    /// The components of the chosen entity, by their full names, as plain data.
    chosen: Vec<(String, Value)>,
    /// Every signal and its value.
    signals: Vec<(String, Signal)>,
}

impl Lists {
    fn of(game: &mira::app::App, chosen: Option<Entity>) -> Self {
        let world = &game.world;
        let mut made_of = Vec::new();
        let mut entities: std::collections::BTreeMap<Entity, Vec<String>> = Default::default();
        if let Some(registry) = world.get_resource::<TypeRegistry>() {
            for component in registry.iter() {
                let name = short(component.name);
                for entity in (component.entities)(world) {
                    entities.entry(entity).or_default().push(name.to_owned());
                }
                if let Some(value) = chosen.and_then(|chosen| (component.get)(world, chosen)) {
                    made_of.push((component.name.to_owned(), value));
                }
            }
        }
        let signals = world
            .get_resource::<Signals>()
            .map_or(Vec::new(), |signals| {
                signals
                    .graph()
                    .into_iter()
                    .map(|node| (node.name, node.value))
                    .collect()
            });
        Self {
            entities: entities
                .into_iter()
                .map(|(entity, components)| {
                    // A parent that is gone is no parent: the entity stands at the top.
                    let parent = world
                        .get::<Parent>(entity)
                        .map(|parent| parent.0)
                        .filter(|parent| world.contains_entity(*parent));
                    (entity, components, parent)
                })
                .collect(),
            chosen: made_of,
            signals,
        }
    }
}

/// The app: a game, and the window's view of it.
pub struct Editor {
    game: mira::app::App,
    graphics: Option<Graphics>,
    hosted: bool,
    /// The viewport's room, in pixels, and how many of them make a point.
    size: (u32, u32),
    scale: f32,
    /// The game's frame, and the picture of it the window draws. Kept while the game keeps
    /// drawing into the same texture.
    shown: Option<(wgpu::Texture, Image)>,
    mouselook: bool,
    /// What the bar says of the game, as last looked at.
    status: Status,
    layout: Dock,
    lists: Lists,
    /// The entity chosen in the tree, and the ones whose children are hidden.
    chosen: Option<Entity>,
    shut: Vec<Entity>,
    /// The agent, what has been said with it, what is being written to it, and whether it
    /// is at work on something.
    agent: Box<dyn Agent>,
    said: Vec<Said>,
    writing: Document,
    working: bool,
    heard: (Sender<Heard>, Receiver<Heard>),
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Status {
    paused: bool,
    frame: u64,
}

impl Editor {
    pub fn new(game: mira::app::App) -> Self {
        Self {
            game,
            graphics: None,
            hosted: false,
            size: (0, 0),
            scale: 1.0,
            shown: None,
            mouselook: false,
            status: Status::default(),
            layout: std::fs::read_to_string(LAYOUT_FILE)
                .ok()
                .and_then(|kept| kept_layout(&kept))
                .unwrap_or_else(first_layout),
            lists: Lists::default(),
            chosen: None,
            shut: Vec::new(),
            agent: Box::new(NoAgent),
            said: Vec::new(),
            writing: Document::new(""),
            working: false,
            heard: channel(),
        }
    }

    /// Gives the app the agent its conversation panel talks to.
    pub fn with_agent(mut self, agent: impl Agent + 'static) -> Self {
        self.agent = Box::new(agent);
        self
    }

    /// What has been said between the person and the agent.
    pub fn said(&self) -> &[Said] {
        &self.said
    }

    /// Takes in what the agent has said since last looked. Says whether there was anything.
    fn listen(&mut self) -> bool {
        let mut any = false;
        while let Ok(heard) = self.heard.1.try_recv() {
            any = true;
            match heard {
                // Its answer grows where it stands, until something else is said.
                Heard::Text(more) => match self.said.last_mut() {
                    Some(last) if last.who == Speaker::Them => last.text += &more,
                    _ => self.said.push(Said::new(Speaker::Them, more)),
                },
                Heard::Did(what) => self.said.push(Said::new(Speaker::Note, what)),
                Heard::Done => self.working = false,
                Heard::Failed(why) => {
                    self.working = false;
                    self.said.push(Said::new(Speaker::Note, why));
                }
            }
        }
        any
    }

    /// The game being shown.
    pub fn game(&self) -> &mira::app::App {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut mira::app::App {
        &mut self.game
    }

    /// The entity chosen in the tree, whose parts the inspector shows.
    pub fn chosen(&self) -> Option<Entity> {
        self.chosen
    }

    fn resource<R: 'static>(&mut self) -> Option<&mut R> {
        self.game.world.get_resource_mut::<R>()
    }

    /// Passes on to the game what was done in the viewport.
    fn hear(&mut self, event: ViewportEvent) {
        let scale = self.scale;
        // The game counts in pixels; the window, in points.
        let pixels = move |at: Point| Vec2::new(at.x * scale, at.y * scale);
        match event {
            ViewportEvent::Moved(at) => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.position = Some(pixels(at));
                }
            }
            ViewportEvent::Pressed(at, button) | ViewportEvent::Released(at, button) => {
                let down = matches!(event, ViewportEvent::Pressed(..));
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.position = Some(pixels(at));
                }
                let button = match button {
                    PointerButton::Primary => MouseButton::Left,
                    PointerButton::Secondary => MouseButton::Right,
                    _ => MouseButton::Middle,
                };
                if let Some(buttons) = self.resource::<ButtonInput<MouseButton>>() {
                    if down {
                        buttons.press(button);
                    } else {
                        buttons.release(button);
                    }
                }
            }
            ViewportEvent::Wheel(_, delta) => {
                // The window says how far content moves; the game, how far the wheel turned:
                // the other way, and in lines.
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.scroll += Vec2::new(-delta.x, -delta.y) / 48.0;
                }
            }
            ViewportEvent::Motion(delta) => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.delta += Vec2::new(delta.x, delta.y);
                }
            }
            ViewportEvent::Key(key) => {
                let Some(code) = key_code(&key) else { return };
                if let Some(keys) = self.resource::<ButtonInput<KeyCode>>() {
                    if key.pressed {
                        keys.press(code);
                    } else {
                        keys.release(code);
                    }
                }
            }
            // Nothing is held any more: the game must not go on thinking so.
            ViewportEvent::AllReleased => {
                if let Some(keys) = self.resource::<ButtonInput<KeyCode>>() {
                    keys.release_all();
                }
                if let Some(buttons) = self.resource::<ButtonInput<MouseButton>>() {
                    buttons.release_all();
                }
            }
            ViewportEvent::Focused(false) => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.position = None;
                }
            }
            ViewportEvent::Focused(true) | ViewportEvent::Captured(_) => {}
        }
    }

    /// What a panel shows.
    fn panel(&self, panel: &str) -> Element<Message> {
        match panel {
            GAME => viewport(self.shown.as_ref().map(|(_, image)| image))
                .on_resize(Message::Resized)
                .on_input(Message::Input)
                // The game is always being drawn, running or held still.
                .playing(true)
                .capture(self.mouselook)
                .into(),
            ENTITIES => scrollable(
                container(
                    tree(&self.entity_tree(None), self.chosen.as_ref())
                        .on_select(Message::Chosen)
                        .on_toggle(Message::Opened)
                        .on_move(Message::Moved),
                )
                .padding(6.0),
            )
            .into(),
            INSPECTOR => self.inspector(),
            AGENT => {
                let asking = prompt(&self.writing, Message::Writing)
                    .on_submit(Message::Ask)
                    .placeholder(if self.working {
                        "The agent is working…"
                    } else {
                        "Ask the agent about the game, or to change it"
                    });
                let stop = button("Stop").on_press_maybe(self.working.then_some(Message::Stop));
                column()
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .push(container(transcript(&self.said)).height(Length::Fill))
                    .push(
                        container(row().spacing(8.0).align(Align::End).push(asking).push(stop))
                            .padding(8.0),
                    )
                    .into()
            }
            SIGNALS => lines(self.lists.signals.iter().map(|(name, value)| {
                let shown = match value {
                    Signal::Bool(value) => value.to_string(),
                    Signal::Number(value) => format!("{value:.2}"),
                };
                (name.clone(), shown, value.is_true())
            })),
            _ => text("").into(),
        }
    }

    /// What the chosen entity is made of, each field in a control that changes it.
    fn inspector(&self) -> Element<Message> {
        let Some(entity) = self.chosen else {
            return lines(
                [(
                    "Choose an entity in the tree.".to_owned(),
                    String::new(),
                    false,
                )]
                .into_iter(),
            );
        };
        let mut rows = column().spacing(6.0).width(Length::Fill);
        rows = rows.push(
            text(format!("entity {}", entity.index()))
                .mono()
                .tone(Tone::Muted),
        );
        for (component, value) in &self.lists.chosen {
            rows = rows.push(text(short(component)).weight(Weight::SEMIBOLD));
            rows = fields(rows, component, &mut Vec::new(), value);
        }
        scrollable(container(rows).padding(10.0)).into()
    }

    /// The entities under `parent` (or at the top), each with its own below it.
    fn entity_tree(&self, parent: Option<Entity>) -> Vec<TreeNode<Entity>> {
        self.lists
            .entities
            .iter()
            .filter(|(_, _, of)| *of == parent)
            .map(|(entity, components, _)| {
                let open = !self.shut.contains(entity);
                // Named by what tells it apart: the components besides where it is.
                let what: Vec<&str> = components
                    .iter()
                    .map(String::as_str)
                    .filter(|name| *name != "Transform" && *name != "Parent")
                    .collect();
                let label = if what.is_empty() {
                    format!("{}", entity.index())
                } else {
                    format!("{}  {}", entity.index(), what.join(", "))
                };
                // A shut entity's children are still built, so it shows that it has some.
                TreeNode::new(*entity, label).with(open, self.entity_tree(Some(*entity)))
            })
            .collect()
    }

    fn look(&self) -> Status {
        Status {
            paused: self
                .game
                .world
                .get_resource::<Live>()
                .is_some_and(Live::is_paused),
            frame: self
                .game
                .world
                .get_resource::<Time>()
                .map_or(0, Time::frame_count),
        }
    }
}

/// A component's name without where it comes from: `mira.Transform` is `Transform`.
fn short(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Puts `new` where `path` leads inside `value`: field names through maps, places through
/// lists. Says whether there was such a place.
fn put(value: &mut Value, path: &[String], new: Value) -> bool {
    let Some((step, rest)) = path.split_first() else {
        *value = new;
        return true;
    };
    let inside = match value {
        Value::Map(fields) => fields
            .iter_mut()
            .find(|(name, _)| name == step)
            .map(|(_, field)| field),
        Value::List(items) => step.parse::<usize>().ok().and_then(|at| items.get_mut(at)),
        _ => None,
    };
    inside.is_some_and(|inside| put(inside, rest, new))
}

/// The numbers of a list that is nothing but two to four of them: a vector, a colour.
fn numbers(items: &[Value]) -> Option<Vec<f64>> {
    if !(2..=4).contains(&items.len()) {
        return None;
    }
    items
        .iter()
        .map(|item| match item {
            Value::Float(value) => Some(*value),
            _ => None,
        })
        .collect()
}

/// A colour, if that is what a value is: fields `r`, `g`, `b` and perhaps `a`, all numbers.
/// mira keeps colours linear.
fn colour(fields: &[(String, Value)]) -> Option<[f64; 4]> {
    let part = |name: &str| {
        fields.iter().find_map(|(field, value)| match value {
            Value::Float(value) if field == name => Some(*value),
            _ => None,
        })
    };
    let named = fields
        .iter()
        .all(|(field, _)| matches!(field.as_str(), "r" | "g" | "b" | "a"));
    (named && fields.len() >= 3).then_some(())?;
    Some([part("r")?, part("g")?, part("b")?, part("a").unwrap_or(1.0)])
}

/// A linear amount of light as the number a screen is told, and back: what a colour code
/// and a colour picker count in.
fn encoded(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn linear(encoded: f64) -> f64 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// A row of the inspector: what the field is called, and the control for it.
fn field_row(name: &str, depth: usize, control: Element<Message>) -> Element<Message> {
    row()
        .spacing(8.0)
        .align(Align::Center)
        .push(
            text(format!("{}{name}", "  ".repeat(depth)))
                .mono()
                .tone(Tone::Muted)
                .width(112.0),
        )
        .push(control)
        .into()
}

/// Adds the controls for a value to the inspector's rows: one for each plain field, and
/// what is inside a field under its name. Each control sends the field's whole new value.
fn fields(
    mut rows: Column<Message>,
    component: &str,
    path: &mut Vec<String>,
    value: &Value,
) -> Column<Message> {
    let depth = path.len();
    let name = path.last().cloned().unwrap_or_default();
    let edited = {
        let (component, path) = (component.to_owned(), path.clone());
        move |value: Value| Message::Edited(component.clone(), path.clone(), value)
    };
    let said = |shown: String| -> Element<Message> { text(shown).mono().into() };
    let control: Element<Message> = match value {
        Value::Bool(on) => toggle(*on, move |on| edited(Value::Bool(on))).into(),
        Value::Float(number) => number_field(*number)
            .on_change(move |number| edited(Value::Float(number)))
            .width(Length::Fill)
            .into(),
        Value::Int(number) => number_field(*number as f64)
            .step(1.0)
            .on_change(move |number| edited(Value::Int(number.round() as i64)))
            .width(Length::Fill)
            .into(),
        Value::List(items) if numbers(items).is_some() => {
            let parts = numbers(items).expect("checked just above");
            let whole = parts.clone();
            vector_field(&parts, 0.1, move |part, number| {
                let mut whole = whole.clone();
                whole[part] = number;
                edited(Value::List(whole.into_iter().map(Value::Float).collect()))
            })
        }
        // A colour is picked as one; how see-through it is stays a number beside it.
        Value::Map(parts) if colour(parts).is_some() => {
            let [r, g, b, a] = colour(parts).expect("checked just above");
            let has_alpha = parts.iter().any(|(field, _)| field == "a");
            let whole = move |[r, g, b, a]: [f64; 4]| {
                let mut fields = vec![
                    ("r".to_owned(), Value::Float(r)),
                    ("g".to_owned(), Value::Float(g)),
                    ("b".to_owned(), Value::Float(b)),
                ];
                if has_alpha {
                    fields.push(("a".to_owned(), Value::Float(a)));
                }
                Value::Map(fields)
            };
            let shown = Color::rgb(encoded(r) as f32, encoded(g) as f32, encoded(b) as f32);
            let picked = edited.clone();
            let picker = color_field(shown).on_change(move |to: Color| {
                picked(whole([
                    linear(to.r as f64),
                    linear(to.g as f64),
                    linear(to.b as f64),
                    a,
                ]))
            });
            if !has_alpha {
                picker.into()
            } else {
                row()
                    .spacing(6.0)
                    .push(picker)
                    .push(
                        number_field(a)
                            .step(0.01)
                            .range(0.0..=1.0)
                            .label("A")
                            .on_change(move |a| edited(whole([r, g, b, a])))
                            .width(72.0),
                    )
                    .into()
            }
        }
        // What has parts is a name, with the parts under it.
        Value::Map(_) | Value::List(_) => {
            if depth > 0 {
                rows = rows.push(field_row(&name, depth - 1, said(String::new())));
            }
            let inside: Vec<(String, &Value)> = match value {
                Value::Map(fields) => fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field))
                    .collect(),
                Value::List(items) => items
                    .iter()
                    .enumerate()
                    .map(|(at, item)| (at.to_string(), item))
                    .collect(),
                _ => Vec::new(),
            };
            for (step, field) in inside {
                path.push(step);
                rows = fields(rows, component, path, field);
                path.pop();
            }
            return rows;
        }
        // Shown, and not yet changed here.
        Value::Null => said("none".to_owned()),
        Value::Text(written) => said(written.clone()),
        Value::Entity(bits) => said(format!("entity {}", Entity::from_bits(*bits).index())),
        Value::Asset { kind, name } => said(format!("{kind} {name}")),
    };
    rows.push(field_row(&name, depth.saturating_sub(1), control))
}

/// A list of rows, each a name and what there is to say of it, scrolling when it is long.
/// A row that is `lit` has its name in full strength; the rest are quieter.
fn lines(rows: impl Iterator<Item = (String, String, bool)>) -> Element<Message> {
    let mut list = column().spacing(4.0).width(Length::Fill);
    for (name, said, lit) in rows {
        let name = text(name).mono().no_wrap();
        let name = if lit { name } else { name.tone(Tone::Muted) };
        list = list.push(
            row()
                .spacing(10.0)
                .push(name)
                .push(text(said).mono().tone(Tone::Muted)),
        );
    }
    scrollable(container(list).padding(10.0)).into()
}

/// The key the game knows a window's key by: where it is on the keyboard, as near as the
/// name of what it types can say.
pub fn key_code(key: &KeyEvent) -> Option<KeyCode> {
    Some(match &key.key {
        Key::Enter => KeyCode::Enter,
        Key::Space => KeyCode::Space,
        Key::Tab => KeyCode::Tab,
        Key::Escape => KeyCode::Escape,
        Key::Backspace => KeyCode::Backspace,
        Key::Delete => KeyCode::Delete,
        Key::Left => KeyCode::ArrowLeft,
        Key::Right => KeyCode::ArrowRight,
        Key::Up => KeyCode::ArrowUp,
        Key::Down => KeyCode::ArrowDown,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::F(n @ 1..=12) => mira::input::key_named(&format!("F{n}"))?,
        Key::Character(typed) => {
            let mut letters = typed.chars();
            let (letter, None) = (letters.next()?, letters.next()) else {
                return None;
            };
            match letter.to_ascii_uppercase() {
                letter @ 'A'..='Z' => mira::input::key_named(&format!("Key{letter}"))?,
                digit @ '0'..='9' => mira::input::key_named(&format!("Digit{digit}"))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

impl App for Editor {
    type Message = Message;

    fn title(&self) -> String {
        "mira".to_owned()
    }

    fn wanted_limits(&self, available: &wgpu::Limits) -> wgpu::Limits {
        // As the game asks for in a window of its own: room for big merged meshes.
        wgpu::Limits {
            max_buffer_size: available.max_buffer_size,
            ..wgpu::Limits::default()
        }
    }

    fn graphics(&mut self, graphics: &Graphics) {
        self.graphics = Some(graphics.clone());
    }

    fn step(&mut self, _now: Instant, _dt: Duration) -> bool {
        let (Some(graphics), (width, height)) = (&self.graphics, self.size) else {
            return false;
        };
        if width == 0 || height == 0 {
            return false;
        }
        if !self.hosted {
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let (device, queue) = (graphics.device.clone(), graphics.queue.clone());
            self.game.host(device, queue, format, width, height);
            self.hosted = true;
        }
        self.game.update();
        let mut changed = self.listen();
        let frame = frame_texture(&self.game.world);
        if frame.as_ref() != self.shown.as_ref().map(|(texture, _)| texture) {
            self.shown = frame.map(|texture| {
                // The window's canvas holds colours as they are stored, so it is given the
                // frame's bytes and not what an sRGB view would make of them.
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(texture.format().remove_srgb_suffix()),
                    ..Default::default()
                });
                let image = Image::from_texture(view, texture.width(), texture.height());
                (texture, image)
            });
            changed = true;
        }
        let status = self.look();
        // The lists are read a few times a second: often enough to watch, and not a walk
        // of every component on every frame.
        if status.frame.is_multiple_of(10) || status.frame != self.status.frame && status.paused {
            let lists = Lists::of(&self.game, self.chosen);
            if lists != self.lists {
                self.lists = lists;
                changed = true;
            }
        }
        changed |= std::mem::replace(&mut self.status, status) != status;
        changed
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Resized(bounds, scale) => {
                let size = (
                    (bounds.w * scale).round() as u32,
                    (bounds.h * scale).round() as u32,
                );
                self.scale = scale;
                if std::mem::replace(&mut self.size, size) != size && self.hosted {
                    self.game.host_resized(size.0, size.1);
                }
            }
            Message::Input(event) => self.hear(event),
            Message::Pause => {
                if let Some(live) = self.resource::<Live>() {
                    if live.is_paused() {
                        live.resume();
                    } else {
                        live.pause();
                    }
                }
            }
            Message::Step => {
                if let Some(live) = self.resource::<Live>() {
                    live.step_frames(1);
                }
            }
            Message::Mouselook => self.mouselook = !self.mouselook,
            Message::Chosen(entity) => {
                self.chosen = Some(entity);
                self.lists = Lists::of(&self.game, self.chosen);
            }
            Message::Opened(entity, open) => {
                self.shut.retain(|shut| *shut != entity);
                if !open {
                    self.shut.push(entity);
                }
            }
            Message::Moved(dragged, onto, place) => {
                // Into an entity makes it the parent; beside one, a child of the same parent.
                let parent = match place {
                    Place::Into => Some(onto),
                    Place::Before | Place::After => {
                        self.game.world.get::<Parent>(onto).map(|parent| parent.0)
                    }
                };
                match parent {
                    Some(parent) => {
                        self.game.world.insert(dragged, Parent(parent));
                    }
                    None => {
                        self.game.world.remove::<Parent>(dragged);
                    }
                }
                self.lists = Lists::of(&self.game, self.chosen);
            }
            Message::Edited(component, path, value) => {
                let Some(entity) = self.chosen else { return };
                let world = &mut self.game.world;
                // The component as it is now, with the one field changed, put back whole.
                world.resource_scope(|world, registry: &mut TypeRegistry| {
                    let Some(kind) = registry.get(&component) else {
                        return;
                    };
                    let Some(mut whole) = (kind.get)(world, entity) else {
                        return;
                    };
                    if put(&mut whole, &path, value) {
                        if let Err(why) = (kind.insert)(world, entity, &whole) {
                            eprintln!("{component} would not take that: {why}");
                        }
                    }
                });
                self.lists = Lists::of(&self.game, self.chosen);
            }
            Message::Writing(action) => {
                self.writing.apply(action);
            }
            Message::Ask => {
                let asked = self.writing.text().trim().to_owned();
                if asked.is_empty() || self.working {
                    return;
                }
                self.writing = Document::new("");
                self.said.push(Said::new(Speaker::You, asked.clone()));
                self.working = true;
                self.agent.ask(&asked, self.heard.0.clone());
            }
            Message::Stop => {
                self.agent.stop();
                if std::mem::take(&mut self.working) {
                    self.said.push(Said::new(Speaker::Note, "Stopped."));
                }
            }
            Message::Arranged(layout) => {
                // Kept for next time; an arrangement that can't be written is still used.
                let _ = std::fs::create_dir_all(".mira")
                    .and_then(|()| std::fs::write(LAYOUT_FILE, layout.encode()));
                self.layout = layout;
            }
        }
    }

    fn view(&self) -> Element<Message> {
        let Status { paused, frame } = self.status;
        let bar = row()
            .spacing(8.0)
            .align(Align::Center)
            .push(button(if paused { "Resume" } else { "Pause" }).on_press(Message::Pause))
            .push(button("Step").on_press_maybe(paused.then_some(Message::Step)))
            .push(
                button(if self.mouselook {
                    "Mouselook: on"
                } else {
                    "Mouselook: off"
                })
                .on_press(Message::Mouselook),
            )
            .push(text(format!(
                "frame {frame}{}",
                if paused { ", paused" } else { "" }
            )));
        let panels = dock(
            &self.layout,
            |panel| panel.to_owned(),
            |panel| self.panel(panel),
            Message::Arranged,
        );
        column()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(container(bar).padding(8.0))
            .push(panels)
            .into()
    }
}

/// Where `mira-mcp` is: `MIRA_MCP` if set, else beside this program (or one folder up, where
/// an example finds it), else whatever the system finds by that name.
fn tools_program() -> std::path::PathBuf {
    if let Some(given) = std::env::var_os("MIRA_MCP") {
        return given.into();
    }
    let beside = std::env::current_exe().ok().and_then(|program| {
        let folder = program.parent()?;
        [folder.join("mira-mcp"), folder.parent()?.join("mira-mcp")]
            .into_iter()
            .find(|there| there.exists())
    });
    beside.unwrap_or_else(|| "mira-mcp".into())
}

/// Opens the engine app on a game, and returns when its window is closed.
///
/// The app's agent is Claude Code, given the game's tools: the game is made to listen for
/// them on a port of its own if it is not listening already. See [`agent::ClaudeCode`].
pub fn run(mut game: mira::app::App) -> Result<(), Box<dyn std::error::Error>> {
    let listening = match game.debugger_address() {
        Some(address) => Some(address),
        None => game.listen_for_debugger("127.0.0.1:0").ok(),
    };
    let editor = Editor::new(game);
    let editor = match listening {
        Some(address) => {
            editor.with_agent(agent::ClaudeCode::new(tools_program(), address.to_string()))
        }
        // An agent with no way to the game would only guess: better none.
        None => editor,
    };
    neo::run(editor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
            text: None,
        }
    }

    #[test]
    fn a_windows_keys_are_the_games_keys() {
        let typed = |letter: &str| key_code(&key(Key::Character(letter.to_owned())));
        assert_eq!(typed("w"), Some(KeyCode::KeyW));
        assert_eq!(
            typed("W"),
            Some(KeyCode::KeyW),
            "held with shift, the same key"
        );
        assert_eq!(typed("7"), Some(KeyCode::Digit7));
        assert_eq!(key_code(&key(Key::Space)), Some(KeyCode::Space));
        assert_eq!(key_code(&key(Key::Left)), Some(KeyCode::ArrowLeft));
        assert_eq!(key_code(&key(Key::F(5))), Some(KeyCode::F5));
        // What the game has no key for is not passed on as some other key.
        assert_eq!(typed("é"), None);
        assert_eq!(typed("ab"), None);
        assert_eq!(key_code(&key(Key::F(40))), None);
        assert_eq!(key_code(&key(Key::Other)), None);
    }

    #[test]
    fn the_arrangement_is_kept_only_while_it_fits_the_panels_there_are() {
        let first = first_layout();
        assert_eq!(first.shown(), [GAME, AGENT, ENTITIES, INSPECTOR]);
        // Written and read back, it is the same; rearranged, it is still taken.
        assert_eq!(kept_layout(&first.encode()), Some(first.clone()));
        let stacked = first.with(SIGNALS, ENTITIES, Side::Middle);
        assert_eq!(
            kept_layout(&format!("{}\n", stacked.encode())),
            Some(stacked)
        );
        // One from a version with other panels, or no arrangement at all, is not.
        assert_eq!(kept_layout(&Dock::tabs([GAME, "Console"]).encode()), None);
        assert_eq!(kept_layout(&Dock::tabs([GAME]).encode()), None);
        assert_eq!(kept_layout("not a layout"), None);
    }

    #[test]
    fn the_lists_say_what_is_in_the_game() {
        use mira::prelude::*;
        let mut game = mira::app::App::new();
        game.add_plugins(mira::transform::TransformPlugin)
            .add_plugins(SignalPlugin);
        let entity = game.world.spawn(Transform::from_xyz(1.0, 2.0, 3.0));
        game.world.resource_mut::<Signals>().set("open", true);
        let lists = Lists::of(&game, Some(entity));
        assert_eq!(
            lists.entities,
            [(entity, vec!["Transform".to_owned()], None)]
        );
        assert_eq!(lists.signals, [("open".to_owned(), Signal::Bool(false))]);
        game.update();
        assert_eq!(
            Lists::of(&game, None).signals,
            [("open".to_owned(), Signal::Bool(true))]
        );
        assert_eq!(lists.chosen[0].0, "mira.Transform");
    }

    #[test]
    fn a_field_changed_in_the_inspector_is_changed_in_the_game() {
        use mira::prelude::*;
        let mut game = mira::app::App::new();
        game.add_plugins(mira::transform::TransformPlugin);
        let entity = game.world.spawn(Transform::from_xyz(1.0, 2.0, 3.0));
        let mut editor = Editor::new(game);
        editor.update(Message::Chosen(entity));
        let edit = |editor: &mut Editor, path: &[&str], value: Value| {
            let path = path.iter().map(|step| step.to_string()).collect();
            editor.update(Message::Edited("mira.Transform".into(), path, value));
        };
        // A whole vector, as the vector field sends it; then one part of another.
        let moved = Value::List(vec![
            Value::Float(5.0),
            Value::Float(2.0),
            Value::Float(3.0),
        ]);
        edit(&mut editor, &["translation"], moved);
        edit(&mut editor, &["scale", "1"], Value::Float(4.0));
        let at = *editor.game().world.get::<Transform>(entity).unwrap();
        assert_eq!(at.translation, Vec3::new(5.0, 2.0, 3.0));
        assert_eq!(at.scale, Vec3::new(1.0, 4.0, 1.0));
        // The inspector shows what the game now has.
        let shown = &editor.lists.chosen[0].1;
        assert_eq!(shown.get_path("translation.0"), Some(&Value::Float(5.0)));

        // A field that isn't there, or a value the component can't be made from, changes
        // nothing.
        edit(&mut editor, &["weight"], Value::Float(9.0));
        edit(&mut editor, &["translation"], Value::Text("north".into()));
        assert_eq!(*editor.game().world.get::<Transform>(entity).unwrap(), at);

        // Where a path leads, and where it doesn't.
        let mut value = Value::Map(vec![(
            "a".into(),
            Value::List(vec![Value::Int(1), Value::Int(2)]),
        )]);
        assert!(put(&mut value, &["a".into(), "1".into()], Value::Int(7)));
        assert_eq!(value.get_path("a.1"), Some(&Value::Int(7)));
        assert!(!put(&mut value, &["a".into(), "5".into()], Value::Int(7)));
        assert!(!put(&mut value, &["b".into()], Value::Int(7)));
        assert_eq!(
            numbers(&[Value::Float(1.0), Value::Float(2.0)]),
            Some(vec![1.0, 2.0])
        );
        assert_eq!(numbers(&[Value::Float(1.0)]), None);
        assert_eq!(numbers(&[Value::Float(1.0), Value::Int(2)]), None);

        // A colour is known by its fields, and goes to a picker and back unchanged.
        let rgba = |parts: &[(&str, f64)]| -> Vec<(String, Value)> {
            parts
                .iter()
                .map(|(name, value)| (name.to_string(), Value::Float(*value)))
                .collect()
        };
        assert_eq!(
            colour(&rgba(&[("r", 0.7), ("g", 0.1), ("b", 0.1), ("a", 0.5)])),
            Some([0.7, 0.1, 0.1, 0.5])
        );
        assert_eq!(
            colour(&rgba(&[("r", 0.7), ("g", 0.1), ("b", 0.1)])),
            Some([0.7, 0.1, 0.1, 1.0])
        );
        assert_eq!(colour(&rgba(&[("x", 0.7), ("y", 0.1), ("z", 0.1)])), None);
        assert_eq!(colour(&rgba(&[("r", 0.7), ("g", 0.1)])), None);
        for amount in [0.0, 0.002, 0.18, 0.5, 1.0] {
            assert!((linear(encoded(amount)) - amount).abs() < 1e-9, "{amount}");
        }
        assert!(
            (encoded(0.214) - 0.5).abs() < 0.001,
            "middle grey on a screen"
        );
    }

    #[test]
    fn entities_are_a_tree_that_dragging_rearranges() {
        use mira::prelude::*;
        let mut game = mira::app::App::new();
        game.add_plugins(mira::transform::TransformPlugin);
        let tank = game.world.spawn(Transform::IDENTITY);
        let turret = game.world.spawn((Transform::IDENTITY, Parent(tank)));
        let barrel = game.world.spawn((Transform::IDENTITY, Parent(turret)));
        let crate_ = game.world.spawn(Transform::IDENTITY);
        let mut editor = Editor::new(game);
        editor.lists = Lists::of(&editor.game, None);
        let shape = |nodes: &[TreeNode<Entity>]| -> Vec<(Entity, usize)> {
            nodes
                .iter()
                .map(|node| (node.id, node.children.len()))
                .collect()
        };
        let top = editor.entity_tree(None);
        assert_eq!(shape(&top), [(tank, 1), (crate_, 0)]);
        assert_eq!(shape(&top[0].children), [(turret, 1)]);
        assert!(top[0].open && top[0].label.starts_with(&tank.index().to_string()));

        // Shut, an entity keeps its children but does not show them open.
        editor.update(Message::Opened(tank, false));
        assert!(!editor.entity_tree(None)[0].open);
        editor.update(Message::Opened(tank, true));

        // Dropped into the crate, the barrel is the crate's; beside the tank, nobody's.
        editor.update(Message::Moved(barrel, crate_, Place::Into));
        assert_eq!(
            editor.game.world.get::<Parent>(barrel),
            Some(&Parent(crate_))
        );
        assert_eq!(shape(&editor.entity_tree(None)), [(tank, 1), (crate_, 1)]);
        editor.update(Message::Moved(barrel, tank, Place::After));
        assert_eq!(editor.game.world.get::<Parent>(barrel), None);
        // Beside the turret, it is the tank's, as the turret is.
        editor.update(Message::Moved(crate_, turret, Place::Before));
        assert_eq!(editor.game.world.get::<Parent>(crate_), Some(&Parent(tank)));

        // Choosing one shows what it is made of.
        editor.update(Message::Chosen(turret));
        let made_of: Vec<&str> = editor
            .lists
            .chosen
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(made_of, ["mira.Transform", "mira.Parent"]);
    }

    /// An agent that says what it was told to, for tests.
    struct Scripted(Vec<Heard>, std::rc::Rc<std::cell::Cell<u32>>);

    impl Agent for Scripted {
        fn ask(&mut self, asked: &str, heard: Sender<Heard>) {
            let _ = heard.send(Heard::Did(format!("asked: {asked}")));
            for said in self.0.drain(..) {
                let _ = heard.send(said);
            }
        }

        fn stop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
    }

    #[test]
    fn a_conversation_with_the_agent_is_kept_as_it_is_said() {
        let stops = std::rc::Rc::new(std::cell::Cell::new(0));
        let script = vec![
            Heard::Text("The clock ".into()),
            Heard::Text("is paused.".into()),
            Heard::Did("mira_signals".into()),
            Heard::Text("Blue is on a site.".into()),
            Heard::Done,
        ];
        let mut editor =
            Editor::new(mira::app::App::new()).with_agent(Scripted(script, stops.clone()));
        let write = |editor: &mut Editor, text: &str| {
            editor.writing = Document::new(text);
            editor.update(Message::Ask);
        };
        // Nothing written, nothing asked.
        write(&mut editor, "  ");
        assert!(editor.said().is_empty() && !editor.working);

        write(&mut editor, "Why is the clock stopped?");
        assert!(editor.working && editor.writing.text().is_empty());
        // Asked again while it works: not sent, and what was written is kept.
        write(&mut editor, "And now?");
        assert_eq!(editor.said().len(), 1);
        assert_eq!(editor.writing.text(), "And now?");

        assert!(editor.listen());
        let said: Vec<(Speaker, &str)> = editor
            .said()
            .iter()
            .map(|said| (said.who, said.text.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (Speaker::You, "Why is the clock stopped?"),
                (Speaker::Note, "asked: Why is the clock stopped?"),
                // What comes in pieces is one answer, until something else happens.
                (Speaker::Them, "The clock is paused."),
                (Speaker::Note, "mira_signals"),
                (Speaker::Them, "Blue is on a site."),
            ]
        );
        assert!(!editor.working && !editor.listen());

        // Stopped while at work, it is told so and the conversation says so; when not at
        // work, stopping says nothing.
        write(&mut editor, "And now?");
        editor.update(Message::Stop);
        assert_eq!(stops.get(), 1);
        assert_eq!(
            editor.said().last().map(|said| said.text.as_str()),
            Some("Stopped.")
        );
        let length = editor.said().len();
        editor.update(Message::Stop);
        assert_eq!(editor.said().len(), length);

        // With no agent, asking says how to have one.
        let mut alone = Editor::new(mira::app::App::new());
        write(&mut alone, "Hello?");
        alone.listen();
        assert!(alone.said()[1].text.contains("There is no agent"));
        assert!(!alone.working);
    }

    #[test]
    fn what_is_done_in_the_viewport_reaches_the_game() {
        let mut game = mira::app::App::new();
        game.add_plugins(mira::input::InputPlugin);
        let mut editor = Editor::new(game);
        editor.update(Message::Resized(Rect::new(0.0, 40.0, 400.0, 300.0), 2.0));
        assert_eq!(editor.size, (800, 600));

        // Points in the window are pixels in the game.
        let at = Point::new(100.0, 50.0);
        editor.update(Message::Input(ViewportEvent::Pressed(
            at,
            PointerButton::Primary,
        )));
        editor.update(Message::Input(ViewportEvent::Key(key(Key::Character(
            "d".into(),
        )))));
        editor.update(Message::Input(ViewportEvent::Motion(Point::new(3.0, -2.0))));
        // The wheel turned up a notch: content moving down, as the window tells it.
        let wheel = ViewportEvent::Wheel(at, Point::new(0.0, -48.0));
        editor.update(Message::Input(wheel));
        let world = &editor.game().world;
        let mouse = world.resource::<Mouse>();
        assert_eq!(mouse.position, Some(Vec2::new(200.0, 100.0)));
        assert_eq!(
            (mouse.delta, mouse.scroll),
            (Vec2::new(3.0, -2.0), Vec2::new(0.0, 1.0))
        );
        assert!(world
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left));
        assert!(world
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyD));

        // The viewport let go of everything: nothing stays held in the game.
        editor.update(Message::Input(ViewportEvent::AllReleased));
        let world = &editor.game().world;
        assert!(!world
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left));
        assert!(!world
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyD));
    }
}
