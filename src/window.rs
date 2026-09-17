use std::{cell::Cell, sync::Arc};

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
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: "voxl".into(),
            width: 1280,
            height: 720,
            vsync: true,
        }
    }
}

/// The primary window. Inserted as a resource once the window has been created.
pub struct Window {
    handle: Arc<winit::window::Window>,
    cursor_grabbed: Cell<bool>,
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
        self.cursor_grabbed.get()
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
                self.cursor_grabbed.set(grabbed);
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
        app.init_resource::<WindowSettings>()
            .add_event::<WindowResized>()
            .add_event::<WindowFocused>();
    }
}

struct Runner {
    app: App,
    window: Option<Arc<winit::window::Window>>,
}

pub(crate) fn run(app: App) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut runner = Runner { app, window: None };
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
        let attributes = winit::window::Window::default_attributes()
            .with_title(&settings.title)
            .with_inner_size(LogicalSize::new(settings.width, settings.height));
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
            cursor_grabbed: Cell::new(false),
        });
        self.window = Some(handle);
        self.app.startup();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
