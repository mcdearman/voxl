use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use glam::{UVec2, Vec2};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, DeviceEvents, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, WindowId},
};

use crate::{
    app::{App, Plugin},
    ecs::Events,
    input::{ButtonInput, Mouse, MouseButton},
};

#[derive(Clone, Debug)]
pub struct WindowSettings {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub vsync: bool,
    /// Whether the window is shown. A hidden window still renders, off screen, so a game can
    /// be run and looked at (in screenshots) without anything appearing: for tests, tools
    /// and agents. `MIRA_HIDDEN=1` hides it whatever this says.
    pub visible: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: "mira".into(),
            width: 1280,
            height: 720,
            vsync: true,
            visible: true,
        }
    }
}

/// Everything the window was told since the last frame, as the windowing library gave it:
/// for code that needs more than the input resources keep, such as a user interface that
/// wants typed text, the pointer's every move, and the order things happened in. Cleared at
/// the end of each frame. A resource.
#[derive(Default)]
pub struct WindowEvents(pub Vec<WindowEvent>);

fn clear_window_events(mut events: crate::ecs::ResMut<WindowEvents>) {
    events.0.clear();
}

/// The primary window. Inserted as a resource once the window has been created.
pub struct Window {
    handle: Arc<winit::window::Window>,
    cursor_grabbed: AtomicBool,
}

impl Window {
    pub fn handle(&self) -> &Arc<winit::window::Window> {
        &self.handle
    }

    pub fn size(&self) -> UVec2 {
        let size = self.handle.inner_size();
        UVec2::new(size.width, size.height)
    }

    pub fn set_title(&self, title: &str) {
        self.handle.set_title(title);
    }

    pub fn cursor_grabbed(&self) -> bool {
        self.cursor_grabbed.load(Ordering::Relaxed)
    }

    /// Locks and hides the cursor (or releases it). Falls back between grab modes because
    /// platforms support different ones.
    pub fn set_cursor_grabbed(&self, grabbed: bool) {
        let result = if grabbed {
            self.handle
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.handle.set_cursor_grab(CursorGrabMode::Confined))
        } else {
            self.handle.set_cursor_grab(CursorGrabMode::None)
        };
        match result {
            Ok(()) => {
                self.handle.set_cursor_visible(!grabbed);
                self.cursor_grabbed.store(grabbed, Ordering::Relaxed);
            }
            Err(err) => log::warn!("failed to change cursor grab: {err}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WindowResized {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct WindowFocused(pub bool);

pub struct WindowPlugin;

impl Plugin for WindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowEvents>()
            .add_systems(crate::app::Stage::Last, clear_window_events);
        app.init_resource::<WindowSettings>()
            .add_event::<WindowResized>()
            .add_event::<WindowFocused>();
    }
}

struct Runner {
    app: App,
    window: Option<Arc<winit::window::Window>>,
    /// The window is hidden, so nothing asks for it to be redrawn: frames are run from the
    /// loop itself, at about sixty a second.
    hidden: bool,
    last_frame: Option<std::time::Instant>,
}

pub(crate) fn run(app: App) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut runner = Runner {
        app,
        window: None,
        hidden: false,
        last_frame: None,
    };
    event_loop.run_app(&mut runner)?;
    Ok(())
}

impl Runner {
    fn send<E: 'static>(&mut self, event: E) {
        if let Some(events) = self.app.world.get_resource_mut::<Events<E>>() {
            events.send(event);
        }
    }

    fn resource<R: 'static>(&mut self) -> Option<&mut R> {
        self.app.world.get_resource_mut::<R>()
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let settings = self
            .app
            .world
            .get_resource::<WindowSettings>()
            .cloned()
            .unwrap_or_default();
        let hidden =
            !settings.visible || std::env::var("MIRA_HIDDEN").is_ok_and(|hidden| hidden != "0");
        let attributes = winit::window::Window::default_attributes()
            .with_title(&settings.title)
            .with_inner_size(LogicalSize::new(settings.width, settings.height))
            .with_visible(!hidden);
        self.hidden = hidden;
        if hidden {
            log::info!("the window is hidden; rendering off screen");
        }
        let handle = match event_loop.create_window(attributes) {
            Ok(handle) => Arc::new(handle),
            Err(err) => {
                log::error!("failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };
        event_loop.listen_device_events(DeviceEvents::WhenFocused);
        self.app.world.insert_resource(Window {
            handle: handle.clone(),
            cursor_grabbed: AtomicBool::new(false),
        });
        // The operating system wants its windows handled from the thread that made them.
        self.app.world.pin_to_main_thread::<Window>();
        self.window = Some(handle);
        self.app.startup();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(events) = self.resource::<WindowEvents>() {
            events.0.push(event.clone());
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.send(WindowResized {
                width: size.width,
                height: size.height,
            }),
            WindowEvent::Focused(focused) => {
                if !focused {
                    // Key-up events are lost while unfocused, so don't leave keys stuck down.
                    if let Some(keys) = self.resource::<ButtonInput<KeyCode>>() {
                        keys.release_all();
                    }
                    if let Some(buttons) = self.resource::<ButtonInput<MouseButton>>() {
                        buttons.release_all();
                    }
                }
                self.send(WindowFocused(focused));
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        ..
                    },
                ..
            } => {
                if let Some(keys) = self.resource::<ButtonInput<KeyCode>>() {
                    match state {
                        ElementState::Pressed => keys.press(code),
                        ElementState::Released => keys.release(code),
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(buttons) = self.resource::<ButtonInput<MouseButton>>() {
                    match state {
                        ElementState::Pressed => buttons.press(button),
                        ElementState::Released => buttons.release(button),
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.position = Some(Vec2::new(position.x as f32, position.y as f32));
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.position = None;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(mouse) = self.resource::<Mouse>() {
                    mouse.scroll += match delta {
                        MouseScrollDelta::LineDelta(x, y) => Vec2::new(x, y),
                        MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32) / 40.0,
                    };
                }
            }
            WindowEvent::RedrawRequested => {
                self.app.update();
                if self.app.should_exit() {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if let Some(mouse) = self.resource::<Mouse>() {
                mouse.delta += Vec2::new(dx as f32, dy as f32);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = &self.window else {
            return;
        };
        if !self.hidden {
            window.request_redraw();
            return;
        }
        // Nobody redraws a window that isn't shown, so the frame is run here, paced by
        // sleeping since there is no display to wait for.
        let frame = std::time::Duration::from_micros(16_667);
        if let Some(spent) = self.last_frame.map(|last| last.elapsed()) {
            if spent < frame {
                std::thread::sleep(frame - spent);
            }
        }
        self.last_frame = Some(std::time::Instant::now());
        self.app.update();
        if self.app.should_exit() {
            event_loop.exit();
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }
}
