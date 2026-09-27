"""Turns a scanned modern man into French soldiers of 1813, posed and ready to render.

    blender -b --python soldiers.py -- <rocketbox Male_Adult_08.gltf> <horse.gltf> <output dir>

For each troop type (fusilier, grenadier, gunner, officer) this repaints the clothing in the
body texture as a uniform, adds a shako, pack, cartridge box, sidearms and musket attached to
the right bones, and bakes a set of poses into <type>.glb: standing at order arms, presenting
(aiming), shouldered arms, and eight frames of a marching step. Each pose is one object whose
name the engine looks up.

The uniform is painted, not modelled: every triangle of the body is rasterised into its texture,
and each texel is coloured by where it lies on the body in the rest pose. That puts lapels,
crossbelts, buttons, cuffs and gaiters on the real creases of the scanned clothing.

The source figure is Microsoft Rocketbox (MIT licence).
"""

import math
import os
import sys
import tempfile

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Vector

src, horse_src, out_dir = sys.argv[-3:]
out_dir = os.path.abspath(out_dir)
os.makedirs(out_dir, exist_ok=True)


def srgb(hex_color):
    return np.array([((hex_color >> s) & 255) / 255.0 for s in (16, 8, 0)])


UNIFORMS = {
    "fusilier": dict(coat=srgb(0x1F2A4E), lapels=srgb(0xDCD8CC), facings=srgb(0xA3232A), breeches=srgb(0xD6D1C3),
                     legs="gaiters", belts=True, plume=srgb(0x2A5A9A), epaulettes=None, pack=True, musket=True),
    "grenadier": dict(coat=srgb(0x1F2A4E), lapels=srgb(0xDCD8CC), facings=srgb(0xA3232A), breeches=srgb(0xD6D1C3),
                      legs="gaiters", belts=True, plume=srgb(0xB01E28), epaulettes=srgb(0xB01E28), pack=True, musket=True),
    "gunner": dict(coat=srgb(0x1C2544), lapels=srgb(0x1C2544), facings=srgb(0xA3232A), breeches=srgb(0x1C2544),
                   legs="gaiters", belts=True, plume=srgb(0xB01E28), epaulettes=srgb(0xB01E28), pack=False, musket=False),
    "officer": dict(coat=srgb(0x1F2A4E), lapels=srgb(0xDCD8CC), facings=srgb(0xA3232A), breeches=srgb(0xD6D1C3),
                    legs="boots", belts=False, plume=srgb(0xC9A040), epaulettes=srgb(0xD4AF55), pack=False, musket=False),
}

# ---- Load ---------------------------------------------------------------------------------

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=src)
arm = next(o for o in bpy.data.objects if o.type == "ARMATURE")
body = next(o for o in bpy.data.objects if o.type == "MESH")
for o in list(bpy.data.objects):
    if o not in (arm, body):
        bpy.data.objects.remove(o)
bpy.context.view_layer.update()
to_arm = arm.matrix_world.inverted()


def bone(name):
    return arm.pose.bones["Bip01 " + name]


def world(p_arm):
    return arm.matrix_world @ p_arm


REST = {pb.name: (world(pb.head).copy(), world(pb.tail).copy(), (arm.matrix_world @ pb.matrix).copy())
        for pb in arm.pose.bones}

# ---- Painting the uniform ------------------------------------------------------------------

body_mat = next(m for m in body.data.materials if m.name == "m014_body")
# The converted figure leaves metalness unset, which glTF reads as fully metallic: cloth and
# skin would shine like chrome. Wool is matt; skin has a soft sheen.
for _m, _rough in (("m014_body", 0.88), ("m014_head", 0.55), ("m014_opacity", 0.7)):
    _bsdf = bpy.data.materials[_m].node_tree.nodes.get("Principled BSDF")
    if _bsdf:
        _bsdf.inputs["Metallic"].default_value = 0.0
        _bsdf.inputs["Roughness"].default_value = _rough
base_node = next(n for n in body_mat.node_tree.nodes
                 if n.type == "TEX_IMAGE" and n.outputs[0].links and n.outputs[0].links[0].to_socket.name == "Base Color")
source = base_node.image
W, H = source.size
original = np.array(source.pixels[:], dtype=np.float32).reshape(H, W, 4)[:, :, :3]

lum = original @ np.array([0.2126, 0.7152, 0.0722])
r, g, b = original[..., 0], original[..., 1], original[..., 2]
SHIRT, JEANS, SKIN, OTHER = 1, 2, 3, 0
cls = np.zeros((H, W), np.int8)
cls[(b > r + 0.06) & (lum > 0.4)] = SHIRT
cls[(b > r + 0.02) & (lum < 0.3) & (lum > 0.02)] = JEANS
_v, _u = np.mgrid[0:H, 0:W]
# Everything on the trouser islands that isn't skin is trousers, whatever its colour.
cls[((_u < 0.29 * W) | (_u > 0.68 * W)) & (_v > 0.58 * H) & (cls != SKIN)] = JEANS
cls[(r > b + 0.08) & (r > 0.35)] = SKIN
means = {c: max(lum[cls == c].mean(), 1e-3) for c in (SHIRT, JEANS, SKIN)}

me = body.data
me.calc_loop_triangles()
uv_layer = me.uv_layers.active.data
mw = body.matrix_world
nw = mw.to_3x3().inverted().transposed()
body_index = list(me.materials).index(body_mat)
verts = np.array([tuple(mw @ v.co) for v in me.vertices])
normals = np.array([tuple((nw @ v.normal).normalized()) for v in me.vertices])


def segment_distance(p, a, b):
    """Distance from each point to segment ab (numpy, rows of p)."""
    a, b = np.asarray(a), np.asarray(b)
    ab = b - a
    t = np.clip(((p - a) @ ab) / (ab @ ab), 0.0, 1.0)
    return np.linalg.norm(p - (a + t[:, None] * ab), axis=1), t


# Landmarks in the rest pose (metres; the figure faces -Y, its right is -X).
ELBOW = {s: np.array(tuple(REST[f"Bip01 {n} Forearm"][0])) for s, n in ((1, "L"), (-1, "R"))}
WRIST = {s: np.array(tuple(REST[f"Bip01 {n} Hand"][0])) for s, n in ((1, "L"), (-1, "R"))}


def paint(u):
    tex = original.copy()
    # First recolour every shirt and jeans texel, including the padding round each UV island
    # that no triangle covers, so no modern blue bleeds into distant mipmaps.
    shirt = cls == SHIRT
    tex[shirt] = u["coat"] * (np.clip(lum[shirt] / means[SHIRT], 0.5, 1.5) ** 1.8)[:, None]
    v, uu = np.mgrid[0:H, 0:W]
    leg_islands = ((uu < 0.29 * W) | (uu > 0.68 * W)) & (v > 0.58 * H) & (cls != SKIN)
    tex[leg_islands] = u["breeches"] * (np.clip(lum[leg_islands] / means[JEANS], 0.4, 1.8) ** 0.7)[:, None]

    def rule(P, N, texels):
        x, y, z = P[:, 0], P[:, 1], P[:, 2]
        ax = np.abs(x)
        c = cls[texels]
        base = original[texels]
        out = base.copy()
        s_shirt = np.clip(lum[texels] / means[SHIRT], 0.5, 1.5)[:, None] ** 1.8
        s_jeans = np.clip(lum[texels] / means[JEANS], 0.4, 1.8)[:, None] ** 0.7
        coat = (c == SHIRT)
        # Sleeves down to the wrist over the bare forearms, with a coloured cuff.
        side = np.sign(x)
        arm_t = np.zeros_like(x)
        arm_d = np.full_like(x, 9.0)
        for s in (1, -1):
            k = side == s
            d, t = segment_distance(P[k], ELBOW[s], WRIST[s])
            arm_d[k], arm_t[k] = d, t
        on_forearm = (arm_d < 0.12) & (ax > 0.28)
        sleeve = on_forearm & (arm_t < 0.93) & (c == SKIN)
        coat |= sleeve
        out[coat] = u["coat"] * np.where(sleeve[coat, None], 0.95, s_shirt[coat])
        cuff = coat & on_forearm & (arm_t > 0.72) & (arm_t < 0.93)
        out[cuff] = u["facings"] * np.where(sleeve[cuff, None], 1.0, s_shirt[cuff])
        # Collar.
        collar = coat & (z > 1.47) & (np.hypot(x, y - 0.02) < 0.085)
        out[collar] = u["facings"] * s_shirt[collar]
        # The plastron of lapels, closed to the waist and piped in the facing colour.
        front = (N[:, 1] < -0.25) & (y < 0.0)
        lapel = (c == SHIRT) & front & (ax < 0.105) & (z > 1.02) & (z < 1.45)
        out[lapel] = u["lapels"] * s_shirt[lapel]
        piping = lapel & (ax > 0.096)
        out[piping] = u["facings"] * s_shirt[piping]
        # Two rows of brass buttons.
        for bz in np.arange(1.07, 1.44, 0.055):
            for bx in (-0.075, 0.075):
                button = lapel & ((x - bx) ** 2 + (z - bz) ** 2 < 0.0085 ** 2)
                out[button] = srgb(0xC9A040) * (0.8 + 0.4 * np.clip((z[button] - bz) / 0.008 + 0.5, 0, 1))[:, None]
        # White buff crossbelts, front and back.
        if u["belts"]:
            belt = np.zeros_like(coat)
            for sx in (1, -1):
                d, _ = segment_distance(np.stack([x, z], 1), (sx * 0.17, 1.46), (-sx * 0.15, 0.92))
                belt |= (d < 0.027) & ((c == SHIRT) | lapel) & (ax < 0.22)
            edge = belt & (np.abs(d) > 0.02)
            out[belt] = srgb(0xE6E1D2) * s_shirt[belt]
            out[belt & edge] *= 0.85
        # Breeches, and below the knee black gaiters buttoned up the outside.
        legs = c == JEANS
        out[legs] = u["breeches"] * s_jeans[legs]
        if u["legs"] == "gaiters":
            gaiter = legs & (z < 0.52)
            out[gaiter] = srgb(0x1A1A1E) * s_jeans[gaiter]
            outer = gaiter & (x * N[:, 0] > 0.0) & (np.abs(N[:, 0]) > 0.55)
            for bz in np.arange(0.14, 0.5, 0.045):
                bt = outer & (np.abs(z - bz) < 0.006)
                out[bt] = srgb(0x2E2A26)
        else:
            boot = legs & (z < 0.56)
            out[boot] = srgb(0x121214) * (0.7 + 0.6 * s_jeans[boot])
        return out

    # Rasterise every body triangle into the texture.
    for tri in me.loop_triangles:
        if tri.material_index != body_index:
            continue
        uv = np.array([tuple(uv_layer[l].uv) for l in tri.loops]) * [W, H]
        P3 = verts[list(tri.vertices)]
        N3 = normals[list(tri.vertices)]
        lo = np.floor(uv.min(0)).astype(int)
        hi = np.ceil(uv.max(0)).astype(int)
        lo = np.clip(lo - 1, 0, [W - 1, H - 1])
        hi = np.clip(hi + 1, 0, [W - 1, H - 1])
        xs, ys = np.meshgrid(np.arange(lo[0], hi[0] + 1), np.arange(lo[1], hi[1] + 1))
        px = np.stack([xs.ravel() + 0.5, ys.ravel() + 0.5], 1)
        a, b_, c_ = uv
        v0, v1 = b_ - a, c_ - a
        den = v0[0] * v1[1] - v1[0] * v0[1]
        if abs(den) < 1e-9:
            continue
        d = px - a
        w1 = (d[:, 0] * v1[1] - v1[0] * d[:, 1]) / den
        w2 = (v0[0] * d[:, 1] - d[:, 0] * v0[1]) / den
        w0 = 1 - w1 - w2
        # A little outside the triangle too, so seams don't show the old colour.
        inside = (w0 > -0.08) & (w1 > -0.08) & (w2 > -0.08)
        if not inside.any():
            continue
        wts = np.stack([w0, w1, w2], 1)[inside]
        P = wts @ P3
        N = wts @ N3
        tx = xs.ravel()[inside]
        ty = ys.ravel()[inside]
        tex[ty, tx] = rule(P, N, (ty, tx))
    return tex


