"""Dresses scanned Rocketbox people for Paris in 1810, poses them, and exports them.

    blender -b --python people.py -- <Rocketbox Avatars/Adults dir> <fabric textures dir> <out dir>

Each figure keeps its scanned head and hands; its body texture is repainted into period
clothing (keeping the scan's folds and shading), and real garments are modelled over it:
a Guard grenadier's greatcoat skirt and bearskin, a high-waisted muslin gown, a tailcoat, top
hat and riding boots. Figures are subdivided once so their silhouettes are smooth.

Microsoft Rocketbox is MIT licensed; the fabrics are Poly Haven scans (CC0).
"""

import math
import os
import sys

import bpy
import numpy as np
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rig  # noqa: E402
from rig import along, box, cylinder, lathe, luminance, material, smooth, sphere, srgb  # noqa: E402

adults, fabrics, out_dir = [os.path.abspath(a) for a in sys.argv[-3:]]
os.makedirs(out_dir, exist_ok=True)
work = os.path.join(out_dir, "_work")
os.makedirs(work, exist_ok=True)


def fabric(name):
    return [os.path.join(fabrics, f"{name}_{k}.jpg") for k in ("diff", "nor_gl", "rough")]


def cloth(name, color, texture="poly_wool_herringbone", roughness=1.0, contrast=0.8):
    diff, nor, rough = fabric(texture)
    tinted = rig.tinted_texture(diff, color, os.path.join(work, f"{name}.jpg"), contrast)
    return material(name, roughness=roughness, textures=(tinted, nor, rough))


def is_skin(orig, p):
    """Skin in the body texture: skin-coloured texels at the hands or the neck (in the rest
    pose, arms out). Colour alone can't tell skin from beige or olive cloth."""
    r, g, b = orig[:, 0], orig[:, 1], orig[:, 2]
    x, z = p[:, 0], p[:, 2]
    tone = (r > g + 0.04) & (r > b + 0.1) & (r > 0.25)
    return tone & ((np.abs(x) > 0.56) | ((z > 1.46) & (np.abs(x) < 0.12)))


def recolour(orig, color, contrast=1.0, reference=None):
    """Recolours texels to `color`, keeping their light and shade relative to `reference`."""
    lum = luminance(orig)
    ref = reference if reference is not None else max(float(lum.mean()), 1e-3)
    return color[None, :] * np.clip((lum / ref) ** contrast, 0.3, 1.8)[:, None]


def pose(fig, walking, feet_apart=0.1, stride=0.6):
    """Standing at ease, or caught mid-stride."""
    if walking:
        fig.walk(stride)
    else:
        fig.stand(feet_apart)
        fig.arms_down()


def named(name, walking):
    return name + ("_walk" if walking else "")


animations = os.path.join(os.path.dirname(adults), "rocketbox_animations")
# Statues don't move: they are frozen in their pose.
STATIC = {"statue", "victory"}


def clips(fig):
    """The walks, idles and conversation gestures each figure carries, by name."""
    s = "f" if "Female" in fig.source else "m"
    stroll = "f_walk_stroll_01" if s == "f" else "m_walk_slow_01"
    files = {
        "walk": f"{s}_walk_neutral_01", "stroll": stroll,
        "idle": f"{s}_idle_neutral_01", "idle2": f"{s}_idle_neutral_02",
        "talk": f"{s}_gestic_talk_neutral_01", "talk2": f"{s}_gestic_talk_neutral_02", "talk3": f"{s}_gestic_talk_relaxed_01",
    }
    if "Child" in fig.source:
        files["run"] = f"{s}_run_neutral_01"
    return {name: os.path.join(animations, f"{file}.fbx") for name, file in files.items()}


def finish(fig, name, extras):
    """Writes the figure: skinned to its skeleton with its clips, or, for a statue, frozen."""
    if name not in STATIC:
        rig.bind(fig, extras)
        rig.add_clips(fig, clips(fig))
        rig.export_skinned(fig, extras, os.path.join(out_dir, f"{name}.glb"))
        print("BUILT", name)
        return
    sub = fig.body.modifiers.new("smooth", "SUBSURF")
    sub.levels = sub.render_levels = 1
    sub.uv_smooth = "PRESERVE_BOUNDARIES"
    obj = rig.bake(fig, name, extras)
    # A stride sinks the hips; bring the feet back down to the ground.
    obj.data.transform(Matrix.Translation((0, 0, -fig.drop)))
    rig.export(obj, os.path.join(out_dir, f"{name}.glb"))
    print("BUILT", name)


def profile(points, width, frame, mat):
    import bmesh
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
    rig.soften(bm, width * 0.25)
    return rig.mesh_object("stock", bm, mat)


def musket(butt, direction):
    """A Charleville musket with bayonet fixed, butt at `butt`."""
    d = direction.normalized()
    up = Vector((0, 0, 1)) if abs(d.z) < 0.9 else Vector((0, -1, 0))
    side = d.cross(up).normalized()
    top = side.cross(d).normalized()
    frame = Matrix((side, top, d)).transposed().to_4x4()
    wood = material("walnut", color=(0.1, 0.045, 0.02), roughness=0.42)
    steel = material("musket_steel", color=(0.42, 0.43, 0.45), roughness=0.32, metallic=1.0)
    brass = material("brass", color=(0.62, 0.45, 0.16), roughness=0.38, metallic=1.0)
    stock = [(0.0, -0.085), (0.0, 0.035), (0.08, 0.036), (0.3, 0.024), (0.36, 0.018), (0.42, 0.016),
             (1.36, 0.012), (1.36, -0.01), (0.9, -0.014), (0.46, -0.02), (0.38, -0.034), (0.3, -0.05),
             (0.12, -0.075)]
    parts = [profile(stock, 0.042, Matrix.Translation(butt) @ frame, wood)]
    # Carried in the right hand.
    hand = lambda: [p.__setitem__("bone", "R Hand") for p in parts]
    parts.append(cylinder(0.012, 0.011, 1.2, 12, along(butt + d * 0.33 + top * 0.022, butt + d * 1.53 + top * 0.022), steel))
    for t in (0.72, 1.05, 1.32):
        parts.append(cylinder(0.025, 0.025, 0.025, 12, along(butt + d * t, butt + d * (t + 0.025)), brass))
    parts.append(box((0.01, 0.035, 0.16), Matrix.Translation(butt + d * 0.44 + top * 0.015) @ frame, steel))
    tip = butt + d * 1.52 + top * 0.04 + side * 0.006
    parts.append(box((0.006, 0.02, 0.44), Matrix.Translation(tip + d * 0.22) @ frame, steel, bevel=0.002))
    hand()
    return parts


