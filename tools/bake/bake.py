"""Sprite bake pipeline: procedural low-poly models -> crisp iso pixel sprites
with albedo + normal maps for dynamic lighting.

Run (from the repo root):
    blender -b --factory-startup -P tools/bake/bake.py -- --out assets/sprites [--only SUBSTR] [--debug]

Projection contract with the game client (see docs/DESIGN.md §5, §9):
  * 2:1 dimetric, camera elevation 30 deg.
  * One cell = 1 x 1 Blender units on the ground; its diamond is 48 x 24 px.
  * One z-level = LEVEL Blender units tall = exactly 30 px on screen.
  * Map +x = Blender +X (east), map +y = Blender -Y (south). Models face +Y (north).
  * Camera angle a (0..3) sits at azimuth 45 + 90a deg; a=0 is south-east.
  * Rotatable sprites are baked with the camera fixed at a=0 and the model
    rotated clockwise; the client picks index (facing + camera) % n.

Passes (all rendered with Workbench FLAT + vertex colours, so we control every
pixel exactly):
  * albedo  - unlit base colour (tintable layers are white),
  * normal  - camera-space normal encoded as rgb = n * 0.5 + 0.5
              (emissive faces are written as (0.5, 0.5, 0.5) = "no normal"),
  * id      - layer id, used to split layered sprites (colonists).

Output: atlas.png (albedo), atlas_n.png (normals, same layout), manifest.ron
"""

import math
import os
import struct
import sys
import zlib

import bpy
import numpy as np
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(__file__))
import models  # noqa: E402

TILE_W = 48
PX_PER_UNIT = TILE_W / math.sqrt(2.0)  # horizontal pixels per Blender unit at 45 deg
LEVEL = 30.0 / (PX_PER_UNIT * math.cos(math.radians(30.0)))  # units per z-level
models.LEVEL = LEVEL

# Canvas presets: (w, h, anchor_x, anchor_y). The anchor is the projected
# ground centre of the cell (x+0.5, y+0.5, z).
CANVAS = {
    "block": (48, 54, 24, 42),
    "object": (144, 196, 72, 160),
    "pawn": (80, 104, 40, 84),
    "item": (48, 60, 24, 45),
}


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    out, only, debug = "assets/sprites", None, False
    i = 0
    while i < len(argv):
        if argv[i] == "--out":
            out = argv[i + 1]
            i += 1
        elif argv[i] == "--only":
            only = argv[i + 1]
            i += 1
        elif argv[i] == "--debug":
            debug = True
        i += 1
    return out, only, debug


# --------------------------------------------------------------------- scene

def setup_scene():
    s = bpy.context.scene
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    s.render.engine = "BLENDER_WORKBENCH"
    s.display.render_aa = "OFF"
    s.display.shading.light = "FLAT"
    s.display.shading.color_type = "VERTEX"
    s.display.shading.show_shadows = False
    s.display.shading.show_cavity = False
    s.display.shading.show_object_outline = False
    s.render.film_transparent = True
    s.render.resolution_percentage = 100
    s.render.image_settings.file_format = "PNG"
    s.render.image_settings.color_mode = "RGBA"
    cam_data = bpy.data.cameras.new("BakeCam")
    cam_data.type = "ORTHO"
    cam_data.clip_start = 0.1
    cam_data.clip_end = 200.0
    cam = bpy.data.objects.new("BakeCam", cam_data)
    s.collection.objects.link(cam)
    s.camera = cam
    return s, cam


def place_camera(cam, w, h, ax, ay):
    """Ortho camera so world origin lands on pixel boundary (ax, ay) of a w x h canvas."""
    s = bpy.context.scene
    s.render.resolution_x = w
    s.render.resolution_y = h
    big = max(w, h)
    cam.data.ortho_scale = big / PX_PER_UNIT
    cam.data.shift_x = -(ax - w / 2.0) / big
    cam.data.shift_y = (ay - h / 2.0) / big
    cam.rotation_euler = (math.radians(60.0), 0.0, math.radians(45.0))
    fwd = cam.rotation_euler.to_matrix() @ Vector((0.0, 0.0, -1.0))
    cam.location = -fwd * 50.0
    bpy.context.view_layer.update()


def srgb_to_linear(c):
    c = max(0.0, min(1.0, c))
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def set_pass_colors(cam, mode):
    """Writes per-face colours for one pass into a face-corner attribute. The
    render sRGB-encodes the attribute, so we store linear values."""
    to_cam = cam.matrix_world.to_3x3().inverted()
    for obj in bpy.context.scene.objects:
        if obj.type != "MESH":
            continue
        me = obj.data
        if "Col" in me.color_attributes:
            me.color_attributes.remove(me.color_attributes["Col"])
        attr = me.color_attributes.new("Col", "BYTE_COLOR", "CORNER")
        nrm_mat = obj.matrix_world.to_3x3().inverted().transposed()
        for poly in me.polygons:
            mat = me.materials[poly.material_index] if me.materials else None
            if mode == "albedo":
                base = mat.diffuse_color if mat else (1, 0, 1, 1)
                c = (base[0], base[1], base[2])
            elif mode == "normal":
                if mat is not None and mat.get("emit", 0.0) > 0.5:
                    c = (0.5, 0.5, 0.5)
                else:
                    n = (to_cam @ (nrm_mat @ poly.normal)).normalized()
                    c = (n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5)
            else:  # id
                layer = mat.get("layer", 0) if mat else 0
                c = (layer / 8.0, 0.0, 0.0)
            col = (srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), 1.0)
            for li in poly.loop_indices:
                attr.data[li].color = col
        me.color_attributes.active_color = attr