def save_texture(name, tex):
    img = bpy.data.images.new(name, W, H, alpha=False)
    rgba = np.concatenate([tex, np.ones((H, W, 1), np.float32)], 2)
    img.pixels = rgba.ravel()
    # Only an intermediate: the export embeds it.
    path = os.path.join(tempfile.gettempdir(), f"{name}.png")
    img.filepath_raw = path
    img.file_format = "PNG"
    img.save()
    return img


# ---- Posing ---------------------------------------------------------------------------------

def reset_pose():
    for pb in arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    bpy.context.view_layer.update()


def aim(pb, direction_world):
    """Turns a bone about its head so it points along a world direction."""
    head, tail = world(pb.head), world(pb.tail)
    q = (tail - head).normalized().rotation_difference(direction_world.normalized())
    R = (arm.matrix_world.inverted().to_3x3() @ q.to_matrix() @ arm.matrix_world.to_3x3()).to_4x4()
    h = pb.head.copy()
    pb.matrix = Matrix.Translation(h) @ R @ Matrix.Translation(-h) @ pb.matrix
    bpy.context.view_layer.update()


def turn(pb, axis_world, angle):
    head = world(pb.head)
    R = (arm.matrix_world.inverted().to_3x3() @ Matrix.Rotation(angle, 3, axis_world) @ arm.matrix_world.to_3x3()).to_4x4()
    h = pb.head.copy()
    pb.matrix = Matrix.Translation(h) @ R @ Matrix.Translation(-h) @ pb.matrix
    bpy.context.view_layer.update()


def reach(upper, lower, end, target, pole):
    """Two-bone IK: bends upper and lower so the end bone's head reaches target, the middle
    joint pushed toward pole."""
    S = world(upper.head)
    L1 = (world(upper.tail) - S).length
    L2 = (world(end.head) - world(lower.head)).length
    d = target - S
    dist = min(max(d.length, 1e-3), (L1 + L2) * 0.999)
    dn = d.normalized()
    cos_a = (L1 * L1 + dist * dist - L2 * L2) / (2 * L1 * dist)
    sin_a = math.sqrt(max(0.0, 1 - cos_a * cos_a))
    p = (pole - dn * pole.dot(dn)).normalized()
    E = S + dn * L1 * cos_a + p * L1 * sin_a
    aim(upper, E - S)
    aim(lower, (S + dn * dist) - world(lower.head))


def arm_to(side, target, pole):
    reach(bone(f"{side} UpperArm"), bone(f"{side} Forearm"), bone(f"{side} Hand"), target, pole)


def leg_to(side, target, pole=Vector((0, -1, 0))):
    reach(bone(f"{side} Thigh"), bone(f"{side} Calf"), bone(f"{side} Foot"), target, pole)
    # Keep the foot level.
    foot = bone(f"{side} Foot")
    rest_dir = REST[foot.name][1] - REST[foot.name][0]
    aim(foot, rest_dir)


BACK = Vector((0, 1, 0))
DOWN = Vector((0, 0, -1))
ANKLE_Z = REST["Bip01 L Calf"][1].z


def stand():
    for side, x in (("L", 0.1), ("R", -0.1)):
        leg_to(side, Vector((x, 0.01, ANKLE_Z)))


def pose_order():
    stand()
    arm_to("R", Vector((-0.27, -0.06, 0.93)), Vector((-0.6, 1.0, 0)))
    arm_to("L", Vector((0.24, 0.02, 0.86)), Vector((0.4, 1.0, 0)))
    return musket_at(Vector((-0.3, -0.07, 0.0)), Vector((0, 0, 1)))


def pose_present():
    # Right foot drawn back, weight forward, cheek to the stock.
    leg_to("L", Vector((0.12, -0.12, ANKLE_Z)))
    leg_to("R", Vector((-0.16, 0.2, ANKLE_Z)))
    turn(bone("Spine1"), Vector((0, 0, 1)), 0.15)
    turn(bone("Head"), Vector((1, 0, 0)), -0.12)
    butt = Vector((-0.14, 0.04, 1.39))
    muzzle_dir = Vector((0.03, -1.0, 0.02)).normalized()
    arm_to("R", butt + muzzle_dir * 0.2 + Vector((0, 0, -0.04)), Vector((-1, 0.2, -0.6)))
    arm_to("L", butt + muzzle_dir * 0.55 + Vector((0.02, 0, -0.04)), Vector((0.5, 0, -1)))
    return musket_at(butt, muzzle_dir)


def pose_shouldered(step=None):
    if step is None:
        stand()
        swing = 0.0
    else:
        swing = march_legs(step)
    butt = Vector((0.2, -0.12, 0.95))
    arm_to("L", butt + Vector((-0.01, 0.01, 0.03)), Vector((1, 0.6, 0)))
    arm_to("R", Vector((-0.25, -0.02 - swing * 0.2, 0.86 + abs(swing) * 0.06)), Vector((-0.3, 1, 0)))
    return musket_at(butt, Vector((0.03, 0.2, 1.0)).normalized())


def march_legs(step):
    """Eight frames of a quick-march stride (0.65 m); returns the arm swing."""
    phase = step / 8.0 * math.tau
    stride = 0.33
    pelvis = bone("Pelvis")
    bob = -0.02 * abs(math.cos(phase))
    pelvis.location = (0, 0, 0)
    turn(pelvis, Vector((0, 0, 1)), 0.05 * math.sin(phase))
    for side, x, offset in (("L", 0.1, 0.0), ("R", -0.1, math.pi)):
        p = phase + offset
        forward = stride * math.sin(p)
        # The swinging foot lifts on the way forward.
        lift = 0.09 * max(0.0, math.cos(p)) ** 2
        leg_to(side, Vector((x, -forward, ANKLE_Z + lift - bob)))
    return math.sin(phase)


# ---- Equipment --------------------------------------------------------------------------------

def material(name, color, roughness=0.8, metallic=0.0):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*[float(v) ** 2.2 for v in color], 1)
    bsdf.inputs["Roughness"].default_value = roughness
    bsdf.inputs["Metallic"].default_value = metallic
    return m


def mesh_object(name, bm, mat):
    me = bpy.data.meshes.new(name)
    bm.to_mesh(me)
    bm.free()
    for poly in me.polygons:
        poly.use_smooth = True
    me.materials.append(mat)
    o = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(o)
    return o


def soften(bm, width):
    """Rounds every hard edge a little. Nothing made by hand has razor edges, and the rounding
    catches the light the way real leather, wood and metal do."""
    hard = [e for e in bm.edges if e.calc_face_angle(0.0) > 0.6]
    if hard and width > 0.0:
        bmesh.ops.bevel(bm, geom=hard, offset=width, segments=2, profile=0.5, affect="EDGES", clamp_overlap=True)


