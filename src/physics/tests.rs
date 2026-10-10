use glam::{Quat, Vec3};

use super::{
    fluid::{Emitter, ParticleFluid, WaterSurface},
    step, BodyKind, CharacterController, Collider, Joint, JointKind, PhysicsWorld, RigidBody,
};
use crate::{
    ecs::{Entity, Schedule, World},
    time::FixedTime,
    transform::{GlobalTransform, Transform},
};

struct Sim {
    world: World,
    schedule: Schedule,
}

impl Sim {
    fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(FixedTime::default());
        world.insert_resource(PhysicsWorld::default());
        let mut schedule = Schedule::default();
        schedule.add_systems((step, super::query::move_characters, super::fluid::step_water, super::fluid::step_particles));
        schedule.initialize(&mut world);
        Self { world, schedule }
    }

    fn ground(&mut self) -> Entity {
        self.world.spawn((Transform::IDENTITY, GlobalTransform::default(), Collider::ground()))
    }

    fn body(&mut self, at: Vec3, collider: Collider, body: RigidBody) -> Entity {
        self.world.spawn((Transform::from_translation(at), GlobalTransform::default(), collider, body))
    }

    fn run(&mut self, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            self.schedule.run(&mut self.world);
        }
    }

    fn at(&self, e: Entity) -> Vec3 {
        self.world.get::<Transform>(e).unwrap().translation
    }
}

#[test]
fn a_ball_falls_and_rests_on_the_ground() {
    let mut sim = Sim::new();
    sim.ground();
    let ball = sim.body(Vec3::new(0.0, 5.0, 0.0), Collider::sphere(0.5), RigidBody::dynamic());
    sim.run(0.5);
    let falling = sim.at(ball).y;
    assert!(falling < 4.0 && falling > 0.5, "should be falling freely, at {falling}");
    sim.run(4.0);
    let y = sim.at(ball).y;
    assert!((y - 0.5).abs() < 0.03, "should rest on the ground, at {y}");
}

#[test]
fn a_bouncy_ball_bounces() {
    let mut sim = Sim::new();
    sim.ground();
    let ball = sim.body(Vec3::new(0.0, 3.0, 0.0), Collider::sphere(0.2).with_restitution(0.8), RigidBody::dynamic());
    let mut peak_after = 0.0f32;
    let mut touched = false;
    for _ in 0..240 {
        sim.run(1.0 / 60.0);
        let y = sim.at(ball).y;
        if y < 0.25 {
            touched = true;
        }
        if touched {
            peak_after = peak_after.max(y);
        }
    }
    assert!(touched && peak_after > 1.2, "should bounce well up again, peaked at {peak_after}");
}

#[test]
fn boxes_stack_and_stay_stacked() {
    let mut sim = Sim::new();
    sim.ground();
    let boxes: Vec<Entity> = (0..5)
        .map(|i| sim.body(Vec3::new(0.0, 0.25 + i as f32 * 0.52, 0.0), Collider::cuboid(Vec3::splat(0.25)), RigidBody::dynamic()))
        .collect();
    sim.run(4.0);
    for (i, b) in boxes.iter().enumerate() {
        let p = sim.at(*b);
        assert!((p.y - (0.25 + i as f32 * 0.5)).abs() < 0.06, "box {i} should be in the stack, at {p}");
        assert!(p.x.abs() < 0.05 && p.z.abs() < 0.05, "box {i} should not have slid off, at {p}");
    }
}

#[test]
fn a_box_slides_to_a_stop_with_friction() {
    let mut sim = Sim::new();
    sim.ground();
    let b = sim.body(Vec3::new(0.0, 0.25, 0.0), Collider::cuboid(Vec3::splat(0.25)).with_friction(0.5), RigidBody::dynamic().with_velocity(Vec3::X * 4.0));
    sim.run(3.0);
    let p = sim.at(b);
    // v² / (2 μ g) ≈ 16 / 9.8 ≈ 1.6 m.
    assert!(p.x > 1.0 && p.x < 2.4, "should slide about 1.6 m, went {}", p.x);
    let v = sim.world.get::<RigidBody>(b).unwrap().linear_velocity;
    assert!(v.length() < 0.05, "should have stopped, at {v}");
}

