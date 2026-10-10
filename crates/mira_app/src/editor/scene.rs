//! Working on the scene in the picture of it: which entity is under the pointer, where the
//! pointer is on the ground, and where an entity is on the screen.
//!
//! Places in the picture are counted from its top-left corner, across and down, from 0 to 1,
//! so nothing here depends on how big the picture is drawn.

use glam::{Mat4, Vec2, Vec3};

use mira::render::Camera;

/// A line from the eye through a place in the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub from: Vec3,
    /// Not of any particular length.
    pub along: Vec3,
}

/// The line of sight through a place in the picture, for a camera placed in the world by
/// `placed`, in a picture `aspect` times as wide as it is tall.
pub fn sight(camera: &Camera, placed: Mat4, at: Vec2, aspect: f32) -> Ray {
    // From the middle: right and up are positive, the edges are at one.
    let (across, up) = (at.x * 2.0 - 1.0, 1.0 - at.y * 2.0);
    let (widening, half_height) = camera.spread();
    // A perspective camera looks out from a point; an orthographic one, straight ahead from
    // wherever in its view the place is.
    let from = Vec3::new(across * half_height * aspect, up * half_height, 0.0);
    let along = Vec3::new(across * widening * aspect, up * widening, -1.0);
    Ray {
        from: placed.transform_point3(from),
        along: placed.transform_vector3(along),
    }
}

/// Where a point of the world is in the picture, if it is in front of the camera.
pub fn in_picture(camera: &Camera, placed: Mat4, point: Vec3, aspect: f32) -> Option<Vec2> {
    let seen = placed.inverse().transform_point3(point);
    let ahead = -seen.z;
    if ahead <= camera.near {
        return None;
    }
    let (widening, half_height) = camera.spread();
    // How far from the middle the edge of the view is, at that depth.
    let half = widening * ahead + half_height;
    Some(Vec2::new(
        (seen.x / (half * aspect) + 1.0) * 0.5,
        (1.0 - seen.y / half) * 0.5,
    ))
}

/// How far along a ray it enters a box, if it does: the box being `least` to `most` in a
/// space of its own, put in the world by `placed`. The distance is in lengths of the ray's
/// `along`, so that hits on different boxes can be compared.
pub fn enters(ray: Ray, placed: Mat4, least: Vec3, most: Vec3) -> Option<f32> {
    if placed.determinant().abs() < 1e-12 {
        return None;
    }
    // The ray in the box's own space, where the box is square to the axes.
    let inward = placed.inverse();
    let (from, along) = (
        inward.transform_point3(ray.from),
        inward.transform_vector3(ray.along),
    );
    let (mut nearest, mut farthest) = (0.0f32, f32::INFINITY);
    for axis in 0..3 {
        let (from, along, least, most) = (from[axis], along[axis], least[axis], most[axis]);
        if along.abs() < 1e-9 {
            // Running alongside these two faces: between them, or missing altogether.
            if from < least || from > most {
                return None;
            }
            continue;
        }
        let (a, b) = ((least - from) / along, (most - from) / along);
        nearest = nearest.max(a.min(b));
        farthest = farthest.min(a.max(b));
    }
    (nearest <= farthest).then_some(nearest)
}

/// Where a ray meets level ground at a height, if it is heading for it.
pub fn on_ground(ray: Ray, height: f32) -> Option<Vec3> {
    if ray.along.y.abs() < 1e-6 {
        return None;
    }
    let along = (height - ray.from.y) / ray.along.y;
    (along > 0.0).then(|| ray.from + ray.along * along)
}

/// How far along a line (from `start`, in the direction `axis`, in lengths of `axis`) the
/// point is that comes nearest a ray: where on a handle the pointer is. Nothing if the
/// line runs straight at the eye, where the pointer can't say how far along it is.
pub fn along(ray: Ray, start: Vec3, axis: Vec3) -> Option<f32> {
    // The standard nearest points of two lines.
    let between = start - ray.from;
    let (aa, ab, bb) = (
        axis.dot(axis),
        axis.dot(ray.along),
        ray.along.dot(ray.along),
    );
    let (ac, bc) = (axis.dot(between), ray.along.dot(between));
    let apart = aa * bb - ab * ab;
    (apart.abs() > 1e-6 * aa * bb).then(|| (ab * bc - bb * ac) / apart)
}