def cylinder(r1, r2, depth, segs, matrix, mat, name="part", bevel=None):
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=segs, radius1=r1, radius2=r2, depth=depth, matrix=matrix)
    soften(bm, bevel if bevel is not None else min(r1, r2, depth) * 0.15)
    return mesh_object(name, bm, mat)


def box(size, matrix, mat, name="part", bevel=None):
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0, matrix=matrix @ Matrix.Diagonal((*size, 1)))
    soften(bm, bevel if bevel is not None else min(size) * 0.2)
    return mesh_object(name, bm, mat)


def profile(points, width, frame, mat, name="part"):
    """A solid cut to a side profile: `points` are (along, up) pairs in the plane of the
    frame's Z (along) and Y (up) axes, extruded `width` across its X axis."""
    bm = bmesh.new()
    front = [bm.verts.new(frame @ Vector((width / 2, u, a))) for a, u in points]
    back = [bm.verts.new(frame @ Vector((-width / 2, u, a))) for a, u in points]
    bm.faces.new(front)
    bm.faces.new(list(reversed(back)))
    n = len(points)
    for i in range(n):
        j = (i + 1) % n
        bm.faces.new([front[j], front[i], back[i], back[j]])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    soften(bm, width * 0.25)
    return mesh_object(name, bm, mat)


def sphere(radius, matrix, mat, name="part"):
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=12, v_segments=8, radius=radius, matrix=matrix)
    return mesh_object(name, bm, mat)


def along(a, b):
    """A matrix placing a unit Z-axis primitive centred between a and b."""
    d = b - a
    rot = Vector((0, 0, 1)).rotation_difference(d.normalized()).to_matrix().to_4x4()
    return Matrix.Translation((a + b) / 2) @ rot


def musket_at(butt, direction):
    """A Charleville musket, butt at `butt`, lying along `direction`, sling underneath."""
    d = direction.normalized()
    up = Vector((0, 0, 1)) if abs(d.z) < 0.9 else Vector((0, -1, 0))
    side = d.cross(up).normalized()
    top = side.cross(d).normalized()
    wood = material("walnut", srgb(0x4A2E1A), 0.45)
    steel = material("steel", srgb(0x6E7178), 0.45, 1.0)
    brass = material("brass", srgb(0xAE8B3E), 0.42, 1.0)
    buff = material("buff", srgb(0xE3DDCC), 0.8)
    parts = []
    frame = Matrix((side, top, d)).transposed().to_4x4()

    def seg(t0, t1, w, h, offset, mat):
        a = butt + d * t0 + top * offset
        b = butt + d * t1 + top * offset
        m = Matrix.Translation((a + b) / 2) @ frame
        parts.append(box((w, h, t1 - t0), m, mat))

    # The stock in one piece, cut to the Charleville's profile: deep butt with its comb, a slim
    # wrist behind the lock, and the long fore-end under the barrel.
    stock = [
        (0.0, -0.085), (0.0, 0.035), (0.08, 0.036), (0.3, 0.024), (0.36, 0.018), (0.42, 0.016),
        (1.36, 0.012), (1.36, -0.01), (0.9, -0.014), (0.46, -0.02), (0.38, -0.034), (0.3, -0.05),
        (0.12, -0.075),
    ]
    parts.append(profile(stock, 0.042, Matrix.Translation(butt) @ frame, wood))
    parts.append(box((0.044, 0.12, 0.006), Matrix.Translation(butt + top * -0.025 + d * 0.003) @ frame, brass))
    parts.append(cylinder(0.012, 0.011, 1.2, 10, along(butt + d * 0.33 + top * 0.022, butt + d * 1.53 + top * 0.022), steel))
    for t in (0.72, 1.05, 1.32):
        parts.append(cylinder(0.026, 0.026, 0.025, 10, along(butt + d * t, butt + d * (t + 0.025)), brass))
    seg(0.36, 0.52, 0.01, 0.035, 0.015, steel)  # lock
    # The bayonet on its socket, beside the muzzle.
    parts.append(cylinder(0.016, 0.016, 0.08, 8, along(butt + d * 1.44 + top * 0.022, butt + d * 1.52 + top * 0.022), steel))
    a = butt + d * 1.52 + top * 0.04 + side * 0.006
    parts.append(box((0.006, 0.02, 0.44), Matrix.Translation(a + d * 0.22) @ frame, steel))
    # Sling hanging in a shallow curve beneath.
    for i in range(6):
        t0, t1 = 0.25 + i * 0.13, 0.25 + (i + 1) * 0.13
        sag = lambda t: -0.04 - 0.05 * math.sin((t - 0.25) / 0.78 * math.pi)
        p0 = butt + d * t0 + top * sag(t0)
        p1 = butt + d * t1 + top * sag(t1)
        parts.append(box((0.025, 0.004, (p1 - p0).length), along(p0, p1), buff))
    return parts


def rest_equipment(u):
    """Equipment in the rest pose, as (bone it rides on, objects)."""
    felt = material("felt", srgb(0x141416), 0.85)
    leather = material("leather", srgb(0x0E0E10), 0.35)
    brass = material("brass", srgb(0xAE8B3E), 0.42, 1.0)
    plume = material("plume_" + hex(int(u["plume"][0] * 255)), u["plume"], 0.9)
    cowhide = material("cowhide", srgb(0x6B4A2E), 0.75)
    greatcoat = material("greatcoat", srgb(0x8C8374), 0.9)
    buff = material("buff", srgb(0xE3DDCC), 0.8)
    out = []

    head = []
    base_z = 1.745
    steel = material("steel", srgb(0x6E7178), 0.45, 1.0)
    headgear = u.get("headgear", "shako")
    if headgear == "shako":
        head.append(cylinder(0.104, 0.118, 0.19, 24, Matrix.Translation((0, 0.005, base_z + 0.095)), felt, "shako"))
        head.append(cylinder(0.121, 0.121, 0.012, 24, Matrix.Translation((0, 0.005, base_z + 0.192)), leather))
        head.append(cylinder(0.106, 0.106, 0.02, 24, Matrix.Translation((0, 0.005, base_z + 0.012)), leather))
        # Peak, tilted down over the eyes.
        peak = Matrix.Translation((0, -0.1, base_z + 0.005)) @ Matrix.Rotation(0.35, 4, "X")
        head.append(box((0.2, 0.08, 0.008), peak @ Matrix.Translation((0, -0.03, 0)), leather))
        # The lozenge plate with the crowned eagle, curved to the front of the shako.
        plate = Matrix.Translation((0, -0.114, base_z + 0.08)) @ Matrix.Rotation(-0.06, 4, "X") @ Matrix.Rotation(math.pi / 2, 4, "X")
        head.append(cylinder(0.058, 0.058, 0.004, 4, plate @ Matrix.Diagonal((0.8, 1.0, 1.0, 1)), brass, bevel=0.0015))
        head.append(sphere(0.02, plate @ Matrix.Translation((0, 0.004, 0)) @ Matrix.Diagonal((1, 0.3, 1.2, 1)), brass))
        # Chin scales, a row of little brass plates down each cheek.
        for s in (1, -1):
            for i in range(6):
                t = i / 5
                p = Vector((s * (0.1 - t * 0.03), -0.02 - t * 0.04, base_z + 0.02 - t * 0.09))
                head.append(box((0.006, 0.018, 0.016), Matrix.Translation(p) @ Matrix.Rotation(-s * 0.3, 4, "Z"), brass, bevel=0.002))
        # White cords festooned across the front.
        cord = material("cord", srgb(0xE8E4D8), 0.9)
        for i in range(10):
            a0, a1 = i / 10 * math.pi, (i + 1) / 10 * math.pi
            p0 = Vector((math.cos(a0) * 0.115, -0.1 - math.sin(a0) * 0.022, base_z + 0.05 - math.sin(a0) * 0.035))
            p1 = Vector((math.cos(a1) * 0.115, -0.1 - math.sin(a1) * 0.022, base_z + 0.05 - math.sin(a1) * 0.035))
            head.append(cylinder(0.005, 0.005, (p1 - p0).length, 6, along(p0, p1), cord, bevel=0.0))
        head.append(sphere(0.022, Matrix.Translation((0, -0.117, base_z + 0.165)), material("cockade", srgb(0x2A3D8F), 0.6)))
        head.append(sphere(0.034, Matrix.Translation((0, -0.1, base_z + 0.225)) @ Matrix.Diagonal((1, 1, 1.3, 1)), plume))
    elif headgear == "helmet":
        # Cuirassier's helmet: steel skull, brass crest, black horsehair mane, fur turban.
        head.append(sphere(0.112, Matrix.Translation((0, 0.005, base_z + 0.02)) @ Matrix.Diagonal((1, 1.08, 0.95, 1)), steel))
        head.append(cylinder(0.118, 0.118, 0.05, 24, Matrix.Translation((0, 0.005, base_z + 0.0)), material("fur", srgb(0x3A2A1E), 0.95)))
        crest = Matrix.Translation((0, 0.0, base_z + 0.13)) @ Matrix.Rotation(math.pi / 2, 4, "Y")
        head.append(cylinder(0.02, 0.02, 0.22, 10, crest @ Matrix.Rotation(math.pi / 2, 4, "X"), brass))
        mane = material("horsehair", srgb(0x0C0C0C), 0.6)
        for i in range(5):
            a = Vector((0, 0.08 + i * 0.03, base_z + 0.13 - i * 0.02))
            b = Vector((0, 0.16 + i * 0.03, base_z - 0.22 - i * 0.03))
            head.append(box((0.05 - i * 0.006, 0.01, (b - a).length), along(a, b), mane))
        peak = Matrix.Translation((0, -0.11, base_z - 0.01)) @ Matrix.Rotation(0.2, 4, "X")
        head.append(box((0.19, 0.07, 0.008), peak @ Matrix.Translation((0, -0.025, 0)), steel))
    else:
        # A bicorne of black felt, worn sideways ("en bataille") or fore and aft.
        sideways = headgear == "bicorne_side"
        turn = Matrix.Identity(4) if sideways else Matrix.Rotation(math.pi / 2, 4, "Z")
        # An upright half-moon of stiffened felt, its lower half hidden in the crown.
        brim = turn @ Matrix.Translation((0, 0.0, base_z + 0.03)) @ Matrix.Diagonal((0.24, 0.04, 0.13, 1))
        head.append(sphere(1.0, brim, felt, "bicorne"))
        head.append(cylinder(0.105, 0.1, 0.07, 20, Matrix.Translation((0, 0.005, base_z + 0.01)), felt))
        cockade = turn @ Matrix.Translation((-0.1 if sideways else 0.1, -0.042, base_z + 0.1))
        head.append(sphere(0.022, cockade, material("cockade", srgb(0x2A3D8F), 0.6)))
        if not sideways:
            head.append(sphere(1.0, Matrix.Translation((0, 0.0, base_z + 0.165)) @ Matrix.Diagonal((0.015, 0.12, 0.02, 1)), material("plume_white", srgb(0xF0EEE8), 0.9)))
    out.append(("Head", head))

    torso = []
    if u["pack"]:
        torso.append(box((0.33, 0.12, 0.34), Matrix.Translation((0, 0.2, 1.3)), cowhide, "pack"))
        torso.append(cylinder(0.062, 0.062, 0.4, 16, Matrix.Translation((0, 0.2, 1.52)) @ Matrix.Rotation(math.pi / 2, 4, "Y"), greatcoat))
        for x in (-0.12, 0.12):
            torso.append(cylinder(0.066, 0.066, 0.02, 16, Matrix.Translation((x, 0.2, 1.52)) @ Matrix.Rotation(math.pi / 2, 4, "Y"), buff))
    if u["epaulettes"] is not None:
        ep = material("epaulette_" + hex(int(u["epaulettes"][1] * 255)), u["epaulettes"], 0.7)
        for s in (1, -1):
            torso.append(box((0.13, 0.13, 0.025), Matrix.Translation((s * 0.19, 0.02, 1.47)) @ Matrix.Rotation(-s * 0.25, 4, "Y"), ep))
            torso.append(cylinder(0.055, 0.045, 0.06, 12, Matrix.Translation((s * 0.245, 0.02, 1.425)) @ Matrix.Rotation(-s * 1.1, 4, "Y"), ep))
    out.append(("Spine2", torso))

    hips = []
    if u["belts"]:
        hips.append(box((0.22, 0.07, 0.15), Matrix.Translation((-0.1, 0.15, 0.94)), leather, "cartridge_box"))
        hips.append(box((0.08, 0.006, 0.06), Matrix.Translation((-0.1, 0.188, 0.95)), brass))
        # Short sabre and bayonet scabbard on the left hip.
        hips.append(cylinder(0.018, 0.014, 0.62, 8, along(Vector((0.19, 0.06, 0.93)), Vector((0.23, 0.18, 0.35))), leather))
        hips.append(box((0.03, 0.09, 0.03), Matrix.Translation((0.19, 0.04, 0.96)), brass))
    elif u["legs"] == "boots":
        # An officer's sword.
        hips.append(cylinder(0.014, 0.012, 0.85, 8, along(Vector((0.2, 0.0, 0.95)), Vector((0.26, 0.22, 0.15))), leather))
        hips.append(box((0.04, 0.1, 0.02), Matrix.Translation((0.2, -0.03, 0.98)), brass))
    out.append(("Pelvis", hips))
    return out