# ---- The Guard grenadier ------------------------------------------------------------------------

def fur_texture(path):
    """Strands of black bear fur: dark roots, faintly brown tips, alpha thinning outward."""
    n = 512
    rng = np.random.default_rng(3)
    strands = rng.random((n, n)).astype(np.float32)
    # Stretch the noise into short hairs running up the image.
    for _ in range(3):
        strands = (strands + np.roll(strands, 1, 0) + np.roll(strands, -1, 0)) / 3
    strands = (strands - strands.min()) / (strands.max() - strands.min())
    color = np.stack([0.035 + 0.05 * strands, 0.028 + 0.035 * strands, 0.022 + 0.025 * strands], -1)
    rgba = np.concatenate([color, strands[..., None]], -1)
    image = bpy.data.images.new("fur", n, n, alpha=True)
    image.pixels = rgba.ravel()
    image.filepath_raw = path
    image.file_format = "PNG"
    image.save()
    return path


def bearskin(fig):
    """The Guard grenadier's tall bearskin: a felt core wrapped in shells of fur, a brass plate,
    white cords, and the red plume on the left."""
    # It sits on the brow, above the eyes, fitted round the skull, and rises some 35 cm.
    base = rig.head_top(fig) - 0.115
    center, half = rig.head_section(fig, base)
    rx, ry, cy = half[0] + 0.012, half[1] + 0.012, center[1]
    rings = [(base, rx, ry, cy), (base + 0.08, rx + 0.012, ry + 0.012, cy + 0.004), (base + 0.2, rx + 0.03, ry + 0.022, cy + 0.01),
             (base + 0.29, rx + 0.026, ry + 0.018, cy + 0.016), (base + 0.34, rx - 0.01, ry - 0.012, cy + 0.02),
             (base + 0.365, 0.02, 0.02, cy + 0.02)]
    parts = [lathe("bearskin_core", rings, material("bearskin_core", color=(0.02, 0.016, 0.013), roughness=0.9), segments=40)]
    fur = fur_texture(os.path.join(work, "fur.png"))
    for k in range(7):
        grow = 0.004 * (k + 1)
        shell = [(z, rx + grow, ry + grow, cy) for z, rx, ry, cy in rings]
        mat = material(f"fur_{k}", roughness=0.75, textures=(fur, None, None), alpha_cutoff=0.35 + k * 0.08)
        parts.append(lathe(f"fur_{k}", shell, mat, segments=40, uv_scale=0.12))
    brass = material("brass", color=(0.62, 0.45, 0.16), roughness=0.38, metallic=1.0)
    parts.append(box((0.1, 0.006, 0.12), Matrix.Translation((0, -0.145, base + 0.08)) @ Matrix.Rotation(-0.12, 4, "X"), brass, bevel=0.002))
    cord = material("cord_white", color=(0.8, 0.78, 0.72), roughness=0.9)
    for i in range(12):
        a0, a1 = i / 12 * math.pi, (i + 1) / 12 * math.pi
        p0 = Vector((math.cos(a0) * 0.15, -0.14 - math.sin(a0) * 0.02, base + 0.2 - math.sin(a0) * 0.06))
        p1 = Vector((math.cos(a1) * 0.15, -0.14 - math.sin(a1) * 0.02, base + 0.2 - math.sin(a1) * 0.06))
        parts.append(cylinder(0.006, 0.006, (p1 - p0).length, 6, along(p0, p1), cord, bevel=0.0))
    plume = material("plume_red", color=(0.45, 0.03, 0.03), roughness=0.95)
    parts.append(sphere(1.0, Matrix.Translation((0.14, 0.0, base + 0.3)) @ Matrix.Diagonal((0.025, 0.03, 0.14, 1)), plume))
    return parts


