//! More than one view of the world in a frame. A camera with a [`ViewTarget`] draws into a
//! picture of its own, beside the frame the window (or a host) shows: the game as its player
//! sees it next to the scene as an editor does, a mirror, a map in a corner.
//!
//! Each such view is the whole renderer run again: after the frame is drawn, everything that
//! belongs to one view (the targets, the history frames are blended over, the camera and
//! what it sees) is set aside, the view's own put in its place, and the drawing stages run
//! once more. So a second view costs what the first did, and everything that draws, a
//! plugin's pipelines too, is in it without knowing.

use std::collections::HashMap;

use super::{
    gpu::{Gpu, Targets},
    post::PostRenderer,
    taa::Taa,
    Camera, Offscreen, RenderFrame,
};
use crate::{
    app::{App, Stage},
    ecs::{Component, Entity, World},
};

/// On a camera: it draws into a picture of its own, this many pixels, every frame, whether
/// or not it is `active`. The picture is [`view_texture`]. A camera with one is never the
/// one the window shows.
///
/// ```ignore
/// let mirror = commands.spawn((at, Camera::default(), ViewTarget::new(512, 512))).id();
/// // … later, with the world:
/// let picture = mira::render::view_texture(world, mirror);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewTarget {
    pub width: u32,
    pub height: u32,
}

impl Component for ViewTarget {}

impl ViewTarget {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// What one view keeps from frame to frame, set in the renderer's place while it draws.
struct ViewState {
    size: (u32, u32),
    targets: Targets,
    surface: Option<wgpu::Surface<'static>>,
    taa: Taa,
    frame: RenderFrame,
    offscreen: Option<Offscreen>,
    bloom: Option<(u32, u32, Vec<wgpu::TextureView>)>,
}

impl ViewState {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Self {
        let mut config = gpu.config.clone();
        (config.width, config.height) = size;
        Self {
            size,
            targets: Targets::new(&gpu.device, &config),
            surface: None,
            taa: Taa::new(gpu),
            frame: RenderFrame::default(),
            offscreen: None,
            bloom: None,
        }
    }

    /// Changes places with the renderer's own state. Twice puts everything back.
    fn swap(&mut self, world: &mut World) {
        let gpu = world.resource_mut::<Gpu>();
        std::mem::swap(&mut gpu.targets, &mut self.targets);
        std::mem::swap(&mut gpu.config.width, &mut self.size.0);
        std::mem::swap(&mut gpu.config.height, &mut self.size.1);
        // With no window to draw to, a frame is drawn off screen: into the view's picture.
        std::mem::swap(&mut gpu.surface, &mut self.surface);
        std::mem::swap(world.resource_mut::<Taa>(), &mut self.taa);
        std::mem::swap(world.resource_mut::<RenderFrame>(), &mut self.frame);
        world.resource_mut::<PostRenderer>().swap_bloom(&mut self.bloom);
        let theirs = world.remove_resource::<Offscreen>();
        if let Some(mine) = self.offscreen.take() {
            world.insert_resource(mine);
        }
        self.offscreen = theirs;
    }
}

/// The views drawn beside the frame, by camera.
#[derive(Default)]
pub(crate) struct ExtraViews(HashMap<Entity, ViewState>);

/// The picture a camera with a [`ViewTarget`] last drew: tone-mapped and opaque, in the
/// frame's format, as [`frame_texture`](super::frame_texture) is. None until it has drawn
/// once. A new texture replaces it when the target's size changes, so ask each frame.
pub fn view_texture(world: &World, camera: Entity) -> Option<wgpu::Texture> {
    let view = world.get_resource::<ExtraViews>()?.0.get(&camera)?;
    view.offscreen.as_ref().map(|offscreen| offscreen.0.clone())
}

/// Draws every camera that has a target of its own. Called once the frame is drawn.
pub(crate) fn draw(app: &mut App) {
    let world = &mut app.world;
    if !world.contains_resource::<Gpu>() || !world.contains_resource::<ExtraViews>() {
        return;
    }
    let wanted: Vec<(Entity, (u32, u32))> = world
        .query::<(Entity, &Camera, &ViewTarget)>()
        .iter()
        .map(|(entity, _, target)| (entity, (target.width.max(1), target.height.max(1))))
        .collect();
    let mut views = std::mem::take(&mut world.resource_mut::<ExtraViews>().0);
    views.retain(|camera, _| wanted.iter().any(|(entity, _)| entity == camera));
    for (camera, size) in wanted {
        let world = &mut app.world;
        let mut view = match views.remove(&camera) {
            Some(view) if view.size == size => view,
            _ => ViewState::new(world.resource::<Gpu>(), size),
        };
        view.frame.through = Some(camera);
        view.swap(world);
        for stage in [Stage::Extract, Stage::Prepare, Stage::Render] {
            app.run_stage(stage);
        }
        view.swap(&mut app.world);
        views.insert(camera, view);
    }
    app.world.resource_mut::<ExtraViews>().0 = views;
}
