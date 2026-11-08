"""Procedural low-poly models for the sprite bake.

Every builder creates objects around the origin = ground centre of a cell,
model facing +Y (north). Colours are display colours (no colour management).
"""

import math
import random

import bpy
import bmesh

LEVEL = 1.0  # set by bake.py: Blender units per z-level

_mats = {}


def hexc(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) / 255.0 for i in (0, 2, 4))


# Layer ids for layered (recolourable) sprites. Tintable layers are white.
DETAIL, SKIN, TOP, BOTTOM, HAIR, TOOL = 0, 1, 2, 3, 4, 5
WHITE = "#ffffff"


def mat(color, emit=0.0, layer=DETAIL):
    rgb = hexc(color) if isinstance(color, str) else color
    key = (rgb, emit, layer)
    m = _mats.get(key)
    if m is None or m.name not in bpy.data.materials:
        m = bpy.data.materials.new(f"m{len(_mats)}")
        m.diffuse_color = (*rgb, 1.0)
        m["emit"] = emit
        m["layer"] = layer
        _mats[key] = m
    return m


def _finish(obj, color, emit=0.0, layer=DETAIL):
    obj.data.materials.clear()
    obj.data.materials.append(mat(color, emit, layer))
    return obj


def _mesh_object(build_bm, loc, rot, scale=(1, 1, 1)):
    """Creates a mesh object from a bmesh builder, baking scale into vertices
    (no bpy.ops: deterministic, context-free)."""
    from mathutils import Euler, Matrix, Vector
    bm = bmesh.new()
    build_bm(bm)
    sm = Matrix.Diagonal(Vector((*scale, 1.0)))
    bmesh.ops.transform(bm, matrix=sm, verts=bm.verts)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    me = bpy.data.meshes.new("m")
    bm.to_mesh(me)
    bm.free()
    o = bpy.data.objects.new("o", me)
    bpy.context.scene.collection.objects.link(o)
    o.matrix_world = Matrix.Translation(Vector(loc)) @ Euler(rot).to_matrix().to_4x4()
    return o


def box(size, loc, color, rot=(0, 0, 0), emit=0.0, layer=DETAIL):
    o = _mesh_object(lambda bm: bmesh.ops.create_cube(bm, size=1.0), loc, rot, size)
    return _finish(o, color, emit, layer)


def cyl(r, depth, loc, color, verts=8, rot=(0, 0, 0), emit=0.0, layer=DETAIL):
    o = _mesh_object(lambda bm: bmesh.ops.create_cone(bm, cap_ends=True, segments=verts, radius1=r, radius2=r, depth=depth), loc, rot)
    return _finish(o, color, emit, layer)


def cone(r1, r2, depth, loc, color, verts=8, rot=(0, 0, 0), emit=0.0, layer=DETAIL):
    o = _mesh_object(lambda bm: bmesh.ops.create_cone(bm, cap_ends=True, segments=verts, radius1=r1, radius2=r2, depth=depth), loc, rot)
    return _finish(o, color, emit, layer)


def ico(r, loc, color, subdiv=1, scale=(1, 1, 1), emit=0.0, layer=DETAIL):
    o = _mesh_object(lambda bm: bmesh.ops.create_icosphere(bm, subdivisions=subdiv, radius=r), loc, (0, 0, 0), scale)
    return _finish(o, color, emit, layer)


def two_tone_box(size, loc, top, side):
    """Box whose upward-facing faces use `top`, others `side`."""
    o = box(size, loc, side)
    o.data.materials.append(mat(top))
    for p in o.data.polygons:
        if p.normal.z > 0.5:
            p.material_index = 1
    return o


