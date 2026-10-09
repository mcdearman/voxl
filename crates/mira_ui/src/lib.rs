//! In-game user interface for mira, drawn with [Armature](https://github.com/mcdearman/armature).
//!
//! Armature lays out and draws a tree of widgets and has no look of its own; this crate is
//! the thin layer that lets one live inside a mira game: the game's window events are handed
//! to it, it is drawn over the finished frame, and what it acts on (a click on a panel) is
//! kept from the game.
//!
//! ```ignore
//! app.add_plugins(UiPlugin::new(Hud::default).sync(|world, hud| {
//!     hud.health = world.resource::<Player>().health;   // game state into the interface
//! }));
//! ```
//!
//! The interface is an Armature [`App`]: a value holding what it shows, a `view` that turns it
//! into widgets, and an `update` for the messages its widgets send. Leave `App::frame` alone
//! (it draws a window's background), or the whole screen counts as interface.

use std::{sync::Arc, time::Instant};

pub use armature;
use armature::{
    App, Event, Fonts, Key, KeyEvent, Modifiers, Point, PointerButton, Scheme, Size, Status, Ui,
};
use armature_render::{Renderer, SurfaceTarget};
use mira::{
    app::{App as Game, Plugin, Stage},
    ecs::World,
    input::{ButtonInput, KeyCode, MouseButton},
    render::{Gpu, Overlays},
    window::{Window, WindowEvents},
};
use winit::{
    event::{ElementState, MouseScrollDelta, WindowEvent},
    keyboard::{Key as WinitKey, NamedKey, PhysicalKey},
};

pub mod kit;
pub mod signal_graph;

/// Brings game state into the interface each frame, before it is drawn.
pub type Sync<A> = fn(&World, &mut A);

/// Adds an Armature interface to a game.
pub struct UiPlugin<A: App> {
    make: fn() -> A,
    sync: Option<Sync<A>>,
}

impl<A: App + 'static> UiPlugin<A> {
    /// An interface made by `make` once the game has a window to draw in.
    pub fn new(make: fn() -> A) -> Self {
        Self { make, sync: None }
    }

    /// Calls `sync` every frame with the world and the interface, to copy in what it shows.
    pub fn sync(mut self, sync: Sync<A>) -> Self {
        self.sync = Some(sync);
        self
    }
}

impl<A: App + 'static> Plugin for UiPlugin<A> {
    fn build(&self, app: &mut Game) {
        app.insert_resource(Setup::<A> {
            make: self.make,
            sync: self.sync,
        });
        app.add_systems(Stage::PreUpdate, hear::<A>);
        app.world.init_resource::<Overlays>();
        app.world.resource_mut::<Overlays>().0.push(draw::<A>);
    }
}

struct Setup<A: App> {
    make: fn() -> A,
    sync: Option<Sync<A>>,
}

/// A running interface. It is made on the first frame drawn, lives in the world, and is only
/// ever touched from the main thread (Armature's `Ui` can't leave the thread it was made on).
pub struct UiHost<A: App> {
    pub ui: Ui<A>,
    renderer: Renderer,
    modifiers: Modifiers,
    pointer: Point,
    size: Size,
}

impl<A: App> UiHost<A> {
    /// The interface's own state.
    pub fn app(&mut self) -> &mut A {
        self.ui.app_mut()
    }
}

fn scale_of(world: &World) -> f32 {
    world
        .get_resource::<Window>()
        .map_or(1.0, |window| window.handle().scale_factor() as f32)
}

fn button(button: winit::event::MouseButton) -> (PointerButton, Option<MouseButton>) {
    match button {
        winit::event::MouseButton::Left => (PointerButton::Primary, Some(MouseButton::Left)),
        winit::event::MouseButton::Right => (PointerButton::Secondary, Some(MouseButton::Right)),
        winit::event::MouseButton::Middle => (PointerButton::Middle, Some(MouseButton::Middle)),
        other => (PointerButton::Other(0), Some(other)),
    }
}

fn key(key: &WinitKey) -> Key {
    match key {
        WinitKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Space => Key::Space,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => Key::Other,
        },
        WinitKey::Character(text) => Key::Character(text.to_string()),
        _ => Key::Other,
    }
}