#[test]
fn a_rolling_sphere_meets_a_wall() {
    let mut sim = Sim::new();
    sim.ground();
    sim.world.spawn((Transform::from_xyz(3.0, 1.0, 0.0), GlobalTransform::default(), Collider::cuboid(Vec3::new(0.2, 1.0, 2.0))));
    let ball = sim.body(Vec3::new(0.0, 0.3, 0.0), Collider::sphere(0.3), RigidBody::dynamic().with_velocity(Vec3::X * 6.0));
    sim.run(3.0);
    assert!(sim.at(ball).x < 2.5, "the wall should stop it, at {}", sim.at(ball));
}

#[test]
fn a_pendulum_swings_on_a_rope() {
    let mut sim = Sim::new();
    let bob = sim.body(Vec3::new(2.0, 5.0, 0.0), Collider::sphere(0.1), RigidBody::dynamic());
    sim.world.spawn(Joint { a: bob, b: None, anchor_a: Vec3::ZERO, anchor_b: Vec3::new(0.0, 5.0, 0.0), kind: JointKind::Distance { min: 2.0, max: 2.0 } });
    let mut lowest = f32::MAX;
    for _ in 0..90 {
        sim.run(1.0 / 60.0);
        let p = sim.at(bob);
        lowest = lowest.min(p.y);
        let length = (p - Vec3::new(0.0, 5.0, 0.0)).length();
        assert!((length - 2.0).abs() < 0.1, "the rope should keep its length, is {length}");
    }
    assert!(lowest < 3.2, "it should swing down to the bottom, lowest {lowest}");
}

#[test]
fn a_hinged_door_turns_only_about_its_hinge() {
    let mut sim = Sim::new();
    let door = sim.body(Vec3::new(0.5, 1.0, 0.0), Collider::cuboid(Vec3::new(0.5, 1.0, 0.05)), RigidBody::dynamic());
    sim.world.spawn(Joint {
        a: door,
        b: None,
        anchor_a: Vec3::new(-0.5, 0.0, 0.0),
        anchor_b: Vec3::new(0.0, 1.0, 0.0),
        kind: JointKind::Hinge { axis_a: Vec3::Y, axis_b: Vec3::Y, limits: None },
    });
    // Swinging about the hinge: spinning, with the centre moving round it.
    let body = sim.world.get_mut::<RigidBody>(door).unwrap();
    body.angular_velocity = Vec3::Y * 2.0;
    body.linear_velocity = (Vec3::Y * 2.0).cross(Vec3::new(0.5, 0.0, 0.0));
    sim.run(1.0);
    let t = sim.world.get::<Transform>(door).unwrap();
    let up = t.rotation * Vec3::Y;
    assert!(up.dot(Vec3::Y) > 0.99, "it should stay upright, up is {up}");
    let hinge = t.translation + t.rotation * Vec3::new(-0.5, 0.0, 0.0);
    assert!(hinge.distance(Vec3::new(0.0, 1.0, 0.0)) < 0.05, "it should stay on its hinge, at {hinge}");
    assert!(t.rotation.angle_between(Quat::IDENTITY) > 0.5, "it should have swung");
}

#[test]
fn rays_hit_the_nearest_collider() {
    let mut sim = Sim::new();
    sim.ground();
    let b = sim.body(Vec3::new(0.0, 1.0, 0.0), Collider::cuboid(Vec3::splat(0.5)), RigidBody::kinematic());
    sim.run(1.0 / 60.0);
    let physics = sim.world.resource::<PhysicsWorld>();
    let hit = physics.raycast(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, 20.0, None).unwrap();
    assert_eq!(hit.entity, b);
    assert!((hit.point.y - 1.5).abs() < 1e-3 && hit.normal.y > 0.99);
    let ground = physics.raycast(Vec3::new(3.0, 5.0, 0.0), Vec3::NEG_Y, 20.0, None).unwrap();
    assert!(ground.point.y.abs() < 1e-3);
}

#[test]
fn a_kinematic_body_pushes_a_dynamic_one() {
    let mut sim = Sim::new();
    sim.ground();
    let pusher = sim.body(Vec3::new(-1.0, 0.5, 0.0), Collider::cuboid(Vec3::splat(0.5)), RigidBody::kinematic().with_velocity(Vec3::X * 1.0));
    let crate_ = sim.body(Vec3::new(0.6, 0.25, 0.0), Collider::cuboid(Vec3::splat(0.25)), RigidBody::dynamic());
    sim.run(2.0);
    assert!(sim.at(pusher).x > 0.9);
    assert!(sim.at(crate_).x > sim.at(pusher).x + 0.7, "the crate should be pushed ahead");
    assert_eq!(sim.world.get::<RigidBody>(pusher).unwrap().kind, BodyKind::Kinematic);
}

