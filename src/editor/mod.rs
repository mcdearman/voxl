//! The engine app: a game shown and worked on inside a [Neo](https://github.com/mcdearman/neo)
//! window. Bare `cargo run` opens it (`src/main.rs`).
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     let game = my_game::build()?;      // a mira `App`, as it would be run
//!     mira::editor::run(game)?;
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
//! conversation with an agent that is working on the game. How they are arranged is kept in
//! `.mira/editor.layout` in the folder the app is run from. See `docs/EDITOR.md`.

pub mod agent;
pub mod scene;

use std::{
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

use agent::{Agent, Heard, NoAgent};

use glam::{Mat4, Vec3};

use crate::{
    assets::Assets,
    ecs::Entity,
    input::{ButtonInput, KeyCode, Mouse, MouseButton},
    live::Live,
    prelude::Vec2,
    reflect::{Scene, TypeRegistry, Value},
    render::frame_texture,
    render::{Camera, Mesh, Mesh3d},
    signal::{Signal, Signals},
    time::Time,
    transform::GlobalTransform,
    transform::Parent,
};
use neo::prelude::*;
use neo::{wgpu, Color, Graphics, Image, Key, KeyEvent, Point, PointerButton, Rect};
use neo_desktop::{AppPrefs, Desktop, DesktopMsg};

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
    /// What the pointer does in the picture of the game from now on.
    Tool(Tool),
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
    /// A use of a tool in the conversation was opened, or shut.
    Unfolded(String, bool),
    /// A drag on a field began, or ended: what is changed in between is one change.
    Scrub(bool),
    /// The last change is taken back; or the last one taken back is made again.
    Undo,
    Redo,
    /// The scene is written to its file.
    Save,
    /// An entity's name is being typed over in the tree.
    Naming(TreeEdit<Entity>),
    /// The settings every Neo app has: the panel opened or shut, the window's glass turned
    /// on or off, the desktop's appearance looked at again.
    Desktop(DesktopMsg),
}

/// What the pointer does in the picture of the game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Chooses the entity under it, and moves it over the ground when dragged. The game
    /// hears nothing of it.
    #[default]
    Move,
    /// Is the game's: clicks and keys go to it, as when it is played in a window of its own.
    Play,
    /// Is the game's and held in the picture, hidden, for games that turn with the mouse.
    /// Escape lets it go.
    Look,
}

/// An entity being moved over the ground in the picture.
#[derive(Clone, Copy, Debug)]
struct Held {
    entity: Entity,
    /// How high the ground it slides over is: where it was taken hold of.
    height: f32,
    /// From where it was taken hold of to its own place.
    reach: Vec3,
    /// Taken by one of its handles: which way that handle runs, where the entity was, and
    /// how far along the handle the pointer was. It then moves only that way.
    handle: Option<(Vec3, Vec3, f32)>,
}

/// The handles drawn on the chosen entity: where it is in the picture, and where the end
/// of each of the three (east, up, south) is. All counted 0 to 1 across and down.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Handles {
    middle: Vec2,
    ends: [Vec2; 3],
    /// How long a handle is in the world, so that it is the same length in the picture
    /// however far off the entity is.
    length: f32,
}

/// The ways the handles run, and what they are drawn in: red east, green up, blue south.
const WAYS: [(Vec3, [f32; 3]); 3] = [
    (Vec3::X, [0.94, 0.33, 0.31]),
    (Vec3::Y, [0.45, 0.82, 0.35]),
    (Vec3::Z, [0.33, 0.55, 0.96]),
];

/// One thing changed in the game from the app: a component of an entity as it was and as
/// it became. Either may be nothing, for a component that was added or taken away.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    entity: Entity,
    component: String,
    /// The way to the field that was changed, to tell one drag's changes from another's.
    path: Vec<String>,
    before: Option<Value>,
    after: Option<Value>,
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
    /// What the named ones are called.
    names: Vec<(Entity, String)>,
    /// The components of the chosen entity, by their full names, as plain data.
    chosen: Vec<(String, Value)>,
    /// Every signal and its value.
    signals: Vec<(String, Signal)>,
}

impl Lists {
    fn of(game: &crate::app::App, chosen: Option<Entity>) -> Self {
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
            names: world
                .get_resource::<TypeRegistry>()
                .and_then(|registry| registry.get("mira.Name"))
                .map_or(Vec::new(), |kind| {
                    (kind.entities)(world)
                        .into_iter()
                        .filter_map(|entity| match (kind.get)(world, entity) {
                            Some(Value::Text(name)) => Some((entity, name)),
                            _ => None,
                        })
                        .collect()
                }),
            signals,
        }
    }
}

/// The app: a game, and the window's view of it.
pub struct Editor {
    game: crate::app::App,
    graphics: Option<Graphics>,
    hosted: bool,
    /// The viewport's room, in pixels, and how many of them make a point.
    size: (u32, u32),
    scale: f32,
    /// The game's frame, and the picture of it the window draws. Kept while the game keeps
    /// drawing into the same texture.
    shown: Option<(wgpu::Texture, Image)>,
    tool: Tool,
    /// The entity being dragged in the picture, and the part of the picture the chosen one
    /// covers (left, top, right, bottom, each from 0 to 1).
    held: Option<Held>,
    outline: Option<[f32; 4]>,
    handles: Option<Handles>,
    /// What the bar says of the game, as last looked at.
    status: Status,
    layout: Dock,
    lists: Lists,
    /// The entity chosen in the tree, and the ones whose children are hidden.
    chosen: Option<Entity>,
    shut: Vec<Entity>,
    /// The agent, what has been said with it, what is being written to it, and whether it
    /// is at work on something.
    /// What has been changed from the app, newest last, and what has been taken back.
    done: Vec<Change>,
    undone: Vec<Change>,
    /// Whether a drag on a field is under way, and whether the next change belongs with
    /// the last one (the same drag, or the same run of typing).
    scrubbing: bool,
    joins: bool,
    changed_at: Option<Instant>,
    /// Where the scene is kept, and what the bar last had to say about it.
    scene: std::path::PathBuf,
    told: String,
    /// The entity whose name is being typed in the tree, and what has been typed.
    naming: Option<(Entity, String)>,
    /// How the app looks: Neo's appearance as the person has set it for their desktop, and
    /// this app's own say in whether its window is glass.
    desktop: Desktop,
    agent: Box<dyn Agent>,
    said: Vec<Entry<String, Message>>,
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
    pub fn new(game: crate::app::App) -> Self {
        Self {
            game,
            graphics: None,
            hosted: false,
            size: (0, 0),
            scale: 1.0,
            shown: None,
            tool: Tool::default(),
            held: None,
            outline: None,
            handles: None,
            status: Status::default(),
            layout: std::fs::read_to_string(LAYOUT_FILE)
                .ok()
                .and_then(|kept| kept_layout(&kept))
                .unwrap_or_else(first_layout),
            lists: Lists::default(),
            chosen: None,
            shut: Vec::new(),
            done: Vec::new(),
            undone: Vec::new(),
            scrubbing: false,
            joins: false,
            changed_at: None,
            scene: "scene.json".into(),
            told: String::new(),
            naming: None,
            // By the app's name, not the program's, so that an example of it is the same app.
            desktop: Desktop::with_prefs_file(AppPrefs::path_for("mira")),
            agent: Box::new(NoAgent),
            said: Vec::new(),
            writing: Document::new(""),
            working: false,
            heard: channel(),
        }
    }