def speckle(rng, colors, n, size=0.09, top=True, sides=True):
    """Small protruding chips on the visible faces of a unit block."""
    for _ in range(n):
        c = rng.choice(colors)
        s = size * rng.uniform(0.7, 1.3)
        face = rng.choice(([0] if top else []) + ([1, 2, 3, 4] if sides else []))
        u = rng.uniform(-0.38, 0.38)
        v = rng.uniform(0.1, LEVEL - 0.1)
        if face == 0:
            loc = (u, rng.uniform(-0.38, 0.38), LEVEL - s * 0.3)
        elif face == 1:
            loc = (0.5 - s * 0.3, u, v)
        elif face == 2:
            loc = (-0.5 + s * 0.3, u, v)
        elif face == 3:
            loc = (u, 0.5 - s * 0.3, v)
        else:
            loc = (u, -0.5 + s * 0.3, v)
        box((s, s, s), loc, c)


# ---------------------------------------------------------------- terrain

BLOCKS = {
    # name: (top, side, speckle colours, speckle count, extras)
    "soil": ("#6e5440", "#5d4636", ["#544030", "#7a604a"], 6, "pebbles"),
    "grass": ("#5f7a3a", "#5d4636", ["#6d8a44", "#4f6a30"], 6, "tufts"),
    "granite": ("#76726c", "#68645f", ["#5e5a55", "#86827b"], 8, "cracks"),
    "iron": ("#706c66", "#635f5a", ["#a0522d", "#b8643a", "#8a4428"], 12, "cracks"),
    "salvage": ("#5a5550", "#4e4a46", ["#8a4a2a", "#3a7a78", "#9a9a96"], 14, "pebbles"),
    "ruin": ("#9a968c", "#8a867c", ["#6a665e", "#7a3f2a", "#b0aca2"], 10, "cracks"),
}
BLOCK_VARIANTS = 4


def block_builder(name):
    top, side, chips, n, extra = BLOCKS[name]

    def build(_frame, variant):
        two_tone_box((1, 1, LEVEL), (0, 0, LEVEL / 2), top, side)
        rng = random.Random(f"{name}{variant}")
        speckle(rng, chips, n)
        if extra == "tufts":
            for _ in range(rng.randint(1, 4)):
                x, y = rng.uniform(-0.38, 0.38), rng.uniform(-0.38, 0.38)
                cone(0.035, 0.0, rng.uniform(0.07, 0.13), (x, y, LEVEL + 0.04), rng.choice(["#7a9a48", "#6a8a3e"]), verts=4)
        elif extra == "pebbles":
            for _ in range(rng.randint(0, 3)):
                r = rng.uniform(0.03, 0.06)
                ico(r, (rng.uniform(-0.38, 0.38), rng.uniform(-0.38, 0.38), LEVEL), rng.choice(["#8a8478", "#6a6458"]), scale=(1, 1, 0.6))
        elif extra == "cracks":
            for _ in range(rng.randint(0, 2)):
                box((rng.uniform(0.2, 0.5), 0.025, 0.01), (rng.uniform(-0.2, 0.2), rng.uniform(-0.3, 0.3), LEVEL + 0.002),
                    "#4e4a45", rot=(0, 0, rng.uniform(0, math.pi)))
    return build


def ramp_builder(name):
    top, side = BLOCKS[name][0], BLOCKS[name][1]

    def build(_frame, _variant):
        me = bpy.data.meshes.new("ramp")
        bm = bmesh.new()
        h = LEVEL
        v = [bm.verts.new(p) for p in [
            (-0.5, -0.5, 0), (0.5, -0.5, 0), (0.5, 0.5, 0), (-0.5, 0.5, 0),  # bottom
            (-0.5, 0.5, h), (0.5, 0.5, h),  # top (north) edge
        ]]
        for f in [(v[0], v[1], v[5], v[4]), (v[3], v[2], v[1], v[0]), (v[2], v[3], v[4], v[5]), (v[1], v[2], v[5]), (v[3], v[0], v[4])]:
            bm.faces.new(f)
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
        bm.to_mesh(me)
        bm.free()
        o = bpy.data.objects.new("ramp", me)
        bpy.context.scene.collection.objects.link(o)
        me.materials.append(mat(side))
        me.materials.append(mat(top))
        for p in me.polygons:
            if p.normal.z > 0.3:
                p.material_index = 1
    return build


