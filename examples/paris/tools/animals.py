"""Animates Rocketbox animals for Paris and exports them as skinned glTF with their clips.

    blender -b --python animals.py -- <rocketbox_animals dir> <out dir>

The animals come rigged but without animation, so their gaits are made here, bone by bone:
a four-beat walk for horses, dogs and the pig (left hind, left fore, right hind, right fore,
each leg swinging forward with the joints below it flexing to lift the foot, planted and
sweeping back), a strut for the birds with the head holding still and then thrusting forward,
and idles: breathing, looking about, grazing, rooting, pecking, and a wagging tail. The cart
horses also get a collar and belly-band of leather.

Microsoft Rocketbox is MIT licensed.
"""

import math
import os
import sys
from types import SimpleNamespace

import bmesh
import bpy
from mathutils import Matrix, Quaternion, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rig  # noqa: E402

source_dir, out_dir = [os.path.abspath(a) for a in sys.argv[-2:]]
os.makedirs(out_dir, exist_ok=True)
FPS = 24


def load(name):
    """Imports an animal, relinking its textures, and returns its armature and mesh."""
    bpy.ops.wm.read_factory_settings(use_empty=True)
    rig._materials.clear()
    fbx = os.path.join(source_dir, name, "Export", f"{name}.fbx")
    bpy.ops.import_scene.fbx(filepath=fbx)
    arm = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    arm.animation_data_clear()
    body = next(o for o in bpy.data.objects if o.type == "MESH")
    for o in list(bpy.data.objects):
        if o not in (arm, body):
            bpy.data.objects.remove(o)
    textures = os.path.join(source_dir, name, "Textures")
    for image in bpy.data.images:
        base = os.path.splitext(os.path.basename(image.filepath.replace("\\", "/")))[0]
        for candidate in (base + ".png", base.lower() + ".png"):
            path = os.path.join(textures, candidate)
            if os.path.exists(path):
                image.filepath = path
                image.reload()
                break
    # Their bump maps are heights, not normals: leave them out rather than light them wrongly.
    for mat in body.data.materials:
        if not mat or not mat.use_nodes:
            continue
        for node in list(mat.node_tree.nodes):
            if node.type == "NORMAL_MAP" or node.type == "BUMP":
                mat.node_tree.nodes.remove(node)
        bsdf = mat.node_tree.nodes.get("Principled BSDF")
        if bsdf:
            for link in list(bsdf.inputs["Normal"].links):
                mat.node_tree.links.remove(link)
            bsdf.inputs["Metallic"].default_value = 0.0
    bpy.context.view_layer.update()
    return arm, body


class Poser:
    """Keys rotations of bones about world axes, relative to their rest pose."""

    def __init__(self, arm, prefix):
        self.arm = arm
        self.prefix = prefix
        self.axes = {}
        for pb in arm.pose.bones:
            pb.rotation_mode = "QUATERNION"

    def has(self, name):
        return self.prefix + name in self.arm.pose.bones

    def local_axis(self, pb, world_axis):
        key = (pb.name, tuple(world_axis))
        if key not in self.axes:
            rest = (self.arm.matrix_world @ pb.bone.matrix_local).to_3x3().normalized()
            self.axes[key] = (rest.inverted() @ Vector(world_axis)).normalized()
        return self.axes[key]

    def set(self, name, pitch=0.0, yaw=0.0, roll=0.0):
        """Turns a bone about world X (pitch: positive swings its tip back and down), Z (yaw)
        and Y (roll), each from its rest pose relative to its parent."""
        if not self.has(name):
            return
        pb = self.arm.pose.bones[self.prefix + name]
        q = Quaternion(self.local_axis(pb, (1, 0, 0)), pitch)
        q = Quaternion(self.local_axis(pb, (0, 0, 1)), yaw) @ q
        q = Quaternion(self.local_axis(pb, (0, 1, 0)), roll) @ q
        pb.rotation_quaternion = q

    def key(self, frame):
        for pb in self.arm.pose.bones:
            pb.keyframe_insert("rotation_quaternion", frame=frame)

    def clear(self):
        for pb in self.arm.pose.bones:
            pb.rotation_quaternion = Quaternion()
            pb.location = Vector()


