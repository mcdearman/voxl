//! The fountain's water, simulated by the engine's fluids. The basin is a `WaterSurface`, a
//! height field obeying the wave equation, so rings spread from wherever it is struck, cross,
//! reflect off the wall and die away. Each lion's mouth is an `Emitter` of a `ParticleFluid`:
//! the jet is thousands of small blobs of water that hold together as a stream as they arc
//! down, fray and break into drops as they fall, and vanish into the basin where they land,
//! each one pushing the surface down a little, so the jets drive the waves themselves.

use voxl::{glam::Vec2, prelude::*};

use crate::square;

/// Cells across the basin, and metres per cell.
const CELL: f32 = 0.0875;
const SIZE: f32 = 9.8;
const RADIUS: f32 = 4.76;
/// The width of the stream from a lion's mouth (its radius).
const NOZZLE: f32 = 0.028;

/// The basin's surface, for stirring.
pub struct Basin {
    rng: u32,
}

pub fn spawn(world: &mut World, water: Material) {
    let center = Vec3::new(square::FOUNTAIN.x, square::BASIN_LEVEL, square::FOUNTAIN.z);
    // Inside the basin wall, and outside the pedestal standing in its middle.
    let mut surface = WaterSurface::new(SIZE, SIZE, CELL).with_shape(|o| o.length() < RADIUS + CELL && (o.x.abs() > 2.2 || o.y.abs() > 2.2));
    surface.wave_speed = 2.1;
    surface.damping = 0.35;
    surface.coupling = 0.35;
    let mesh = world.resource_mut::<Assets<Mesh>>().add(surface.mesh(center));
    surface.mesh = Some(mesh);
    world.spawn((Transform::from_translation(center), surface));
    world.spawn((Transform::IDENTITY, Mesh3d(mesh), Material { waves: 0.15, ..water }, NotShadowCaster));

    // The jets: a stream a few centimetres across from each mouth, surging a little.
    let mut jets = ParticleFluid::new(0.022);
    jets.max_particles = 3000;
    jets.lifetime = 2.0;
    jets.viscosity = 0.08;
    jets.drop_size = 0.9;
    jets.cohesion = 0.35;
    for (origin, velocity) in square::spouts() {
        // As many particles as fill the stream at the fluid's own density: more and the
        // solver would push them apart into a spray.
        let rate = std::f32::consts::PI * NOZZLE * NOZZLE * velocity.length() / jets.spacing.powi(3);
        jets = jets.with_emitter(Emitter { origin, velocity, rate, spread: 0.03, radius: NOZZLE, pulse: 0.15 });
    }
    let droplets = world.resource_mut::<Assets<Mesh>>().add(Mesh::default());
    jets.mesh = Some(droplets);
    world.spawn(jets);
    // Falling water catches the light brightly and is whitened by the air in it.
    let stream = Material { color: Color::rgb(0.22, 0.25, 0.25), roughness: 0.03, waves: 0.4, ..water };
    world.spawn((Transform::IDENTITY, Mesh3d(droplets), stream, NotShadowCaster));
    world.insert_resource(Basin { rng: 0x9e37_79b9 });
}

impl Basin {
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }
}

/// Drops falling back from the spray, now and then, dimple the rest of the basin.
pub fn stir(basin: Option<ResMut<Basin>>, mut surfaces: Query<&mut WaterSurface>) {
    let Some(mut basin) = basin else { return };
    for mut water in &mut surfaces {
        for _ in 0..3 {
            let (a, r) = (basin.random() * std::f32::consts::TAU, 2.2 + basin.random() * 2.4);
            let at = square::fountain_center() + Vec2::new(a.cos(), a.sin()) * r;
            if basin.random() < 0.4 {
                water.disturb(Vec3::new(at.x, square::BASIN_LEVEL, at.y), 0.08, -0.25);
            }
        }
    }
}