def attach(equipment):
    """Copies each bone's equipment from the rest pose into the current pose."""
    placed = []
    for bone_name, objects in equipment:
        pb = bone(bone_name)
        delta = (arm.matrix_world @ pb.matrix) @ REST[pb.name][2].inverted()
        for o in objects:
            c = o.copy()
            c.data = o.data.copy()
            c.data.transform(delta)
            bpy.context.scene.collection.objects.link(c)
            placed.append(c)
    return placed


def frozen(obj):
    """A copy of a (possibly deformed) object's current shape, in world space."""
    dg = bpy.context.evaluated_depsgraph_get()
    mesh = bpy.data.meshes.new_from_object(obj.evaluated_get(dg))
    mesh.transform(obj.matrix_world)
    copy = bpy.data.objects.new(obj.name + "_frozen", mesh)
    bpy.context.scene.collection.objects.link(copy)
    return copy


def bake(name, extra, skins=()):
    """Freezes the posed body, anything else riding on the skeleton, and the equipment into
    one object."""
    obj = frozen(body)
    obj.name = name
    parts = [frozen(s) for s in skins]
    bpy.ops.object.select_all(action="DESELECT")
    for o in [obj] + parts + list(extra):
        o.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.join()
    return obj


def shell(name, keep, offset, mat):
    """A second skin over part of the body, pushed out along its normals: a cuirass, a coat
    skirt. It keeps the body's vertex weights, so it bends with every pose."""
    o = body.copy()
    o.data = body.data.copy()
    o.name = name
    bpy.context.scene.collection.objects.link(o)
    bm = bmesh.new()
    bm.from_mesh(o.data)
    doomed = [f for f in bm.faces if not keep((mw @ f.calc_center_median()), f.material_index)]
    bmesh.ops.delete(bm, geom=doomed, context="FACES")
    for v in bm.verts:
        v.co += v.normal * offset
    bm.to_mesh(o.data)
    bm.free()
    o.data.materials.clear()
    o.data.materials.append(mat)
    for poly in o.data.polygons:
        poly.material_index = 0
    return o



# ---- Build every troop type --------------------------------------------------------------------

body_mat.node_tree.nodes  # keep a reference


# ---- The gun crew -----------------------------------------------------------------------------
#
# Five gunners who work each gun. Each is exported skinned with a set of clips the engine plays
# on a schedule timed to the gun (see artillery.rs):
#
#   idle, idle2   standing about, captured from life (Rocketbox motion capture)
#   walk, run     moving between posts, captured from life
#   push          leaning into the wheels or the trail as the gun is run back up
#   and each man's own work: No. 1 "sponge" and "ram", No. 2 "load", No. 3 "thumb" (stopping the
#   vent) and "prime", No. 4 "blow" (on his match) and "fire", No. 5 "hand" (passing up a
#   cartridge from the chest).
#
# The keyed clips ease between poses and carry a little noise in the back, neck and shoulders,
# so no two moments are quite alike.
#
# Gun space: x to the gun's right, y up, z toward the trail; the muzzle is toward -z. These
# must match artillery.rs.

import random as _random

ONLY = os.environ.get("ONLY", "")
FPS = 24
MUZZLE = Vector((0.0, 1.15, -1.95))
VENT = Vector((0.0, 1.34, 0.55))
# Where each man stands at his post (x, z) and the way he faces there.
POSTS = {
    "sponge": ((1.05, -1.75), (-1.0, 0.0)),
    "load": ((-1.05, -1.75), (1.0, 0.0)),
    "vent": ((0.6, 0.45), (-1.0, 0.0)),
    "fire": ((-1.15, 0.9), (1.0, 0.0)),
    "carry": ((-1.5, -1.2), (0.6, -0.8)),
}
PUSH = {"sponge": ((1.25, 0.15), (0.0, -1.0)), "load": ((-1.25, 0.15), (0.0, -1.0)),
        "vent": ((0.35, 2.35), (0.0, -1.0)), "fire": ((-0.35, 2.35), (0.0, -1.0))}
MOCAP = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))),
                     "res", "paris", "sources", "rocketbox_animations")


def to_fig(post, p, push=False):
    """A point in gun space as seen by the man at his post: his forward is -Y, his right -X."""
    spot, facing = (PUSH if push else POSTS)[post]
    rel = (p.x - spot[0], p.z - spot[1])
    fwd = rel[0] * facing[0] + rel[1] * facing[1]
    right = rel[0] * -facing[1] + rel[1] * facing[0]
    return Vector((-right, -fwd, p.y))


def lean(angle):
    for name in ("Spine", "Spine1"):
        turn(bone(name), Vector((1, 0, 0)), angle * 0.5)


def feet(stride=0.0, bend=0.0, spread=0.11):
    for side, x, s in (("L", spread, 1.0), ("R", -spread, -1.0)):
        leg_to(side, Vector((x, -stride * s * 0.5, ANKLE_Z + bend)))


def arms_down():
    arm_to("R", Vector((-0.24, 0.0, 0.88)), Vector((-0.4, 1, 0)))
    arm_to("L", Vector((0.24, 0.0, 0.88)), Vector((0.4, 1, 0)))


def left_down():
    arm_to("L", Vector((0.24, 0.0, 0.88)), Vector((0.4, 1, 0)))