def clip(poser, name, seconds, pose):
    """Records `pose(t)` (t from 0 to 1 over the loop) as an action called `name`."""
    arm = poser.arm
    arm.animation_data_create()
    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    arm.animation_data.action = action
    frames = max(int(round(seconds * FPS)), 8)
    for f in range(frames + 1):
        poser.clear()
        pose((f % frames) / frames)
        poser.key(f)
    arm.animation_data.action = None
    poser.clear()


TAU = math.tau


def quadruped(poser, stride, knee, lift, spine=0.03, neck=0.04, tail=0.12, droop=0.0):
    """A four-beat walk: each upper leg swings `stride` radians either way; as it swings
    forward the joints below fold by `knee` and `lift` to carry the foot clear."""
    def pose(t):
        for leg, phase in (("L Thigh", 0.0), ("L UpperArm", 0.25), ("R Thigh", 0.5), ("R UpperArm", 0.75)):
            side = leg[0]
            a = TAU * (t + phase)
            swing = stride * math.sin(a)
            # Forward swing while the leg moves forward: cos(a) < 0.
            fold = max(0.0, -math.cos(a))
            if "Thigh" in leg:
                poser.set(f"{side} Thigh", pitch=swing)
                poser.set(f"{side} Calf", pitch=-knee * 0.5 * fold)
                poser.set(f"{side} Foot", pitch=knee * fold)
                poser.set(f"{side} Toe0", pitch=-lift * fold)
            else:
                poser.set(f"{side} UpperArm", pitch=swing)
                poser.set(f"{side} Forearm", pitch=knee * fold)
                poser.set(f"{side} Hand", pitch=lift * fold)
                poser.set(f"{side} Finger0", pitch=lift * 0.5 * fold)
        # The back sways with the legs, the head nods twice a stride, the tail swings.
        poser.set("Spine1", roll=spine * math.sin(TAU * t))
        poser.set("Neck", pitch=neck * math.sin(TAU * 2 * t))
        poser.set("Tail", pitch=-droop, yaw=tail * math.sin(TAU * t))
        poser.set("Tail1", pitch=-droop * 0.25, yaw=tail * 0.7 * math.sin(TAU * t - 0.6))
    return pose


def quadruped_idle(poser, graze, tail, wag=False, droop=0.0):
    """Standing: breathing, the head going down to graze or root and coming up to look about,
    the tail swishing (or, for a dog, wagging)."""
    def pose(t):
        down = graze * max(0.0, math.sin(TAU * t)) ** 2
        poser.set("Neck", pitch=down, yaw=0.25 * math.sin(TAU * t * 2 + 1.0) * (1.0 - down / max(graze, 1e-3)))
        poser.set("Neck1", pitch=down * 0.5)
        poser.set("Spine1", pitch=0.012 * math.sin(TAU * t * 6))
        speed = 12 if wag else 2
        poser.set("Tail", pitch=-droop, yaw=tail * math.sin(TAU * t * speed))
        poser.set("Tail1", pitch=-droop * 0.25, yaw=tail * 0.8 * math.sin(TAU * t * speed - 0.8))
        poser.set("Ear L", pitch=0.15 * max(0.0, math.sin(TAU * t * 3)) ** 8)
        poser.set("Ear R", pitch=0.15 * max(0.0, math.sin(TAU * t * 3 + 2.0)) ** 8)
    return pose