def clear_models():
    for o in list(bpy.data.objects):
        if o.type != "CAMERA":
            bpy.data.objects.remove(o, do_unlink=True)
    for m in list(bpy.data.meshes):
        if m.users == 0:
            bpy.data.meshes.remove(m)


TMP = os.path.join(bpy.app.tempdir or "/tmp", "colony_bake_tmp.png")


def render_pass(cam, mode, w, h):
    set_pass_colors(cam, mode)
    s = bpy.context.scene
    s.render.filepath = TMP
    bpy.ops.render.render(write_still=True)
    img = bpy.data.images.load(TMP, check_existing=False)
    px = np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4)[::-1].copy()
    bpy.data.images.remove(img)
    return px


# ---------------------------------------------------------------- post-process

N4 = ((0, 1), (0, -1), (1, 0), (-1, 0))


def shift(a, dy, dx):
    """out[y, x] = a[y + dy, x + dx] (zero outside)."""
    out = np.zeros_like(a)
    h, w = a.shape[0], a.shape[1]
    ys = slice(max(dy, 0), h + min(dy, 0))
    yd = slice(max(-dy, 0), h + min(-dy, 0))
    xs = slice(max(dx, 0), w + min(dx, 0))
    xd = slice(max(-dx, 0), w + min(-dx, 0))
    out[yd, xd] = a[ys, xs]
    return out


def postprocess(alb, nrm, ids, outline=True, levels=24):
    """Threshold alpha, posterise albedo, add a selective outline (a darkened
    copy of the neighbouring colour). Returns albedo, normal (float RGBA) and
    ids (int array or None)."""
    mask = alb[..., 3] > 0.5
    a = np.zeros_like(alb)
    a[..., :3] = np.round(np.clip(alb[..., :3], 0, 1) * (levels - 1)) / (levels - 1)
    a[..., 3] = mask
    n = np.zeros_like(nrm)
    n[..., :3] = nrm[..., :3]
    n[..., 3] = mask
    a[~mask] = 0
    n[~mask] = 0
    idv = None
    if ids is not None:
        idv = np.round(ids[..., 0] * 8).astype(np.int32)
        idv[~mask] = -1
    if outline:
        acc = np.zeros(mask.shape + (3,), dtype=np.float32)
        cnt = np.zeros(mask.shape, dtype=np.float32)
        nb_n = np.zeros_like(n)
        nb_id = np.full(mask.shape, -1, dtype=np.int32)
        for dy, dx in N4:
            m = shift(mask, dy, dx)
            acc += shift(a[..., :3], dy, dx) * m[..., None]
            cnt += m
            take = m & (nb_n[..., 3] == 0)
            nb_n[take] = shift(n, dy, dx)[take]
            if idv is not None:
                sid = shift(idv, dy, dx)
                t2 = m & (nb_id < 0)
                nb_id[t2] = sid[t2]
        edge = (~mask) & (cnt > 0)
        a[edge, :3] = acc[edge] / cnt[edge, None] * 0.42
        a[edge, 3] = 1.0
        n[edge] = nb_n[edge]
        n[edge, 3] = 1.0
        if idv is not None:
            idv[edge] = nb_id[edge]
    return a, n, idv


def trim(alb, nrm, ax, ay):
    alpha = alb[..., 3] > 0
    if not alpha.any():
        return alb[:1, :1], nrm[:1, :1], 0, 0
    ys, xs = np.where(alpha)
    y0, y1, x0, x1 = ys.min(), ys.max() + 1, xs.min(), xs.max() + 1
    return alb[y0:y1, x0:x1], nrm[y0:y1, x0:x1], ax - x0, ay - y0


# -------------------------------------------------------------------- sprites

class Sprite:
    def __init__(self, name, variants, dirs, frames):
        self.name = name
        self.variants = variants
        self.dirs = dirs
        self.frames = frames
        self.images = []  # (albedo, normal, ax, ay) ordered variant, dir, frame


def build_rotated(job, frame, variant, r):
    clear_models()
    job["build"](frame, variant)
    step = 360.0 / job.get("rotations", 1)
    rot = Matrix.Rotation(math.radians(-step * r), 4, "Z")
    for o in bpy.context.scene.objects:
        if o.type == "MESH" and o.parent is None:
            o.matrix_world = rot @ o.matrix_world
    bpy.context.view_layer.update()