def hold_upright(side="R", base=Vector((0, 0, 0))):
    """Holding a staff upright at his side; `base` follows the hips when they move."""
    x = -0.26 if side == "R" else 0.26
    arm_to(side, base + Vector((x, -0.08, 1.02)), Vector((x * 2, 1, 0)))
    aim(bone(f"{side} Hand"), Vector((0, 0, 1)))


def staff_toward(post, grip_fwd, lead_gap=0.38, low=0.0):
    m = to_fig(post, MUZZLE)
    d = Vector((m.x, m.y, 0.0)).normalized()
    rear = Vector((0.0, 0.0, 1.05 - low)) + d * grip_fwd + Vector((-0.1, 0, 0))
    front = rear + d * lead_gap + Vector((0.08, 0, 0.04))
    arm_to("R", rear, Vector((-0.5, 0.3, -1)))
    arm_to("L", front, Vector((0.5, 0.3, -1)))
    aim(bone("R Hand"), (m - rear).normalized())


def stand():
    feet()
    arms_down()


def retarget(path, name, keep_staff=False):
    """Motion capture onto this skeleton: each bone takes the captured bone's turn from its own
    rest pose (the two skeletons' bones needn't point the same way), the hips follow the
    captured hips scaled to this man's height. With `keep_staff`, the right arm is then set to
    hold the sponge staff upright, following the hips."""
    before_o = {o.as_pointer() for o in bpy.data.objects}
    before_a = {a.as_pointer() for a in bpy.data.actions}
    bpy.ops.import_scene.fbx(filepath=path)
    imported = [o for o in bpy.data.objects if o.as_pointer() not in before_o]
    src = next((o for o in imported if o.type == "ARMATURE"), None)
    if src is None or not src.animation_data or not src.animation_data.action:
        print("no motion in", path)
        return
    start, end = (int(round(v)) for v in src.animation_data.action.frame_range)
    tm, sm = arm.matrix_world, src.matrix_world
    tmr = tm.to_3x3().normalized()
    order = sorted(arm.pose.bones, key=lambda pb: len(pb.bone.parent_recursive))
    shared = {pb.name for pb in order if pb.name in src.pose.bones}
    t_rest = {pb.name: tm @ pb.bone.matrix_local for pb in order}
    s_rest = {n: sm @ src.data.bones[n].matrix_local for n in shared}
    root = "Bip01 Pelvis"
    ratio = t_rest[root].translation.z / max(s_rest[root].translation.z, 1e-3)
    scene = bpy.context.scene
    captured = []
    for f in range(start, end + 1):
        scene.frame_set(f)
        pose = {}
        for n in shared:
            now = sm @ src.pose.bones[n].matrix
            rot = now.to_3x3().normalized() @ s_rest[n].to_3x3().normalized().inverted() @ t_rest[n].to_3x3().normalized()
            pose[n] = (rot, now.translation.copy())
        captured.append(pose)
    for o in imported:
        bpy.data.objects.remove(o, do_unlink=True)
    for a in [a for a in bpy.data.actions if a.as_pointer() not in before_a]:
        bpy.data.actions.remove(a)

    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    arm.animation_data_create()
    arm.animation_data.action = action
    for pb in order:
        pb.rotation_mode = "QUATERNION"
    inv_tm = tm.inverted()
    for i, pose in enumerate(captured):
        placed = {}
        for pb in order:
            n = pb.name
            rest_local = pb.bone.matrix_local
            parent = pb.bone.parent
            rel = (parent.matrix_local.inverted() @ rest_local) if parent else rest_local
            if n in pose:
                rot = (tmr.inverted() @ pose[n][0]).to_4x4()
                if parent is None or n == root:
                    hips = t_rest[root].translation + (pose[n][1] - s_rest[root].translation) * ratio
                    pos = inv_tm @ hips
                else:
                    pos = (placed[parent.name] @ rel).translation
                a = Matrix.Translation(pos) @ rot
            else:
                a = placed[parent.name] @ rel if parent else rest_local
            placed[n] = a
            basis = (rel.inverted() @ placed[parent.name].inverted() @ a) if parent else rest_local.inverted() @ a
            pb.matrix_basis = basis
            pb.keyframe_insert("rotation_quaternion", frame=i)
            pb.keyframe_insert("location", frame=i)
    if keep_staff:
        arm_bones = ["R Clavicle", "R UpperArm", "R Forearm", "R Hand"]
        for i in range(len(captured)):
            scene.frame_set(i)
            pel = world(bone("Pelvis").head)
            rest_pel = REST["Bip01 Pelvis"][0]
            hold_upright(base=Vector((pel.x - rest_pel.x, pel.y - rest_pel.y, pel.z - rest_pel.z)))
            for b in arm_bones:
                if "Bip01 " + b in arm.pose.bones:
                    bone(b).keyframe_insert("rotation_quaternion", frame=i)
                    bone(b).keyframe_insert("location", frame=i)
    arm.animation_data.action = None
    reset_pose()


def keyed(name, keys, noise=0.025):
    """A clip from poses at given times (seconds), eased between, with a little noise."""
    arm.animation_data_create()
    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    arm.animation_data.action = action
    for pb in arm.pose.bones:
        pb.rotation_mode = "QUATERNION"
    for t, pose in sorted(keys, key=lambda k: k[0]):
        reset_pose()
        pose()
        frame = int(round(t * FPS))
        for pb in arm.pose.bones:
            pb.keyframe_insert("rotation_quaternion", frame=frame)
            pb.keyframe_insert("location", frame=frame)
    rng = _random.Random(hash(name) & 0xFFFF)
    for fc in action.fcurves:
        if fc.data_path.endswith("rotation_quaternion") and any(b in fc.data_path for b in ("Spine1", "Spine2", "Neck", "Head", "Clavicle")) and fc.array_index > 0:
            mod = fc.modifiers.new("NOISE")
            mod.scale = 14 + rng.random() * 10
            mod.strength = noise
            mod.phase = rng.random() * 100
    arm.animation_data.action = None
    reset_pose()


def push_clip(post):
    """Two steps, leaning into the wheel (Nos. 1 and 2) or bent to the trail (3 and 4)."""
    def pose(s):
        def f():
            if post in ("sponge", "load"):
                feet(s, 0.03)
                lean(0.35)
                side = 1 if post == "sponge" else -1
                wheel = to_fig(post, Vector((0.8 * side, 1.25, -0.25)), push=True)
                arm_to("L", wheel + Vector((0.06, 0, 0)), Vector((0.5, 0, -1)))
                arm_to("R", wheel + Vector((-0.12, 0.05, -0.06)), Vector((-0.5, 0, -1)))
            else:
                feet(s, 0.1)
                lean(0.75)
                trail = to_fig(post, Vector((0.0, 0.62, 2.2)), push=True)
                arm_to("L", trail + Vector((0.1, 0, 0)), Vector((0.5, 0.5, -1)))
                arm_to("R", trail + Vector((-0.1, 0, 0)), Vector((-0.5, 0.5, -1)))
        return f
    keyed("push", [(0.0, pose(0.3)), (0.45, pose(-0.3)), (0.9, pose(0.3))], noise=0.03)