/// Hands the frame's window events to the interface, and takes from the game the presses
/// the interface acted on.
fn hear<A: App + 'static>(world: &mut World) {
    if !world.contains_resource::<UiHost<A>>() {
        return;
    }
    let events = world
        .get_resource::<WindowEvents>()
        .map_or(Vec::new(), |events| events.0.clone());
    let scale = scale_of(world);
    let mut taken_keys: Vec<KeyCode> = Vec::new();
    let mut taken_buttons: Vec<MouseButton> = Vec::new();
    {
        let host = world.resource_mut::<UiHost<A>>();
        for event in events {
            let mut code = None;
            let mut pressed_button = None;
            let translated = match event {
                WindowEvent::CursorMoved { position, .. } => {
                    host.pointer = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                    Some(Event::PointerMoved { pos: host.pointer })
                }
                WindowEvent::CursorLeft { .. } => Some(Event::PointerLeft),
                WindowEvent::MouseInput {
                    state,
                    button: which,
                    ..
                } => {
                    let (pointer, game) = button(which);
                    pressed_button = game;
                    Some(match state {
                        ElementState::Pressed => Event::PointerPressed {
                            pos: host.pointer,
                            button: pointer,
                        },
                        ElementState::Released => Event::PointerReleased {
                            pos: host.pointer,
                            button: pointer,
                        },
                    })
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let delta = match delta {
                        MouseScrollDelta::LineDelta(x, y) => Point::new(x * 20.0, y * 20.0),
                        MouseScrollDelta::PixelDelta(p) => {
                            Point::new(p.x as f32 / scale, p.y as f32 / scale)
                        }
                    };
                    Some(Event::Wheel {
                        pos: host.pointer,
                        delta,
                    })
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    let state = modifiers.state();
                    host.modifiers = Modifiers {
                        shift: state.shift_key(),
                        ctrl: state.control_key(),
                        alt: state.alt_key(),
                        logo: state.super_key(),
                    };
                    host.ui.set_modifiers(host.modifiers);
                    None
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(physical) = event.physical_key {
                        code = Some(physical);
                    }
                    Some(Event::Key(KeyEvent {
                        key: key(&event.logical_key),
                        pressed: event.state == ElementState::Pressed,
                        repeat: event.repeat,
                        modifiers: host.modifiers,
                        text: event.text.as_ref().map(|text| text.to_string()),
                    }))
                }
                WindowEvent::Focused(focused) => Some(Event::WindowFocus(focused)),
                _ => None,
            };
            let Some(translated) = translated else {
                continue;
            };
            if host.ui.event(host.renderer.text(), translated) == Status::Captured {
                taken_keys.extend(code);
                taken_buttons.extend(pressed_button);
            }
        }
    }
    // What the interface took is not the game's to act on as well.
    if let Some(keys) = world.get_resource_mut::<ButtonInput<KeyCode>>() {
        taken_keys.into_iter().for_each(|key| keys.consume(key));
    }
    if let Some(buttons) = world.get_resource_mut::<ButtonInput<MouseButton>>() {
        taken_buttons
            .into_iter()
            .for_each(|button| buttons.consume(button));
    }
}

/// Draws the interface over the finished frame.
fn draw<A: App + 'static>(world: &mut World, target: &wgpu::TextureView) {
    let (make, sync) = {
        let setup = world.resource::<Setup<A>>();
        (setup.make, setup.sync)
    };
    let scale = scale_of(world);
    let (device, queue, format, width, height) = {
        let gpu = world.resource::<Gpu>();
        (
            gpu.device.clone(),
            gpu.queue.clone(),
            gpu.config.format,
            gpu.config.width,
            gpu.config.height,
        )
    };
    let size = Size::new(width as f32 / scale, height as f32 / scale);
    if !world.contains_resource::<UiHost<A>>() {
        let app = make();
        let renderer = Renderer::new(device, queue, app.fonts());
        let mut ui = Ui::new(app, size, Scheme::Dark);
        // The game draws every frame anyway, so there is nobody to wake.
        ui.start(Arc::new(|| {}));
        world.insert_resource(UiHost {
            ui,
            renderer,
            modifiers: Modifiers::default(),
            pointer: Point::new(-1.0, -1.0),
            size,
        });
        // Armature's interface holds reference-counted values, so it stays where it was made.
        world.pin_to_main_thread::<UiHost<A>>();
    }
    // Taken out of the world while it is drawn, so that `sync` can read the world.
    let mut host = world
        .remove_resource::<UiHost<A>>()
        .expect("made just above");
    if host.size != size {
        host.size = size;
        host.ui.resize(size);
    }
    if let Some(sync) = sync {
        sync(world, host.ui.app_mut());
        host.ui.refresh(host.renderer.text());
    }
    let now = Instant::now();
    host.ui.tick(now);
    let scene = host.ui.draw(host.renderer.text(), now);
    let surface = SurfaceTarget {
        format,
        unpremultiply: false,
    };
    host.renderer
        .render_over(&scene, target, surface, width, height, scale);
    world.insert_resource(host);
}

/// The default fonts: the system's.
pub fn system_fonts() -> Fonts {
    Fonts::system()
}