def grenadier():
    fig = rig.Figure(os.path.join(adults, "Male_Adult_15", "Export", "Male_Adult_15.fbx"))
    blue = srgb(0x1B2440)
    white = srgb(0xE4E0D6)
    black = srgb(0x121214)

    def rule(p, n, orig, texels):
        x, y, z = p[:, 0], p[:, 1], p[:, 2]
        out = recolour(orig, blue, 1.1)
        skin = is_skin(orig, p) & (z > 0.3)
        out[skin] = orig[skin]
        # Black gaiters and shoes below the greatcoat.
        low = z < 0.5
        out[low] = recolour(orig[low], black, 1.0)
        # Crossbelts of whitened buff leather, front and back.
        for sx in (1, -1):
            t = np.clip((1.46 - z) / 0.54, 0, 1)
            cx = sx * 0.17 + (-sx * 0.32) * t
            belt = (np.abs(x - cx) < 0.028) & (z > 0.92) & (z < 1.47) & (np.abs(x) < 0.22) & ~skin
            out[belt] = white * np.clip(luminance(orig[belt]) / 0.6, 0.6, 1.2)[:, None]
        # Two rows of brass buttons down the breast.
        front = (n[:, 1] < -0.3) & (y < 0)
        for bz in np.arange(0.98, 1.42, 0.07):
            for bx in (-0.075, 0.075):
                btn = front & ((x - bx) ** 2 + (z - bz) ** 2 < 0.009 ** 2)
                out[btn] = srgb(0xB08A3C)
        return out

    fig.repaint("_body", rule, os.path.join(work, "grenadier_body.png"))
    fig.stand(0.1)
    fig.arms_down(("L",))
    # Order arms: the right hand grips the musket at the hip, butt by the right foot.
    butt = Vector((-0.2, -0.1, 0.0))
    fig.arm_to("R", butt + Vector((0.0, 0.0, 0.93)), Vector((-0.4, 1.0, 0.0)))
    fig.relax_hand("R", 0.9)

    wool = cloth("greatcoat", blue * 0.55, contrast=0.6)
    skirt = [(1.12, 0.17, 0.12, 0.0), (1.0, 0.2, 0.15, 0.0), (0.8, 0.23, 0.17, 0.01), (0.6, 0.26, 0.19, 0.02),
             (0.42, 0.29, 0.21, 0.03)]
    folds = lambda th, t: 1.0 + (0.012 + 0.05 * t) * math.sin(th * 9 + 0.7) + 0.02 * t * math.sin(th * 23)
    extras = [lathe("greatcoat_skirt", skirt, wool, segments=64, folds=folds)]
    epaulette = material("epaulette_red", color=(0.42, 0.03, 0.04), roughness=0.85)
    for s, side in ((1, "L"), (-1, "R")):
        shoulder = fig.world(fig.bone(f"{side} UpperArm").head)
        extras.append(box((0.13, 0.12, 0.02), Matrix.Translation(shoulder + Vector((-s * 0.03, 0.0, 0.04))), epaulette))
        for i in range(14):
            a = i / 14 * math.tau
            p = shoulder + Vector((s * 0.03 + math.cos(a) * 0.055, math.sin(a) * 0.055, 0.02))
            extras.append(cylinder(0.006, 0.006, 0.07, 6, Matrix.Translation(p - Vector((0, 0, 0.035))), epaulette, bevel=0.0))
    extras += bearskin(fig)
    extras += musket(butt, Vector((0.0, 0.0, 1.0)))
    finish(fig, "grenadier", extras)


# ---- The gentleman ------------------------------------------------------------------------------

def top_hat(fig):
    """A gentleman's hat of 1810: a tall crown belling out toward the top, and a brim curled
    up at the sides and dipping front and back. It is fitted to the measured head and hair
    at the brow line, and tipped a little back, as it was worn."""
    import bmesh
    top = rig.head_top(fig)
    brow = top - 0.1
    center, half = rig.head_section(fig, brow)
    rx, ry = half[0] + 0.006, half[1] + 0.006
    cx, cy = center[0], center[1]
    felt = material("hat_felt", color=(0.012, 0.011, 0.011), roughness=0.5)
    band = material("hat_band", color=(0.008, 0.008, 0.008), roughness=0.3)
    parts = [lathe("hat_crown", [(brow, rx, ry, 0.0), (brow + 0.05, rx * 0.97, ry * 0.97, 0.0),
                                 (brow + 0.13, rx * 1.02, ry * 1.02, 0.0), (brow + 0.175, rx * 1.1, ry * 1.1, 0.0),
                                 (brow + 0.178, rx * 0.2, ry * 0.2, 0.0)], felt, segments=48)]
    parts.append(lathe("hat_band", [(brow + 0.003, rx + 0.003, ry + 0.003, 0.0),
                                    (brow + 0.03, rx * 0.975 + 0.003, ry * 0.975 + 0.003, 0.0)], band, segments=48))
    # The brim, as a ring of quads: its outer edge curls up at the sides.
    bm = bmesh.new()
    n = 48
    rings = []
    for k, (grow, lift, drop) in enumerate(((0.0, 0.0, 0.0), (0.03, 0.006, 0.0), (0.055, 0.025, 0.012))):
        row = []
        for i in range(n):
            a = i / n * math.tau
            side = math.cos(a) ** 2
            z = brow + lift * side - drop * (1 - side)
            row.append(bm.verts.new((math.cos(a) * (rx + grow), math.sin(a) * (ry + grow), z)))
        rings.append(row)
    for k in range(2):
        for i in range(n):
            j = (i + 1) % n
            bm.faces.new((rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i]))
    bmesh.ops.solidify(bm, geom=bm.faces[:], thickness=0.004)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    parts.append(rig.mesh_object("hat_brim", bm, felt))
    place = Matrix.Translation((cx, cy, 0.0)) @ Matrix.Translation((0, 0, brow)) @ Matrix.Rotation(-0.08, 4, "X") @ Matrix.Translation((0, 0, -brow))
    for p in parts:
        p.data.transform(place)
    return parts


def gentleman(walking=False):
    fig = rig.Figure(os.path.join(adults, "Male_Adult_02", "Export", "Male_Adult_02.fbx"))
    coat = srgb(0x1E3326)
    waistcoat = srgb(0xE3DDCF)
    breeches = srgb(0xC4AE84)
    boots = srgb(0x121110)
    cuff = srgb(0x9A6A3E)

    def rule(p, n, orig, texels):
        x, y, z = p[:, 0], p[:, 1], p[:, 2]
        out = orig.copy()
        skin = is_skin(orig, p)
        upper = (z > 0.95) & ~skin
        out[upper] = recolour(orig[upper], coat, 0.45)
        # The white waistcoat and shirt showing where the coat is cut away at the front.
        # The waistcoat shows below the coat's square-cut front, and shirt and cravat in the
        # V of the lapels above.
        facing = (n[:, 1] < -0.2) & (y < 0) & ~skin
        vee = (z > 1.3) & (np.abs(x) < (z - 1.3) / 0.17 * 0.07)
        front = facing & (((z > 0.99) & (z < 1.045) & (np.abs(x) < 0.13)) | vee)
        out[front] = recolour(orig[front], waistcoat, 0.4)
        legs = (z <= 0.95) & ~skin
        out[legs] = recolour(orig[legs], breeches, 0.5)
        # Top boots: black to below the knee, with a tan turned-down cuff.
        boot = (z < 0.5) & ~skin
        out[boot] = recolour(orig[boot], boots, 0.8)
        band = boot & (z > 0.44)
        out[band] = recolour(orig[band], cuff, 0.6)
        return out

    fig.repaint("_body", rule, os.path.join(work, "gentleman_body.png"))
    pose(fig, walking, 0.12)
    wool = cloth("tailcoat", coat, contrast=0.5)
    extras = []
    # The coat's tails, from the waist to the back of the knee.
    for s in (1, -1):
        tail = [(1.02, 0.07, 0.03, 0.13), (0.8, 0.075, 0.03, 0.14), (0.58, 0.065, 0.025, 0.13)]
        panel = lathe("tail", tail, wool, segments=16)
        panel.location.x = s * 0.07
        extras.append(panel)
    # A white cravat wound high round the neck.
    neck = fig.world(fig.bone("Neck").head)
    stock = material("cravat", roughness=0.9, textures=fabric("rough_linen"))
    extras.append(lathe("cravat", [(neck.z - 0.03, 0.068, 0.062, neck.y), (neck.z + 0.05, 0.062, 0.058, neck.y)], stock, segments=32))
    extras += top_hat(fig)
    finish(fig, named("gentleman", walking), extras)