# ----------------------------------------------------------------- plants

def ashpine(scale):
    def build(_frame, variant):
        rng = random.Random(f"ash{variant}")
        s = scale * rng.uniform(0.9, 1.1)
        lean = (rng.uniform(-0.06, 0.06), rng.uniform(-0.06, 0.06))
        cyl(0.08 * s, 0.9 * s, (0, 0, 0.45 * s), "#4a3b30", verts=6)
        greens = ["#3d574a", "#45614f", "#56705c"]
        tiers = [(0.46, 0.75), (0.36, 1.15), (0.24, 1.5)]
        if variant == 2:
            tiers = [(0.5, 0.7), (0.42, 1.05), (0.3, 1.38), (0.18, 1.68)]
        for i, (r, z) in enumerate(tiers):
            cone(r * s * rng.uniform(0.92, 1.08), 0.0, 0.62 * s, (lean[0] * i, lean[1] * i, z * s), greens[min(i, 2)], verts=7,
                 rot=(0, 0, rng.uniform(0, 1)))
        cone(0.1 * s, 0.0, 0.25 * s, (lean[0] * 3, lean[1] * 3, (tiers[-1][1] + 0.35) * s), "#7d8c80", verts=6)
    return build


def duskberry(young, ripe):
    def build(_frame, variant):
        s = 0.6 if young else 1.0
        rng = random.Random(f"dusk{variant}")
        for _ in range(4):
            r = rng.uniform(0.15, 0.26)
            x, y = rng.uniform(-0.16, 0.16), rng.uniform(-0.16, 0.16)
            ico(r * s, (x * s, y * s, (r * 0.9) * s), rng.choice(["#3d5233", "#46603a"]), scale=(1, 1, 0.85))
        if ripe:
            for _ in range(14):
                a = rng.uniform(0, math.tau)
                rr = rng.uniform(0.12, 0.3)
                ico(0.045, (math.cos(a) * rr, math.sin(a) * rr, rng.uniform(0.12, 0.4)), rng.choice(["#7a3b8f", "#9a4aa8", "#5e2a70"]), subdiv=1)
    return build


def scrub(_frame, variant):
    rng = random.Random(f"scrub{variant}")
    for _ in range(rng.randint(4, 7)):
        x, y = rng.uniform(-0.3, 0.3), rng.uniform(-0.3, 0.3)
        h = rng.uniform(0.18, 0.34)
        cone(0.05, 0.0, h, (x, y, h / 2), rng.choice(["#8a8f4a", "#6f7a3a", "#9a9a58"]), verts=4,
             rot=(rng.uniform(-0.3, 0.3), rng.uniform(-0.3, 0.3), 0))


# ------------------------------------------------------------------ items

def item_wood(_f, _v):
    for (x, z) in [(-0.1, 0.08), (0.1, 0.08), (0.0, 0.22)]:
        cyl(0.08, 0.55, (x, 0, z), "#7a5a3a", verts=7, rot=(math.pi / 2, 0, 0.3))
        cyl(0.06, 0.56, (x, 0, z), "#a07a50", verts=7, rot=(math.pi / 2, 0, 0.3))


def item_stone(_f, _v):
    rng = random.Random("stone")
    for _ in range(5):
        r = rng.uniform(0.07, 0.13)
        ico(r, (rng.uniform(-0.2, 0.2), rng.uniform(-0.2, 0.2), r * 0.8), rng.choice(["#8a8a85", "#7a7a76", "#9a9a94"]), scale=(1, 1, 0.8))


def item_blocks(_f, _v):
    for z in range(2):
        for x in (-0.11, 0.11):
            for y in (-0.08, 0.08):
                box((0.2, 0.14, 0.12), (x, y, 0.06 + z * 0.13), "#9aa0a8")


