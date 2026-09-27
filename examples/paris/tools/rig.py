"""Loading and posing Microsoft Rocketbox figures in Blender, shared by the Paris tools."""

import math
import os

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Vector


class Figure:
    """A Rocketbox figure: its armature, its skinned mesh, and the rest pose of every bone."""

    def __init__(self, fbx):
        bpy.ops.wm.read_factory_settings(use_empty=True)
        # A fresh file frees every material, so forget the ones made for the last figure.
        _materials.clear()
        # How far a pose lowers the figure (a stride shortens the legs' reach); applied at export.
        self.drop = 0.0
        self.source = fbx
        bpy.ops.import_scene.fbx(filepath=fbx)
        self.arm = next(o for o in bpy.data.objects if o.type == "ARMATURE")
        # The FBX carries a one-frame take that would override any pose we set.
        self.arm.animation_data_clear()
        self.body = next(o for o in bpy.data.objects if o.type == "MESH")
        # The children's skeletons are named Bip02; call them Bip01 like everyone else's, so
        # the posing and the animations find them (their skin groups are renamed with them).
        for bone in self.arm.data.bones:
            if bone.name.startswith("Bip02"):
                bone.name = "Bip01" + bone.name[5:]
        for o in list(bpy.data.objects):
            if o not in (self.arm, self.body):
                bpy.data.objects.remove(o)
        # The FBX names its textures by bare file name; they live in ../Textures.
        textures = os.path.join(os.path.dirname(os.path.dirname(fbx)), "Textures")
        for image in bpy.data.images:
            path = os.path.join(textures, os.path.basename(image.filepath.replace("\\", "/")))
            # The sources are kept as PNG to save space; the FBX still names the original TGAs.
            if not os.path.exists(path):
                path = os.path.splitext(path)[0] + ".png"
            if os.path.exists(path):
                image.filepath = path
                image.reload()
        bpy.context.view_layer.update()
        self.rest = {
            pb.name: (self.world(pb.head), self.world(pb.tail), (self.arm.matrix_world @ pb.matrix).copy())
            for pb in self.arm.pose.bones
        }
        me = self.body.data
        me.calc_loop_triangles()
        mw = self.body.matrix_world
        nw = mw.to_3x3().inverted().transposed()
        self.verts = np.array([tuple(mw @ v.co) for v in me.vertices])
        self.normals = np.array([tuple((nw @ v.normal).normalized()) for v in me.vertices])
        self.height = self.verts[:, 2].max()

    def world(self, p):
        return self.arm.matrix_world @ p

    def bone(self, name):
        return self.arm.pose.bones["Bip01 " + name]

    def material(self, suffix):
        return next(m for m in self.body.data.materials if m.name.endswith(suffix))

    # ---- Posing ---------------------------------------------------------------------------

    def aim(self, pb, direction, child=None):
        """Turns a bone about its head so that its child joint (or, without one, its tail) lies
        along a world direction. FBX bone tails don't point at their children, so limbs aim
        by the joint they lead to."""
        head = self.world(pb.head)
        tip = self.world(child.head) if child is not None else self.world(pb.tail)
        q = (tip - head).normalized().rotation_difference(direction.normalized())
        to_arm = self.arm.matrix_world.inverted().to_3x3()
        R = (to_arm @ q.to_matrix() @ self.arm.matrix_world.to_3x3()).to_4x4()
        h = pb.head.copy()
        pb.matrix = Matrix.Translation(h) @ R @ Matrix.Translation(-h) @ pb.matrix
        bpy.context.view_layer.update()

    def turn(self, pb, axis, angle):
        to_arm = self.arm.matrix_world.inverted().to_3x3()
        R = (to_arm @ Matrix.Rotation(angle, 3, axis) @ self.arm.matrix_world.to_3x3()).to_4x4()
        h = pb.head.copy()
        pb.matrix = Matrix.Translation(h) @ R @ Matrix.Translation(-h) @ pb.matrix
        bpy.context.view_layer.update()

    def reach(self, upper, lower, end, target, pole):
        """Two-bone IK: bends upper and lower so the end bone's head reaches target."""
        s = self.world(upper.head)
        l1 = (self.world(lower.head) - s).length
        l2 = (self.world(end.head) - self.world(lower.head)).length
        d = target - s
        dist = min(max(d.length, 1e-3), (l1 + l2) * 0.999)
        dn = d.normalized()
        cos_a = (l1 * l1 + dist * dist - l2 * l2) / (2 * l1 * dist)
        sin_a = math.sqrt(max(0.0, 1 - cos_a * cos_a))
        p = (pole - dn * pole.dot(dn)).normalized()
        elbow = s + dn * l1 * cos_a + p * l1 * sin_a
        self.aim(upper, elbow - s, lower)
        self.aim(lower, (s + dn * dist) - self.world(lower.head), end)

    def arm_to(self, side, target, pole):
        b = self.bone
        self.reach(b(f"{side} UpperArm"), b(f"{side} Forearm"), b(f"{side} Hand"), target, pole)

    def relax_hand(self, side, curl=0.35):
        """Fingers gently curled, as a hand at rest."""
        for f in range(5):
            for j in ("", "1", "2"):
                name = f"Bip01 {side} Finger{f}{j}"
                if name in self.arm.pose.bones:
                    pb = self.arm.pose.bones[name]
                    axis = (self.world(pb.tail) - self.world(pb.head)).cross(Vector((0, 0, 1)))
                    if axis.length > 1e-4 and f > 0:
                        self.turn(pb, axis.normalized(), curl)

    def stand(self, feet_apart=0.1):
        """Weight on both feet, arms hanging naturally by the sides."""
        hip = self.world(self.bone("Pelvis").head).z
        for side, x in (("L", 1), ("R", -1)):
            thigh = self.bone(f"{side} Thigh")
            ankle_z = self.rest[f"Bip01 {side} Foot"][0].z
            self.reach(thigh, self.bone(f"{side} Calf"), self.bone(f"{side} Foot"),
                       Vector((x * feet_apart, 0.0, ankle_z)), Vector((0, -1, 0)))
            foot = self.bone(f"{side} Foot")
            rest = self.rest[foot.name]
            self.aim(foot, rest[1] - rest[0])
        _ = hip

    def walk(self, stride=0.6, arm_swing=0.17):
        """Mid-stride: the left foot ahead and flat, the right behind on its toes, the arms
        swinging opposite. The hips sink so both feet can reach the ground."""
        half = stride / 2
        hip = self.rest["Bip01 L Thigh"][0].z
        ankle = self.rest["Bip01 L Foot"][0].z
        leg = (hip - ankle) * 0.985
        self.drop = leg - math.sqrt(max(leg * leg - half * half, 0.0))
        for side, x, ahead in (("L", 1, 1), ("R", -1, -1)):
            thigh = self.bone(f"{side} Thigh")
            ankle_z = self.rest[f"Bip01 {side} Foot"][0].z
            lift = 0.0 if ahead > 0 else 0.06
            # Figures face -Y. The ankle targets are in the unlowered pose.
            target = Vector((x * 0.085, -ahead * half, ankle_z + self.drop + lift))
            self.reach(thigh, self.bone(f"{side} Calf"), self.bone(f"{side} Foot"), target, Vector((0, -1, 0)))
            foot = self.bone(f"{side} Foot")
            rest = self.rest[foot.name]
            direction = rest[1] - rest[0]
            if ahead < 0:
                direction = Matrix.Rotation(0.5, 3, "X") @ direction
            self.aim(foot, direction)
        for side, x, leg_ahead in (("L", 1, 1), ("R", -1, -1)):
            shoulder = self.world(self.bone(f"{side} UpperArm").head)
            swing = arm_swing * leg_ahead
            self.arm_to(side, Vector((x * 0.23, shoulder.y + swing, shoulder.z - 0.58)), Vector((x * 0.3, 1.0, 0.0)))
            self.relax_hand(side)

    def arms_down(self, sides=("L", "R")):
        for side in sides:
            x = 1 if side == "L" else -1
            shoulder = self.world(self.bone(f"{side} UpperArm").head)
            self.arm_to(side, Vector((x * 0.24, shoulder.y + 0.03, shoulder.z - 0.6)),
                        Vector((x * 0.3, 1.0, 0.0)))
            self.relax_hand(side)

    # ---- Painting the texture -------------------------------------------------------------

    def rasterize(self, material_index, width, height, rule, tex):
        """Calls `rule(P, N, (ys, xs))` for the texels of every triangle of one material, with
        each texel's rest-pose position and normal, and writes back what it returns."""
        me = self.body.data
        uv_layer = me.uv_layers.active.data
        for tri in me.loop_triangles:
            if tri.material_index != material_index:
                continue
            uv = np.array([tuple(uv_layer[l].uv) for l in tri.loops]) * [width, height]
            p3 = self.verts[list(tri.vertices)]
            n3 = self.normals[list(tri.vertices)]
            lo = np.clip(np.floor(uv.min(0)).astype(int) - 1, 0, [width - 1, height - 1])
            hi = np.clip(np.ceil(uv.max(0)).astype(int) + 1, 0, [width - 1, height - 1])
            xs, ys = np.meshgrid(np.arange(lo[0], hi[0] + 1), np.arange(lo[1], hi[1] + 1))
            px = np.stack([xs.ravel() + 0.5, ys.ravel() + 0.5], 1)
            a, b, c = uv
            v0, v1 = b - a, c - a
            den = v0[0] * v1[1] - v1[0] * v0[1]
            if abs(den) < 1e-9:
                continue
            d = px - a
            w1 = (d[:, 0] * v1[1] - v1[0] * d[:, 1]) / den
            w2 = (v0[0] * d[:, 1] - d[:, 0] * v0[1]) / den
            w0 = 1 - w1 - w2
            inside = (w0 > -0.06) & (w1 > -0.06) & (w2 > -0.06)
            if not inside.any():
                continue
            wts = np.stack([w0, w1, w2], 1)[inside]
            tx, ty = xs.ravel()[inside], ys.ravel()[inside]
            tex[ty, tx] = rule(wts @ p3, wts @ n3, (ty, tx))

    def repaint(self, suffix, rule, out_path):
        """Repaints one material's colour texture by `rule`, saves it and swaps it in."""
        mat = self.material(suffix)
        node = next(n for n in mat.node_tree.nodes
                    if n.type == "TEX_IMAGE" and n.outputs[0].links
                    and n.outputs[0].links[0].to_socket.name == "Base Color")
        w, h = node.image.size
        pixels = np.array(node.image.pixels[:], dtype=np.float32).reshape(h, w, 4)
        original = pixels[:, :, :3]
        alpha = pixels[:, :, 3:]
        tex = original.copy()
        index = list(self.body.data.materials).index(mat)
        self.rasterize(index, w, h, lambda p, n, t: rule(p, n, original[t], t), tex)
        # Alpha is kept: hair and fringes are cut out with it.
        image = bpy.data.images.new(os.path.basename(out_path), w, h, alpha=True)
        image.pixels = np.concatenate([tex, alpha], 2).ravel()
        image.filepath_raw = out_path
        image.file_format = "PNG"
        image.save()
        node.image = image