def bake_job(cam, job):
    """Returns a list of Sprites (one for plain jobs, one per layer for layered jobs)."""
    w, h, ax, ay = CANVAS[job["canvas"]]
    place_camera(cam, w, h, ax, ay)
    variants = job.get("variants", 1)
    rotations = job.get("rotations", 1)
    frame_ids = job.get("frame_ids", list(range(job.get("frames", 1))))
    layers = job.get("layers")  # {suffix: id} for layered jobs
    outline = job.get("outline", True)
    if layers:
        sprites = {k: Sprite(f'{job["name"]}_{k}' if k else job["name"], variants, rotations, len(frame_ids)) for k in layers}
    else:
        sprites = {None: Sprite(job["name"], variants, rotations, len(frame_ids))}
    for v in range(variants):
        for r in range(rotations):
            for f in frame_ids:
                build_rotated(job, f, v, r)
                alb = render_pass(cam, "albedo", w, h)
                nrm = render_pass(cam, "normal", w, h)
                ids = render_pass(cam, "id", w, h) if layers else None
                a, n, idv = postprocess(alb, nrm, ids, outline=outline)
                if layers:
                    for k, lid in layers.items():
                        m = idv == lid
                        la = a * m[..., None]
                        ln = n * m[..., None]
                        sprites[k].images.append(trim(la, ln, ax, ay))
                else:
                    sprites[None].images.append(trim(a, n, ax, ay))
    return list(sprites.values())


# ---------------------------------------------------------------------- atlas

def pack(sprites, width=2048):
    imgs = [(s, i) for s in sprites for i in range(len(s.images))]
    order = sorted(imgs, key=lambda t: (-t[0].images[t[1]][0].shape[0], t[0].name, t[1]))
    x = y = shelf_h = 0
    pos = {}
    for s, i in order:
        p = s.images[i][0]
        hh, ww = p.shape[0], p.shape[1]
        if x + ww + 1 > width:
            x = 0
            y += shelf_h + 1
            shelf_h = 0
        pos[(s.name, i)] = (x, y)
        x += ww + 1
        shelf_h = max(shelf_h, hh)
    height = 1 << (y + shelf_h + 1 - 1).bit_length()
    alb = np.zeros((height, width, 4), dtype=np.uint8)
    nrm = np.zeros((height, width, 4), dtype=np.uint8)
    for s, i in imgs:
        a, n, _, _ = s.images[i]
        px, py = pos[(s.name, i)]
        alb[py:py + a.shape[0], px:px + a.shape[1]] = np.round(np.clip(a, 0, 1) * 255).astype(np.uint8)
        nrm[py:py + n.shape[0], px:px + n.shape[1]] = np.round(np.clip(n, 0, 1) * 255).astype(np.uint8)
    return alb, nrm, pos


def write_png(path, rgba):
    h, w = rgba.shape[0], rgba.shape[1]
    raw = b"".join(b"\x00" + rgba[y].tobytes() for y in range(h))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def write_manifest(path, sprites, pos, shape):
    lines = ["// Generated by tools/bake/bake.py - do not edit.", "("]
    lines.append(f'    atlas: "atlas.png", normals: "atlas_n.png", width: {shape[1]}, height: {shape[0]},')
    lines.append(f"    tile_w: {TILE_W},")
    lines.append("    sprites: {")
    for s in sorted(sprites, key=lambda s: s.name):
        rects = []
        for i, (a, _, ax, ay) in enumerate(s.images):
            x, y = pos[(s.name, i)]
            rects.append(f"({x}, {y}, {a.shape[1]}, {a.shape[0]}, {int(ax)}, {int(ay)})")
        lines.append(f'        "{s.name}": (variants: {s.variants}, dirs: {s.dirs}, frames: {s.frames}, rects: [{", ".join(rects)}]),')
    lines.append("    },")
    lines.append(")")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


# ----------------------------------------------------------------------- main

def main():
    out, only, debug = parse_args()
    os.makedirs(out, exist_ok=True)
    _, cam = setup_scene()
    sprites = []
    for job in models.catalogue():
        if only and only not in job["name"]:
            continue
        got = bake_job(cam, job)
        sprites.extend(got)
        print(f'baked {job["name"]}: {len(got)} sprite(s), {len(got[0].images)} frames each', flush=True)
        if debug:
            for sp in got:
                for i, (a, n, _, _) in enumerate(sp.images[:16]):
                    write_png(os.path.join(out, f"debug_{sp.name}_{i}.png"), np.round(a * 255).astype(np.uint8))
                    write_png(os.path.join(out, f"debug_{sp.name}_{i}_n.png"), np.round(n * 255).astype(np.uint8))
    alb, nrm, pos = pack(sprites)
    write_png(os.path.join(out, "atlas.png"), alb)
    write_png(os.path.join(out, "atlas_n.png"), nrm)
    write_manifest(os.path.join(out, "manifest.ron"), sprites, pos, alb.shape)
    print(f"atlas {alb.shape[1]}x{alb.shape[0]}, {len(sprites)} sprites", flush=True)


main()