    /// Says where the scene is kept: what Save writes.
    pub fn with_scene(mut self, scene: impl Into<std::path::PathBuf>) -> Self {
        self.scene = scene.into();
        self
    }

    /// A component of an entity as plain data, if it has it.
    fn component(&self, entity: Entity, component: &str) -> Option<Value> {
        let world = &self.game.world;
        let registry = world.get_resource::<TypeRegistry>()?;
        (registry.get(component)?.get)(world, entity)
    }

    /// Makes a component of an entity what `to` says (nothing takes it away), without
    /// remembering that it was done. Says whether the game took it.
    fn put_component(&mut self, entity: Entity, component: &str, to: Option<&Value>) -> bool {
        let taken = self
            .game
            .world
            .resource_scope(|world, registry: &mut TypeRegistry| {
                let Some(kind) = registry.get(component) else {
                    return false;
                };
                match to {
                    Some(value) => (kind.insert)(world, entity, value).is_ok(),
                    None => {
                        (kind.remove)(world, entity);
                        true
                    }
                }
            });
        self.lists = Lists::of(&self.game, self.chosen);
        taken
    }

    /// Changes a component of an entity, and remembers it so that it can be taken back.
    /// What is changed in one drag, or typed in one run, is remembered as one change.
    fn change(&mut self, entity: Entity, component: &str, path: Vec<String>, to: Option<Value>) {
        let before = self.component(entity, component);
        if before == to || !self.put_component(entity, component, to.as_ref()) {
            return;
        }
        let now = Instant::now();
        let soon = self
            .changed_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(800));
        self.changed_at = Some(now);
        let same = self.done.last().is_some_and(|last| {
            (last.entity, last.component.as_str(), &last.path) == (entity, component, &path)
        });
        // A drag joins up to its end; typing joins while it keeps coming.
        if same && (self.joins || !self.scrubbing && soon && !path.is_empty()) {
            self.done.last_mut().expect("there is a last").after = to;
        } else {
            self.done.push(Change {
                entity,
                component: component.to_owned(),
                path,
                before,
                after: to,
            });
        }
        self.joins = self.scrubbing;
        self.undone.clear();
    }

    /// Whether the settings panel is showing.
    pub fn settings_open(&self) -> bool {
        self.desktop.settings_open
    }

    /// Whether there is a change to take back, and one to make again.
    pub fn can_undo(&self) -> (bool, bool) {
        (!self.done.is_empty(), !self.undone.is_empty())
    }

    /// Gives the app the agent its conversation panel talks to.
    pub fn with_agent(mut self, agent: impl Agent + 'static) -> Self {
        self.agent = Box::new(agent);
        self
    }

    /// What has been said between the person and the agent, a use of a tool as a note of
    /// the tool's name.
    pub fn said(&self) -> Vec<Said> {
        self.said
            .iter()
            .filter_map(|entry| match entry {
                Entry::Said(said) => Some(said.clone()),
                Entry::Tool(tool) => Some(Said::new(Speaker::Note, tool.name.clone())),
                Entry::Ask(_) => None,
            })
            .collect()
    }

    /// The row for one use of a tool.
    fn tool(&mut self, id: &str) -> Option<&mut ToolRow<String>> {
        self.said.iter_mut().rev().find_map(|entry| match entry {
            Entry::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    fn note(&mut self, who: Speaker, text: impl Into<String>) {
        self.said.push(Entry::Said(Said::new(who, text.into())));
    }

    /// Takes in what the agent has said since last looked. Says whether there was anything.
    fn listen(&mut self) -> bool {
        let mut any = false;
        while let Ok(heard) = self.heard.1.try_recv() {
            any = true;
            match heard {
                // Its answer grows where it stands, until something else is said.
                Heard::Text(more) => match self.said.last_mut() {
                    Some(Entry::Said(last)) if last.who == Speaker::Them => last.text += &more,
                    _ => self.note(Speaker::Them, more),
                },
                // A use of a tool is a row of its own, filled in as more is known of it.
                Heard::Tool { id, name } => {
                    self.said.push(Entry::Tool(ToolRow::new(id, name, "")));
                }
                Heard::Given { id, more } => {
                    if let Some(tool) = self.tool(&id) {
                        tool.input += &more;
                        // Shut, the row says what the tool was given, as far as fits.
                        tool.summary = tool.input.chars().take(60).collect();
                    }
                }
                Heard::Back {
                    id,
                    text,
                    failed,
                    picture,
                } => {
                    if let Some(tool) = self.tool(&id) {
                        tool.result = text;
                        tool.state = if failed {
                            ToolState::Failed
                        } else {
                            ToolState::Done
                        };
                        tool.image = picture
                            .map(|(width, height, pixels)| Image::new(width, height, pixels));
                    }
                }
                Heard::Done => self.working = false,
                Heard::Failed(why) => {
                    self.working = false;
                    self.note(Speaker::Note, why);
                }
            }
        }
        any
    }

    /// The game being shown.
    pub fn game(&self) -> &crate::app::App {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut crate::app::App {
        &mut self.game
    }

    /// The entity chosen in the tree, whose parts the inspector shows.
    pub fn chosen(&self) -> Option<Entity> {
        self.chosen
    }

    fn resource<R: 'static>(&mut self) -> Option<&mut R> {
        self.game.world.get_resource_mut::<R>()
    }

    /// The camera the game is seen through, and where it is.
    fn eye(&mut self) -> Option<(Camera, Mat4)> {
        let world = &mut self.game.world;
        let seen = world.query::<(&Camera, &GlobalTransform)>();
        let eye = seen
            .iter()
            .find(|(camera, _)| camera.active)
            .map(|(camera, placed)| (*camera, placed.0));
        eye
    }

    /// How wide the picture is for how tall, and a place in it (in points) counted from 0
    /// to 1 across and down.
    fn place(&self, at: Point) -> (f32, Vec2) {
        let (wide, tall) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let at = Vec2::new(at.x * self.scale / wide, at.y * self.scale / tall);
        (wide / tall, at)
    }

    /// The box an entity fills, in its own space: its mesh's, or a small one for what has
    /// none (a light, a camera).
    fn extent(&self, entity: Entity) -> (Vec3, Vec3) {
        let world = &self.game.world;
        let mesh = world
            .get::<Mesh3d>(entity)
            .zip(world.get_resource::<Assets<Mesh>>())
            .and_then(|(mesh, meshes)| meshes.get(mesh.0));
        let Some(mesh) = mesh.filter(|mesh| !mesh.vertices.is_empty()) else {
            return (Vec3::splat(-0.25), Vec3::splat(0.25));
        };
        mesh.vertices.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(least, most), vertex| {
                let at = Vec3::from(vertex.position);
                (least.min(at), most.max(at))
            },
        )
    }

    /// The entity nearest the eye along a line of sight, and where the line enters it.
    fn under(&mut self, ray: scene::Ray) -> Option<(Entity, Vec3)> {
        let placed: Vec<(Entity, Mat4)> = self
            .game
            .world
            .query::<(Entity, &GlobalTransform)>()
            .iter()
            .map(|(entity, placed)| (entity, placed.0))
            .collect();
        placed
            .into_iter()
            // The eye is not something seen.
            .filter(|(entity, _)| self.game.world.get::<Camera>(*entity).is_none())
            .filter_map(|(entity, placed)| {
                let (least, most) = self.extent(entity);
                Some((entity, scene::enters(ray, placed, least, most)?))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(entity, along)| (entity, ray.from + ray.along * along))
    }

    /// Does in the scene what was done in the picture of it: a press chooses what is under
    /// the pointer and takes hold of it, a drag slides it over the ground, letting go
    /// leaves it there. The whole drag is one change to take back.
    fn work(&mut self, event: ViewportEvent) {
        match event {
            ViewportEvent::Pressed(at, PointerButton::Primary) => {
                let Some((camera, eye)) = self.eye() else {
                    return;
                };
                let (aspect, at) = self.place(at);
                let ray = scene::sight(&camera, eye, at, aspect);
                self.held = None;
                // A handle of the chosen entity comes before whatever is behind it.
                if let Some(held) = self.handle_at(at, ray) {
                    self.held = Some(held);
                    self.update(Message::Scrub(true));
                    return;
                }
                let Some((entity, taken)) = self.under(ray) else {
                    return;
                };
                self.update(Message::Chosen(entity));
                let Some(placed) = self.game.world.get::<GlobalTransform>(entity) else {
                    return;
                };
                let own = placed.0.transform_point3(Vec3::ZERO);
                self.held = Some(Held {
                    entity,
                    height: taken.y,
                    reach: own - taken,
                    handle: None,
                });
                self.update(Message::Scrub(true));
            }
            ViewportEvent::Moved(at) => {
                let (Some(held), Some((camera, eye))) = (self.held, self.eye()) else {
                    return;
                };
                let (aspect, at) = self.place(at);
                let ray = scene::sight(&camera, eye, at, aspect);
                let to = match held.handle {
                    // By a handle: as far that way as the pointer has gone along it.
                    Some((way, from, taken)) => {
                        scene::along(ray, from, way).map(|now| from + way * (now - taken))
                    }
                    None => scene::on_ground(ray, held.height).map(|ground| ground + held.reach),
                };
                let Some(mut to) = to else {
                    return;
                };
                // Where it is to be in the world, and so where under its parent.
                let world = &self.game.world;
                let parent = world
                    .get::<Parent>(held.entity)
                    .and_then(|parent| world.get::<GlobalTransform>(parent.0));
                if let Some(parent) = parent {
                    to = parent.0.inverse().transform_point3(to);
                }
                let Some(mut whole) = self.component(held.entity, "mira.Transform") else {
                    return;
                };
                let translation = Value::List(
                    to.to_array()
                        .into_iter()
                        .map(|part| Value::Float(part as f64))
                        .collect(),
                );
                let path = vec!["translation".to_owned()];
                if put(&mut whole, &path, translation) {
                    self.change(held.entity, "mira.Transform", path, Some(whole));
                }
            }
            ViewportEvent::Released(_, PointerButton::Primary) | ViewportEvent::AllReleased
                if self.held.take().is_some() =>
            {
                self.update(Message::Scrub(false));
            }
            _ => {}
        }
    }

    /// The handles of the chosen entity, as they are in the picture now.
    fn handled(&mut self) -> Option<Handles> {
        let entity = self.chosen?;
        let (camera, eye) = self.eye()?;
        let own = self
            .game
            .world
            .get::<GlobalTransform>(entity)?
            .0
            .transform_point3(Vec3::ZERO);
        let aspect = self.place(Point::new(0.0, 0.0)).0;
        // A seventh of the picture's height long, wherever the entity is.
        let ahead = -eye.inverse().transform_point3(own).z;
        let (widening, half_height) = camera.spread();
        let length = (widening * ahead + half_height) * 2.0 / 7.0;
        let seen = |point| scene::in_picture(&camera, eye, point, aspect);
        Some(Handles {
            middle: seen(own)?,
            ends: [
                seen(own + WAYS[0].0 * length)?,
                seen(own + WAYS[1].0 * length)?,
                seen(own + WAYS[2].0 * length)?,
            ],
            length,
        })
    }

    /// The handle of the chosen entity that a place in the picture is on, taken hold of.
    fn handle_at(&mut self, at: Vec2, ray: scene::Ray) -> Option<Held> {
        let (entity, handles) = (self.chosen?, self.handled()?);
        // In points, so that near means the same across and down.
        let (wide, tall) = (
            self.size.0 as f32 / self.scale,
            self.size.1 as f32 / self.scale,
        );
        let points = |place: Vec2| Vec2::new(place.x * wide, place.y * tall);
        let pointer = points(at);
        let middle = points(handles.middle);
        // How far the pointer is from each handle's line in the picture; the nearest
        // within reach is the one, its outer two thirds only, to leave the middle for
        // taking hold of the entity itself.
        let nearest = (0..3)
            .filter_map(|which| {
                let end = points(handles.ends[which]);
                let line = end - middle;
                let far = ((pointer - middle).dot(line) / line.length_squared().max(1e-6))
                    .clamp(0.33, 1.0);
                let off = (pointer - (middle + line * far)).length();
                (off < 9.0).then_some((which, off))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        let way = WAYS[nearest].0;
        let from = self
            .game
            .world
            .get::<GlobalTransform>(entity)?
            .0
            .transform_point3(Vec3::ZERO);
        Some(Held {
            entity,
            height: from.y,
            reach: Vec3::ZERO,
            handle: Some((way, from, scene::along(ray, from, way)?)),
        })
    }

    /// The part of the picture the chosen entity covers, as it is now.
    fn outlined(&mut self) -> Option<[f32; 4]> {
        let entity = self.chosen?;
        let (camera, eye) = self.eye()?;
        let placed = self.game.world.get::<GlobalTransform>(entity)?.0;
        let (least, most) = self.extent(entity);
        let aspect = self.place(Point::new(0.0, 0.0)).0;
        scene::covers(&camera, eye, aspect, placed, least, most)
    }

    /// Passes on to the game what was done in the viewport.
    fn hear(&mut self, event: ViewportEvent) {
        if self.tool == Tool::Move {
            return self.work(event);
        }
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
            GAME => {
                let picture = viewport(self.shown.as_ref().map(|(_, image)| image))
                    .on_resize(Message::Resized)
                    .on_input(Message::Input)
                    // The game is always being drawn, running or held still.
                    .playing(true)
                    .capture(self.tool == Tool::Look);
                // Over the picture, a frame round the chosen entity while things are being
                // moved; the game's own picture is left alone.
                let frame = (self.tool == Tool::Move).then_some(self.outline).flatten();
                stack()
                    .push(picture)
                    .push(Element::new(Outline(
                        frame,
                        (self.tool == Tool::Move).then_some(self.handles).flatten(),
                    )))
                    .into()
            }
            ENTITIES => scrollable(
                container(
                    tree(&self.entity_tree(None), self.chosen.as_ref())
                        .on_select(Message::Chosen)
                        .on_toggle(Message::Opened)
                        .on_move(Message::Moved)
                        .on_edit(Message::Naming)
                        .editing(
                            self.naming.as_ref().map(|(entity, _)| entity),
                            self.naming.as_ref().map_or("", |(_, typed)| typed),
                        )
                        // A row can also be carried out of the tree, to a field that names
                        // an entity.
                        .draggable(true),
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
                    .push(if self.said.is_empty() {
                        // Before anything is said: what this is for.
                        container(
                            column()
                                .spacing(6.0)
                                .align(Align::Center)
                                .push(icon(icons::BOT).size(26.0).tone(Tone::Faint))
                                .push(
                                    text("An agent that can see this game, read everything in it, and change it.")
                                        .tone(Tone::Muted),
                                )
                                .push(
                                    text("Ask why something is happening, or for a change to be made.")
                                        .size(12.0)
                                        .tone(Tone::Faint),
                                ),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(Align::Center)
                        .align_y(Align::Center)
                    } else {
                        container(conversation(&self.said, Message::Unfolded)).height(Length::Fill)
                    })
                    .push(
                        container(row().spacing(8.0).align(Align::End).push(asking).push(stop))
                            .padding(8.0),
                    )
                    .into()
            }
            SIGNALS => {
                // A lamp for each, lit while it is true; its value at the right.
                let mut rows = column().spacing(2.0).width(Length::Fill);
                for (name, value) in &self.lists.signals {
                    let shown = match value {
                        Signal::Bool(value) => value.to_string(),
                        Signal::Number(value) => format!("{value:.2}"),
                    };
                    let lit = value.is_true();
                    let lamp = if lit { Tone::Good } else { Tone::Faint };
                    let name = text(name.clone()).mono().size(12.5).no_wrap();
                    rows = rows.push(
                        container(
                            row()
                                .spacing(8.0)
                                .align(Align::Center)
                                .push(icon(icons::CIRCLE_DOT).size(11.0).tone(lamp))
                                .push(if lit { name } else { name.tone(Tone::Muted) })
                                .push(Space::fill_x())
                                .push(text(shown).mono().size(12.5).tone(Tone::Muted)),
                        )
                        .padding([4.0, 3.0]),
                    );
                }
                if self.lists.signals.is_empty() {
                    rows = rows.push(text("This game has no signals.").tone(Tone::Muted));
                }
                scrollable(container(rows).padding(8.0)).into()
            }
            _ => text("").into(),
        }
    }

    /// What the chosen entity is made of, each field in a control that changes it.
    fn inspector(&self) -> Element<Message> {
        let Some(entity) = self.chosen else {
            return container(
                column()
                    .spacing(6.0)
                    .align(Align::Center)
                    .push(icon(icons::SLIDERS_HORIZONTAL).size(24.0).tone(Tone::Faint))
                    .push(
                        text("Choose an entity in the tree to see what it is made of.")
                            .tone(Tone::Muted),
                    ),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Align::Center)
            .align_y(Align::Center)
            .into();
        };
        let mut rows = column().spacing(6.0).width(Length::Fill);
        // What it is called, large, and which entity it is, small.
        let named = self.lists.names.iter().find(|(named, _)| *named == entity);
        rows = rows.push(
            row()
                .spacing(8.0)
                .align(Align::Center)
                .push(
                    text(named.map_or("Unnamed", |(_, name)| name.as_str()))
                        .size(16.0)
                        .weight(Weight::SEMIBOLD),
                )
                .push(
                    text(format!("entity {}", entity.index()))
                        .size(12.0)
                        .tone(Tone::Faint),
                ),
        );
        for (component, value) in &self.lists.chosen {
            // Each component under a rule and its name.
            rows = rows.push(container(Divider::horizontal()).padding([6.0, 0.0, 2.0, 0.0]));
            rows = rows.push(
                text(short(component))
                    .size(12.0)
                    .weight(Weight::SEMIBOLD)
                    .tone(Tone::Accent),
            );
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
                    .filter(|name| !matches!(*name, "Transform" | "Parent" | "Name"))
                    .collect();
                let named = self.lists.names.iter().find(|(named, _)| named == entity);
                let label = match named {
                    // By its name if it has one; else by what it is made of.
                    Some((_, name)) => name.clone(),
                    None if what.is_empty() => format!("{}", entity.index()),
                    None => format!("{}  {}", entity.index(), what.join(", ")),
                };
                // A shut entity's children are still built, so it shows that it has some.
                // A picture of what it mostly is.
                let has = |name: &str| components.iter().any(|component| component == name);
                let glyph = if has("Camera") {
                    icons::VIDEO
                } else if has("DirectionalLight") {
                    icons::SUN
                } else if has("Mesh3d") {
                    icons::BOX
                } else {
                    icons::CIRCLE_DOT
                };
                TreeNode::new(*entity, label)
                    .icon(glyph)
                    .with(open, self.entity_tree(Some(*entity)))
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

/// A frame drawn over the picture round what is chosen. It takes up the picture's whole
/// space and nothing that is done there: the picture underneath hears it all.
struct Outline(Option<[f32; 4]>, Option<Handles>);

impl neo::Widget<Message> for Outline {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, _: &mut neo::Cx, limits: neo::Limits) -> neo::Size {
        limits.max
    }

    fn draw(&self, cx: &mut neo::DrawCx) {
        let within = cx.bounds();
        // The handles: a line each way from the middle, with a knob to take hold of.
        if let Some(handles) = self.1 {
            let at = |place: Vec2| {
                Point::new(within.x + place.x * within.w, within.y + place.y * within.h)
            };
            cx.scene.push_clip(within);
            for (end, (_, [r, g, b])) in handles.ends.iter().zip(WAYS) {
                let (from, to, colour) = (at(handles.middle), at(*end), Color::rgb(r, g, b));
                cx.scene.line(from, to, 2.0, colour);
                cx.scene.fill(
                    Rect::new(to.x - 4.5, to.y - 4.5, 9.0, 9.0),
                    4.5,
                    colour,
                    None,
                );
            }
            cx.scene.pop_clip();
        }
        let Some([left, top, right, bottom]) = self.0 else {
            return;
        };
        let frame = Rect::new(
            within.x + left * within.w - 3.0,
            within.y + top * within.h - 3.0,
            (right - left) * within.w + 6.0,
            (bottom - top) * within.h + 6.0,
        );
        cx.scene.push_clip(within);
        let edge = Some((1.5, Color::rgb(1.0, 0.72, 0.2)));
        cx.scene.fill(frame, 4.0, Color::TRANSPARENT, edge);
        cx.scene.pop_clip();
    }

    fn event(&mut self, _: &mut neo::EventCx<Message>, _: &neo::Event) -> neo::Status {
        neo::Status::Ignored
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
            .on_scrub(Message::Scrub)
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
            vector_field_scrubbed(
                &parts,
                0.1,
                move |part, number| {
                    let mut whole = whole.clone();
                    whole[part] = number;
                    edited(Value::List(whole.into_iter().map(Value::Float).collect()))
                },
                Message::Scrub,
            )
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
            let picker = color_field(shown)
                .on_scrub(Message::Scrub)
                .on_change(move |to: Color| {
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
        Value::Text(written) => text_input("", written.clone())
            .on_input(move |written| edited(Value::Text(written)))
            .width(Length::Fill)
            .into(),
        // An entity is named by dropping one on the field from the tree; a click on the
        // field goes to the entity it names.
        Value::Entity(bits) => {
            let named = Entity::from_bits(*bits);
            let label = format!("entity {}", named.index());
            let field = reference_field(icons::BOX, Some(&label), Message::Chosen(named), None);
            drop_area(field, move |dropped: Entity, _| {
                edited(Value::Entity(dropped.to_bits()))
            })
            .into()
        }
        Value::Asset { kind, name } => said(format!("{kind} {name}")),
    };
    rows.push(field_row(&name, depth.saturating_sub(1), control))
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
        Key::F(n @ 1..=12) => crate::input::key_named(&format!("F{n}"))?,
        Key::Character(typed) => {
            let mut letters = typed.chars();
            let (letter, None) = (letters.next()?, letters.next()) else {
                return None;
            };
            match letter.to_ascii_uppercase() {
                letter @ 'A'..='Z' => crate::input::key_named(&format!("Key{letter}"))?,
                digit @ '0'..='9' => crate::input::key_named(&format!("Digit{digit}"))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

impl App for Editor {
    type Message = Message;

    /// Neo's look, as the person has set it for their desktop (light or dark, accent,
    /// corners, how see-through and how blurred), with the window glass unless it was
    /// turned off for this app in its settings. Glass is the app's default whatever the
    /// desktop's own windows are.
    fn theme(&self, system: Scheme) -> Theme {
        let mut theme = self.desktop.theme(system);
        theme.glass.enabled = self.desktop.prefs.glass;
        theme
    }

    fn app_menu(&self) -> Vec<MenuEntry<Message>> {
        self.desktop.app_menu(Message::Desktop)
    }

    fn subscriptions(&self) -> Vec<Subscription<Message>> {
        // Follows the desktop's appearance when it is changed elsewhere.
        vec![Desktop::subscription(Message::Desktop(DesktopMsg::Poll))]
    }

    fn on_key(&self, key: &KeyEvent) -> Option<Message> {
        // Command on a Mac, Control elsewhere; with Shift, Z goes the other way.
        let held = key.modifiers.logo || key.modifiers.ctrl;
        let Key::Character(letter) = &key.key else {
            return None;
        };
        match (key.pressed && held, letter.to_lowercase().as_str()) {
            (true, "z") if key.modifiers.shift => Some(Message::Redo),
            (true, "z") => Some(Message::Undo),
            (true, "y") => Some(Message::Redo),
            (true, "s") => Some(Message::Save),
            _ => None,
        }
    }

    /// Room for a game and the panels round it.
    fn window(&self) -> WindowSettings {
        WindowSettings {
            size: neo::Size::new(1440.0, 900.0),
            min_size: Some(neo::Size::new(960.0, 600.0)),
            app_id: Some("dev.mira.Editor".to_owned()),
            ..WindowSettings::default()
        }
    }

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
        let handles = self.handled();
        changed |= std::mem::replace(&mut self.handles, handles) != handles;
        let outline = self.outlined();
        changed |= std::mem::replace(&mut self.outline, outline) != outline;
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
            Message::Tool(tool) => {
                self.tool = tool;
                self.held = None;
            }
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
                let parent = parent.map(|parent| Value::Entity(parent.to_bits()));
                self.joins = false;
                self.change(dragged, "mira.Parent", Vec::new(), parent);
            }
            Message::Edited(component, path, value) => {
                let Some(entity) = self.chosen else { return };
                // The component as it is now, with the one field changed, put back whole.
                let Some(mut whole) = self.component(entity, &component) else {
                    return;
                };
                if put(&mut whole, &path, value) {
                    self.change(entity, &component, path, Some(whole));
                }
            }
            Message::Desktop(message) => {
                self.desktop.update(message);
            }
            Message::Scrub(began) => {
                self.scrubbing = began;
                self.joins = false;
            }
            Message::Undo => {
                if let Some(change) = self.done.pop() {
                    self.put_component(change.entity, &change.component, change.before.as_ref());
                    self.undone.push(change);
                    self.joins = false;
                }
            }
            Message::Redo => {
                if let Some(change) = self.undone.pop() {
                    self.put_component(change.entity, &change.component, change.after.as_ref());
                    self.done.push(change);
                    self.joins = false;
                }
            }
            Message::Save => {
                let world = &self.game.world;
                let saved = match world.get_resource::<TypeRegistry>() {
                    Some(registry) => Scene::capture(world, registry).save(&self.scene),
                    None => Err(anyhow::anyhow!("the game has nothing registered to save")),
                };
                self.told = match saved {
                    Ok(()) => format!("saved {}", self.scene.display()),
                    Err(why) => format!("not saved: {why}"),
                };
            }
            Message::Naming(edit) => match edit {
                TreeEdit::Begin(entity) => {
                    let named = self.component(entity, "mira.Name");
                    let now = match named {
                        Some(Value::Text(name)) => name,
                        _ => String::new(),
                    };
                    self.naming = Some((entity, now));
                }
                TreeEdit::Typed(typed) => {
                    if let Some((_, name)) = &mut self.naming {
                        *name = typed;
                    }
                }
                TreeEdit::Done => {
                    if let Some((entity, name)) = self.naming.take() {
                        let name = name.trim();
                        // No name is no component, not an empty one.
                        let to = (!name.is_empty()).then(|| Value::Text(name.to_owned()));
                        self.joins = false;
                        self.change(entity, "mira.Name", Vec::new(), to);
                    }
                }
                TreeEdit::Dropped => self.naming = None,
            },
            Message::Writing(action) => {
                self.writing.apply(action);
            }
            Message::Ask => {
                let asked = self.writing.text().trim().to_owned();
                if asked.is_empty() || self.working {
                    return;
                }
                self.writing = Document::new("");
                self.note(Speaker::You, asked.clone());
                self.working = true;
                self.agent.ask(&asked, self.heard.0.clone());
            }
            Message::Unfolded(id, open) => {
                if let Some(tool) = self.tool(&id) {
                    tool.open = open;
                }
            }
            Message::Stop => {
                self.agent.stop();
                if std::mem::take(&mut self.working) {
                    self.note(Speaker::Note, "Stopped.");
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
        // The tool bar is mira's own, not a row of Neo's buttons: small flat icons side by
        // side with nothing between them, a thin line between one group and the next, and
        // the one filled button the one that runs the game.
        let tool = |glyph, press: Option<Message>| {
            icon_button(glyph, 30.0)
                .kind(ButtonKind::Ghost)
                .on_press_maybe(press)
        };
        let gap = || {
            container(Divider::vertical())
                .height(18.0)
                .padding([5.0, 0.0])
        };
        let (undo, redo) = self.can_undo();
        let said = if self.told.is_empty() {
            format!("frame {frame}")
        } else {
            format!("{}  ·  frame {frame}", self.told)
        };
        let bar = row()
            .align(Align::Center)
            .push(
                icon_button(if paused { icons::PLAY } else { icons::PAUSE }, 30.0)
                    .kind(ButtonKind::Accent)
                    .on_press(Message::Pause),
            )
            .push(tool(icons::STEP_FORWARD, paused.then_some(Message::Step)))
            .push(gap())
            .push(tool(icons::UNDO_2, undo.then_some(Message::Undo)))
            .push(tool(icons::REDO_2, redo.then_some(Message::Redo)))
            .push(tool(icons::SAVE, Some(Message::Save)))
            .push(gap())
            // What the pointer does in the picture: move things, play the game, look about.
            .push(
                tool(icons::MOUSE_POINTER_2, Some(Message::Tool(Tool::Move)))
                    .selected(self.tool == Tool::Move),
            )
            .push(
                tool(icons::GAMEPAD_2, Some(Message::Tool(Tool::Play)))
                    .selected(self.tool == Tool::Play),
            )
            .push(
                tool(icons::MOUSE, Some(Message::Tool(Tool::Look)))
                    .selected(self.tool == Tool::Look),
            )
            .push(Space::fill_x())
            .push(
                text(if paused {
                    format!("paused  ·  {said}")
                } else {
                    said
                })
                .size(12.0)
                .tone(Tone::Muted),
            )
            .push(gap())
            .push(tool(
                icons::SETTINGS,
                Some(Message::Desktop(DesktopMsg::OpenSettings)),
            ));
        // No strip of its own: the buttons sit side by side with nothing between them, a
        // little in from the window's edge, and one line marks where the bar ends and the
        // panels begin.
        let bar = column()
            .width(Length::Fill)
            .push(container(bar).padding([10.0, 3.0]).width(Length::Fill))
            .push(Divider::horizontal());
        let panels = dock(
            &self.layout,
            |panel| panel.to_owned(),
            |panel| self.panel(panel),
            Message::Arranged,
        );
        let whole = column()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(bar)
            .push(panels);
        // The settings panel, over everything while it is open.
        self.desktop
            .with_settings(whole, "mira Settings", Message::Desktop, vec![])
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
pub fn run(game: crate::app::App) -> Result<(), Box<dyn std::error::Error>> {
    app(game).run()
}

/// The engine app on a game, with its agent, not yet opened: for saying more about it
/// first (`with_scene`, `with_agent`) and then [`Editor::run`].
pub fn app(mut game: crate::app::App) -> Editor {
    let listening = match game.debugger_address() {
        Some(address) => Some(address),
        None => game.listen_for_debugger("127.0.0.1:0").ok(),
    };
    let editor = Editor::new(game);
    match listening {
        Some(address) => {
            editor.with_agent(agent::ClaudeCode::new(tools_program(), address.to_string()))
        }
        // An agent with no way to the game would only guess: better none.
        None => editor,
    }
}

impl Editor {
    /// Opens the app's window, and returns when it is closed.
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        neo::run(self)?;
        Ok(())
    }
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
        use crate::prelude::*;
        let mut game = crate::app::App::new();
        game.add_plugins(crate::transform::TransformPlugin)
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
        use crate::prelude::*;
        let mut game = crate::app::App::new();
        game.add_plugins(crate::transform::TransformPlugin);
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
        use crate::prelude::*;
        let mut game = crate::app::App::new();
        game.add_plugins(crate::transform::TransformPlugin);
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
            let _ = heard.send(Heard::Text(format!("asked: {asked}. ")));
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
            Heard::Tool {
                id: "t1".into(),
                name: "mira_signals".into(),
            },
            Heard::Given {
                id: "t1".into(),
                more: "{\"only\": \"true\"}".into(),
            },
            Heard::Back {
                id: "t1".into(),
                text: "blue.contesting = true".into(),
                failed: false,
                picture: Some((1, 1, vec![9, 9, 9, 255])),
            },
            Heard::Text("Blue is on a site.".into()),
            Heard::Done,
        ];
        let mut editor =
            Editor::new(crate::app::App::new()).with_agent(Scripted(script, stops.clone()));
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
        let all = editor.said();
        let said: Vec<(Speaker, &str)> = all
            .iter()
            .map(|said| (said.who, said.text.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (Speaker::You, "Why is the clock stopped?"),
                // What comes in pieces is one answer, until something else happens.
                (
                    Speaker::Them,
                    "asked: Why is the clock stopped?. The clock is paused."
                ),
                (Speaker::Note, "mira_signals"),
                (Speaker::Them, "Blue is on a site."),
            ]
        );
        assert!(!editor.working && !editor.listen());
        // The use of the tool is a row: what it was given, what came back, and its picture;
        // it opens and shuts.
        let Some(Entry::Tool(tool)) = editor.said.get(2) else {
            panic!("a row for the tool");
        };
        assert_eq!(
            (tool.summary.as_str(), tool.result.as_str()),
            ("{\"only\": \"true\"}", "blue.contesting = true")
        );
        assert!(tool.state == ToolState::Done && tool.image.is_some() && !tool.open);
        editor.update(Message::Unfolded("t1".into(), true));
        assert!(matches!(&editor.said[2], Entry::Tool(tool) if tool.open));

        // Stopped while at work, it is told so and the conversation says so; when not at
        // work, stopping says nothing.
        write(&mut editor, "And now?");
        editor.update(Message::Stop);
        assert_eq!(stops.get(), 1);
        assert_eq!(
            editor
                .said()
                .last()
                .map(|said| said.text.clone())
                .as_deref(),
            Some("Stopped.")
        );
        let length = editor.said().len();
        editor.update(Message::Stop);
        assert_eq!(editor.said().len(), length);

        // With no agent, asking says how to have one.
        let mut alone = Editor::new(crate::app::App::new());
        write(&mut alone, "Hello?");
        alone.listen();
        assert!(alone.said()[1].text.contains("There is no agent"));
        assert!(!alone.working);
    }

    #[test]
    fn what_is_changed_can_be_taken_back_and_made_again() {
        use crate::prelude::*;
        let mut game = crate::app::App::new();
        game.add_plugins(crate::transform::TransformPlugin);
        let entity = game.world.spawn(Transform::from_xyz(1.0, 0.0, 0.0));
        let other = game.world.spawn(Transform::IDENTITY);
        let mut editor = Editor::new(game);
        editor.update(Message::Chosen(entity));
        let x = |editor: &Editor| {
            editor
                .game()
                .world
                .get::<Transform>(entity)
                .unwrap()
                .translation
                .x
        };
        let slide = |editor: &mut Editor, to: f64| {
            let whole = Value::List(vec![Value::Float(to), Value::Float(0.0), Value::Float(0.0)]);
            editor.update(Message::Edited(
                "mira.Transform".into(),
                vec!["translation".into()],
                whole,
            ));
        };
        assert_eq!(editor.can_undo(), (false, false));

        // A drag is many changes and one thing to take back.
        editor.update(Message::Scrub(true));
        for to in [2.0, 3.0, 4.0] {
            slide(&mut editor, to);
        }
        editor.update(Message::Scrub(false));
        // A second drag of the same field is a second thing.
        editor.update(Message::Scrub(true));
        slide(&mut editor, 9.0);
        editor.update(Message::Scrub(false));
        assert_eq!((x(&editor), editor.done.len()), (9.0, 2));
        editor.update(Message::Undo);
        assert_eq!(x(&editor), 4.0);
        editor.update(Message::Undo);
        assert_eq!((x(&editor), editor.can_undo()), (1.0, (false, true)));
        editor.update(Message::Undo);
        assert_eq!(x(&editor), 1.0, "nothing more to take back");
        editor.update(Message::Redo);
        editor.update(Message::Redo);
        assert_eq!((x(&editor), editor.can_undo()), (9.0, (true, false)));
        // A new change after taking one back leaves nothing to make again.
        editor.update(Message::Undo);
        slide(&mut editor, 5.0);
        assert_eq!(editor.can_undo(), (true, false));
        // Changing a thing to what it is already is no change.
        let before = editor.done.len();
        slide(&mut editor, 5.0);
        assert_eq!(editor.done.len(), before);

        // A parent given and a name given are taken back as what they were: nothing.
        editor.update(Message::Moved(entity, other, Place::Into));
        assert_eq!(
            editor.game().world.get::<Parent>(entity),
            Some(&Parent(other))
        );
        editor.update(Message::Naming(TreeEdit::Begin(entity)));
        editor.update(Message::Naming(TreeEdit::Typed(" Tank ".into())));
        editor.update(Message::Naming(TreeEdit::Done));
        assert_eq!(
            editor.game().world.get::<Name>(entity),
            Some(&Name::new("Tank"))
        );
        assert_eq!(editor.entity_tree(Some(other))[0].label, "Tank");
        editor.update(Message::Undo);
        assert_eq!(editor.game().world.get::<Name>(entity), None);
        editor.update(Message::Undo);
        assert_eq!(editor.game().world.get::<Parent>(entity), None);
        editor.update(Message::Redo);
        assert_eq!(
            editor.game().world.get::<Parent>(entity),
            Some(&Parent(other))
        );
        // A name being typed and dropped changes nothing; an empty one takes the name away.
        editor.update(Message::Naming(TreeEdit::Begin(entity)));
        editor.update(Message::Naming(TreeEdit::Typed("Lorry".into())));
        editor.update(Message::Naming(TreeEdit::Dropped));
        assert_eq!(editor.game().world.get::<Name>(entity), None);

        // The keys for these.
        let key = |letter: &str, shift: bool| KeyEvent {
            key: Key::Character(letter.into()),
            pressed: true,
            repeat: false,
            modifiers: neo::Modifiers {
                logo: true,
                shift,
                ..Default::default()
            },
            text: None,
        };
        assert!(matches!(
            editor.on_key(&key("z", false)),
            Some(Message::Undo)
        ));
        assert!(matches!(
            editor.on_key(&key("Z", true)),
            Some(Message::Redo)
        ));
        assert!(matches!(
            editor.on_key(&key("s", false)),
            Some(Message::Save)
        ));
        assert!(editor.on_key(&key("q", false)).is_none());
    }

    #[test]
    fn the_scene_is_saved_and_can_be_spawned_again() {
        use crate::prelude::*;
        let build = || {
            let mut game = crate::app::App::new();
            game.add_plugins(crate::transform::TransformPlugin);
            game
        };
        let mut game = build();
        let tank = game
            .world
            .spawn((Transform::from_xyz(3.0, 0.0, 0.0), Name::new("Tank")));
        game.world
            .spawn((Transform::IDENTITY, Name::new("Turret"), Parent(tank)));
        let file = std::env::temp_dir().join(format!("mira-scene-{}.json", std::process::id()));
        let mut editor = Editor::new(game).with_scene(&file);
        editor.update(Message::Save);
        assert!(editor.told.starts_with("saved "), "{}", editor.told);

        let mut again = build();
        let scene = Scene::load(&file).expect("the saved scene reads");
        again
            .world
            .resource_scope(|world, registry: &mut TypeRegistry| {
                scene.spawn(world, registry);
            });
        let lists = Lists::of(&again, None);
        let mut names: Vec<&str> = lists.names.iter().map(|(_, name)| name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Tank", "Turret"]);
        // The turret is still the tank's, whatever their ids are now.
        let parents: Vec<Option<Entity>> = lists
            .entities
            .iter()
            .map(|(_, _, parent)| *parent)
            .collect();
        assert_eq!(parents.iter().filter(|parent| parent.is_some()).count(), 1);
        let _ = std::fs::remove_file(&file);

        // Somewhere that can't be written says so.
        let mut nowhere = Editor::new(build()).with_scene("/no/such/folder/scene.json");
        nowhere.update(Message::Save);
        assert!(nowhere.told.starts_with("not saved"), "{}", nowhere.told);
    }

    #[test]
    fn the_window_is_glass_unless_turned_off_for_the_app() {
        let mut editor = Editor::new(crate::app::App::new());
        // Whatever the desktop's own windows are, and whatever was last chosen here.
        editor.desktop.appearance.glass.enabled = false;
        editor.desktop.prefs.glass = true;
        let theme = editor.theme(Scheme::Dark);
        assert!(theme.glass.enabled, "glass by default");
        assert_eq!(
            theme.scheme,
            editor.desktop.appearance.theme(Scheme::Dark).scheme
        );
        editor.desktop.prefs.glass = false;
        assert!(!editor.theme(Scheme::Dark).glass.enabled);
        // The settings panel opens from the bar and from the app's menu, and shuts.
        assert!(!editor.settings_open() && editor.app_menu().len() == 1);
        editor.update(Message::Desktop(DesktopMsg::OpenSettings));
        assert!(editor.settings_open());
        editor.update(Message::Desktop(DesktopMsg::CloseSettings));
        assert!(!editor.settings_open());
    }

    #[test]
    fn an_entity_is_chosen_and_moved_in_the_picture() {
        use crate::prelude::*;
        let mut game = crate::app::App::new();
        game.add_plugins(crate::transform::TransformPlugin);
        // Looking straight down from ten metres up, north at the top of the picture.
        let eye = Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z);
        game.world.spawn((eye, Camera::default()));
        let near = game.world.spawn(Transform::from_xyz(2.0, 3.0, 0.0));
        let far = game.world.spawn(Transform::from_xyz(2.0, 0.0, 0.0));
        let carrier = game.world.spawn(Transform::from_xyz(-3.0, 0.0, 1.0));
        let carried = game
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, -2.0), Parent(carrier)));
        game.update();
        let mut editor = Editor::new(game);
        editor.update(Message::Resized(Rect::new(0.0, 0.0, 800.0, 600.0), 1.0));
        assert_eq!(editor.tool, Tool::Move);

        // Where a point of the world is in the picture, in points.
        let shown = |editor: &mut Editor, point: Vec3| {
            let (camera, eye) = editor.eye().expect("a camera");
            let at = scene::in_picture(&camera, eye, point, 800.0 / 600.0).expect("in view");
            Point::new(at.x * 800.0, at.y * 600.0)
        };
        let place = |editor: &Editor, entity: Entity| {
            editor
                .game()
                .world
                .get::<Transform>(entity)
                .unwrap()
                .translation
        };
        let press = |at| Message::Input(ViewportEvent::Pressed(at, PointerButton::Primary));
        let release = |at| Message::Input(ViewportEvent::Released(at, PointerButton::Primary));

        // Two things in a line from the eye: the nearer is the one chosen.
        // (Taken hold of by the middle of its top, so that where it goes is easy to say.)
        let at = shown(&mut editor, Vec3::new(2.0, 3.25, 0.0));
        editor.update(press(at));
        assert_eq!(editor.chosen(), Some(near));
        // Dragged, it slides over level ground at the height it was taken hold of, to
        // under the pointer, and keeps its height.
        let to = shown(&mut editor, Vec3::new(-1.0, 3.25, 2.0));
        editor.update(Message::Input(ViewportEvent::Moved(to)));
        editor.update(Message::Input(ViewportEvent::Moved(to)));
        editor.update(release(to));
        let moved = place(&editor, near);
        assert!(
            (moved - Vec3::new(-1.0, 3.0, 2.0)).length() < 0.02,
            "{moved}"
        );
        assert_eq!(
            place(&editor, far),
            Vec3::new(2.0, 0.0, 0.0),
            "the one behind stays"
        );
        // The whole drag is one change.
        assert_eq!(editor.done.len(), 1);
        editor.update(Message::Undo);
        assert_eq!(place(&editor, near), Vec3::new(2.0, 3.0, 0.0));

        // A child is moved to where the pointer is in the world, whatever its parent is.
        editor.game_mut().update();
        let at = shown(&mut editor, Vec3::new(-3.0, 0.25, -1.0));
        editor.update(press(at));
        assert_eq!(editor.chosen(), Some(carried));
        let to = shown(&mut editor, Vec3::new(1.0, 0.25, 1.0));
        editor.update(Message::Input(ViewportEvent::Moved(to)));
        editor.update(release(to));
        let local = place(&editor, carried);
        assert!(
            (local - Vec3::new(4.0, 0.0, 0.0)).length() < 0.02,
            "{local}"
        );

        // The chosen entity has handles: east, up and south. Taken by the one that points
        // up, it moves up and down and no other way.
        editor.game_mut().update();
        let own = Vec3::new(1.0, 0.0, 1.0);
        let handles = editor.handled().expect("handles on the chosen one");
        assert!(handles.length > 0.5 && handles.ends[0].x > handles.middle.x);
        let up = Point::new(handles.ends[2].x * 800.0, handles.ends[2].y * 600.0);
        editor.update(press(up));
        assert!(
            matches!(editor.held, Some(Held { handle: Some((way, ..)), .. }) if way == Vec3::Z)
        );
        // (Seen from above, south is the one that can be read; up points at the eye.)
        let further = shown(&mut editor, own + Vec3::Z * (handles.length + 1.5));
        editor.update(Message::Input(ViewportEvent::Moved(further)));
        editor.update(release(further));
        let local = place(&editor, carried);
        assert!(
            (local - Vec3::new(4.0, 0.0, 1.5)).length() < 0.05,
            "{local}"
        );
        editor.game_mut().update();

        // A press on nothing takes hold of nothing; the chosen entity is framed in the
        // picture; with another tool, the picture is the game's and nothing is moved.
        editor.update(press(Point::new(790.0, 10.0)));
        assert!(editor.held.is_none());
        editor.game_mut().update();
        let [left, top, right, bottom] = editor.outlined().expect("the chosen one is in view");
        assert!(left < right && top < bottom && right - left < 0.3);
        editor.update(Message::Tool(Tool::Play));
        let before = place(&editor, carried);
        let at = shown(&mut editor, Vec3::new(1.0, 0.25, 1.0));
        editor.update(press(at));
        editor.update(Message::Input(ViewportEvent::Moved(Point::new(10.0, 10.0))));
        assert_eq!(place(&editor, carried), before);
    }

    #[test]
    fn what_is_done_in_the_viewport_reaches_the_game() {
        let mut game = crate::app::App::new();
        game.add_plugins(crate::input::InputPlugin);
        let mut editor = Editor::new(game);
        editor.update(Message::Resized(Rect::new(0.0, 40.0, 400.0, 300.0), 2.0));
        // With the tool that makes the picture the game's.
        editor.update(Message::Tool(Tool::Play));
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