def work_clips(post):
    m = to_fig(post, MUZZLE)
    v = to_fig(post, VENT)
    if post == "sponge":
        up = lambda: (feet(), hold_upright(), left_down())
        ready = lambda: (feet(0.3), staff_toward(post, 0.25))
        keyed("idle_staff", [(0.0, up), (2.0, lambda: (feet(0.05), lean(0.03), hold_upright(), left_down())), (4.0, up)])
        keyed("sponge", [(0.0, up), (0.4, ready), (0.9, lambda: (feet(0.4), lean(0.12), staff_toward(post, 0.62))),
                         (1.1, lambda: (feet(0.4), lean(0.14), staff_toward(post, 0.6))), (1.3, lambda: (feet(0.4), lean(0.12), staff_toward(post, 0.63))),
                         (1.6, ready), (2.0, up)])
        keyed("ram", [(0.0, up), (0.4, ready), (1.0, lambda: (feet(0.45), lean(0.18), staff_toward(post, 0.7))),
                      (1.3, lambda: (feet(0.4), lean(0.1), staff_toward(post, 0.55))), (1.7, lambda: (feet(0.45), lean(0.22), staff_toward(post, 0.74))),
                      (2.3, ready), (2.8, up)])
    elif post == "load":
        over = lambda: (feet(0.45), lean(0.25), arm_to("R", m + Vector((-0.06, 0.12, 0.0)), Vector((-0.4, 0.3, -1))), arm_to("L", m + Vector((0.06, 0.12, 0.0)), Vector((0.4, 0.3, -1))))
        keyed("load", [(0.0, stand), (0.3, lambda: (feet(), arm_to("R", Vector((-0.15, 0.1, 0.95)), Vector((-0.4, 1, 0))), left_down())),
                       (0.7, lambda: (feet(0.2), arm_to("R", Vector((-0.1, -0.25, 1.2)), Vector((-0.4, 0.5, -1))), arm_to("L", Vector((0.1, -0.25, 1.2)), Vector((0.4, 0.5, -1))))),
                       (1.3, over), (1.6, lambda: (feet(0.45), lean(0.28), arm_to("R", m + Vector((-0.06, 0.08, 0.02)), Vector((-0.4, 0.3, -1))), arm_to("L", m + Vector((0.06, 0.08, 0.02)), Vector((0.4, 0.3, -1))))),
                       (1.9, over), (2.1, stand)])
    elif post == "vent":
        thumb = lambda: (feet(0.2), lean(0.3), arm_to("L", v + Vector((0.0, 0.05, 0.02)), Vector((0.5, 0.3, -1))), arm_to("R", Vector((-0.24, 0.0, 0.88)), Vector((-0.4, 1, 0))))
        prick = lambda up: (feet(0.2), lean(0.3), arm_to("L", v + Vector((0.05, 0.08, 0.02)), Vector((0.5, 0.3, -1))), arm_to("R", v + Vector((-0.04, 0.06, 0.08 + up)), Vector((-0.5, 0.3, -1))))
        keyed("thumb", [(0.0, thumb), (1.0, lambda: (feet(0.22), lean(0.32), arm_to("L", v + Vector((0.0, 0.05, 0.02)), Vector((0.5, 0.3, -1))), arm_to("R", Vector((-0.22, -0.03, 0.9)), Vector((-0.4, 1, 0))))), (2.0, thumb)])
        keyed("prime", [(0.0, thumb), (0.3, lambda: prick(0.0)), (0.6, lambda: prick(0.1)), (0.9, lambda: prick(0.0)), (1.4, lambda: prick(0.05)), (1.9, stand)])
    elif post == "fire":
        def reach(frac):
            feet(0.35 * frac, 0.0)
            lean(-0.05 * frac)
            d = Vector((v.x, v.y, 0.0)).normalized()
            hand = Vector((-0.2, 0.0, 1.1)) + (v + Vector((0, 0, 0.05)) - Vector((-0.2, 0.0, 1.1)) - d * 0.8) * frac
            arm_to("R", hand, Vector((-0.5, 0.5, -1)))
            aim(bone("R Hand"), (v - hand).normalized() if frac > 0.3 else Vector((0, 0, 1)))
            left_down()
        up = lambda: (feet(), hold_upright(), left_down())
        blow = lambda: (feet(), arm_to("R", Vector((-0.12, -0.2, 1.5)), Vector((-0.5, 0.5, -1))), aim(bone("R Hand"), Vector((0.3, -0.3, 1))), left_down())
        keyed("blow", [(0.0, stand), (0.4, up), (0.8, blow), (1.1, blow), (1.4, up)])
        keyed("fire", [(0.0, up), (0.5, lambda: reach(0.5)), (0.9, lambda: reach(1.0)), (1.1, lambda: reach(1.0))])
    elif post == "carry":
        give = lambda: (feet(0.3), lean(0.15), arm_to("R", Vector((-0.08, -0.45, 1.15)), Vector((-0.4, 0.5, -1))), arm_to("L", Vector((0.08, -0.45, 1.15)), Vector((0.4, 0.5, -1))))
        keyed("hand", [(0.0, stand), (0.25, lambda: (feet(), arm_to("L", Vector((0.2, 0.1, 0.95)), Vector((0.4, 1, 0))), arm_to("R", Vector((-0.24, 0.0, 0.88)), Vector((-0.4, 1, 0))))),
                       (0.5, give), (0.65, give), (0.8, stand)])


def bind_parts(objs, bone_name):
    for o in objs:
        o.data.transform(o.matrix_world)
        o.matrix_world = Matrix.Identity(4)
        g = o.vertex_groups.new(name="Bip01 " + bone_name)
        g.add([v.index for v in o.data.vertices], 1.0, "REPLACE")


def staff_prop(length_back, length_front, radius, mat, head=None):
    h, t, _ = REST["Bip01 R Hand"]
    d = (t - h).normalized()
    grip = h + d * 0.07
    parts = [cylinder(radius, radius, length_back + length_front, 10, along(grip - d * length_back, grip + d * length_front), mat, bevel=0.0)]
    if head:
        parts += head(grip + d * length_front, d)
    return parts


if ONLY == "crew":
    u = UNIFORMS["gunner"]
    tex = paint(u)
    img = save_texture("gunner_body", tex)
    mat = body_mat.copy()
    mat.name = "gunner_uniform"
    node = next(n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image == source)
    node.image = img
    body.data.materials[body_index] = mat
    wood = material("staff_wood", srgb(0x4A3322), 0.7)
    sheepskin = material("sponge", srgb(0x2A2622), 0.95)
    iron = material("linstock_iron", srgb(0x1E1C1A), 0.6, 1.0)
    leather = material("satchel", srgb(0x1A120C), 0.6)
    for post in POSTS:
        reset_pose()
        for a in list(bpy.data.actions):
            bpy.data.actions.remove(a)
        staff = post == "sponge"
        retarget(os.path.join(MOCAP, "m_idle_neutral_01.fbx"), "idle", keep_staff=staff)
        retarget(os.path.join(MOCAP, "m_idle_neutral_02.fbx"), "idle2", keep_staff=staff)
        retarget(os.path.join(MOCAP, "m_walk_neutral_01.fbx"), "walk", keep_staff=staff)
        retarget(os.path.join(MOCAP, "m_run_neutral_01.fbx"), "run", keep_staff=staff)
        if post != "carry":
            push_clip(post)
        work_clips(post)
        gear = rest_equipment(u)
        extras = []
        for bone_name, objs in gear:
            bind_parts(objs, bone_name)
            extras += objs
        if post == "sponge":
            props = staff_prop(0.8, 2.0, 0.022, wood, lambda tip, d: [cylinder(0.075, 0.075, 0.28, 16, along(tip - d * 0.28, tip), sheepskin, bevel=0.0)])
            bind_parts(props, "R Hand")
        elif post == "fire":
            glow = bpy.data.materials.new("slow_match")
            glow.use_nodes = True
            glow.node_tree.nodes["Principled BSDF"].inputs["Emission"].default_value = (1.0, 0.35, 0.05, 1)
            glow.node_tree.nodes["Principled BSDF"].inputs["Emission Strength"].default_value = 20.0
            props = staff_prop(0.3, 0.72, 0.016, wood, lambda tip, d: [cylinder(0.02, 0.012, 0.12, 8, along(tip, tip + d * 0.12), iron, bevel=0.0), sphere(0.018, Matrix.Translation(tip + d * 0.13), glow)])
            bind_parts(props, "R Hand")
        elif post == "carry":
            # The leather satchel he brings the cartridges up in, on his left hip.
            props = [box((0.26, 0.1, 0.22), Matrix.Translation((0.2, -0.02, 0.92)), leather)]
            bind_parts(props, "Pelvis")
        else:
            props = []
        extras += props
        dup = body.copy()
        dup.data = body.data.copy()
        bpy.context.scene.collection.objects.link(dup)
        bpy.ops.object.select_all(action="DESELECT")
        for o in [dup] + extras:
            o.select_set(True)
        bpy.context.view_layer.objects.active = dup
        bpy.ops.object.join()
        bpy.ops.object.select_all(action="DESELECT")
        dup.select_set(True)
        arm.select_set(True)
        bpy.context.view_layer.objects.active = arm
        bpy.ops.export_scene.gltf(filepath=os.path.join(out_dir, f"crew_{post}.glb"), export_format="GLB", use_selection=True,
                                  export_yup=True, export_apply=False, export_skins=True, export_animations=True,
                                  export_animation_mode="ACTIONS", export_image_format="JPEG", export_force_sampling=True)
        bpy.data.objects.remove(dup)
        print("BUILT", f"crew_{post}", [a.name for a in bpy.data.actions])
    sys.exit(0)


# ---- Faces ------------------------------------------------------------------------------------
#
# One scanned man would make a battalion of twins. Each variant here repaints his head texture:
# a different complexion, hair colour, sideburns, sometimes a moustache (grenadiers were
# expected to wear one) and a day or two of stubble. The engine picks one per soldier.

head_mat = next(m for m in body.data.materials if m.name == "m014_head")
head_index = list(me.materials).index(head_mat)
head_node = next(n for n in head_mat.node_tree.nodes
                 if n.type == "TEX_IMAGE" and n.outputs[0].links and n.outputs[0].links[0].to_socket.name == "Base Color")
HW, HH = head_node.image.size
head_original = np.array(head_node.image.pixels[:], dtype=np.float32).reshape(HH, HW, 4)[:, :, :3]
head_lum = head_original @ np.array([0.2126, 0.7152, 0.0722])

FACES = [
    dict(tone=(1.0, 1.0, 1.0), hair=0x3B2A1E, moustache=False, stubble=0.2),
    dict(tone=(0.94, 0.88, 0.84), hair=0x1A1512, moustache=True, stubble=0.4),
    dict(tone=(1.04, 0.99, 0.96), hair=0x7A5A38, moustache=False, stubble=0.1),
    dict(tone=(0.86, 0.79, 0.73), hair=0x2A2018, moustache=True, stubble=0.5),
    dict(tone=(1.02, 0.94, 0.9), hair=0x5A3A22, moustache=True, stubble=0.25),
    dict(tone=(0.91, 0.86, 0.81), hair=0x3A2A20, moustache=False, stubble=0.6),
]