#[test]
fn a_body_moved_by_the_game_kicks_a_ball() {
    let mut sim = Sim::new();
    sim.ground();
    // A walker the game moves by hand, 3 m/s, into a resting ball.
    let walker = sim.body(Vec3::new(-2.0, 0.9, 0.0), Collider::capsule(1.8, 0.3), RigidBody::animated());
    let ball = sim.body(Vec3::new(0.0, 0.11, 0.0), Collider::sphere(0.11), RigidBody::dynamic().with_density(80.0));
    for _ in 0..60 {
        sim.world.get_mut::<Transform>(walker).unwrap().translation.x += 3.0 / 60.0;
        sim.run(1.0 / 60.0);
    }
    // The walker went where it was put, and the ball flew ahead faster than it walked.
    assert!((sim.at(walker).x - 1.0).abs() < 1e-3, "walker at {}", sim.at(walker));
    let v = sim.world.get::<RigidBody>(ball).unwrap().linear_velocity;
    assert!(sim.at(ball).x > 1.4 && v.x > 2.0, "ball at {} going {v}", sim.at(ball));
}

#[test]
fn a_character_walks_up_a_step_and_stops_at_a_wall() {
    let mut sim = Sim::new();
    sim.ground();
    sim.world.spawn((Transform::from_xyz(2.0, 0.1, 0.0), GlobalTransform::default(), Collider::cuboid(Vec3::new(0.5, 0.1, 2.0))));
    sim.world.spawn((Transform::from_xyz(5.0, 1.5, 0.0), GlobalTransform::default(), Collider::cuboid(Vec3::new(0.2, 1.5, 2.0))));
    sim.run(1.0 / 60.0);
    let mut c = CharacterController::new(1.8, 0.3);
    c.desired_velocity = Vec3::X * 2.0;
    let walker = sim.world.spawn((Transform::from_xyz(0.0, 0.9, 0.0), GlobalTransform::default(), c));
    sim.run(1.0);
    let on_step = sim.at(walker);
    assert!(on_step.x > 1.2 && on_step.x < 2.4 && (on_step.y - 1.1).abs() < 0.08, "should be up on the step, at {on_step}");
    sim.run(3.0);
    let p = sim.at(walker);
    assert!(p.x < 4.52 && p.x > 4.3, "the wall should stop it, at {p}");
}

#[test]
fn wood_floats_and_ripples_spread() {
    let mut sim = Sim::new();
    let mut water = WaterSurface::new(10.0, 10.0, 0.2);
    water.wave_speed = 2.0;
    let pool = sim.world.spawn((Transform::from_xyz(0.0, 0.0, 0.0), water));
    let log = sim.body(Vec3::new(0.0, 1.0, 0.0), Collider::cuboid(Vec3::new(0.4, 0.1, 0.1)), RigidBody::dynamic().with_density(500.0));
    sim.run(6.0);
    let y = sim.at(log).y;
    let level = sim.world.get::<WaterSurface>(pool).unwrap().height_at(sim.at(log));
    assert!((y - level).abs() < 0.08, "a half-dense log should float half under, at {y} with the water at {level}");
    // Something dropped in: the ring moves outward.
    sim.world.get_mut::<WaterSurface>(pool).unwrap().disturb(Vec3::new(2.0, 0.0, 2.0), 0.3, -3.0);
    sim.run(0.5);
    let w = sim.world.get::<WaterSurface>(pool).unwrap();
    let near = w.height_at(Vec3::new(2.0, 0.0, 2.0)).abs() + w.height_at(Vec3::new(2.8, 0.0, 2.0)).abs();
    let far = w.height_at(Vec3::new(-4.0, 0.0, -4.0)).abs();
    assert!(near > far, "the ripple should be near where it fell, not across the pool");
}

#[test]
fn a_jet_falls_in_an_arc_and_pools() {
    let mut sim = Sim::new();
    sim.ground();
    let fluid = ParticleFluid::new(0.05).with_emitter(Emitter {
        origin: Vec3::new(0.0, 1.0, 0.0),
        velocity: Vec3::new(2.0, 1.0, 0.0),
        rate: 400.0,
        spread: 0.05,
        radius: 0.02,
        pulse: 0.0,
    });
    let e = sim.world.spawn(fluid);
    sim.run(1.5);
    let f = sim.world.get::<ParticleFluid>(e).unwrap();
    assert!(f.positions.len() > 200, "the jet should have made particles");
    assert!(f.positions.iter().all(|p| p.y > -0.05), "none should fall through the ground");
    let reach = f.positions.iter().map(|p| p.x).fold(0.0f32, f32::max);
    assert!(reach > 0.8, "the jet should carry out along its arc, reached {reach}");
}


