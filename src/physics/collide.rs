//! Narrow phase: where two shapes touch. Each touching pair gives one or more manifolds, a
//! normal and up to four points; boxes resting on boxes get whole faces of points (by clipping
//! one face against the other) so that stacks stand steady.

use glam::{Mat3, Vec3};

use super::shape::{Aabb, Iso, Shape, TriMesh};

/// Contacts closer than this, though not yet touching, are kept, so bodies arriving fast stop
/// at the surface instead of passing into it (speculative contacts).
pub const MARGIN: f32 = 0.02;

#[derive(Clone, Debug)]
pub struct Manifold {
    /// Unit normal, from the first shape toward the second.
    pub normal: Vec3,
    /// Points in world space, and how far the shapes overlap there (negative: a gap).
    pub points: Vec<(Vec3, f32)>,
}

fn flip(mut m: Manifold) -> Manifold {
    m.normal = -m.normal;
    m
}

/// Every manifold between two shapes.
pub fn contact(a: &Shape, ia: &Iso, b: &Shape, ib: &Iso, out: &mut Vec<Manifold>) {
    use Shape::*;
    let one = |m: Option<Manifold>, out: &mut Vec<Manifold>| {
        if let Some(m) = m.filter(|m| !m.points.is_empty()) {
            out.push(m);
        }
    };
    match (a, b) {
        (Sphere { radius: ra }, Sphere { radius: rb }) => one(sphere_sphere(ia.position, *ra, ib.position, *rb), out),
        (Sphere { radius }, Capsule { half_height, radius: rb }) => {
            let (p, q) = segment(ib, *half_height);
            one(sphere_sphere(ia.position, *radius, closest_on_segment(ia.position, p, q), *rb), out)
        }
        (Capsule { .. }, Sphere { .. }) => contact_flipped(a, ia, b, ib, out),
        (Capsule { half_height: ha, radius: ra }, Capsule { half_height: hb, radius: rb }) => {
            one(capsule_capsule(segment(ia, *ha), *ra, segment(ib, *hb), *rb), out)
        }
        (Sphere { radius }, Cuboid { half }) => one(sphere_box(ia.position, *radius, ib, *half), out),
        (Cuboid { .. }, Sphere { .. }) => contact_flipped(a, ia, b, ib, out),
        (Capsule { half_height, radius }, Cuboid { half }) => one(capsule_box(segment(ia, *half_height), *radius, ib, *half), out),
        (Cuboid { .. }, Capsule { .. }) => contact_flipped(a, ia, b, ib, out),
        (Cuboid { half: ha }, Cuboid { half: hb }) => one(box_box(ia, *ha, ib, *hb), out),
        (_, HalfSpace { normal }) => one(against_plane(a, ia, ib.rotation * *normal, ib.position), out),
        (HalfSpace { .. }, _) => contact_flipped(a, ia, b, ib, out),
        (_, TriMesh(mesh)) => against_mesh(a, ia, mesh, ib, out),
        (TriMesh(_), _) => contact_flipped(a, ia, b, ib, out),
    }
}

fn contact_flipped(a: &Shape, ia: &Iso, b: &Shape, ib: &Iso, out: &mut Vec<Manifold>) {
    let start = out.len();
    contact(b, ib, a, ia, out);
    for m in &mut out[start..] {
        *m = flip(m.clone());
    }
}

fn segment(iso: &Iso, half_height: f32) -> (Vec3, Vec3) {
    let axis = iso.rotation * Vec3::Y * half_height;
    (iso.position - axis, iso.position + axis)
}

pub fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    a + ab * t
}

