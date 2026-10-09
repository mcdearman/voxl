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
//! the game, its entities as a tree, what the chosen entity is made of, and its signals. How they are arranged is kept in `.mira/editor.layout`
//! in the folder the app is run from. See mira's `docs/EDITOR.md` for what is to come.

use std::time::{Duration, Instant};

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
use neo::{wgpu, Graphics, Image, Key, KeyEvent, Point, PointerButton, Rect};

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
}

/// The panels, by the names the layout knows them by.
const GAME: &str = "Game";
const ENTITIES: &str = "Entities";
const SIGNALS: &str = "Signals";
const INSPECTOR: &str = "Inspector";

/// Where the arrangement of the panels is kept, in the folder the app is run from.
const LAYOUT_FILE: &str = ".mira/editor.layout";

/// The arrangement to start from: the game, and down its right side the entities (with the
/// signals behind them) over what the chosen one is made of.
fn first_layout() -> Dock {
    Dock::beside(
        Dock::tabs([GAME]),
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
    (panels == [ENTITIES, GAME, INSPECTOR, SIGNALS]).then_some(layout)
}

/// What the lists show of the game, read from it now and then.
#[derive(Clone, Default, PartialEq)]
struct Lists {
    /// Every entity with a component the game has registered, those components, and the
    /// entity it is a child of.
    entities: Vec<(Entity, Vec<String>, Option<Entity>)>,
    /// The components of the chosen entity, by name, as plain data.
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
                let name = component.name.rsplit('.').next().unwrap_or(component.name);
                for entity in (component.entities)(world) {
                    entities.entry(entity).or_default().push(name.to_owned());
                }
                if let Some(value) = chosen.and_then(|chosen| (component.get)(world, chosen)) {
                    made_of.push((name.to_owned(), value));
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
        }
    }

    /// The game being shown.
    pub fn game(&self) -> &mira::app::App {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut mira::app::App {
        &mut self.game
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
            INSPECTOR => match self.chosen {
                None => lines(
                    [(
                        "Choose an entity in the tree.".to_owned(),
                        String::new(),
                        false,
                    )]
                    .into_iter(),
                ),
                Some(entity) => lines(
                    [(format!("entity {}", entity.index()), String::new(), true)]
                        .into_iter()
                        .chain(self.lists.chosen.iter().flat_map(|(name, value)| {
                            let mut rows = vec![(name.clone(), String::new(), true)];
                            said(value, 1, &mut rows);
                            rows
                        })),
                ),
            },
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

/// Writes a value out as rows, a field to a row, what is inside a field indented under it.
fn said(value: &Value, depth: usize, rows: &mut Vec<(String, String, bool)>) {
    let indent = "  ".repeat(depth);
    let plain = |value: &Value| match value {
        Value::Null => Some("none".to_owned()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Int(value) => Some(value.to_string()),
        Value::Float(value) => Some(format!("{value:.3}")),
        Value::Text(value) => Some(value.clone()),
        Value::Entity(bits) => Some(format!("entity {}", Entity::from_bits(*bits).index())),
        Value::Asset { kind, name } => Some(format!("{kind} {name}")),
        // A short list of numbers reads best on one line: a vector, a colour.
        Value::List(items) if items.len() <= 4 => items
            .iter()
            .map(|item| match item {
                Value::Float(value) => Some(format!("{value:.3}")),
                Value::Int(value) => Some(value.to_string()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join("  ")),
        Value::List(_) | Value::Map(_) => None,
    };
    let fields: Vec<(String, &Value)> = match value {
        Value::Map(fields) => fields
            .iter()
            .map(|(name, field)| (name.clone(), field))
            .collect(),
        Value::List(items) if plain(value).is_none() => items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item))
            .collect(),
        other => {
            rows.push((indent, plain(other).unwrap_or_default(), false));
            return;
        }
    };
    for (name, field) in fields {
        let shown = plain(field);
        rows.push((
            format!("{indent}{name}"),
            shown.clone().unwrap_or_default(),
            false,
        ));
        if shown.is_none() {
            said(field, depth + 1, rows);
        }
    }
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
        let mut changed = false;
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

/// Opens the engine app on a game, and returns when its window is closed.
pub fn run(game: mira::app::App) -> Result<(), Box<dyn std::error::Error>> {
    neo::run(Editor::new(game))?;
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
        assert_eq!(first.shown(), [GAME, ENTITIES, INSPECTOR]);
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
        // What the chosen entity is made of, a field to a row.
        let mut rows = Vec::new();
        said(&lists.chosen[0].1, 1, &mut rows);
        assert_eq!(lists.chosen[0].0, "Transform");
        assert_eq!(
            rows[0],
            (
                "  translation".to_owned(),
                "1.000  2.000  3.000".to_owned(),
                false
            )
        );
        assert!(rows.iter().any(|(name, _, _)| name == "  scale"));
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
        assert_eq!(made_of, ["Transform", "Parent"]);
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