# ---- Hats and caps ----------------------------------------------------------------------------

def fitted(fig, below_top):
    """Where a hat meets the head: the height `below_top` under the crown of the head, and the
    head's centre and half-widths there (over the hair)."""
    brow = rig.head_top(fig) - below_top
    center, half = rig.head_section(fig, brow)
    return brow, center, half


def round_hat(fig, name, crown, brim, color, flare=1.06, curl=0.02):
    """A round felt hat of the 1800s: a crown `crown` metres tall, slightly belled, and a brim
    `brim` wide curling up at the sides."""
    import bmesh
    brow, center, half = fitted(fig, 0.1)
    rx, ry = half[0] + 0.006, half[1] + 0.006
    felt = material(f"{name}_felt", color=color, roughness=0.55)
    band = material(f"{name}_band", color=tuple(c * 0.5 for c in color), roughness=0.35)
    parts = [lathe(f"{name}_crown", [(brow, rx, ry, 0.0), (brow + crown * 0.3, rx * 0.98, ry * 0.98, 0.0),
                                     (brow + crown * 0.8, rx * (flare - 0.03), ry * (flare - 0.03), 0.0),
                                     (brow + crown, rx * flare, ry * flare, 0.0), (brow + crown + 0.003, rx * 0.2, ry * 0.2, 0.0)],
                   felt, segments=48)]
    parts.append(lathe(f"{name}_band", [(brow + 0.003, rx + 0.003, ry + 0.003, 0.0),
                                        (brow + 0.028, rx * 0.985 + 0.003, ry * 0.985 + 0.003, 0.0)], band, segments=48))
    bm = bmesh.new()
    n = 48
    rings = []
    for grow, lift, drop in ((0.0, 0.0, 0.0), (brim * 0.55, curl * 0.25, 0.0), (brim, curl, curl * 0.5)):
        row = []
        for i in range(n):
            a = i / n * math.tau
            side = math.cos(a) ** 2
            row.append(bm.verts.new((math.cos(a) * (rx + grow), math.sin(a) * (ry + grow), brow + lift * side - drop * (1 - side))))
        rings.append(row)
    for k in range(2):
        for i in range(n):
            j = (i + 1) % n
            bm.faces.new((rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i]))
    bmesh.ops.solidify(bm, geom=bm.faces[:], thickness=0.004)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    parts.append(rig.mesh_object(f"{name}_brim", bm, felt))
    place = Matrix.Translation((center[0], center[1], 0.0)) @ Matrix.Translation((0, 0, brow)) @ Matrix.Rotation(-0.06, 4, "X") @ Matrix.Translation((0, 0, -brow))
    for p in parts:
        p.data.transform(place)
    return parts


def bicorne(fig):
    """A black felt bicorne worn crosswise: two half-moon flaps rising from the head, meeting
    in points over each ear, with a tricolour cockade."""
    import bmesh
    brow, center, half = fitted(fig, 0.1)
    rx, ry = half[0] + 0.01, half[1] + 0.012
    reach, peak = 0.24, 0.15
    felt = material("bicorne_felt", color=(0.012, 0.011, 0.011), roughness=0.5)
    bm = bmesh.new()
    nx, nt = 33, 8
    for sign in (-1, 1):
        grid = []
        for i in range(nx):
            u = -1 + 2 * i / (nx - 1)
            x = u * reach
            bottom = brow - 0.005 + 0.035 * u * u
            top = max(brow + peak * (1 - u * u) ** 0.7 + 0.03 * u * u, bottom + 0.012)
            inside = max(1 - (x / (rx + 0.02)) ** 2, 0.0)
            width = (ry + 0.004) * inside ** 0.5
            row = []
            for k in range(nt):
                t = k / (nt - 1)
                w = (width + 0.008) * (1 - t) ** 1.2 + 0.004
                row.append(bm.verts.new((x, sign * w, bottom + t * (top - bottom))))
            grid.append(row)
        for i in range(nx - 1):
            for k in range(nt - 1):
                quad = (grid[i][k], grid[i + 1][k], grid[i + 1][k + 1], grid[i][k + 1])
                bm.faces.new(quad if sign > 0 else tuple(reversed(quad)))
    bmesh.ops.solidify(bm, geom=bm.faces[:], thickness=0.004)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    parts = [rig.mesh_object("bicorne", bm, felt)]
    # The cockade on the front flap, toward the right point.
    for r, color, dy in ((0.03, (0.05, 0.1, 0.35), 0.0), (0.021, (0.8, 0.78, 0.72), -0.003), (0.011, (0.5, 0.04, 0.04), -0.006)):
        disc = material(f"cockade_{r}", color=color, roughness=0.8)
        place = Matrix.Translation((0.1, -ry * 0.55 - 0.01 + dy, brow + 0.07)) @ Matrix.Rotation(math.pi / 2 - 0.25, 4, "X")
        parts.append(cylinder(r, r, 0.004, 20, place, disc, bevel=0.0))
    for p in parts:
        p.data.transform(Matrix.Translation((center[0], center[1], 0.0)))
    return parts