#[test]
fn droplets_are_closed_little_spheres() {
    let mut fluid = ParticleFluid::new(0.02);
    fluid.spawn(Vec3::ZERO, Vec3::ZERO);
    fluid.spawn(Vec3::X, Vec3::X * 4.0);
    let mesh = fluid.droplets(0.012);
    assert_eq!(mesh.indices.len() % 3, 0);
    let per = mesh.indices.len() / 2;
    assert!(per >= 3 * 24, "only {} triangles a droplet", per / 3);
    // Every face points outward, from its droplet's centre.
    for (k, tri) in mesh.indices.chunks(3).enumerate() {
        let center = if k * 3 < per { Vec3::ZERO } else { Vec3::X };
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        assert!((b - a).cross(c - a).dot((a + b + c) / 3.0 - center) > 0.0, "face {k} points inward");
    }
    // The moving one is drawn out along its motion.
    let xs = |range: std::ops::Range<usize>| mesh.vertices[range].iter().map(|v| v.position[0]).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let n = mesh.vertices.len() / 2;
    let (still, moving) = (xs(0..n), xs(n..2 * n));
    assert!(moving.1 - moving.0 > 2.0 * (still.1 - still.0));
}

#[test]
fn surface_tension_keeps_a_jet_together() {
    // The same sideways jet with and without cohesion: how wide is it half a metre down?
    let spread = |cohesion: f32| {
        let mut fluid = ParticleFluid::new(0.022);
        fluid.cohesion = cohesion;
        let (radius, velocity) = (0.028, Vec3::X * 3.0);
        let rate = std::f32::consts::PI * radius * radius * 3.0 / fluid.spacing.powi(3);
        fluid = fluid.with_emitter(Emitter { origin: Vec3::Y * 2.0, velocity, rate, spread: 0.03, radius, pulse: 0.0 });
        for _ in 0..40 {
            fluid.step(1.0 / 60.0, Vec3::new(0.0, -9.81, 0.0), None, &mut []);
        }
        let band: Vec<f32> = fluid.positions.iter().filter(|p| p.y < 1.6 && p.y > 1.4).map(|p| p.z).collect();
        assert!(band.len() > 10);
        let mean = band.iter().sum::<f32>() / band.len() as f32;
        (band.iter().map(|z| (z - mean) * (z - mean)).sum::<f32>() / band.len() as f32).sqrt()
    };
    let (loose, held) = (spread(0.0), spread(0.35));
    assert!(held < loose * 0.8, "with cohesion {held} m wide, without {loose} m");
}