def bird_walk(poser, stride):
    """A strut: legs alternate, the head holds still and thrusts forward each step."""
    def pose(t):
        for side, phase in (("L", 0.0), ("R", 0.5)):
            a = TAU * (t + phase)
            fold = max(0.0, -math.cos(a))
            poser.set(f"{side} Thigh", pitch=stride * math.sin(a))
            poser.set(f"{side} Calf", pitch=0.7 * fold)
            poser.set(f"{side} Foot", pitch=-0.9 * fold)
            poser.set(f"{side} Toe0", pitch=0.6 * fold)
            poser.set(f"{side} Toe1", pitch=0.6 * fold)
        # Sawtooth: slowly back relative to the body, then a quick thrust, twice a stride.
        s = (t * 2) % 1.0
        thrust = s / 0.8 if s < 0.8 else (1.0 - s) / 0.2
        poser.set("Neck", pitch=0.35 * (thrust - 0.5))
        poser.set("Neck1", pitch=-0.25 * (thrust - 0.5))
        poser.set("Tail", yaw=0.1 * math.sin(TAU * t))
    return pose


def bird_idle(poser):
    """Pecking at the ground, then looking about."""
    def pose(t):
        pecks = max(0.0, math.sin(TAU * t * 3)) ** 3 if t < 0.5 else 0.0
        poser.set("Neck", pitch=1.1 * pecks)
        poser.set("Neck1", pitch=0.5 * pecks)
        poser.set("Head", yaw=0.4 * math.sin(TAU * t * 2) * (0.0 if t < 0.5 else 1.0))
        poser.set("Tail", yaw=0.08 * math.sin(TAU * t * 4))
    return pose


def harness(arm, prefix):
    """A collar round the horse's neck and a band round its belly, in dark leather, bound to
    the neck and the middle of the back."""
    leather = rig.material("harness_leather", color=(0.03, 0.02, 0.012), roughness=0.55)
    brass = rig.material("harness_brass", color=(0.62, 0.45, 0.16), roughness=0.35, metallic=1.0)
    mw = arm.matrix_world
    parts = []

    def ring(center, axis, major, minor, mat, name):
        rot = Vector((0, 0, 1)).rotation_difference(axis.normalized()).to_matrix().to_4x4()
        # A torus: a small circle swept round a big one.
        mesh = bpy.data.meshes.new(name)
        bm = bmesh.new()
        n, m = 36, 10
        verts = []
        for i in range(n):
            a = i / n * math.tau
            row = []
            for j in range(m):
                b = j / m * math.tau
                p = Vector(((major + minor * math.cos(b)) * math.cos(a), (major + minor * math.cos(b)) * math.sin(a), minor * math.sin(b) * 1.8))
                row.append(bm.verts.new(Matrix.Translation(center) @ rot @ p))
            verts.append(row)
        for i in range(n):
            for j in range(m):
                bm.faces.new((verts[i][j], verts[(i + 1) % n][j], verts[(i + 1) % n][(j + 1) % m], verts[i][(j + 1) % m]))
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
        bm.to_mesh(mesh)
        bm.free()
        for poly in mesh.polygons:
            poly.use_smooth = True
        mesh.materials.append(mat)
        obj = bpy.data.objects.new(name, mesh)
        bpy.context.scene.collection.objects.link(obj)
        return obj

    neck = mw @ arm.data.bones[prefix + "Neck"].head_local
    neck1 = mw @ arm.data.bones[prefix + "Neck1"].head_local
    collar = ring(neck + (neck1 - neck) * 0.55, neck1 - neck, 0.3, 0.045, leather, "collar")
    collar["bone"] = prefix + "Neck"
    parts.append(collar)
    spine = mw @ arm.data.bones[prefix + "Spine1"].head_local
    band = ring(spine - Vector((0, 0, 0.18)), Vector((0, 1, 0)), 0.38, 0.03, leather, "bellyband")
    band["bone"] = prefix + "Spine1"
    parts.append(band)
    for s in (1, -1):
        stud = rig.sphere(0.03, Matrix.Translation(neck + (neck1 - neck) * 0.55 + Vector((s * 0.3, 0, 0))), brass)
        stud["bone"] = prefix + "Neck"
        parts.append(stud)
    return parts