def item_ore(_f, _v):
    rng = random.Random("ore")
    for _ in range(5):
        r = rng.uniform(0.07, 0.12)
        ico(r, (rng.uniform(-0.2, 0.2), rng.uniform(-0.2, 0.2), r * 0.8), rng.choice(["#8a5a44", "#b0643a", "#6e4a3a"]), scale=(1, 1, 0.8))


def item_scrap(_f, _v):
    rng = random.Random("scrap")
    for _ in range(6):
        box((rng.uniform(0.12, 0.3), rng.uniform(0.06, 0.16), 0.03), (rng.uniform(-0.15, 0.15), rng.uniform(-0.15, 0.15), rng.uniform(0.02, 0.12)),
            rng.choice(["#7d7d80", "#9b5a3a", "#5f6266"]), rot=(rng.uniform(-0.4, 0.4), rng.uniform(-0.4, 0.4), rng.uniform(0, 3)))


def item_components(_f, _v):
    box((0.3, 0.22, 0.12), (0, 0, 0.06), "#2f6f6a")
    box((0.08, 0.08, 0.04), (-0.06, 0.03, 0.13), "#c8b040")
    box((0.06, 0.1, 0.04), (0.08, -0.04, 0.13), "#40c8a0", emit=1.0)


def item_parts(_f, _v):
    for (x, y, z) in [(-0.1, 0, 0.03), (0.1, 0.05, 0.03), (0.0, -0.02, 0.09)]:
        cyl(0.11, 0.05, (x, y, z), "#a0a4a8", verts=10)
        cyl(0.04, 0.06, (x, y, z), "#5a5e62", verts=6)


def item_berries(_f, _v):
    cyl(0.18, 0.1, (0, 0, 0.05), "#7a5a3a", verts=10)
    rng = random.Random("berries")
    for _ in range(12):
        a, r = rng.uniform(0, math.tau), rng.uniform(0, 0.13)
        ico(0.045, (math.cos(a) * r, math.sin(a) * r, 0.12 + rng.uniform(0, 0.04)), rng.choice(["#7a3b8f", "#9a4aa8"]))


def item_meal(_f, _v):
    cyl(0.17, 0.09, (0, 0, 0.045), "#c8b89a", verts=12)
    cyl(0.14, 0.02, (0, 0, 0.095), "#8a5a30", verts=12)
    box((0.03, 0.2, 0.02), (0.08, 0.02, 0.12), "#a0a4a8", rot=(0, 0, 0.5))


# -------------------------------------------------------------- buildings

def wood_wall(_f, _v):
    box((1, 1, LEVEL), (0, 0, LEVEL / 2), "#8b6a45")
    for i in range(4):
        x = -0.375 + i * 0.25
        box((0.03, 1.02, LEVEL * 0.98), (x, 0, LEVEL / 2), "#6e5236")
        box((1.02, 0.03, LEVEL * 0.98), (0, x, LEVEL / 2), "#6e5236")
    box((1.02, 1.02, 0.06), (0, 0, LEVEL - 0.03), "#a07a50")


LOW = 0.3  # fraction of a level kept when walls are drawn cut down


def low_wall(color, cap="#2a2724"):
    def build(_f, _v):
        h = LEVEL * LOW
        box((1, 1, h), (0, 0, h / 2), color)
        box((0.9, 0.9, 0.02), (0, 0, h + 0.005), cap)
    return build


def door_low(_f, _v):
    h = LEVEL * LOW
    for x in (-0.44, 0.44):
        box((0.12, 0.2, h), (x, 0, h / 2), "#6b4f33")
    box((0.76, 0.08, h), (0, 0, h / 2), "#9a7650")


