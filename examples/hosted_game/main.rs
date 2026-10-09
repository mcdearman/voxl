//! A game run inside another program: the way an editor shows one.
//!
//! The host here opens the graphics device itself, as a program with its own interface
//! would, and owns the loop. It gives the device to the game (`App::host`), steps it
//! (`App::update`), and takes each frame as a texture (`render::frame_texture`) to show
//! however it likes. This host has no window: it runs the sacred-site game for a second,
//! changes the size of the space it gives it, and saves the last frame.
//!
//! ```sh
//! cargo run --example hosted_game -- frame.png
//! ```

use mira::render::Screenshot;

#[path = "../sacred_sites/game.rs"]
mod game;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let saved = std::env::args().nth(1).unwrap_or("hosted.png".to_owned());

    // The host's own device, opened with nothing asked of it.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;

    let mut app = game::build(false)?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    app.host(device.clone(), queue, format, 640, 360);
    for frame in 0..60 {
        if frame == 30 {
            app.host_resized(960, 540);
        }
        app.update();
        let shown = mira::render::frame_texture(&app.world).expect("a frame to show");
        // What a host with an sRGB-encoded canvas samples: the bytes as stored.
        let _view = shown.create_view(&wgpu::TextureViewDescriptor {
            format: Some(format.remove_srgb_suffix()),
            ..Default::default()
        });
        if frame == 0 || frame == 59 {
            println!(
                "frame {frame}: {}x{} {:?}",
                shown.width(),
                shown.height(),
                shown.format()
            );
        }
    }
    app.world.resource_mut::<Screenshot>().request(&saved);
    app.update();
    // Reading the frame back takes the device a moment.
    app.update();
    println!("saved {saved}");
    Ok(())
}