def smooth(v, lo, hi, feather):
    """1 inside [lo, hi], fading to 0 over `feather` either side."""
    def step(e0, e1, x):
        t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
        return t * t * (3 - 2 * t)
    return step(lo - feather, lo + feather, v) * (1.0 - step(hi - feather, hi + feather, v))


def srgb(hex_color):
    return np.array([((hex_color >> s) & 255) / 255.0 for s in (16, 8, 0)], dtype=np.float32)


def luminance(c):
    return c @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)


# ---- Modelled parts ---------------------------------------------------------------------------

_materials = {}


def material(name, color=(1, 1, 1), roughness=0.8, metallic=0.0, textures=None, alpha_cutoff=None):
    """A principled material; `textures` optionally gives colour, normal and roughness images."""
    if name in _materials:
        return _materials[name]
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nodes, links = m.node_tree.nodes, m.node_tree.links
    bsdf = nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*color, 1)
    bsdf.inputs["Roughness"].default_value = roughness
    bsdf.inputs["Metallic"].default_value = metallic
    if textures:
        color_path, normal_path, rough_path = textures
        tex = nodes.new("ShaderNodeTexImage")
        tex.image = bpy.data.images.load(color_path, check_existing=True)
        links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
        if alpha_cutoff is not None:
            links.new(tex.outputs["Alpha"], bsdf.inputs["Alpha"])
        if normal_path:
            nt = nodes.new("ShaderNodeTexImage")
            nt.image = bpy.data.images.load(normal_path, check_existing=True)
            nt.image.colorspace_settings.name = "Non-Color"
            nm = nodes.new("ShaderNodeNormalMap")
            links.new(nt.outputs["Color"], nm.inputs["Color"])
            links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
        if rough_path:
            rt = nodes.new("ShaderNodeTexImage")
            rt.image = bpy.data.images.load(rough_path, check_existing=True)
            rt.image.colorspace_settings.name = "Non-Color"
            links.new(rt.outputs["Color"], bsdf.inputs["Roughness"])
    if alpha_cutoff is not None:
        m.blend_method = "CLIP"
        m.alpha_threshold = alpha_cutoff
        m.use_backface_culling = False
    _materials[name] = m
    return m