/// The closest points between two segments (Ericson, Real-Time Collision Detection 5.1.9).
pub fn closest_segments(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let (s, t);
    if a <= 1e-9 && e <= 1e-9 {
        return (p1, p2);
    }
    if a <= 1e-9 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-9 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut ss = if denom > 1e-9 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut tt = (b * ss + f) / e;
            if tt < 0.0 {
                tt = 0.0;
                ss = (-c / a).clamp(0.0, 1.0);
            } else if tt > 1.0 {
                tt = 1.0;
                ss = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = ss;
            t = tt;
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

fn sphere_sphere(a: Vec3, ra: f32, b: Vec3, rb: f32) -> Option<Manifold> {
    let d = b - a;
    let dist = d.length();
    let depth = ra + rb - dist;
    if depth < -MARGIN {
        return None;
    }
    let n = if dist > 1e-6 { d / dist } else { Vec3::Y };
    Some(Manifold { normal: n, points: vec![(a + n * (ra - depth * 0.5), depth)] })
}

fn capsule_capsule((p1, q1): (Vec3, Vec3), ra: f32, (p2, q2): (Vec3, Vec3), rb: f32) -> Option<Manifold> {
    let (a, b) = closest_segments(p1, q1, p2, q2);
    let mut m = sphere_sphere(a, ra, b, rb)?;
    // Lying side by side: touch along the whole overlap, not at one point.
    let (da, db) = ((q1 - p1).normalize_or(Vec3::Y), (q2 - p2).normalize_or(Vec3::Y));
    if da.dot(db).abs() > 0.98 {
        let n = m.normal;
        let depth = m.points[0].1;
        let project = |p: Vec3| (p - p1).dot(da);
        let (lo, hi) = (project(p2).min(project(q2)).max(0.0), project(p2).max(project(q2)).min((q1 - p1).length()));
        if hi > lo + 0.01 {
            m.points = [lo, hi].iter().map(|&t| (p1 + da * t + n * (ra - depth * 0.5), depth)).collect();
        }
    }
    Some(m)
}

/// A sphere against a box: normal from the sphere to the box.
fn sphere_box(c: Vec3, r: f32, ib: &Iso, half: Vec3) -> Option<Manifold> {
    let local = ib.inverse_transform_point(c);
    let clamped = local.clamp(-half, half);
    let delta = local - clamped;
    let dist = delta.length();
    if dist > 1e-6 {
        let depth = r - dist;
        if depth < -MARGIN {
            return None;
        }
        let n = -(ib.rotation * (delta / dist));
        let surface = ib.transform_point(clamped);
        return Some(Manifold { normal: n, points: vec![(surface, depth)] });
    }
    // The centre is inside: push out through the nearest face.
    let gaps = half - local.abs();
    let axis = if gaps.x <= gaps.y && gaps.x <= gaps.z { 0 } else if gaps.y <= gaps.z { 1 } else { 2 };
    let mut out = Vec3::ZERO;
    out[axis] = local[axis].signum();
    let n = -(ib.rotation * out);
    Some(Manifold { normal: n, points: vec![(c, r + gaps[axis])] })
}

fn capsule_box((p, q): (Vec3, Vec3), r: f32, ib: &Iso, half: Vec3) -> Option<Manifold> {
    // Distance to a box is convex along a segment: find the closest point by golden section.
    let dist = |t: f32| {
        let x = ib.inverse_transform_point(p.lerp(q, t));
        (x - x.clamp(-half, half)).length() - (half - x.abs()).min_element().max(0.0)
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let g = 0.618_034;
    for _ in 0..24 {
        let (m1, m2) = (hi - (hi - lo) * g, lo + (hi - lo) * g);
        if dist(m1) < dist(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let best = p.lerp(q, (lo + hi) * 0.5);
    let mut m = sphere_box(best, r, ib, half)?;
    // Lying flat on a face: the two ends touch too.
    for end in [p, q] {
        if let Some(e) = sphere_box(end, r, ib, half) {
            if e.normal.dot(m.normal) > 0.95 && end.distance(best) > 0.02 {
                m.points.push(e.points[0]);
            }
        }
    }
    Some(m)
}

fn box_axes(iso: &Iso) -> [Vec3; 3] {
    let m = Mat3::from_quat(iso.rotation);
    [m.x_axis, m.y_axis, m.z_axis]
}

fn box_box(ia: &Iso, ha: Vec3, ib: &Iso, hb: Vec3) -> Option<Manifold> {
    let (a, b) = (box_axes(ia), box_axes(ib));
    let d = ib.position - ia.position;
    let radius = |axes: &[Vec3; 3], h: Vec3, l: Vec3| axes[0].dot(l).abs() * h.x + axes[1].dot(l).abs() * h.y + axes[2].dot(l).abs() * h.z;
    // (separation, axis, kind): kind 0..3 a face of A, 3..6 a face of B, 6.. an edge pair.
    let mut best = (f32::MIN, Vec3::ZERO, 0usize);
    let mut face_best = f32::MIN;
    for (k, l) in a.iter().chain(b.iter()).enumerate() {
        let s = d.dot(*l).abs() - radius(&a, ha, *l) - radius(&b, hb, *l);
        if s > MARGIN {
            return None;
        }
        if s > best.0 {
            best = (s, *l, k);
        }
    }
    face_best = face_best.max(best.0);
    for i in 0..3 {
        for j in 0..3 {
            let l = a[i].cross(b[j]);
            let len = l.length();
            if len < 1e-4 {
                continue;
            }
            let l = l / len;
            let s = d.dot(l).abs() - radius(&a, ha, l) - radius(&b, hb, l);
            if s > MARGIN {
                return None;
            }
            // Prefer faces: they give a whole patch of points.
            if s > face_best + 0.02 && s > best.0 {
                best = (s, l, 6 + i * 3 + j);
            }
        }
    }
    let (s, mut n, kind) = best;
    if d.dot(n) < 0.0 {
        n = -n;
    }
    let depth = -s;
    if kind >= 6 {
        // Edge against edge: one point between the two closest edges.
        let (i, j) = ((kind - 6) / 3, (kind - 6) % 3);
        let edge = |iso: &Iso, axes: &[Vec3; 3], h: Vec3, k: usize, toward: Vec3| {
            let mut c = iso.position;
            for m in 0..3 {
                if m != k {
                    c += axes[m] * h[m] * axes[m].dot(toward).signum();
                }
            }
            (c - axes[k] * h[k], c + axes[k] * h[k])
        };
        let (pa, qa) = edge(ia, &a, ha, i, n);
        let (pb, qb) = edge(ib, &b, hb, j, -n);
        let (x, y) = closest_segments(pa, qa, pb, qb);
        return Some(Manifold { normal: n, points: vec![((x + y) * 0.5, depth)] });
    }
    // Face: clip the other box's most opposed face against this one.
    let (reference, ref_axes, ref_half, incident, inc_axes, inc_half, normal, flipped) = if kind < 3 {
        (ia, a, ha, ib, b, hb, n, false)
    } else {
        (ib, b, hb, ia, a, ha, -n, true)
    };
    let k = kind % 3;
    let face_center = reference.position + normal * ref_half[k];
    let (u, v) = ((k + 1) % 3, (k + 2) % 3);
    // The incident face.
    let (mut ik, mut best_dot) = (0, 0.0);
    for (m, axis) in inc_axes.iter().enumerate() {
        let dd = axis.dot(normal).abs();
        if dd > best_dot {
            best_dot = dd;
            ik = m;
        }
    }
    let inc_normal = inc_axes[ik] * -inc_axes[ik].dot(normal).signum();
    let inc_center = incident.position + inc_normal * inc_half[ik];
    let (iu, iv) = ((ik + 1) % 3, (ik + 2) % 3);
    let (eu, ev) = (inc_axes[iu] * inc_half[iu], inc_axes[iv] * inc_half[iv]);
    let mut poly = vec![inc_center + eu + ev, inc_center - eu + ev, inc_center - eu - ev, inc_center + eu - ev];
    for (axis, extent) in [(ref_axes[u], ref_half[u]), (ref_axes[v], ref_half[v])] {
        for sign in [1.0f32, -1.0] {
            let plane_n = axis * sign;
            let offset = plane_n.dot(face_center) + extent;
            poly = clip(&poly, plane_n, offset);
        }
    }
    let mut points: Vec<(Vec3, f32)> = poly
        .into_iter()
        .filter_map(|p| {
            let depth = normal.dot(face_center) - normal.dot(p);
            (depth >= -MARGIN).then_some((p + normal * depth * 0.5, depth))
        })
        .collect();
    reduce(&mut points);
    let n = if flipped { -normal } else { normal };
    Some(Manifold { normal: n, points })
}

/// Clips a polygon to the half space `n · p <= offset` (Sutherland-Hodgman).
fn clip(poly: &[Vec3], n: Vec3, offset: f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (n.dot(a) - offset, n.dot(b) - offset);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// At most four points: the deepest, and those spreading the patch widest.
fn reduce(points: &mut Vec<(Vec3, f32)>) {
    if points.len() <= 4 {
        return;
    }
    let mut keep = Vec::with_capacity(4);
    let deepest = (0..points.len()).max_by(|&a, &b| points[a].1.total_cmp(&points[b].1)).unwrap();
    keep.push(points[deepest]);
    while keep.len() < 4 {
        let next = points
            .iter()
            .copied()
            .max_by(|x, y| {
                let dx = keep.iter().map(|k: &(Vec3, f32)| k.0.distance_squared(x.0)).fold(f32::MAX, f32::min);
                let dy = keep.iter().map(|k: &(Vec3, f32)| k.0.distance_squared(y.0)).fold(f32::MAX, f32::min);
                dx.total_cmp(&dy)
            })
            .unwrap();
        keep.push(next);
    }
    *points = keep;
}

/// A shape resting on the plane through `point` facing `n`: normal from the shape to the plane.
fn against_plane(shape: &Shape, iso: &Iso, n: Vec3, point: Vec3) -> Option<Manifold> {
    let height = |p: Vec3| (p - point).dot(n);
    let mut points = Vec::new();
    match shape {
        Shape::Sphere { radius } => points.push((iso.position - n * *radius, radius - height(iso.position))),
        Shape::Capsule { half_height, radius } => {
            let (p, q) = segment(iso, *half_height);
            for e in [p, q] {
                points.push((e - n * *radius, radius - height(e)));
            }
        }
        Shape::Cuboid { half } => {
            for i in 0..8 {
                let c = Vec3::new(
                    if i & 1 == 0 { -half.x } else { half.x },
                    if i & 2 == 0 { -half.y } else { half.y },
                    if i & 4 == 0 { -half.z } else { half.z },
                );
                let p = iso.transform_point(c);
                points.push((p, -height(p)));
            }
        }
        _ => {}
    }
    points.retain(|p| p.1 >= -MARGIN);
    reduce(&mut points);
    Some(Manifold { normal: -n, points })
}

/// The closest point on a triangle (Ericson 5.1.5).
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

fn against_mesh(shape: &Shape, iso: &Iso, mesh: &TriMesh, mesh_iso: &Iso, out: &mut Vec<Manifold>) {
    // Work in the mesh's space.
    let local = mesh_iso.inverse_transform_point(iso.position);
    let local_rot = mesh_iso.rotation.inverse() * iso.rotation;
    let liso = Iso::new(local, local_rot);
    let bounds: Aabb = shape.aabb(&liso).expand(MARGIN);
    let mut tris = Vec::new();
    mesh.overlapping(&bounds, &mut tris);
    let to_world = |m: Manifold| Manifold {
        normal: mesh_iso.rotation * m.normal,
        points: m.points.into_iter().map(|(p, d)| (mesh_iso.transform_point(p), d)).collect(),
    };
    let start = out.len();
    for t in tris {
        let [a, b, c] = mesh.triangle(t);
        let m = match shape {
            Shape::Sphere { radius } => sphere_triangle(local, *radius, a, b, c),
            Shape::Capsule { half_height, radius } => {
                let (p, q) = segment(&liso, *half_height);
                // The segment point closest to the triangle, then its two ends.
                let mut best: Option<Manifold> = None;
                for s in [p, q, closest_segment_triangle(p, q, a, b, c)] {
                    if let Some(m) = sphere_triangle(s, *radius, a, b, c) {
                        match &mut best {
                            Some(bm) if bm.normal.dot(m.normal) > 0.95 && m.points[0].0.distance(bm.points[0].0) > 0.02 => {
                                bm.points.push(m.points[0]);
                            }
                            None => best = Some(m),
                            _ => {}
                        }
                    }
                }
                best
            }
            Shape::Cuboid { half } => box_triangle(&liso, *half, a, b, c),
            _ => None,
        };
        if let Some(m) = m.filter(|m| !m.points.is_empty()) {
            // Merge with a manifold of the same facing (flat ground made of many triangles).
            if let Some(existing) = out[start..].iter_mut().find(|e| e.normal.dot(mesh_iso.rotation * m.normal) > 0.99) {
                let w = to_world(m);
                for p in w.points {
                    if existing.points.iter().all(|e| e.0.distance(p.0) > 0.02) {
                        existing.points.push(p);
                    }
                }
                reduce(&mut existing.points);
            } else {
                out.push(to_world(m));
            }
        }
    }
}

fn closest_segment_triangle(p: Vec3, q: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let dist = |t: f32| {
        let x = p.lerp(q, t);
        x.distance(closest_on_triangle(x, a, b, c))
    };
    for _ in 0..20 {
        let (m1, m2) = (hi - (hi - lo) * 0.618, lo + (hi - lo) * 0.618);
        if dist(m1) < dist(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    p.lerp(q, (lo + hi) * 0.5)
}

/// Normal from the sphere toward the triangle.
fn sphere_triangle(c: Vec3, r: f32, a: Vec3, b: Vec3, t: Vec3) -> Option<Manifold> {
    let closest = closest_on_triangle(c, a, b, t);
    let delta = c - closest;
    let dist = delta.length();
    let depth = r - dist;
    if depth < -MARGIN {
        return None;
    }
    let face = (b - a).cross(t - a).normalize_or(Vec3::Y);
    let out = if dist > 1e-6 { delta / dist } else if face.dot(c - a) >= 0.0 { face } else { -face };
    Some(Manifold { normal: -out, points: vec![(closest, depth)] })
}

/// A box against a triangle by separating axes; normal from the box toward the triangle.
fn box_triangle(iso: &Iso, half: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<Manifold> {
    let axes = box_axes(iso);
    let tri = [a, b, c];
    let face = (b - a).cross(c - a).normalize_or(Vec3::Y);
    let mut candidates: Vec<Vec3> = vec![face, axes[0], axes[1], axes[2]];
    for e in [b - a, c - b, a - c] {
        for ax in axes {
            let l = e.cross(ax);
            if l.length_squared() > 1e-8 {
                candidates.push(l.normalize());
            }
        }
    }
    let mut best = (f32::MIN, Vec3::ZERO, 0usize);
    for (i, l) in candidates.iter().enumerate() {
        let r = axes[0].dot(*l).abs() * half.x + axes[1].dot(*l).abs() * half.y + axes[2].dot(*l).abs() * half.z;
        let proj: Vec<f32> = tri.iter().map(|p| (*p - iso.position).dot(*l)).collect();
        let (lo, hi) = (proj.iter().cloned().fold(f32::MAX, f32::min), proj.iter().cloned().fold(f32::MIN, f32::max));
        let s = (lo - r).max(-r - hi);
        if s > MARGIN {
            return None;
        }
        // Prefer the triangle's face (only when it holds the box, pick others).
        let s = if i == 0 { s + 0.005 } else { s };
        if s > best.0 {
            best = (s, *l, i);
        }
    }
    let (s, mut n, kind) = best;
    let centroid = (a + b + c) / 3.0;
    if (centroid - iso.position).dot(n) < 0.0 {
        n = -n;
    }
    if kind == 0 {
        // The box resting on the triangle's face: its corners under the face and over the
        // triangle.
        let mut points = Vec::new();
        for i in 0..8 {
            let corner = iso.transform_point(Vec3::new(
                if i & 1 == 0 { -half.x } else { half.x },
                if i & 2 == 0 { -half.y } else { half.y },
                if i & 4 == 0 { -half.z } else { half.z },
            ));
            let depth = (corner - a).dot(n);
            if depth >= -MARGIN {
                let on = corner - n * depth;
                if closest_on_triangle(on, a, b, c).distance(on) < 1e-3 {
                    points.push((corner, depth));
                }
            }
        }
        if !points.is_empty() {
            reduce(&mut points);
            return Some(Manifold { normal: n, points });
        }
    }
    // Otherwise the triangle's point that reaches deepest into the box.
    let deepest = tri.iter().copied().min_by(|p, q| p.dot(n).total_cmp(&q.dot(n))).unwrap();
    Some(Manifold { normal: n, points: vec![(deepest, -s)] })
}
