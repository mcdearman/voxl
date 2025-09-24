mod ecs;
mod gfx;

use std::sync::Arc;

use crate::gfx::render::renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    event::{self, *},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

pub struct App {
    #[cfg(target_arch = "wasm32")]
    proxy: Option<winit::event_loop::EventLoopProxy<State>>,
    renderer: Option<Renderer>,
    last_render_time: instant::Instant,
}

impl App {
    pub fn new(#[cfg(target_arch = "wasm32")] event_loop: &EventLoop<State>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let proxy = Some(event_loop.create_proxy());
        Self {
            renderer: None,
            last_render_time: instant::Instant::now(),
            #[cfg(target_arch = "wasm32")]
            proxy,
        }
    }
}

impl ApplicationHandler<Renderer> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        #[allow(unused_mut)]
        let mut window_attributes = Window::default_attributes();

        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;

            const CANVAS_ID: &str = "canvas";

            let window = wgpu::web_sys::window().unwrap_throw();
            let document = window.document().unwrap_throw();
            let canvas = document.get_element_by_id(CANVAS_ID).unwrap_throw();
            let html_canvas_element = canvas.unchecked_into();
            window_attributes = window_attributes.with_canvas(Some(html_canvas_element));
        }

        let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
        window
            .set_cursor_grab(winit::window::CursorGrabMode::Confined)
            //     // .or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Locked))
            .expect("failed to set cursor grab mode");
        window.set_cursor_visible(false);

        #[cfg(not(target_arch = "wasm32"))]
        {
            // If we are not on web we can use pollster to
            // await the
            self.renderer = Some(pollster::block_on(Renderer::new(window)));
        }

        #[cfg(target_arch = "wasm32")]
        {
            // Run the future asynchronously and use the
            // proxy to send the results to the event loop
            if let Some(proxy) = self.proxy.take() {
                wasm_bindgen_futures::spawn_local(async move {
                    assert!(proxy
                        .send_event(
                            State::new(window)
                                .await
                                .expect("Unable to create canvas!!!")
                        )
                        .is_ok())
                });
            }
        }
        event_loop.listen_device_events(winit::event_loop::DeviceEvents::WhenFocused);
    }

    #[allow(unused_mut)]
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, mut event: Renderer) {
        // This is where proxy.send_event() ends up
        #[cfg(target_arch = "wasm32")]
        {
            event.window.request_redraw();
            event.resize(
                event.window.inner_size().width,
                event.window.inner_size().height,
            );
        }
        self.renderer = Some(event);
    }

    fn device_event(
        &mut self,
        _el: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let state = match &mut self.renderer {
            Some(canvas) => canvas,
            None => return,
        };

        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            state.camera_controller.process_mouse(dx, dy);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let state = match &mut self.renderer {
            Some(canvas) => canvas,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                let now = instant::Instant::now();
                let dt = now - self.last_render_time;
                self.last_render_time = now;
                state.update(dt);
                match state.render() {
                    Ok(_) => {}
                    // Reconfigure the surface if it's lost or outdated
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        state.resize(state.size.width, state.size.height)
                    }
                    // The system is out of memory, we should probably quit
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    // We're ignoring timeouts
                    Err(wgpu::SurfaceError::Timeout) => log::warn!("Surface timeout"),
                    Err(other) => log::warn!("Surface error: {:?}", other),
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        ..
                    },
                ..
            } => match (code, key_state.is_pressed()) {
                (KeyCode::Escape, true) => event_loop.exit(),
                _ => {}
            },
            _ => {}
        }
    }
}

pub fn run() -> anyhow::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        env_logger::init();
    }
    #[cfg(target_arch = "wasm32")]
    {
        console_log::init_with_level(log::Level::Info).unwrap_throw();
    }

    let event_loop = EventLoop::with_user_event().build()?;
    let mut app = App::new(
        #[cfg(target_arch = "wasm32")]
        &event_loop,
    );
    event_loop.run_app(&mut app)?;

    Ok(())
}

// pub async fn run() {
//     cfg_if::cfg_if! {
//         if #[cfg(target_arch = "wasm32")] {
//             std::panic::set_hook(Box::new(console_error_panic_hook::hook));
//             console_log::init_with_level(log::Level::Info).expect("Couldn't initialize logger");
//         } else {
//             env_logger::init();
//         }
//     }

//     let event_loop = EventLoop::new();
//     let title = env!("CARGO_PKG_NAME");
//     let window = winit::window::WindowBuilder::new()
//         .with_title(title)
//         .with_inner_size(winit::dpi::LogicalSize::new(800, 450))
//         .build(&event_loop)
//         .unwrap();

//     window.set_cursor_visible(false);

//     // window
//     //     .set_cursor_grab(winit::window::CursorGrabMode::Confined)
//     //     .expect("failed to set cursor grab mode"); <- this always errors on linux

//     let mut render_state = Renderer::new(&window).await; // NEW!
//     let mut last_render_time = instant::Instant::now();
//     event_loop.run(move |event, _, control_flow| {
//         *control_flow = ControlFlow::Poll;
//         match event {
//             Event::MainEventsCleared => window.request_redraw(),
//             // NEW!
//             Event::DeviceEvent {
//                 event: DeviceEvent::MouseMotion{ delta, },
//                 .. // We're not using device_id currently
//             } => if render_state.mouse_pressed {
//                 render_state.camera_controller.process_mouse(delta.0, delta.1)
//             }
//             // UPDATED!
//             Event::WindowEvent {
//                 ref event,
//                 window_id,
//             } if window_id == window.id() && !render_state.input(event) => {
//                 match event {
//                     #[cfg(not(target_arch="wasm32"))]
//                     WindowEvent::CloseRequested
//                     | WindowEvent::KeyboardInput {
//                         input:
//                             KeyboardInput {
//                                 state: ElementState::Pressed,
//                                 virtual_keycode: Some(VirtualKeyCode::Escape),
//                                 ..
//                             },
//                         ..
//                     } => *control_flow = ControlFlow::Exit,
//                     WindowEvent::Resized(physical_size) => {
//                         render_state.resize(*physical_size);
//                     }
//                     WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
//                         render_state.resize(**new_inner_size);
//                     }
//                     _ => {}
//                 }
//             }
//             Event::RedrawRequested(window_id) if window_id == window.id() => {
//                 let now = instant::Instant::now();
//                 let dt = now - last_render_time;
//                 last_render_time = now;
//                 render_state.update(dt);
//                 match render_state.render() {
//                     Ok(_) => {}
//                     // Reconfigure the surface if it's lost or outdated
//                     Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => render_state.resize(render_state.size),
//                     // The system is out of memory, we should probably quit
//                     Err(wgpu::SurfaceError::OutOfMemory) => *control_flow = ControlFlow::Exit,
//                     // We're ignoring timeouts
//                     Err(wgpu::SurfaceError::Timeout) => log::warn!("Surface timeout"),
//                 }
//             }
//             _ => {}
//         }
//     });
// }

fn main() {
    run().unwrap();
}