def stone_wall(_f, _v):
    box((1, 1, LEVEL), (0, 0, LEVEL / 2), "#8e949c")
    rows = 4
    for r in range(rows):
        z = (r + 0.5) * LEVEL / rows
        box((1.02, 1.02, 0.025), (0, 0, (r + 1) * LEVEL / rows - 0.0125), "#6f747c")
        off = 0.25 if r % 2 else 0.0
        for k in range(2):
            x = -0.25 + k * 0.5 + off
            if abs(x) < 0.5:
                box((0.025, 1.02, LEVEL / rows), (x, 0, z), "#6f747c")
                box((1.02, 0.025, LEVEL / rows), (0, x, z), "#6f747c")


def door(_f, _v):
    for x in (-0.44, 0.44):
        box((0.12, 0.2, LEVEL), (x, 0, LEVEL / 2), "#6b4f33")
    box((1.0, 0.2, 0.12), (0, 0, LEVEL - 0.06), "#6b4f33")
    box((0.76, 0.08, LEVEL - 0.12), (0, 0, (LEVEL - 0.12) / 2), "#9a7650")
    box((0.05, 0.12, 0.05), (0.25, 0.04, LEVEL * 0.45), "#c8b040")


def bed(_f, _v):
    box((0.8, 0.95, 0.18), (0, 0, 0.09), "#6b4f33")
    box((0.82, 0.08, 0.42), (0, 0.46, 0.21), "#5a4029")  # headboard (north)
    box((0.72, 0.84, 0.08), (0, 0, 0.22), "#d8d0c0")
    box((0.74, 0.55, 0.1), (0, -0.16, 0.25), "#7a3a3a")
    box((0.5, 0.16, 0.08), (0, 0.3, 0.29), "#e8e2d8")


def campfire(_f, _v):
    for i in range(8):
        a = i / 8 * math.tau
        box((0.12, 0.1, 0.09), (math.cos(a) * 0.3, math.sin(a) * 0.3, 0.045), "#6e6e6e", rot=(0, 0, a))
    cyl(0.05, 0.5, (0, 0, 0.06), "#5a3f2a", verts=6, rot=(math.pi / 2, 0, 0.7))
    cyl(0.05, 0.5, (0, 0, 0.1), "#5a3f2a", verts=6, rot=(math.pi / 2, 0, -0.7))
    cone(0.16, 0.0, 0.42, (0, 0, 0.3), "#e8742a", verts=6, emit=1.0)
    cone(0.09, 0.0, 0.3, (0.02, -0.02, 0.3), "#f5c542", verts=5, emit=1.0)


def table(top_color="#7a5a3a", w=0.9, d=0.6, h=0.5):
    box((w, d, 0.07), (0, 0, h - 0.035), top_color)
    for x in (-w / 2 + 0.06, w / 2 - 0.06):
        for y in (-d / 2 + 0.06, d / 2 - 0.06):
            box((0.07, 0.07, h - 0.07), (x, y, (h - 0.07) / 2), "#5a4029")


def workbench(_f, _v):
    table()
    box((0.14, 0.18, 0.12), (-0.3, 0.12, 0.56), "#555a60")  # vise
    box((0.22, 0.16, 0.14), (0.2, 0.08, 0.57), "#2f6f6a")  # old-world device
    box((0.08, 0.02, 0.06), (0.2, -0.005, 0.6), "#40c8a0", emit=1.0)
    box((0.3, 0.05, 0.02), (-0.05, -0.18, 0.51), "#a0a4a8", rot=(0, 0, 0.3))


def stonecutter(_f, _v):
    table(top_color="#8a8a85")
    cyl(0.2, 0.03, (0.12, 0.0, 0.62), "#b0b4b8", verts=12, rot=(0, math.pi / 2, 0))
    box((0.24, 0.2, 0.14), (-0.25, 0.05, 0.57), "#9a9aa0")