#[test]
fn a_saved_scene_simulates_the_same_after_loading() {
    use crate::reflect::{Scene, TypeRegistry};

    let mut registry = TypeRegistry::default();
    registry.register::<Transform>();
    registry.register::<RigidBody>();
    registry.register::<Collider>();
    registry.register::<Joint>();
    registry.register::<CharacterController>();

    let mut sim = Sim::new();
    // A floor of two triangles, a pendulum, a lopsided compound body and a walker: every
    // kind of shape, a joint that names another entity, and a body already moving.
    let floor = super::TriMesh::new(
        vec![
            Vec3::new(-20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, 20.0),
            Vec3::new(-20.0, 0.0, 20.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    );
    sim.world.spawn((
        Transform::IDENTITY,
        GlobalTransform::default(),
        Collider::trimesh(floor).with_friction(0.7),
    ));
    let bob = sim.body(
        Vec3::new(2.0, 5.0, 0.0),
        Collider::sphere(0.1),
        RigidBody::dynamic().with_mass(3.0),
    );
    sim.world.spawn(Joint {
        a: bob,
        b: None,
        anchor_a: Vec3::ZERO,
        anchor_b: Vec3::new(0.0, 5.0, 0.0),
        kind: JointKind::Distance { min: 2.0, max: 2.0 },
    });
    let lump = sim.body(
        Vec3::new(-3.0, 2.0, 1.0),
        Collider::compound(vec![
            (super::Iso::IDENTITY, super::Shape::cuboid(Vec3::splat(0.3))),
            (
                super::Iso::new(Vec3::new(0.5, 0.2, 0.0), Quat::from_rotation_z(0.4)),
                super::Shape::Capsule {
                    half_height: 0.3,
                    radius: 0.1,
                },
            ),
        ])
        .with_restitution(0.4),
        RigidBody::dynamic().with_velocity(Vec3::new(1.0, 0.0, -0.5)),
    );
    let door = sim.body(
        Vec3::new(6.0, 1.2, 0.0),
        Collider::cuboid(Vec3::new(0.5, 1.0, 0.05)),
        RigidBody::dynamic(),
    );
    sim.world.spawn(Joint {
        a: door,
        b: Some(lump),
        anchor_a: Vec3::new(-0.5, 0.0, 0.0),
        anchor_b: Vec3::new(0.0, 1.0, 0.0),
        kind: JointKind::Hinge {
            axis_a: Vec3::Y,
            axis_b: Vec3::Y,
            limits: Some((-1.0, 1.0)),
        },
    });
    let mut walker = CharacterController::new(1.8, 0.3);
    walker.desired_velocity = Vec3::new(0.0, 0.0, 1.5);
    let walker = sim.world.spawn((
        Transform::from_xyz(8.0, 1.0, 0.0),
        GlobalTransform::default(),
        walker,
    ));
    sim.run(0.25);

    let scene = Scene::from_json(&Scene::capture(&sim.world, &registry).to_json()).unwrap();
    let mut loaded = Sim::new();
    loaded.world.spawn_empty(); // so the ids differ, and the joints must be pointed at the new ones
    let spawned = scene.spawn(&mut loaded.world, &registry);
    assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
    for &entity in &spawned.entities {
        loaded.world.insert(entity, GlobalTransform::default());
    }

    // The scene lists entities in the order they were made, so pair them up by position.
    let saved = [bob, lump, door, walker];
    let ids: Vec<u64> = scene.entities.iter().map(|e| e.id).collect();
    let new =
        saved.map(|e| spawned.entities[ids.iter().position(|&id| id == e.to_bits()).unwrap()]);
    sim.run(2.0);
    loaded.run(2.0);
    for (old, new) in saved.into_iter().zip(new) {
        let (a, b) = (sim.at(old), loaded.at(new));
        assert!(
            a.distance(b) < 1e-3,
            "{old:?} ended at {a}, its copy at {b}"
        );
    }
    assert!(
        sim.at(walker).z > 2.0,
        "the walker should have walked, is at {}",
        sim.at(walker)
    );
}

#[test]
fn a_character_jumps_and_comes_down_again() {
    let mut sim = Sim::new();
    sim.ground();
    let walker = sim.world.spawn((
        Transform::from_xyz(0.0, 0.9, 0.0),
        GlobalTransform::default(),
        CharacterController::new(1.8, 0.35),
    ));
    sim.run(0.3);
    let height = |sim: &Sim| sim.world.get::<Transform>(walker).unwrap().translation.y;
    let standing = height(&sim);
    assert!(sim.world.get::<CharacterController>(walker).unwrap().grounded);

    sim.world.get_mut::<CharacterController>(walker).unwrap().jump = true;
    // At 4.5 m/s it is at the top, about a metre up, after some 0.46 s.
    sim.run(0.45);
    let top = height(&sim);
    assert!(top > standing + 0.8 && top < standing + 1.3, "the top of the jump was {top}");
    assert!(!sim.world.get::<CharacterController>(walker).unwrap().grounded);
    sim.run(1.0);
    assert!((height(&sim) - standing).abs() < 0.02);
    assert!(sim.world.get::<CharacterController>(walker).unwrap().grounded);

    // Under a low roof it rises only as far as the roof.
    sim.world.spawn((
        Transform::from_xyz(0.0, 2.4, 0.0),
        GlobalTransform(glam::Mat4::from_translation(Vec3::new(0.0, 2.4, 0.0))),
        Collider::cuboid(Vec3::new(2.0, 0.2, 2.0)),
    ));
    sim.run(0.1);
    sim.world.get_mut::<CharacterController>(walker).unwrap().jump = true;
    let mut highest = standing;
    for _ in 0..40 {
        sim.run(1.0 / 60.0);
        highest = highest.max(height(&sim));
    }
    assert!(highest < standing + 0.45, "it went through the roof, to {highest}");
}