def mob_cap(fig, name, color=(0.86, 0.84, 0.78)):
    """A white linen cap gathered into a frilled band, as working women wore."""
    brow, center, half = fitted(fig, 0.105)
    top = rig.head_top(fig)
    rx, ry = half[0] + 0.012, half[1] + 0.012
    linen = cloth(f"{name}_cap", np.array(color, dtype=np.float32), texture="rough_linen", roughness=0.95, contrast=0.8)
    gathers = lambda th, t: 1.0 + 0.035 * math.sin(th * 26) * (1 - t) + 0.02 * math.sin(th * 9)
    rings = [(brow - 0.02, rx + 0.022, ry + 0.022, 0.0), (brow + 0.015, rx + 0.028, ry + 0.028, 0.0),
             (brow + 0.03, rx + 0.012, ry + 0.012, 0.0), (top - 0.03, rx * 0.85 + 0.02, ry * 0.85 + 0.02, 0.0),
             (top + 0.02, rx * 0.5, ry * 0.5, 0.0), (top + 0.034, 0.01, 0.01, 0.0)]
    cap = lathe(f"{name}_cap", rings, linen, segments=64, folds=gathers)
    cap.data.transform(Matrix.Translation((center[0], center[1] + 0.008, 0.0)))
    return [cap]


# ---- Garments ---------------------------------------------------------------------------------