def torch(_f, _v):
    cyl(0.1, 0.06, (0, 0, 0.03), "#5a5a5a", verts=8)
    cyl(0.03, 0.85, (0, 0, 0.45), "#5a4029", verts=6)
    cyl(0.06, 0.12, (0, 0, 0.88), "#4a4a4a", verts=6)
    cone(0.07, 0.0, 0.2, (0, 0, 1.02), "#f0a040", verts=5, emit=1.0)
    cone(0.035, 0.0, 0.12, (0.0, 0.0, 1.0), "#ffe080", verts=4, emit=1.0)


def stairs(_f, _v):
    n = 4
    for i in range(n):
        h = LEVEL * (i + 1) / n
        y = -0.5 + (i + 0.5) / n
        two_tone_box((0.9, 1.0 / n, h), (0, y, h / 2), "#a8a8a2", "#7f7f7a")


# ------------------------------------------------------------------ pawns
# Colonists are layered: skin / top / bottom / hair are baked white and tinted
# per colonist at runtime; details (eyes, boots, belt, helmets) keep colour.

RACES = {
    "human": dict(h=1.0, w=1.0),
    "elf": dict(h=1.08, w=0.88, ears=True),
    "dwarf": dict(h=0.8, w=1.22, beard=True),
}

# Hair styles per race (index = appearance style in the sim).
STYLES = {
    "human": ["short", "long", "bald"],
    "elf": ["long", "braid", "topknot"],
    "dwarf": ["helmet", "bald", "mohawk"],
}

# Frames: 0 idle, 1-4 walk cycle, 5 lying down, 6-7 work (raise, strike).
PAWN_FRAMES = 8
WORK_FRAMES = [6, 7]
TOOLS = ["pickaxe", "axe", "hammer", "sickle"]


def _limb(shoulder, side_x, length, angle, size, color, layer):
    """A limb hanging from `shoulder` rotated by `angle` about X (forward = +)."""
    x, z0 = side_x, shoulder
    y = math.sin(angle) * length / 2
    z = z0 - math.cos(angle) * length / 2
    box((size[0], size[1], length), (x, y, z), color, rot=(angle, 0, 0), layer=layer)
    return (x, math.sin(angle) * length, z0 - math.cos(angle) * length)


def _tool(kind, hand, angle):
    """Tool gripped at `hand`, shaft continuing along the arm direction."""
    d = (0.0, math.sin(angle), -math.cos(angle))
    rot = (math.pi + angle, 0, 0)

    def at(t):
        return (hand[0], hand[1] + d[1] * t, hand[2] + d[2] * t)

    shaft_len = 0.34 if kind != "sickle" else 0.16
    cyl(0.025, shaft_len, at(shaft_len / 2 - 0.04), "#6b4f33", verts=5, rot=rot, layer=TOOL)
    end = at(shaft_len - 0.04)
    if kind == "pickaxe":
        box((0.04, 0.34, 0.05), end, "#8d9196", rot=rot, layer=TOOL)
    elif kind == "axe":
        box((0.03, 0.14, 0.15), (end[0], end[1], end[2]), "#9aa0a6", rot=rot, layer=TOOL)
    elif kind == "hammer":
        box((0.08, 0.16, 0.08), end, "#6e7176", rot=rot, layer=TOOL)
    elif kind == "sickle":
        for i in range(4):
            a = i * 0.45
            box((0.02, 0.1, 0.03), (end[0], end[1] + math.sin(a) * 0.12, end[2] + math.cos(a) * 0.12 - 0.06), "#b0b4b8",
                rot=(math.pi + angle + a, 0, 0), layer=TOOL)