def tinted_texture(src, color, out_path, contrast=1.0):
    """A colour texture recoloured to `color` (sRGB), keeping its light and shade."""
    image = bpy.data.images.load(src)
    w, h = image.size
    px = np.array(image.pixels[:], dtype=np.float32).reshape(h, w, 4)
    lum = luminance(px[:, :, :3])
    shade = np.clip((lum / max(lum.mean(), 1e-3)) ** contrast, 0.2, 2.5)[..., None]
    rgb = np.clip(color[None, None, :] * shade, 0.0, 1.0)
    out = bpy.data.images.new(os.path.basename(out_path), w, h, alpha=False)
    out.pixels = np.concatenate([rgb, np.ones((h, w, 1), np.float32)], 2).ravel()
    out.filepath_raw = out_path
    out.file_format = "JPEG"
    out.save()
    return out_path


def mesh_object(name, bm, mat, smooth_shading=True):
    me = bpy.data.meshes.new(name)
    bm.to_mesh(me)
    bm.free()
    for poly in me.polygons:
        poly.use_smooth = smooth_shading
    me.materials.append(mat)
    o = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(o)
    return o


def soften(bm, width):
    hard = [e for e in bm.edges if e.calc_face_angle(0.0) > 0.6]
    if hard and width > 0.0:
        bmesh.ops.bevel(bm, geom=hard, offset=width, segments=2, profile=0.5, affect="EDGES", clamp_overlap=True)