def rasterize(tex, width, height, material_index, rule):
    """Calls `rule(P, N, (ys, xs))` for the texels of every triangle of one material, with each
    texel's rest-pose position and normal, and writes back what it returns."""
    for tri in me.loop_triangles:
        if tri.material_index != material_index:
            continue
        uv = np.array([tuple(uv_layer[l].uv) for l in tri.loops]) * [width, height]
        P3 = verts[list(tri.vertices)]
        N3 = normals[list(tri.vertices)]
        lo = np.clip(np.floor(uv.min(0)).astype(int) - 1, 0, [width - 1, height - 1])
        hi = np.clip(np.ceil(uv.max(0)).astype(int) + 1, 0, [width - 1, height - 1])
        xs, ys = np.meshgrid(np.arange(lo[0], hi[0] + 1), np.arange(lo[1], hi[1] + 1))
        px = np.stack([xs.ravel() + 0.5, ys.ravel() + 0.5], 1)
        a, b_, c_ = uv
        v0, v1 = b_ - a, c_ - a
        den = v0[0] * v1[1] - v1[0] * v0[1]
        if abs(den) < 1e-9:
            continue
        d = px - a
        w1 = (d[:, 0] * v1[1] - v1[0] * d[:, 1]) / den
        w2 = (v0[0] * d[:, 1] - d[:, 0] * v0[1]) / den
        w0 = 1 - w1 - w2
        inside = (w0 > -0.05) & (w1 > -0.05) & (w2 > -0.05)
        if not inside.any():
            continue
        wts = np.stack([w0, w1, w2], 1)[inside]
        tx, ty = xs.ravel()[inside], ys.ravel()[inside]
        tex[ty, tx] = rule(wts @ P3, wts @ N3, (ty, tx))


def smooth(v, lo, hi, feather):
    """1 inside [lo, hi], fading to 0 over `feather` either side."""
    def step(e0, e1, x):
        t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
        return t * t * (3 - 2 * t)
    return step(lo - feather, lo + feather, v) * (1.0 - step(hi - feather, hi + feather, v))


# Per-texel randomness for hair strands and stubble, fixed so every build matches.
_rng = np.random.default_rng(7)
strands = _rng.random((HH, HW)).astype(np.float32)
# Vertical streaks: hairs grow downward on the face.
streaks = (strands + np.roll(strands, 1, 0) + np.roll(strands, 2, 0) + np.roll(strands, -1, 0)) / 4


def paint_face(face):
    tex = head_original.copy()
    hair = srgb(face["hair"])
    tone = np.array(face["tone"])
    # The scan's own hair colour, so recolouring keeps every strand of its detail.
    rows = np.arange(HH)[:, None] > HH * 0.6
    scalp_mask = (head_lum < 0.25) & (head_lum > 0.03) & rows
    source_hair = np.maximum(head_original[scalp_mask].mean(0), 1e-3)

    def rule(P, N, texels):
        x, y, z = P[:, 0], P[:, 1], P[:, 2]
        base = head_original[texels]
        lum = head_lum[texels]
        noise = strands[texels]
        streak = streaks[texels]
        out = base * tone
        # The scalp: recolour the scanned hair, keeping its strands.
        scalp = smooth(z, 1.69, 2.0, 0.015) * smooth(lum, 0.0, 0.3, 0.05)
        recoloured = base * (hair / source_hair)
        out = out * (1 - scalp[:, None]) + recoloured * scalp[:, None]
        # Sideburns down in front of the ears, thinning toward the jaw.
        burns = smooth(np.abs(x), 0.06, 0.2, 0.006) * smooth(z, 1.6, 1.7, 0.012) * smooth(y, -0.055, 0.02, 0.01)
        burns *= 0.35 + 0.65 * streak
        out = out * (1 - 0.8 * burns[:, None]) + hair * (0.6 + 0.6 * noise[:, None]) * 0.8 * burns[:, None]
        # Stubble: fine dark speckle over jaw, chin and upper lip.
        jaw = smooth(z, 1.55, 1.64, 0.01) * smooth(y, -0.2, -0.02, 0.02) * face["stubble"]
        speck = (noise > 0.55).astype(np.float32) * jaw * 0.6 + jaw * 0.15
        out = out * (1 - speck[:, None]) + hair * 0.5 * speck[:, None]
        if face["moustache"]:
            # Full over the upper lip, drooping a little at the corners.
            droop = 1.626 - (np.abs(x) / 0.04) ** 2 * 0.006
            lip = smooth(np.abs(x), -1.0, 0.038, 0.006) * smooth(z, droop, 1.643, 0.004) * smooth(y, -1.0, -0.092, 0.006)
            lip *= 0.55 + 0.45 * streak
            out = out * (1 - lip[:, None]) + hair * (0.7 + 0.6 * noise[:, None]) * lip[:, None]
        return out

    rasterize(tex, HW, HH, head_index, rule)
    return tex


os.makedirs(os.path.join(out_dir, "heads"), exist_ok=True)
for i, face in enumerate(FACES):
    tex = paint_face(face)
    img = bpy.data.images.new(f"head_{i}", HW, HH, alpha=False)
    img.pixels = np.concatenate([tex, np.ones((HH, HW, 1), np.float32)], 2).ravel()
    img.filepath_raw = os.path.join(out_dir, "heads", f"head_{i}.jpg")
    img.file_format = "JPEG"
    img.save()
with open(os.path.join(out_dir, "heads", "hair.txt"), "w") as f:
    # One hair colour per head, as sRGB hex, for tinting the eyebrows and lashes to match.
    f.write("\n".join(f"{face['hair']:06x}" for face in FACES) + "\n")
print("BUILT faces", len(FACES))


for kind, u in UNIFORMS.items():
    tex = paint(u)
    img = save_texture(f"{kind}_body", tex)
    mat = body_mat.copy()
    mat.name = f"{kind}_uniform"
    node = next(n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image == source)
    node.image = img
    body.data.materials[body_index] = mat

    # Short coat tails hanging behind, over the seat of the breeches.
    tails = shell("tails", lambda c, m: m == body_index and c.y > 0.03 and 0.66 < c.z < 0.95 and abs(c.x) < 0.19, 0.025, material(f"{kind}_tails", u["coat"], 0.9))
    gear = rest_equipment(u)
    for _, objs in gear:
        for o in objs:
            o.hide_set(True)
            o.hide_render = True
    poses = [("rest", None)]
    if u["musket"]:
        poses += [("order", pose_order), ("present", pose_present), ("shouldered", lambda: pose_shouldered())]
        poses += [(f"march{i}", (lambda i=i: pose_shouldered(i))) for i in range(8)]
    else:
        poses += [("attention", lambda: (stand(), arm_to("R", Vector((-0.25, 0.0, 0.86)), Vector((-0.4, 1, 0))), arm_to("L", Vector((0.25, 0.0, 0.86)), Vector((0.4, 1, 0))), [])[-1])]
    baked = []
    for pose_name, pose in poses:
        reset_pose()
        stand()
        extra = pose() if pose else []
        extra = list(extra) + attach(gear)
        baked.append(bake(pose_name, extra, [tails]))
    bpy.data.objects.remove(tails)
    for _, objs in gear:
        for o in objs:
            bpy.data.objects.remove(o)

    bpy.ops.object.select_all(action="DESELECT")
    for o in baked:
        o.hide_set(False)
        o.select_set(True)
    bpy.context.view_layer.objects.active = baked[0]
    bpy.ops.export_scene.gltf(
        filepath=os.path.join(out_dir, f"{kind}.glb"),
        export_format="GLB",
        use_selection=True,
        export_yup=True,
        export_apply=True,
        export_image_format="JPEG",
    )
    for o in baked:
        bpy.data.objects.remove(o)
    body.data.materials[body_index] = body_mat
    print("BUILT", kind, [p for p, _ in poses])


# ---- Horsemen ---------------------------------------------------------------------------------

RIDERS = {
    "cuirassier": dict(coat=srgb(0x1F2A4E), lapels=srgb(0x1F2A4E), facings=srgb(0xA3232A), breeches=srgb(0xD6D1C3),
                       legs="boots", belts=False, plume=srgb(0xA3232A), epaulettes=srgb(0xA3232A), pack=False,
                       musket=False, headgear="helmet", cuirass=True, skirt=None, horses=["bay", "black", "chestnut"],
                       cloth=srgb(0x1F2A4E), lace=srgb(0xE6E1D2)),
    "emperor": dict(coat=srgb(0x6E6C68), lapels=srgb(0x6E6C68), facings=srgb(0x6E6C68), breeches=srgb(0xD6D1C3),
                    legs="boots", belts=False, plume=srgb(0x6E6C68), epaulettes=None, pack=False, musket=False,
                    headgear="bicorne_side", cuirass=False, skirt=srgb(0x6E6C68), horses=["grey"],
                    cloth=srgb(0x7A1E2A), lace=srgb(0xC9A040)),
    "staff": dict(coat=srgb(0x1F2A4E), lapels=srgb(0x1F2A4E), facings=srgb(0xA3232A), breeches=srgb(0xD6D1C3),
                  legs="boots", belts=False, plume=srgb(0xDCD8CC), epaulettes=srgb(0xD4AF55), pack=False, musket=False,
                  headgear="bicorne", cuirass=False, skirt=None, horses=["chestnut", "bay", "black"],
                  cloth=srgb(0x1F2A4E), lace=srgb(0xC9A040)),
    "driver": dict(coat=srgb(0x5F6468), lapels=srgb(0x5F6468), facings=srgb(0x2A3D8F), breeches=srgb(0xC9C3B2),
                   legs="boots", belts=False, plume=srgb(0x5F6468), epaulettes=None, pack=False, musket=False,
                   headgear="shako", cuirass=False, skirt=None, horses=["bay", "chestnut"],
                   cloth=srgb(0x5F6468), lace=srgb(0xE6E1D2)),
}