def _hair(race, style, head, hz, p):
    hl = HAIR
    if style == "helmet":
        box((head + 0.05, head + 0.05, 0.11), (0, 0, hz + head / 2), "#8d8d8d")
        box((0.04, 0.03, 0.12), (0, head / 2 + 0.03, hz + 0.02), "#7a7a7a")  # nose guard
    elif style == "bald":
        pass
    elif style == "mohawk":
        box((0.07, head + 0.02, 0.12), (0, 0, hz + head / 2 + 0.05), WHITE, layer=hl)
    else:
        box((head + 0.03, head + 0.03, 0.08), (0, -0.01, hz + head / 2 + 0.01), WHITE, layer=hl)
        box((head + 0.03, 0.06, head * 0.8), (0, -head / 2 - 0.02, hz + 0.02), WHITE, layer=hl)
        if style == "long":
            box((head + 0.02, 0.08, head * 1.5), (0, -head / 2 - 0.04, hz - 0.1), WHITE, layer=hl)
            for side in (-1, 1):
                box((0.04, head * 0.7, head * 0.9), (side * (head / 2 + 0.02), -0.03, hz - 0.03), WHITE, layer=hl)
        elif style == "braid":
            for i in range(4):
                box((0.06, 0.06, 0.08), (0, -head / 2 - 0.05, hz - 0.08 - i * 0.085), WHITE, layer=hl)
        elif style == "topknot":
            ico(0.07, (0, -0.04, hz + head / 2 + 0.07), WHITE, layer=hl)
    if p.get("beard"):
        box((head * 0.9, 0.08, head * 0.95), (0, head / 2 + 0.03, hz - head * 0.38), WHITE, layer=hl)
        box((head * 0.5, 0.07, 0.12), (0, head / 2 + 0.05, hz - head * 0.9), WHITE, layer=hl)


def pawn_builder(race, style, tool=None):
    p = RACES[race]

    def build(frame, _variant):
        if frame == 5:
            # Lying on the back, head to the north, centred on the cell.
            from mathutils import Matrix
            build(0, 0)
            m = Matrix.Translation((0, -0.5 * p["h"], 0.3)) @ Matrix.Rotation(math.pi, 4, "Z") @ Matrix.Rotation(math.pi / 2, 4, "X")
            for o in bpy.context.scene.objects:
                if o.type == "MESH":
                    o.matrix_world = m @ o.matrix_world
            return
        h, w = p["h"], p["w"]
        walking = 1 <= frame <= 4
        swing = math.radians(32) * math.sin((frame - 1) / 4 * math.tau) if walking else 0.0
        bob = 0.025 * abs(math.cos((frame - 1) / 4 * math.tau)) if walking else 0.0
        leg_l = 0.48 * h
        hip = leg_l + bob
        for side, a in ((-1, swing), (1, -swing)):
            foot = _limb(hip, side * 0.085 * w, leg_l, a, (0.12 * w, 0.13), WHITE, BOTTOM)
            box((0.13 * w, 0.2, 0.07), (foot[0], foot[1] + 0.03, foot[2] + 0.03), "#2d2a26", rot=(a * 0.5, 0, 0))
        torso_h = 0.42 * h
        box((0.36 * w, 0.22 * w, torso_h), (0, 0, hip + torso_h / 2), WHITE, layer=TOP)
        box((0.38 * w, 0.24 * w, 0.06), (0, 0, hip + 0.03), "#3a3028")  # belt
        box((0.06, 0.02, 0.05), (0, 0.125 * w, hip + 0.03), "#c8b040")  # buckle
        sh = hip + torso_h - 0.04
        arm_l = 0.4 * h
        if frame in WORK_FRAMES:
            arms = ((-1, 0.6), (1, 2.5 if frame == 6 else 0.8))
        else:
            arms = ((-1, -swing), (1, swing))
        hands = {}
        for side, a in arms:
            hand = _limb(sh, side * (0.18 * w + 0.055), arm_l, a, (0.1, 0.11), WHITE, TOP)
            box((0.09, 0.1, 0.08), hand, WHITE, layer=SKIN)
            hands[side] = (hand, a)
        if tool and frame in WORK_FRAMES:
            hand, a = hands[1]
            _tool(tool, hand, a)
        head = 0.27
        hz = hip + torso_h + head / 2 + 0.02
        box((0.12, 0.1, 0.05), (0, 0, hip + torso_h + 0.02), WHITE, layer=SKIN)  # neck
        box((head, head, head), (0, 0, hz), WHITE, layer=SKIN)
        box((0.04, 0.03, 0.05), (0, head / 2 + 0.01, hz - 0.01), WHITE, layer=SKIN)  # nose
        for ex in (-0.06, 0.06):
            box((0.04, 0.02, 0.04), (ex, head / 2, hz + 0.03), "#1c1a20")
        if p.get("ears"):
            for side in (-1, 1):
                cone(0.05, 0.0, 0.16, (side * (head / 2 + 0.05), 0, hz + 0.05), WHITE, verts=4,
                     rot=(0, side * math.radians(70), 0), layer=SKIN)
        _hair(race, style, head, hz, p)
    return build