def bind_rigid(arm, parts):
    """Each part rides rigidly on the bone it names."""
    for obj in parts:
        me = obj.data
        me.transform(obj.matrix_world)
        obj.matrix_world = Matrix.Identity(4)
        group = obj.vertex_groups.new(name=obj["bone"])
        group.add([v.index for v in me.vertices], 1.0, "REPLACE")
        mod = obj.modifiers.new("skin", "ARMATURE")
        mod.object = arm


def build(name, out, prefix, clips, extras=None):
    arm, body = load(name)
    poser = Poser(arm, prefix)
    for clip_name, (seconds, pose_maker) in clips.items():
        clip(poser, clip_name, seconds, pose_maker(poser))
    parts = extras(arm, prefix) if extras else []
    bind_rigid(arm, parts)
    fig = SimpleNamespace(arm=arm, body=body)
    rig.export_skinned(fig, parts, os.path.join(out_dir, f"{out}.glb"))
    print("BUILT", out)


D = math.radians
ANIMALS = {
    "horse_bay": lambda: build("Horse_Brown_01", "horse_bay", "horse ", {
        "walk": (1.15, lambda p: quadruped(p, D(17), D(55), D(30), droop=1.25)),
        "idle": (9.0, lambda p: quadruped_idle(p, D(40), D(18), droop=1.25)),
    }, harness),
    "horse_chestnut": lambda: build("Horse_LightBrown_01", "horse_chestnut", "horse ", {
        "walk": (1.15, lambda p: quadruped(p, D(17), D(55), D(30), droop=1.25)),
        "idle": (9.0, lambda p: quadruped_idle(p, D(40), D(18), droop=1.25)),
    }, harness),
    "pig": lambda: build("Pig_Pink_01", "pig", "pig ", {
        "walk": (0.75, lambda p: quadruped(p, D(20), D(40), D(20), tail=0.3, droop=0.8)),
        "idle": (6.0, lambda p: quadruped_idle(p, D(35), D(25), droop=0.8)),
    }),
    "beagle": lambda: build("Dog_Beagle_01", "beagle", "beagle ", {
        "walk": (0.55, lambda p: quadruped(p, D(24), D(50), D(30), tail=0.35)),
        "idle": (5.0, lambda p: quadruped_idle(p, D(15), D(35), wag=True)),
    }),
    "shepherd": lambda: build("Dog_GermanShepard_01", "shepherd", "dog ", {
        "walk": (0.7, lambda p: quadruped(p, D(22), D(50), D(30), tail=0.25)),
        "idle": (6.0, lambda p: quadruped_idle(p, D(15), D(20), wag=True)),
    }),
    "hen_brown": lambda: build("Bird_Chicken_Brown_01", "hen_brown", "chicken ", {
        "walk": (0.5, lambda p: bird_walk(p, D(28))),
        "idle": (4.0, lambda p: bird_idle(p)),
    }),
    "hen_white": lambda: build("Bird_Chicken_White_01", "hen_white", "chicken ", {
        "walk": (0.5, lambda p: bird_walk(p, D(28))),
        "idle": (4.0, lambda p: bird_idle(p)),
    }),
    "rooster": lambda: build("Bird_Rooster_Brown_01", "rooster", "rooster ", {
        "walk": (0.55, lambda p: bird_walk(p, D(28))),
        "idle": (4.5, lambda p: bird_idle(p)),
    }),
    "goose": lambda: build("Bird_Goose_White_01", "goose", "goose ", {
        "walk": (0.7, lambda p: bird_walk(p, D(24))),
        "idle": (5.0, lambda p: bird_idle(p)),
    }),
}

wanted = [w for w in os.environ.get("ANIMALS", "").split(",") if w]
for name, make in ANIMALS.items():
    if not wanted or name in wanted:
        make()