def box(size, matrix, mat, bevel=None):
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0, matrix=matrix @ Matrix.Diagonal((*size, 1)))
    soften(bm, bevel if bevel is not None else min(size) * 0.2)
    return mesh_object("part", bm, mat)


def cylinder(r1, r2, depth, segs, matrix, mat, bevel=None):
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=segs, radius1=r1, radius2=r2, depth=depth, matrix=matrix)
    soften(bm, bevel if bevel is not None else min(r1, r2, depth) * 0.15)
    return mesh_object("part", bm, mat)


def sphere(radius, matrix, mat):
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=10, radius=radius, matrix=matrix)
    return mesh_object("part", bm, mat)


def along(a, b):
    d = b - a
    rot = Vector((0, 0, 1)).rotation_difference(d.normalized()).to_matrix().to_4x4()
    return Matrix.Translation((a + b) / 2) @ rot


def lathe(name, rings, mat, segments=48, closed_top=False, folds=None, uv_scale=0.4, arc=(0.0, math.tau)):
    """A surface of revolution about Z, as for a skirt or a hat.

    `rings` is a list of (z, radius_x, radius_y, centre_y). `folds(theta, t)` returns a radius
    multiplier for drapery, t running 0 at the first ring to 1 at the last. UVs are in
    `uv_scale`-metre tiles, so cloth textures keep their real weave size. `arc` limits the
    sweep, as for an apron that covers only the front."""
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new()
    grid = []
    for k, (z, rx, ry, cy) in enumerate(rings):
        t = k / max(len(rings) - 1, 1)
        row = []
        for i in range(segments + 1):
            theta = arc[0] + i / segments * (arc[1] - arc[0])
            f = folds(theta, t) if folds else 1.0
            row.append(bm.verts.new((math.cos(theta) * rx * f, cy + math.sin(theta) * ry * f, z)))
        grid.append(row)
    for k in range(len(rings) - 1):
        for i in range(segments):
            a, b = grid[k][i], grid[k][i + 1]
            c, d = grid[k + 1][i + 1], grid[k + 1][i]
            face = bm.faces.new((a, b, c, d))
            for loop, (ii, kk) in zip(face.loops, ((i, k), (i + 1, k), (i + 1, k + 1), (i, k + 1))):
                z, rx, ry, _ = rings[kk]
                circumference = math.pi * (rx + ry)
                loop[uv].uv = (ii / segments * circumference / uv_scale, z / uv_scale)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    o = mesh_object(name, bm, mat)
    return o