# --------------------------------------------------------------- catalogue

PAWN_LAYERS = {"skin": SKIN, "top": TOP, "bottom": BOTTOM, "hair": HAIR, "detail": DETAIL}


def catalogue():
    jobs = []
    for b in BLOCKS:
        jobs.append(dict(name=f"block_{b}", canvas="block", build=block_builder(b), outline=False, variants=BLOCK_VARIANTS))
        jobs.append(dict(name=f"ramp_{b}", canvas="block", build=ramp_builder(b), rotations=4, outline=False))
    jobs += [
        dict(name="plant_ashpine", canvas="object", build=ashpine(1.0), variants=3),
        dict(name="plant_ashpine_young", canvas="object", build=ashpine(0.55), variants=2),
        dict(name="plant_duskberry", canvas="object", build=duskberry(False, False), variants=2),
        dict(name="plant_duskberry_young", canvas="object", build=duskberry(True, False), variants=2),
        dict(name="plant_duskberry_ripe", canvas="object", build=duskberry(False, True), variants=2),
        dict(name="plant_scrub", canvas="object", build=scrub, variants=3),
    ]
    for name, fn in [
        ("item_wood", item_wood), ("item_stone", item_stone), ("item_blocks", item_blocks),
        ("item_ore", item_ore), ("item_scrap", item_scrap), ("item_components", item_components),
        ("item_parts", item_parts), ("item_berries", item_berries), ("item_meal", item_meal),
    ]:
        jobs.append(dict(name=name, canvas="item", build=fn))
    jobs += [
        dict(name="bld_wood_wall", canvas="object", build=wood_wall, outline=False),
        dict(name="bld_stone_wall", canvas="object", build=stone_wall, outline=False),
        dict(name="bld_door", canvas="object", build=door, rotations=4),
        dict(name="bld_bed", canvas="object", build=bed, rotations=4),
        dict(name="bld_campfire", canvas="object", build=campfire),
        dict(name="bld_workbench", canvas="object", build=workbench, rotations=4),
        dict(name="bld_stonecutter", canvas="object", build=stonecutter, rotations=4),
        dict(name="bld_stairs", canvas="object", build=stairs, rotations=4, outline=False),
        dict(name="bld_torch", canvas="object", build=torch),
        dict(name="bld_wood_wall_low", canvas="object", build=low_wall("#8b6a45"), outline=False),
        dict(name="bld_stone_wall_low", canvas="object", build=low_wall("#8e949c"), outline=False),
        dict(name="bld_door_low", canvas="object", build=door_low, rotations=4),
    ]
    for race in RACES:
        for si, style in enumerate(STYLES[race]):
            jobs.append(dict(name=f"pawn_{race}_s{si}", canvas="pawn", build=pawn_builder(race, style),
                             rotations=8, frames=PAWN_FRAMES, layers=PAWN_LAYERS))
        for tool in TOOLS:
            jobs.append(dict(name=f"tool_{race}_{tool}", canvas="pawn", build=pawn_builder(race, STYLES[race][0], tool),
                             rotations=8, frame_ids=WORK_FRAMES, layers={"": TOOL}))
    return jobs