before = set(bpy.data.objects)
bpy.ops.import_scene.gltf(filepath=horse_src)
horse_meshes = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
horse_mat = bpy.data.materials["horse_body"]
horse_node = next(n for n in horse_mat.node_tree.nodes
                  if n.type == "TEX_IMAGE" and n.outputs[0].links and n.outputs[0].links[0].to_socket.name == "Base Color")
hw, hh = horse_node.image.size
horse_pixels = np.array(horse_node.image.pixels[:], dtype=np.float32).reshape(hh, hw, 4)[:, :, :3]
horse_lum = horse_pixels @ np.array([0.2126, 0.7152, 0.0722])
white_marks = horse_lum > 0.72
horse_mean = horse_lum[~white_marks].mean()
COATS = {"bay": srgb(0x3A1C0E), "black": srgb(0x161210), "chestnut": srgb(0x62311A), "grey": srgb(0xB9B6AE)}
horse_images = {}


def coat_material(coat):
    if coat not in horse_images:
        tex = horse_pixels.copy()
        shade = np.clip(horse_lum / horse_mean, 0.3, 1.8)[..., None]
        recoloured = COATS[coat] * shade ** (1.3 if coat != "grey" else 0.6)
        # Keep the white socks and blaze.
        tex[~white_marks] = recoloured[~white_marks]
        img = bpy.data.images.new(f"horse_{coat}", hw, hh, alpha=False)
        img.pixels = np.concatenate([tex, np.ones((hh, hw, 1), np.float32)], 2).ravel()
        img.filepath_raw = os.path.join(tempfile.gettempdir(), f"horse_{coat}.png")
        img.file_format = "PNG"
        img.save()
        mat = horse_mat.copy()
        mat.name = f"horse_{coat}"
        node = next(n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image == horse_node.image)
        node.image = img
        horse_images[coat] = mat
    return horse_images[coat]


def horse_copy(coat):
    parts = []
    for o in horse_meshes:
        c = frozen(o)
        for i, m in enumerate(c.data.materials):
            if m and m.name.startswith("horse_body"):
                c.data.materials[i] = coat_material(coat)
        parts.append(c)
    return parts


SEAT = Vector((0.0, 0.08, 1.8))


def saddlery(u, harness=False):
    """Saddle, shabraque, holsters and portmanteau, or a draught collar and traces."""
    leather = material("saddle_leather", srgb(0x3B2616), 0.55)
    cloth = material("cloth_%02x%02x%02x" % tuple(int(v * 255) for v in u["cloth"]), u["cloth"], 0.85)
    lace = material("lace_%02x%02x%02x" % tuple(int(v * 255) for v in u["lace"]), u["lace"], 0.7)
    parts = []
    s = SEAT
    if harness:
        collar = Matrix.Translation((0, -0.78, 1.62)) @ Matrix.Rotation(0.5, 4, "X")
        for i in range(16):
            a0, a1 = i / 16 * math.tau, (i + 1) / 16 * math.tau
            p0 = collar @ Vector((math.cos(a0) * 0.26, 0, math.sin(a0) * 0.36))
            p1 = collar @ Vector((math.cos(a1) * 0.26, 0, math.sin(a1) * 0.36))
            parts.append(cylinder(0.05, 0.05, (p1 - p0).length, 8, along(p0, p1), leather))
        for side in (1, -1):
            a = collar @ Vector((side * 0.26, 0, -0.05))
            b = Vector((side * 0.4, 1.2, 1.3))
            parts.append(box((0.03, 0.01, (b - a).length), along(a, b), leather))
        parts.append(box((0.5, 0.35, 0.06), Matrix.Translation(s + Vector((0, 0.0, 0.0))), leather))
        return parts
    parts.append(box((0.6, 1.0, 0.02), Matrix.Translation(s + Vector((0, 0.05, -0.01))), cloth))
    for side in (1, -1):
        drape = Matrix.Translation(s + Vector((side * 0.33, 0.07, -0.2))) @ Matrix.Rotation(side * 0.12, 4, "Y")
        parts.append(box((0.012, 0.9, 0.38), drape, cloth))
        parts.append(box((0.016, 0.92, 0.035), drape @ Matrix.Translation((0, 0, -0.18)), lace))
    parts.append(box((0.36, 0.52, 0.09), Matrix.Translation(s + Vector((0, 0.02, 0.05))), leather))
    parts.append(box((0.3, 0.08, 0.14), Matrix.Translation(s + Vector((0, -0.24, 0.09))), leather))
    parts.append(box((0.34, 0.08, 0.12), Matrix.Translation(s + Vector((0, 0.28, 0.09))), leather))
    for side in (1, -1):
        parts.append(box((0.14, 0.2, 0.14), Matrix.Translation(s + Vector((side * 0.21, -0.36, 0.05))), cloth))
    roll = Matrix.Translation(s + Vector((0, 0.44, 0.1))) @ Matrix.Rotation(math.pi / 2, 4, "Y")
    parts.append(cylinder(0.085, 0.085, 0.54, 16, roll, cloth))
    for x in (-0.27, 0.27):
        parts.append(cylinder(0.087, 0.087, 0.012, 16, Matrix.Translation(s + Vector((x, 0.44, 0.1))) @ Matrix.Rotation(math.pi / 2, 4, "Y"), cloth))
    return parts


def seated():
    hip = arm.location.z
    for side, x in (("L", 1), ("R", -1)):
        leg_to(side, Vector((x * 0.42, -0.02, hip - 0.72)), pole=Vector((x * 0.8, -1.0, 0.2)))
        arm_to(side, Vector((x * 0.11, -0.36, hip + 0.1)), Vector((x * 0.6, 1.0, -0.5)))
    iron = material("iron", srgb(0x3A3A3C), 0.45, 1.0)
    leather = material("saddle_leather", srgb(0x3B2616), 0.55)
    extra = []
    for side, x in (("L", 1), ("R", -1)):
        foot = world(bone(f"{side} Foot").head)
        top = SEAT + Vector((x * 0.2, 0.05, 0.02))
        extra.append(box((0.03, 0.006, (foot - top).length), along(top, foot), leather))
        extra.append(box((0.12, 0.1, 0.02), Matrix.Translation(foot + Vector((0, -0.03, -0.1))), iron))
        # Reins from the bit to the hand.
        hand = world(bone(f"{side} Hand").head)
        bit = Vector((x * 0.05, -1.36, 1.62))
        extra.append(box((0.015, 0.004, (hand - bit).length), along(bit, hand), leather))
    return extra


for kind, u in RIDERS.items():
    tex = paint(u)
    img = save_texture(f"{kind}_body", tex)
    mat = body_mat.copy()
    mat.name = f"{kind}_uniform"
    node = next(n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image == source)
    node.image = img
    body.data.materials[body_index] = mat
    skins = []
    if u["cuirass"]:
        steel = material("cuirass_steel", srgb(0xA8ADB3), 0.25, 1.0)
        skins.append(shell("cuirass", lambda c, m: m == body_index and 1.03 < c.z < 1.47 and abs(c.x) < 0.2, 0.024, steel))
    if u["skirt"] is not None:
        wool = material("greatcoat_wool", u["skirt"], 0.9)
        skins.append(shell("skirt", lambda c, m: m == body_index and 0.45 < c.z < 1.02 and abs(c.x) < 0.26, 0.035, wool))
    gear = rest_equipment(u)
    for _, objs in gear:
        for o in objs:
            o.hide_set(True)
    baked = []
    for coat in u["horses"]:
        reset_pose()
        rest = arm.location.copy()
        arm.location = (rest.x, SEAT.y - 0.03, SEAT.z + 0.12)
        bpy.context.view_layer.update()
        extra = seated() + attach(gear) + saddlery(u) + horse_copy(coat)
        baked.append(bake(f"mounted_{coat}", extra, skins))
        arm.location = rest
        bpy.context.view_layer.update()
    for o in skins:
        bpy.data.objects.remove(o)
    for _, objs in gear:
        for o in objs:
            bpy.data.objects.remove(o)
    bpy.ops.object.select_all(action="DESELECT")
    for o in baked:
        o.select_set(True)
    bpy.context.view_layer.objects.active = baked[0]
    bpy.ops.export_scene.gltf(filepath=os.path.join(out_dir, f"{kind}.glb"), export_format="GLB",
                              use_selection=True, export_yup=True, export_apply=True, export_image_format="JPEG")
    for o in baked:
        bpy.data.objects.remove(o)
    body.data.materials[body_index] = body_mat
    print("BUILT", kind, u["horses"])

# Team horses in draught harness, for the guns' limbers.
team = []
for coat in ("bay", "chestnut", "black"):
    parts = horse_copy(coat) + saddlery(RIDERS["driver"], harness=True)
    bpy.ops.object.select_all(action="DESELECT")
    for o in parts:
        o.select_set(True)
    bpy.context.view_layer.objects.active = parts[0]
    bpy.ops.object.join()
    parts[0].name = f"team_{coat}"
    team.append(parts[0])
bpy.ops.object.select_all(action="DESELECT")
for o in team:
    o.select_set(True)
bpy.context.view_layer.objects.active = team[0]
bpy.ops.export_scene.gltf(filepath=os.path.join(out_dir, "team.glb"), export_format="GLB",
                          use_selection=True, export_yup=True, export_apply=True, export_image_format="JPEG")
print("BUILT team")