/// The part of the picture a box covers, as its left, top, right and bottom, if all of it
/// is in front of the camera.
pub fn covers(
    camera: &Camera,
    seen_from: Mat4,
    aspect: f32,
    placed: Mat4,
    least: Vec3,
    most: Vec3,
) -> Option<[f32; 4]> {
    let mut edges = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for corner in 0..8 {
        let pick = |bit: usize, axis: usize| {
            if corner >> bit & 1 == 0 {
                least[axis]
            } else {
                most[axis]
            }
        };
        let corner = placed.transform_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)));
        let at = in_picture(camera, seen_from, corner, aspect)?;
        edges = [
            edges[0].min(at.x),
            edges[1].min(at.y),
            edges[2].max(at.x),
            edges[3].max(at.y),
        ];
    }
    Some(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::transform::Transform;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn a_place_in_the_picture_and_a_point_of_the_world_answer_each_other() {
        let placed = Transform::from_xyz(6.0, 4.5, 8.0)
            .looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y)
            .matrix();
        for camera in [Camera::default(), Camera::orthographic(12.0)] {
            // Straight ahead is the middle of the picture.
            let middle = sight(&camera, placed, Vec2::new(0.5, 0.5), 1.6);
            let ahead = placed.transform_vector3(Vec3::NEG_Z);
            assert!(close(middle.along.normalize(), ahead.normalize()));
            // A point seen at a place is on the line of sight through that place.
            for point in [
                Vec3::new(0.0, 0.8, 0.0),
                Vec3::new(-2.0, 0.5, 1.0),
                Vec3::new(3.0, 2.0, -4.0),
            ] {
                let at = in_picture(&camera, placed, point, 1.6).expect("in front");
                let ray = sight(&camera, placed, at, 1.6);
                let to = point - ray.from;
                let off = to - ray.along.normalize() * to.dot(ray.along.normalize());
                assert!(
                    off.length() < 1e-3,
                    "{point} is {off} off its own line of sight"
                );
            }
            // What is behind the camera is not in the picture.
            let behind = placed.transform_point3(Vec3::new(0.0, 0.0, 5.0));
            assert_eq!(in_picture(&camera, placed, behind, 1.6), None);
        }
    }

    #[test]
    fn a_ray_finds_the_nearer_box_and_the_ground() {
        let down = Ray {
            from: Vec3::new(0.0, 10.0, 0.0),
            along: Vec3::new(0.0, -2.0, 0.0),
        };
        let unit = (Vec3::splat(-0.5), Vec3::splat(0.5));
        let low = Transform::from_xyz(0.0, 0.5, 0.0).matrix();
        let high = Transform::from_xyz(0.2, 4.0, 0.0).matrix();
        let aside = Transform::from_xyz(3.0, 0.5, 0.0).matrix();
        let (to_low, to_high) = (
            enters(down, low, unit.0, unit.1).unwrap(),
            enters(down, high, unit.0, unit.1).unwrap(),
        );
        // Distances are in lengths of `along`, here two metres.
        assert!((to_low - 4.5).abs() < 1e-4 && (to_high - 2.75).abs() < 1e-4);
        assert_eq!(enters(down, aside, unit.0, unit.1), None);
        // A box turned and stretched is still found where it is.
        let slab = Transform::from_xyz(0.0, 2.0, 0.0)
            .with_scale(Vec3::new(8.0, 0.2, 8.0))
            .with_rotation(glam::Quat::from_rotation_y(0.7))
            .matrix();
        assert!((enters(down, slab, unit.0, unit.1).unwrap() - 3.95).abs() < 1e-3);
        // One squashed to nothing is not.
        let flat = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));
        assert_eq!(enters(down, flat, unit.0, unit.1), None);

        assert!(close(
            on_ground(down, 1.0).unwrap(),
            Vec3::new(0.0, 1.0, 0.0)
        ));
        let level = Ray {
            from: Vec3::Y,
            along: Vec3::X,
        };
        assert_eq!(on_ground(level, 0.0), None, "it never comes down");
        let up = Ray {
            from: Vec3::ZERO,
            along: Vec3::Y,
        };
        assert_eq!(on_ground(up, -1.0), None, "the ground is behind it");
    }

    #[test]
    fn the_pointer_says_how_far_along_a_handle_it_is() {
        // Looking north from the south, level: a handle running east.
        let eye = Vec3::new(0.0, 1.0, 10.0);
        let toward = |point: Vec3| Ray {
            from: eye,
            along: point - eye,
        };
        let start = Vec3::new(-1.0, 1.0, 0.0);
        for reach in [0.0, 0.5, 3.0, -2.0] {
            let found = along(toward(start + Vec3::X * reach), start, Vec3::X).unwrap();
            assert!((found - reach).abs() < 1e-4, "{reach}: {found}");
        }
        // A longer axis counts in its own lengths.
        let found = along(toward(Vec3::new(3.0, 1.0, 0.0)), start, Vec3::X * 2.0).unwrap();
        assert!((found - 2.0).abs() < 1e-4);
        // A handle pointing straight at the eye can't be read.
        assert_eq!(
            along(toward(Vec3::new(0.0, 1.0, 0.0)), eye + Vec3::NEG_Z, Vec3::Z),
            None
        );
    }

    #[test]
    fn a_box_covers_a_part_of_the_picture() {
        let camera = Camera::default();
        let placed = Transform::from_xyz(0.0, 0.0, 10.0).matrix();
        let unit = (Vec3::splat(-0.5), Vec3::splat(0.5));
        let [left, top, right, bottom] =
            covers(&camera, placed, 1.0, Mat4::IDENTITY, unit.0, unit.1).unwrap();
        // In the middle, as wide as tall, and small at that distance.
        assert!((left + right - 1.0).abs() < 1e-4 && (top + bottom - 1.0).abs() < 1e-4);
        assert!((right - left - (bottom - top)).abs() < 1e-4);
        assert!(right - left > 0.05 && right - left < 0.2);
        // One the camera is inside of covers nothing that can be drawn.
        assert_eq!(covers(&camera, placed, 1.0, placed, unit.0, unit.1), None);
    }
}