def striped_cloth(name, first, second, stripes=3):
    """Linen woven in vertical stripes: `stripes` pairs per 40 cm tile."""
    diff, nor, rough = fabric("rough_linen")
    image = bpy.data.images.load(diff)
    w, h = image.size
    px = np.array(image.pixels[:], dtype=np.float32).reshape(h, w, 4)
    lum = luminance(px[:, :, :3])
    shade = np.clip(lum / max(lum.mean(), 1e-3), 0.3, 2.0)[..., None]
    band = ((np.arange(w) * stripes * 2 // w) % 2)[None, :, None]
    rgb = np.clip(np.where(band == 0, first, second) * shade, 0, 1)
    path = os.path.join(work, f"{name}.jpg")
    out = bpy.data.images.new(name, w, h, alpha=False)
    out.pixels = np.concatenate([rgb, np.ones((h, w, 1), np.float32)], 2).ravel()
    out.filepath_raw = path
    out.file_format = "JPEG"
    out.save()
    return material(name, roughness=0.95, textures=(path, nor, rough))


def skirt(name, waist, hem, mat, r_waist=(0.16, 0.13), r_hem=(0.33, 0.27), fold=0.05):
    rings = [(waist, r_waist[0], r_waist[1], 0.0)]
    for t in (0.12, 0.35, 0.65, 1.0):
        z = waist + (hem - waist) * t
        k = t ** 0.7
        rings.append((z, r_waist[0] + (r_hem[0] - r_waist[0]) * k, r_waist[1] + (r_hem[1] - r_waist[1]) * k, 0.01 * t))
    folds = lambda th, t: 1.0 + (0.01 + fold * t) * math.sin(th * 13 + 0.7) + 0.02 * t * math.sin(th * 31)
    return lathe(name, rings, mat, segments=72, folds=folds)


def apron(name, waist, hem, mat, r_waist=(0.17, 0.14), r_hem=(0.345, 0.285)):
    rings = [(waist, r_waist[0], r_waist[1], 0.0)]
    for t in (0.35, 0.7, 1.0):
        z = waist + (hem - waist) * t
        k = t ** 0.7
        rings.append((z, r_waist[0] + (r_hem[0] - r_waist[0]) * k, r_waist[1] + (r_hem[1] - r_waist[1]) * k, 0.01 * t))
    folds = lambda th, t: 1.0 + 0.012 * t * math.sin(th * 17)
    front = -math.pi / 2
    return lathe(name, rings, mat, segments=24, folds=folds, arc=(front - 0.72, front + 0.72))


def neckerchief(fig, name, color):
    neck = fig.world(fig.bone("Neck").head)
    mat = cloth(f"{name}_kerchief", color, texture="rough_linen", roughness=0.9)
    return lathe(f"{name}_kerchief", [(neck.z - 0.04, 0.072, 0.066, neck.y), (neck.z + 0.03, 0.064, 0.06, neck.y)], mat, segments=32)


def bucket(top_center, side="R"):
    """A wooden water bucket bound with iron hoops, hanging by its handle."""
    wood = material("bucket_wood", color=(0.2, 0.12, 0.06), roughness=0.7)
    iron = material("bucket_iron", color=(0.1, 0.1, 0.1), roughness=0.5, metallic=1.0)
    rim = top_center - Vector((0, 0, 0.17))
    parts = [cylinder(0.12, 0.14, 0.27, 20, Matrix.Translation(rim - Vector((0, 0, 0.135))), wood)]
    for dz, r in ((-0.04, 0.143), (-0.23, 0.127)):
        parts.append(cylinder(r, r, 0.02, 20, Matrix.Translation(rim + Vector((0, 0, dz))), iron, bevel=0.0))
    points = [rim + Vector((0.14 * math.cos(a), 0.0, 0.17 * math.sin(a))) for a in np.linspace(0, math.pi, 10)]
    for a, b in zip(points, points[1:]):
        parts.append(cylinder(0.006, 0.006, (b - a).length, 6, along(a, b), iron, bevel=0.0))
    for p in parts:
        p["bone"] = f"{side} Hand"
    return parts


def body_rule(height, colors):
    """A repainting rule from regions of the rest pose, their heights scaled to a 1.75 m
    figure: `colors` maps each region test to an sRGB colour and contrast, later ones
    painting over earlier ones. Skin is left alone."""
    scale = 1.75 / height

    def rule(p, n, orig, texels):
        x, y, z = p[:, 0], p[:, 1], p[:, 2] * scale
        out = orig.copy()
        skin = is_skin(orig, p)
        for region, (color, contrast) in colors:
            mask = region(x, y, z, n) & ~skin
            out[mask] = recolour(orig[mask], color, contrast)
        return out
    return rule


# ---- The new townsfolk ----------------------------------------------------------------------------

def burgher(name, src, coat, waistcoat, breeches, legs, hat, walking, contrast=0.45):
    """A man of the town in a cut-away tailcoat, waistcoat and breeches, with top boots or
    white stockings and buckled shoes."""
    fig = rig.Figure(os.path.join(adults, src, "Export", f"{src}.fbx"))
    h = fig.height / 1.75
    colors = [
        (lambda x, y, z, n: z > 0.95, (coat, contrast)),
        (lambda x, y, z, n: (n[:, 1] < -0.2) & (y < 0) & (((z > 0.99) & (z < 1.045) & (np.abs(x) < 0.13))
                                                          | ((z > 1.3) & (np.abs(x) < (z - 1.3) / 0.17 * 0.07))), (waistcoat, min(contrast, 0.4))),
        (lambda x, y, z, n: z <= 0.95, (breeches, min(contrast * 1.1, 0.5))),
    ]
    if legs == "boots":
        colors += [(lambda x, y, z, n: z < 0.5, (srgb(0x121110), 0.8)),
                   (lambda x, y, z, n: (z < 0.5) & (z > 0.44), (srgb(0x9A6A3E), 0.6))]
    else:
        colors += [(lambda x, y, z, n: z < 0.52, (srgb(0xDDD8CC), 0.5)),
                   (lambda x, y, z, n: z < 0.08, (srgb(0x151312), 0.8))]
    fig.repaint("_body", body_rule(fig.height, colors), os.path.join(work, f"{name}_body.png"))
    pose(fig, walking, 0.12)
    wool = cloth(f"{name}_coat", coat, contrast=0.5)
    extras = []
    for s in (1, -1):
        tail = [(1.02 * h, 0.07, 0.03, 0.13), (0.8 * h, 0.075, 0.03, 0.14), (0.58 * h, 0.065, 0.025, 0.13)]
        panel = lathe(f"{name}_tail", tail, wool, segments=16)
        panel.location.x = s * 0.07
        extras.append(panel)
    neck = fig.world(fig.bone("Neck").head)
    stock = material("cravat", roughness=0.9, textures=fabric("rough_linen"))
    extras.append(lathe("cravat", [(neck.z - 0.03, 0.068, 0.062, neck.y), (neck.z + 0.05, 0.062, 0.058, neck.y)], stock, segments=32))
    extras += hat(fig)
    finish(fig, named(name, walking), extras)


def labourer(name, src, shirt, waistcoat, trousers, walking, stride=0.6, arm_swing=0.17):
    fig = rig.Figure(os.path.join(adults, src, "Export", f"{src}.fbx"))
    colors = [
        (lambda x, y, z, n: z > 0.97, (shirt, 0.6)),
        (lambda x, y, z, n: (z > 0.97) & (z < 1.44) & (np.abs(x) < 0.21), (waistcoat, 0.5)),
        (lambda x, y, z, n: z <= 0.97, (trousers, 0.5)),
        (lambda x, y, z, n: z < 0.08, (srgb(0x241A14), 0.8)),
    ]
    fig.repaint("_body", body_rule(fig.height, colors), os.path.join(work, f"{name}_body.png"))
    if walking:
        fig.walk(stride, arm_swing)
    else:
        fig.stand(0.15)
        fig.arms_down()
    return fig


def worker(walking=False):
    """A working man: shirt sleeves, a red waistcoat, long trousers and a round hat."""
    fig = labourer("worker", "Male_Adult_04", srgb(0xD9D2C2), srgb(0x7A2320), srgb(0x4C5058), walking)
    extras = [neckerchief(fig, "worker", srgb(0x8A2A1A))]
    extras += round_hat(fig, "worker_hat", 0.11, 0.07, (0.07, 0.05, 0.035))
    finish(fig, named("worker", walking), extras)


def water_carrier(walking=False):
    """A water-carrier in shirt sleeves and a brown waistcoat, a bucket in each hand, filled
    at the fountain."""
    fig = labourer("water_carrier", "Male_Adult_11", srgb(0xCFC6B2), srgb(0x2E2118), srgb(0x6A6A62), walking, 0.5, 0.05)
    extras = round_hat(fig, "carrier_hat", 0.075, 0.04, (0.12, 0.09, 0.06))
    for side in ("L", "R"):
        hand = fig.world(fig.bone(f"{side} Hand").head)
        extras += bucket(hand - Vector((0, 0, 0.05)), side)
    finish(fig, named("water_carrier", walking), extras)


def townswoman(name, src, bodice, skirt_mat, apron_mat, walking, sleeves=None):
    """A woman of the market or a household: a bodice with a white fichu crossed over the
    shoulders, a full skirt and apron to the ankle, and a linen cap."""
    fig = rig.Figure(os.path.join(adults, src, "Export", f"{src}.fbx"))
    h = fig.height / 1.75
    colors = [
        (lambda x, y, z, n: z > 1.0, (bodice, 0.6)),
        (lambda x, y, z, n: (z > 1.0) & (np.abs(x) > 0.22), (sleeves if sleeves is not None else bodice, 0.6)),
        (lambda x, y, z, n: (z > 1.3) & (np.abs(x) < 0.21), (srgb(0xE3DDCF), 0.5)),
        (lambda x, y, z, n: z < 0.12, (srgb(0x241A14), 0.9)),
    ]
    fig.repaint("_body", body_rule(fig.height, colors), os.path.join(work, f"{name}_body.png"))
    body_index = list(fig.body.data.materials).index(fig.material("_body"))
    waist = 1.0 * h
    rig.trim(fig, lambda c, m: m == body_index and 0.12 < c.z < waist - 0.05 and abs(c.x) < 0.45)
    pose(fig, walking, 0.08, stride=0.45)
    # Materials come as makers: a new figure starts a fresh file, freeing any made before.
    skirt_mat, apron_mat = skirt_mat(), apron_mat()
    extras = [skirt(f"{name}_skirt", waist, 0.07 * h, skirt_mat, r_waist=(0.16 * h, 0.13 * h), r_hem=(0.33 * h, 0.27 * h)),
              apron(f"{name}_apron", waist + 0.005, 0.38 * h, apron_mat, r_waist=(0.17 * h, 0.14 * h), r_hem=(0.345 * h, 0.285 * h))]
    extras += mob_cap(fig, name)
    finish(fig, named(name, walking), extras)


def lady_in_spencer(walking=False, name="lady2", src="Female_Adult_01", gown_color=0xEEEAE0, spencer=0x243A63, ribbon=0x2E4A7A):
    """A lady in a muslin gown under a short spencer jacket, and a straw hat."""
    fig = rig.Figure(os.path.join(adults, src, "Export", f"{src}.fbx"))
    h = fig.height / 1.75
    waist = 1.15 * h
    muslin = srgb(gown_color)
    colors = [
        (lambda x, y, z, n: z > -1.0, (muslin, 0.9)),
        (lambda x, y, z, n: z > 1.13, (srgb(spencer), 0.6)),
        (lambda x, y, z, n: z < 0.08, (srgb(0x2A1E18), 1.0)),
    ]
    fig.repaint("_body", body_rule(fig.height, colors), os.path.join(work, f"{name}_body.png"))
    body_index = list(fig.body.data.materials).index(fig.material("_body"))
    rig.trim(fig, lambda c, m: m == body_index and c.z < waist - 0.1 and abs(c.x) < 0.45)
    pose(fig, walking, 0.07, stride=0.42)
    gown = cloth(f"{name}_muslin", muslin, texture="rough_linen", roughness=0.95, contrast=0.6)
    skirt_rings = [(waist, 0.15 * h, 0.12 * h, 0.0), (waist - 0.15, 0.21 * h, 0.17 * h, 0.01), (0.75 * h, 0.25 * h, 0.2 * h, 0.02),
                   (0.4 * h, 0.27 * h, 0.22 * h, 0.03), (0.03, 0.29 * h, 0.24 * h, 0.035)]
    folds = lambda th, t: 1.0 + (0.01 + 0.06 * t) * math.sin(th * 11 + 0.4) + 0.025 * t * math.sin(th * 23 + 1.1)
    extras = [lathe(f"{name}_gown", skirt_rings, gown, segments=72, folds=folds)]
    # A straw bergère: a shallow crown and a wide brim, tied with a ribbon.
    extras += round_hat(fig, f"{name}_straw", 0.075, 0.1, tuple(srgb(0xC9AE78) ** 2.2), flare=1.0, curl=0.035)
    brow, center, half = fitted(fig, 0.1)
    band = material(f"{name}_ribbon", color=tuple(srgb(ribbon) ** 2.2), roughness=0.5)
    extras.append(lathe(f"{name}_ribbon", [(brow + 0.004, half[0] + 0.01, half[1] + 0.01, center[1]), (brow + 0.03, half[0] + 0.009, half[1] + 0.009, center[1])], band, segments=48))
    finish(fig, named(name, walking), extras)


def lady3(walking=False):
    """Another lady: a pale rose gown under a green spencer, her hat trimmed with rose."""
    lady_in_spencer(walking, "lady3", "Female_Adult_12", 0xE8D6CF, 0x2F4A36, 0xA0484A)


# ---- The fountain's statues -----------------------------------------------------------------------

def laurel_wreath(center, normal, radius, mat):
    """A ring of laurel leaves."""
    n = normal.normalized()
    u = n.orthogonal().normalized()
    v = n.cross(u)
    parts = []
    for i in range(18):
        a = i / 18 * math.tau
        p = center + (u * math.cos(a) + v * math.sin(a)) * radius
        t = -u * math.sin(a) + v * math.cos(a)
        frame = Matrix((t.cross(n), n, t)).transposed().to_4x4()
        parts.append(sphere(1.0, Matrix.Translation(p) @ frame @ Matrix.Diagonal((0.018, 0.008, 0.045, 1)), mat))
    return parts


def wings(fig, mat):
    """A Victory's wings, raised from the shoulder blades and swept back: long primaries fanned
    out beneath a layer of shorter coverts."""
    bones = fig.arm.pose.bones
    spine = fig.world((bones["Bip01 Spine2"] if "Bip01 Spine2" in bones else bones["Bip01 Spine1"]).head)
    parts = []
    for s in (1, -1):
        root = spine + Vector((s * 0.07, 0.13, 0.08))
        e1 = Vector((s, 0.5, 0.0)).normalized()
        e2 = Vector((0.0, 0.35, 1.0)).normalized()
        normal = e1.cross(e2).normalized()
        for layer, (count, lo, hi, base) in enumerate(((12, 0.55, 1.45, 0.5), (9, 0.7, 1.4, 0.3))):
            for k in range(count):
                phi = lo + (hi - lo) * k / (count - 1)
                d = (e1 * math.cos(phi) + e2 * math.sin(phi)).normalized()
                # Longest at the leading edge, so each wing sweeps up to a point.
                t = k / (count - 1)
                length = base + (0.7 if layer == 0 else 0.2) * t ** 1.5
                side = d.cross(normal).normalized()
                frame = Matrix((side, normal, d)).transposed().to_4x4()
                mid = root + d * length * 0.5 + normal * (0.01 * layer)
                parts.append(sphere(1.0, Matrix.Translation(mid) @ frame @ Matrix.Diagonal((0.05, 0.01, length * 0.55, 1)), mat))
    return parts


def statue():
    """One of the four figures round the foot of the column (Vigilance, Law, Strength and
    Prudence), draped, joining hands with her neighbours. Turned to stone in the engine."""
    fig = rig.Figure(os.path.join(adults, "Female_Adult_06", "Export", "Female_Adult_06.fbx"))
    body_index = list(fig.body.data.materials).index(fig.material("_body"))
    rig.trim(fig, lambda c, m: m == body_index and c.z < 1.05 and abs(c.x) < 0.45)
    fig.stand(0.08)
    for side, x in (("L", 1), ("R", -1)):
        shoulder = fig.world(fig.bone(f"{side} UpperArm").head)
        fig.arm_to(side, shoulder + Vector((x * 0.5, 0.18, -0.4)), Vector((x * 0.4, 1.0, -0.2)))
        fig.relax_hand(side, 0.6)
    drapery = material("statue_drapery", roughness=0.8)
    skirt_rings = [(1.15, 0.155, 0.125, 0.0), (1.0, 0.22, 0.18, 0.01), (0.75, 0.27, 0.22, 0.02), (0.4, 0.3, 0.24, 0.03), (0.0, 0.33, 0.27, 0.035)]
    folds = lambda th, t: 1.0 + (0.02 + 0.09 * t) * math.sin(th * 9 + 1.3) + 0.03 * t * math.sin(th * 21 + 0.4)
    extras = [lathe("statue_gown", skirt_rings, drapery, segments=72, folds=folds)]
    finish(fig, "statue", extras)


def victory():
    """The gilded Victory on the globe atop the column, arms raised with a laurel wreath in
    each hand, wings spread."""
    fig = rig.Figure(os.path.join(adults, "Female_Adult_01", "Export", "Female_Adult_01.fbx"))
    body_index = list(fig.body.data.materials).index(fig.material("_body"))
    rig.trim(fig, lambda c, m: m == body_index and c.z < 1.05 and abs(c.x) < 0.45)
    fig.stand(0.06)
    gilt = material("victory_gilt", color=(1.0, 0.75, 0.35), roughness=0.3, metallic=1.0)
    extras = []
    for side, x in (("L", 1), ("R", -1)):
        shoulder = fig.world(fig.bone(f"{side} UpperArm").head)
        fig.arm_to(side, shoulder + Vector((x * 0.3, -0.12, 0.5)), Vector((x * 1.0, 0.3, -0.3)))
        fig.relax_hand(side, 0.8)
    for side, x in (("L", 1), ("R", -1)):
        hand = fig.world(fig.bone(f"{side} Hand").head)
        extras += laurel_wreath(hand + Vector((x * 0.03, -0.03, 0.14)), Vector((0, 1, 0)), 0.12, gilt)
    skirt_rings = [(1.15, 0.155, 0.125, 0.0), (1.0, 0.2, 0.17, 0.01), (0.7, 0.24, 0.2, 0.03), (0.35, 0.26, 0.22, 0.05), (0.0, 0.28, 0.24, 0.07)]
    folds = lambda th, t: 1.0 + (0.02 + 0.08 * t) * math.sin(th * 10 + 0.3) + 0.03 * t * math.sin(th * 25)
    extras.append(lathe("victory_gown", skirt_rings, gilt, segments=72, folds=folds))
    extras += wings(fig, gilt)
    finish(fig, "victory", extras)


def citizen(walking=False):
    burgher("citizen", "Male_Adult_07", srgb(0x1C2A4A), srgb(0xE3DDCF), srgb(0xD8CFB8), "boots", bicorne, walking)


def elder(walking=False):
    hat = lambda f: round_hat(f, "elder_hat", 0.14, 0.06, (0.02, 0.018, 0.016))
    burgher("elder", "Male_Adult_13", srgb(0x4A3322), srgb(0xC9A860), srgb(0x1A1A1A), "stockings", hat, walking, contrast=0.12)


def market_woman(walking=False):
    skirt_mat = lambda: cloth("mw_skirt", srgb(0x4A3A2A), contrast=0.6)
    apron_mat = lambda: cloth("mw_apron", srgb(0xD8D2C0), texture="rough_linen")
    townswoman("market_woman", "Female_Adult_09", srgb(0x5A2A22), skirt_mat, apron_mat, walking)


def maid(walking=False):
    townswoman("maid", "Female_Adult_12", srgb(0x6F8298),
               lambda: striped_cloth("maid_stripes", srgb(0x2E4A78), srgb(0xE0DACB)),
               lambda: cloth("maid_apron", srgb(0xE6E1D4), texture="rough_linen"), walking, sleeves=srgb(0xD9D2C2))


BUILDS = {
    "grenadier": grenadier,
    "gentleman": gentleman,
    "lady2": lady_in_spencer,
    "lady3": lady3,
    "worker": worker,
    "water_carrier": water_carrier,
    "citizen": citizen,
    "elder": elder,
    "market_woman": market_woman,
    "maid": maid,
    "statue": statue,
    "victory": victory,
}



# ---- Children ------------------------------------------------------------------------------------

def boy(name, src, waistcoat, trousers, cap):
    """A boy in shirt-sleeves, a waistcoat and trousers, and a small round cap."""
    fig = labourer(name, src, srgb(0xD8D0BE), waistcoat, trousers, False)
    extras = round_hat(fig, f"{name}_cap", 0.055, 0.03, cap)
    finish(fig, name, extras)


def girl(name, src, dress, apron_color):
    """A girl in a long dress and apron, and a linen cap."""
    townswoman(name, src, dress,
               lambda: cloth(f"{name}_dress", dress, contrast=0.6),
               lambda: cloth(f"{name}_apron", apron_color, texture="rough_linen"), False)


BUILDS.update({
    "boy1": lambda: boy("boy1", "Male_Child_01", srgb(0x5A3A22), srgb(0x3E4450), (0.1, 0.08, 0.06)),
    "boy2": lambda: boy("boy2", "Male_Child_02", srgb(0x2F4A36), srgb(0x6A5A44), (0.05, 0.05, 0.06)),
    "girl1": lambda: girl("girl1", "Female_Child_01", srgb(0x7A3A30), srgb(0xE3DDCF)),
    "girl2": lambda: girl("girl2", "Female_Child_02", srgb(0x3A4E78), srgb(0xD8D2C0)),
})

wanted = [w for w in os.environ.get("PEOPLE", "").split(",") if w]
for name, build in BUILDS.items():
    if not wanted or name in wanted:
        build()