def bake(fig, name, extra):
    """Freezes the posed (and smoothed) body and its extra parts into one object."""
    dg = bpy.context.evaluated_depsgraph_get()
    mesh = bpy.data.meshes.new_from_object(fig.body.evaluated_get(dg))
    mesh.transform(fig.body.matrix_world)
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    bpy.ops.object.select_all(action="DESELECT")
    for o in [obj] + list(extra):
        o.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.join()
    return obj


def export(obj, path, max_size=1024):
    """Writes the object as glTF. Textures other than the face are shrunk to `max_size`: a
    body or a coat seen across a square needs far less than a face seen up close."""
    for image in bpy.data.images:
        w, h = image.size
        if w > max_size and "head" not in image.name and image.users > 0:
            image.scale(max_size, max(1, h * max_size // w))
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(filepath=path, export_format="GLB", use_selection=True,
                              export_yup=True, export_apply=True, export_image_format="JPEG")


def trim(fig, doomed):
    """Deletes the body faces `doomed(centre, material_index)` picks out (in the rest pose),
    as for clothing a modelled garment hides completely."""
    bm = bmesh.new()
    bm.from_mesh(fig.body.data)
    mw = fig.body.matrix_world
    faces = [f for f in bm.faces if doomed(mw @ f.calc_center_median(), f.material_index)]
    bmesh.ops.delete(bm, geom=faces, context="FACES")
    bm.to_mesh(fig.body.data)
    bm.free()
    fig.body.data.update()


def head_section(fig, z):
    """The head's centre and half-widths (x, y) at height z, measured from the scanned mesh in
    the rest pose, so a hat can be fitted to the actual skull."""
    me = fig.body.data
    # Hair cards count too: a hat must sit over the hair, not through it.
    wanted = {i for i, m in enumerate(me.materials) if m.name.endswith(("_head", "_opacity"))}
    ids = {v for p in me.polygons if p.material_index in wanted for v in p.vertices}
    pts = np.array([fig.verts[i] for i in ids])
    band = pts[np.abs(pts[:, 2] - z) < 0.012]
    lo, hi = band.min(0), band.max(0)
    return (lo + hi) / 2, (hi - lo) / 2


def head_top(fig):
    me = fig.body.data
    head = list(me.materials).index(fig.material("_head"))
    ids = {v for p in me.polygons if p.material_index == head for v in p.vertices}
    return max(fig.verts[i][2] for i in ids)


# ---- Skinned, animated export ------------------------------------------------------------------

def smoothstep(e0, e1, x):
    t = min(max((x - e0) / (e1 - e0), 0.0), 1.0)
    return t * t * (3 - 2 * t)


def bind(fig, extras):
    """Binds modelled parts to the skeleton, so they move with it. Parts are placed in the
    figure's current pose; rigid ones are carried back to the rest pose on their bone.

    A part names its bone in `obj["bone"]`; failing that, garments hanging from the waist
    (skirts, coat tails) are weighted between the pelvis and the legs, and anything else rides
    on the bone nearest it: the head, the neck, a hand, or the chest."""
    b = fig.bone

    def posed(name):
        return fig.arm.matrix_world @ fig.arm.pose.bones["Bip01 " + name].matrix

    def rest(name):
        return fig.arm.matrix_world @ fig.arm.data.bones["Bip01 " + name].matrix_local

    neck = fig.world(b("Neck").head)
    hands = {s: fig.world(b(f"{s} Hand").head) for s in ("L", "R")}
    waist = fig.world(b("Spine").head).z
    ankle = (fig.world(b("L Foot").head).z + fig.world(b("R Foot").head).z) / 2
    hanging = ("skirt", "gown", "apron", "tail", "greatcoat")
    for obj in extras:
        me = obj.data
        # Bake the object's own transform into its vertices first.
        me.transform(obj.matrix_world)
        obj.matrix_world = Matrix.Identity(4)
        center = sum((v.co for v in me.vertices), Vector()) / max(len(me.vertices), 1)
        bone = obj.get("bone")
        if bone is None and any(h in obj.name for h in hanging):
            groups = {n: obj.vertex_groups.new(name="Bip01 " + n) for n in ("Pelvis", "L Thigh", "R Thigh", "L Calf", "R Calf")}
            for v in me.vertices:
                z, x = v.co.z, v.co.x
                # 0 at the waist, 1 at the ankle.
                t = 1.0 - min(max((z - ankle) / max(waist - ankle, 1e-3), 0.0), 1.0)
                legs = 0.7 * smoothstep(0.05, 0.6, t)
                calves = 0.3 * smoothstep(0.55, 1.0, t)
                left = smoothstep(-0.14, 0.14, x)
                weights = {
                    "Pelvis": 1.0 - legs - calves,
                    "L Thigh": legs * left,
                    "R Thigh": legs * (1 - left),
                    "L Calf": calves * left,
                    "R Calf": calves * (1 - left),
                }
                for n, w in weights.items():
                    if w > 1e-4:
                        groups[n].add([v.index], w, "REPLACE")
            continue
        if bone is None:
            if center.z > neck.z + 0.04:
                bone = "Head"
            elif min((center - h).length for h in hands.values()) < 0.3:
                bone = min(hands, key=lambda s: (center - hands[s]).length) + " Hand"
            elif (center - neck).length < 0.12:
                bone = "Neck"
            else:
                bone = "Spine2" if "Bip01 Spine2" in fig.arm.pose.bones else "Spine1"
        # From where the bone is now to where it rests.
        me.transform(rest(bone) @ posed(bone).inverted())
        group = obj.vertex_groups.new(name="Bip01 " + bone)
        group.add([v.index for v in me.vertices], 1.0, "REPLACE")
    # Back to the rest pose, which the skin binds to.
    for pb in fig.arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    bpy.context.view_layer.update()


def add_clips(fig, clips):
    """Imports Rocketbox animation files and retargets each onto the figure, keeping the
    result as an action under the given name for the exporter to write out.

    An animation's keys are relative to its own file's rest pose, which isn't quite the
    figure's (the arms especially), so they can't be copied across. Instead the figure's bones
    follow the animation skeleton's bones in world space (every rotation, and the pelvis's
    position), and that motion is baked into a new action."""
    for action in list(bpy.data.actions):
        bpy.data.actions.remove(action)
    names = []
    for name, path in clips.items():
        # Compared by pointer: some imported names aren't valid UTF-8, which Python can't hash.
        before_objects = {o.as_pointer() for o in bpy.data.objects}
        before_actions = {a.as_pointer() for a in bpy.data.actions}
        bpy.ops.import_scene.fbx(filepath=path)
        imported = [o for o in bpy.data.objects if o.as_pointer() not in before_objects]
        source = next((o for o in imported if o.type == "ARMATURE"), None)
        if source is None or source.animation_data is None or source.animation_data.action is None:
            print("no animation in", path)
            for o in imported:
                bpy.data.objects.remove(o, do_unlink=True)
            continue
        start, end = (int(round(v)) for v in source.animation_data.action.frame_range)
        # Scale the animation's skeleton to the figure: a child's hips are lower than those of
        # the grown man the motion was recorded on, and its steps shorter.
        hips = fig.rest["Bip01 Pelvis"][0].z
        source_hips = (source.matrix_world @ source.data.bones["Bip01 Pelvis"].head_local).z
        if source_hips > 1e-3:
            source.scale *= hips / source_hips
            bpy.context.view_layer.update()
        # The bake takes these constraints off again when it's done.
        for pb in fig.arm.pose.bones:
            if pb.name not in source.pose.bones:
                continue
            c = pb.constraints.new("COPY_ROTATION")
            c.target, c.subtarget = source, pb.name
            c.target_space = c.owner_space = "WORLD"
            if pb.name == "Bip01 Pelvis":
                c = pb.constraints.new("COPY_LOCATION")
                c.target, c.subtarget = source, pb.name
                c.target_space = c.owner_space = "WORLD"
        fig.arm.animation_data_create()
        fig.arm.animation_data.action = None
        bpy.ops.object.select_all(action="DESELECT")
        bpy.context.view_layer.objects.active = fig.arm
        fig.arm.select_set(True)
        bpy.ops.object.mode_set(mode="POSE")
        bpy.ops.pose.select_all(action="SELECT")
        bpy.ops.nla.bake(frame_start=start, frame_end=end, only_selected=True, visual_keying=True,
                         clear_constraints=True, use_current_action=False, bake_types={"POSE"})
        bpy.ops.object.mode_set(mode="OBJECT")
        baked = fig.arm.animation_data.action
        baked.name = name
        baked.use_fake_user = True
        fig.arm.animation_data.action = None
        names.append(name)
        # Drop the animation file's skeleton and its own actions.
        for o in imported:
            bpy.data.objects.remove(o, do_unlink=True)
        for a in [a for a in bpy.data.actions if a.as_pointer() not in before_actions and a.as_pointer() != baked.as_pointer()]:
            bpy.data.actions.remove(a)
    for pb in fig.arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    return names


def export_skinned(fig, extras, path):
    """Joins the body and its bound parts, and writes the figure with its skeleton and every
    clip as a skinned, animated glTF."""
    for image in bpy.data.images:
        w, h = image.size
        if w > 1024 and "head" not in image.name and image.users > 0:
            image.scale(1024, max(1, h * 1024 // w))
    bpy.ops.object.select_all(action="DESELECT")
    for o in [fig.body] + list(extras):
        o.select_set(True)
    bpy.context.view_layer.objects.active = fig.body
    if extras:
        bpy.ops.object.join()
    bpy.ops.object.select_all(action="DESELECT")
    fig.body.select_set(True)
    fig.arm.select_set(True)
    bpy.context.view_layer.objects.active = fig.arm
    kwargs = dict(filepath=path, export_format="GLB", use_selection=True, export_yup=True,
                  export_apply=False, export_skins=True, export_animations=True,
                  export_image_format="JPEG", export_force_sampling=True)
    try:
        bpy.ops.export_scene.gltf(export_animation_mode="ACTIONS", **kwargs)
    except TypeError:
        bpy.ops.export_scene.gltf(export_nla_strips=False, **kwargs)
