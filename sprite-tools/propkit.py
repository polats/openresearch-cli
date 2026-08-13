#!/usr/bin/env python3
"""propkit — 2D art to a game-ready 3D map prop, as commands rather than prose.

Sibling of spritekit.mjs, and the same bet: the deterministic parts of a pipeline
belong in a tool, not in a paragraph a model re-interprets. Two runs of this
pipeline from written guidance produced 74.4% and 93.0% silhouette IoU, and every
step that varied (voxel size, shell retention, collapse verification, tile height,
origin) has exactly one right answer. Those are encoded here. What is left to
judgement — which route, whether the result reads better than the art it replaces
— stays in the orx-mapprops skill.

Python rather than .mjs (spritekit's language) because every step here is Blender
or PIL/numpy; there is no JS in the game to reuse.

    propkit.py tiers   --game DIR
    propkit.py upres   ART.png -o BIG.png [--refine]   # FIRST: never plan from a thumbnail
    propkit.py isolate IMG -o OUT.png
    propkit.py mesh    IMG -o DIR [--seed N] [--comfy URL]
    propkit.py clean   RAW.glb --tier KIND --game DIR -o OUT.glb [--source ISO.png]
    propkit.py check   OUT.glb --tier KIND --game DIR [--source ISO.png]
    propkit.py atlas   --game DIR [--kind oak] -o ART.png
    propkit.py sweep   RAW.glb --tier KIND --game DIR --budgets 600,1200,2400 -o OUT.glb
    propkit.py author  ART.png --tier KIND --game DIR -o OUT.glb   # foliage route
    propkit.py preview OUT.glb -o DIR [--yaws 0,90,180,270]
    propkit.py texture OUT.glb --ref ART.png -o TEXTURED.glb
    propkit.py compare OUT.glb --source ART.png --tier KIND --game DIR -o CMP.png

Measured facts this encodes, each of which cost a run to learn:

* Background removal is mandatory, and compositing onto white is NOT removal —
  white is still a surface, and image-to-3D reconstructs it. Real isolation halved
  a boulder's triangle count (1,359,688 -> 675,750).
* Neither mesh extractor is usable raw. `surface net` is connected but carries
  sliver faces up to 1,200:1 aspect, which is what makes collapse produce spikes.
  `basic` has perfect 1.414 triangles but is completely unindexed (one component
  per triangle pair).
* Weld alone does not give a manifold: it cleared 1,318,064 boundary edges but
  left 202 non-manifold ones. A FINE voxel pass (0.0065) plus largest-shell
  retention is what makes the result watertight.
* Collapse decimation undershoots its ratio badly on a remeshed manifold —
  requested 2,500, got 6,188 — so it must iterate and then be verified.
* QuadriFlow rejects these meshes. Recorded so nobody re-proposes it.
* Colour can only come from a single-view projection: the installed Hunyuan3D
  output carries POSITION only, so unseen-side colour does not exist to recover.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

# --- measured constants -------------------------------------------------------
# Voxel size for the remesh. Coarser (0.012) fuses adjacent detail into lobes;
# this keeps structure while still producing one watertight shell.
VOXEL_SIZE = 0.0065
# Weld distance. Large enough to index `basic`'s duplicated corners, small enough
# not to merge genuinely separate surfaces.
WELD_DIST = 0.0005
# Collapse undershoots, so cap the passes rather than trusting one.
COLLAPSE_PASSES = 5
# A face this far from equilateral is a sliver, and slivers are what decimation
# turns into spikes.
ASPECT_LIMIT = 10.0

BLENDER = os.environ.get("PROPKIT_BLENDER", "/snap/blender/7599/blender")
COMFY = os.environ.get("PROPKIT_COMFY", "http://127.0.0.1:8188")


def _link(path: str, label: str = None) -> dict:
    """Paste-ready markdown for an artifact, relative to the project's files dir.

    The chat renders a LINKED image or video inline and turns a linked .glb into a
    chip that opens an orbit viewer — but only for paths relative to the files dir.
    An absolute path renders as plain text, so every command hands back the exact
    string to paste. Progress the user cannot see is progress they must take on trust.
    """
    ap = os.path.abspath(path)
    m = re.search(r"/files/[^/]+/(.+)$", ap)
    if not m:
        return {"link": None, "note": "not under a project files dir; a link will not render"}
    rel = m.group(1)
    label = label or os.path.splitext(os.path.basename(rel))[0]
    return {"link": f"[{label}]({rel})", "rel": rel}


def die(msg: str) -> None:
    print(f"propkit: {msg}", file=sys.stderr)
    raise SystemExit(2)


# --- the game's own numbers ---------------------------------------------------
def read_tiers(game: str) -> dict:
    """Prop heights and atlas slots, parsed from the game's source.

    Never hardcoded here. The first sprite contract was written from memory and
    rejected all 25 shipped sprites; derived from the game instead, 75/75 passed.
    """
    scale = os.path.join(game, "src/render/scale.ts")
    textures = os.path.join(game, "src/render/textures.ts")
    for p in (scale, textures):
        if not os.path.isfile(p):
            die(f"not found: {p}\n  --game must point at the target game's checkout")

    s = open(scale).read()
    m = re.search(r"FLORA_TILES:\s*Record<FloraKind,\s*number>\s*=\s*\{(.*?)\}", s, re.S)
    if not m:
        die("could not find FLORA_TILES in scale.ts — has the game moved it?")
    tiles = {k: float(v) for k, v in re.findall(r"(\w+)\s*:\s*([0-9.]+)", m.group(1))}

    t = open(textures).read()
    m = re.search(r"FLORA:\s*Record<FloraKind,\s*FloraSlot>\s*=\s*\{(.*?)\n\}", t, re.S)
    slots = {}
    if m:
        for kind, body in re.findall(r"(\w+)\s*:\s*\{([^}]*)\}", m.group(1)):
            d = {k: int(v) for k, v in re.findall(r"(\w+)\s*:\s*(\d+)", body)}
            if {"w", "h"} <= d.keys():
                slots[kind] = d
    ppu = re.search(r"PX_PER_UNIT\s*=\s*([0-9.]+)", t) or re.search(r"PX_PER_UNIT\s*=\s*([0-9.]+)", s)
    return {
        "tiles": tiles,
        "slots": slots,
        "pxPerUnit": float(ppu.group(1)) if ppu else None,
    }


def tier_of(game: str, kind: str) -> dict:
    t = read_tiers(game)
    if kind not in t["tiles"]:
        die(f"unknown --tier {kind}; the game defines: {', '.join(sorted(t['tiles']))}")
    slot = t["slots"].get(kind, {})
    height = t["tiles"][kind]
    # Triangle budget scales with apparent size: a 3-tile tree earns more than a
    # 0.28-tile tuft. Bands, not a single number, because that is how the game's
    # own scale bands are authored.
    # Measured floors, not aspirations: collapse shatters a remeshed manifold below
    # some ratio and `clean` reverts rather than ship broken geometry. A 3.2-tile
    # conifer could not hold 1200 (reverted at 2 bad edges) but held 2391; an oak
    # made 1200 exactly. Bands are the reachable range, so `budget_unreachable`
    # means the mesh is unusually complex, not that the tool failed.
    budget = 2400 if height >= 2.5 else 800 if height >= 1.0 else 400 if height >= 0.3 else 120
    return {"kind": kind, "height": height, "slot": slot, "budget": budget,
            "pxPerUnit": t["pxPerUnit"]}


# --- isolate ------------------------------------------------------------------
def cmd_isolate(a) -> None:
    from PIL import Image, ImageFilter
    import numpy as np
    from scipy import ndimage

    im = Image.open(a.image).convert("RGB")
    rgb = np.asarray(im)
    hsv = np.asarray(im.convert("HSV")).astype(np.float32)
    sat, val = hsv[..., 1], hsv[..., 2]

    # Key on "saturated OR dark". A concept render's field and its ground shadow
    # are both low-saturation mid-value; the subject is either coloured or in
    # shadow. Keying on saturation alone keeps the shadow (which then becomes
    # geometry); keying on darkness alone loses lit surfaces.
    mask = (sat > a.sat) | (val < a.dark)
    if a.refine:
        # A lit concept render defeats a pure HSV key two ways: pale facets are
        # desaturated AND bright, and the generator's ground shadow is desaturated
        # AND mid-value. Distance from the border colour catches the pale facets;
        # a bottom-band gate on colour drops the shadow without eating the base.
        border = np.concatenate([rgb[0, :, :], rgb[-1, :, :], rgb[:, 0, :], rgb[:, -1, :]])
        bg = np.median(border, axis=0)
        dist = np.abs(rgb.astype(np.int16) - bg.astype(np.int16)).sum(axis=2)
        mask = mask | (dist > a.bg_dist)
        h = rgb.shape[0]
        band = np.zeros(mask.shape, bool); band[int(h * 0.72):, :] = True
        # In the ground band, keep only pixels that are actually coloured — a soft
        # grey ellipse under the subject is shadow, not geometry.
        mask = mask & ~(band & (dist <= a.bg_dist * 1.6) & (sat <= a.sat))
    mask = np.asarray(
        Image.fromarray((mask * 255).astype(np.uint8)).filter(ImageFilter.MedianFilter(5))
    ) > 127
    # Drop everything but the biggest blob: stray keyed specks become stray shells.
    lab, n = ndimage.label(mask)
    if n > 1:
        sizes = ndimage.sum(mask, lab, range(1, n + 1))
        mask = lab == (int(np.argmax(sizes)) + 1)
    if not mask.any():
        die("isolate produced an empty mask — try lowering --sat or raising --dark")

    rgb_out = rgb.copy()
    if a.flatten != "keep":
        fill = {"white": (255, 255, 255), "grey": (128, 128, 128),
                "magenta": (255, 0, 255), "black": (0, 0, 0)}[a.flatten]
        rgb_out[~mask] = fill
    out = np.dstack([rgb_out, (mask * 255).astype(np.uint8)])
    Image.fromarray(out, "RGBA").save(a.out)
    ys, xs = np.where(mask)
    print(json.dumps({
        "out": a.out, **_link(a.out, "isolated"),
        "coverage_pct": round(float(mask.mean()) * 100, 2),
        "bbox_xyxy": [int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1],
        "components_discarded": max(0, n - 1),
    }, indent=2))


# --- mesh ---------------------------------------------------------------------
HUNYUAN_GRAPH = {
    "1": {"class_type": "ImageOnlyCheckpointLoader",
          "inputs": {"ckpt_name": "hunyuan3d-dit-v2-0-fp16.safetensors"}},
    "2": {"class_type": "LoadImage", "inputs": {"image": "__IMAGE__"}},
    "3": {"class_type": "CLIPVisionEncode",
          "inputs": {"clip_vision": ["1", 1], "image": ["2", 0], "crop": "none"}},
    "4": {"class_type": "Hunyuan3Dv2Conditioning", "inputs": {"clip_vision_output": ["3", 0]}},
    "5": {"class_type": "EmptyLatentHunyuan3Dv2", "inputs": {"resolution": 3072, "batch_size": 1}},
    "6": {"class_type": "ModelSamplingAuraFlow", "inputs": {"model": ["1", 0], "shift": 1.0}},
    "7": {"class_type": "KSampler",
          "inputs": {"model": ["6", 0], "seed": 0, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "scheduler": "normal",
                     "positive": ["4", 0], "negative": ["4", 1],
                     "latent_image": ["5", 0], "denoise": 1.0}},
    "8": {"class_type": "VAEDecodeHunyuan3D",
          "inputs": {"samples": ["7", 0], "vae": ["1", 2],
                     "num_chunks": 8000, "octree_resolution": 256}},
    # Both extractors off ONE latent: two topologies for one inference cost.
    "9": {"class_type": "VoxelToMesh",
          "inputs": {"voxel": ["8", 0], "algorithm": "surface net", "threshold": 0.6}},
    "10": {"class_type": "SaveGLB", "inputs": {"mesh": ["9", 0], "filename_prefix": "__P__/surfacenet"}},
    "11": {"class_type": "VoxelToMesh",
           "inputs": {"voxel": ["8", 0], "algorithm": "basic", "threshold": 0.6}},
    "12": {"class_type": "SaveGLB", "inputs": {"mesh": ["11", 0], "filename_prefix": "__P__/basic"}},
}


def comfy_post(path: str, payload: dict, base: str) -> dict:
    req = urllib.request.Request(base + path, json.dumps(payload).encode(),
                                {"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=120))


def cmd_mesh(a) -> None:
    _guard_source_size(a.image, "mesh")
    base = a.comfy.rstrip("/")
    name = os.path.basename(a.image)
    dest = os.path.join(os.path.expanduser("~/ComfyUI/input"), name)
    if os.path.abspath(a.image) != os.path.abspath(dest):
        shutil.copyfile(a.image, dest)

    g = json.loads(json.dumps(HUNYUAN_GRAPH).replace("__IMAGE__", name).replace("__P__", "propkit"))
    g["7"]["inputs"]["seed"] = a.seed
    try:
        pid = comfy_post("/prompt", {"prompt": g}, base)["prompt_id"]
    except urllib.error.HTTPError as e:  # noqa: F821
        die(f"ComfyUI rejected the graph: {e.read().decode()[:400]}")
    print(f"  queued {pid} (seed {a.seed}) — two extractors from one latent", file=sys.stderr)

    deadline = time.time() + a.timeout
    while time.time() < deadline:
        h = json.load(urllib.request.urlopen(f"{base}/history/{pid}", timeout=30))
        if h:
            st = h[pid]["status"]
            if st.get("status_str") != "success":
                die(f"generation failed: {json.dumps(st.get('messages', []))[:500]}")
            os.makedirs(a.out, exist_ok=True)
            got = {}
            for o in h[pid]["outputs"].values():
                for f in o.get("3d", []) + o.get("images", []):
                    src = os.path.join(os.path.expanduser("~/ComfyUI/output"),
                                       f.get("subfolder", ""), f["filename"])
                    if not src.endswith(".glb"):
                        continue
                    which = "basic" if "basic" in f["filename"] else "surfacenet"
                    dst = os.path.join(a.out, f"raw-{which}.glb")
                    shutil.copyfile(src, dst)
                    got[which] = dst
            print(json.dumps({"prompt_id": pid, "seed": a.seed, "meshes": got}, indent=2))
            return
        time.sleep(8)
    die(f"generation did not finish within {a.timeout}s")


# --- Blender-side steps -------------------------------------------------------
CLEAN_PY = r'''
import bpy, bmesh, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=A["raw"])
obj = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
bpy.context.view_layer.objects.active = obj; obj.select_set(True)
rep = {"in_tris": len(obj.data.polygons)}

# 1. WELD. `basic` output is unindexed, so without this every triangle is its own
#    component and nothing downstream can traverse the surface.
bpy.ops.object.mode_set(mode="EDIT")
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.remove_doubles(threshold=A["weld"])
bpy.ops.object.mode_set(mode="OBJECT")
rep["after_weld_tris"] = len(obj.data.polygons)

# 2. FINE VOXEL REMESH. Welding alone leaves non-manifold edges; this is what
#    actually produces a single watertight shell.
#    SKIPPED with --no-remesh: remeshing rebuilds topology from scratch and so
#    DISCARDS UVs and textures. A reconstructor that emits real UVs and a texture
#    (Pixal3D does; Hunyuan3D does not) loses its whole advantage here, so for that
#    input collapse straight from the dense mesh instead.
if A.get("remesh", True):
    r = obj.modifiers.new("rm", "REMESH"); r.mode = "VOXEL"; r.voxel_size = A["voxel"]
    bpy.ops.object.modifier_apply(modifier="rm")
rep["after_remesh_tris"] = len(obj.data.polygons)
rep["remeshed"] = bool(A.get("remesh", True))

# 3. LARGEST SHELL ONLY. Floating fragments survive remeshing and become stray
#    geometry in the game.
bm = bmesh.new(); bm.from_mesh(obj.data)
groups, seen = [], set()
for f in bm.faces:
    if f in seen: continue
    stack, comp = [f], []
    while stack:
        cur = stack.pop()
        if cur in seen: continue
        seen.add(cur); comp.append(cur)
        for e in cur.edges:
            for nf in e.link_faces:
                if nf not in seen: stack.append(nf)
    groups.append(comp)
groups.sort(key=len, reverse=True)
rep["components"] = len(groups)
if len(groups) > 1:
    bmesh.ops.delete(bm, geom=[f for g in groups[1:] for f in g], context="FACES")
    rep["discarded_faces"] = sum(len(g) for g in groups[1:])
bm.to_mesh(obj.data); bm.free()

# 4. VERIFIED ITERATED COLLAPSE. One pass undershoots badly (2500 requested ->
#    6188), so it must loop. But collapse also SHATTERS a remeshed manifold below
#    some ratio: 1200 requested from a 325k mesh produced 610 components and 2410
#    boundary edges. So verify after every pass and keep the last good state. The
#    budget is a target, not a licence to destroy the mesh.
def topology():
    b = bmesh.new(); b.from_mesh(obj.data)
    bad = sum(1 for e in b.edges if len(e.link_faces) != 2)
    seen, comps = set(), 0
    for f in b.faces:
        if f in seen: continue
        comps += 1; stack = [f]
        while stack:
            c = stack.pop()
            if c in seen: continue
            seen.add(c)
            for e in c.edges:
                for nf in e.link_faces:
                    if nf not in seen: stack.append(nf)
    b.free()
    return comps, bad

rep["collapse"] = []
base_c, base_bad = topology()
rep["baseline"] = {"components": base_c, "bad_edges": base_bad}
best = obj.data.copy()
for i in range(A["passes"]):
    n = len(obj.data.polygons)
    if n <= A["budget"]: break
    d = obj.modifiers.new(f"d{i}", "DECIMATE"); d.decimate_type = "COLLAPSE"
    d.ratio = min(1.0, A["budget"] / n)
    bpy.ops.object.modifier_apply(modifier=f"d{i}")
    comps, bad = topology()
    rep["collapse"].append({"pass": i, "tris": len(obj.data.polygons),
                            "components": comps, "bad_edges": bad})
    # Relative test: a pass is acceptable if it did not make connectivity worse
    # than the mesh already was. Absolute perfection is the right bar for a closed
    # marching-cubes mesh and an impossible one for an open-surface reconstruction.
    # A ratio on a baseline of 1 component rejects any split at all: a 40x reduction
    # that went 1 -> 3 components while REDUCING bad edges was thrown away. Allow a
    # small absolute drift instead.
    worse = comps > max(base_c + 8, base_c * 2) or bad > max(base_bad, 0) * 1.5 + 8
    if worse:
        obj.data = best
        rep["collapse"][-1]["reverted"] = True
        rep["budget_unreachable"] = True
        break
    best = obj.data.copy()
rep["final_tris"] = len(obj.data.polygons)

# 4b. REPAIR. Verification can still find a handful of bad edges after collapse.
#     An audited run answered that by hand-writing its own weld pass, so encode it:
#     weld at a hair's width, fill the holes that opens, and re-verify.
if A.get("repair"):
    comps, bad = topology()
    if bad or comps != 1:
        bm = bmesh.new(); bm.from_mesh(obj.data)
        bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=1e-5)
        open_edges = [e for e in bm.edges if len(e.link_faces) == 1]
        if open_edges:
            bmesh.ops.holes_fill(bm, edges=open_edges, sides=8)
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
        bm.to_mesh(obj.data); bm.free()
        after_c, after_b = topology()
        rep["repaired"] = {"before": {"components": comps, "bad_edges": bad},
                           "after": {"components": after_c, "bad_edges": after_b},
                           "tris": len(obj.data.polygons)}
        rep["final_tris"] = len(obj.data.polygons)

# 5. Angle-limited smoothing: full smoothing averages normals across the creases
#    between real features and makes the surface read as melted.
try: bpy.ops.object.shade_auto_smooth(angle=math.radians(35))
except Exception: bpy.ops.object.shade_smooth()

# 6. SCALE to the game's authored height, then set the origin to base-centre —
#    the anchor convention the game's own atlas uses.
vs = [v.co for v in obj.data.vertices]
h = max(v.z for v in vs) - min(v.z for v in vs)
obj.scale = (A["height"] / h,) * 3
bpy.ops.object.transform_apply(scale=True)
vs = [v.co for v in obj.data.vertices]
minz = min(v.z for v in vs)
cx = (min(v.x for v in vs) + max(v.x for v in vs)) / 2
cy = (min(v.y for v in vs) + max(v.y for v in vs)) / 2
for v in obj.data.vertices: v.co.x -= cx; v.co.y -= cy; v.co.z -= minz

# 7. COLOUR by front projection. This is the only colour available: the generator
#    emits POSITION only, so unseen-side colour does not exist to recover. Derive
#    the mapping from the MESH bbox — glTF is Y-up and Blender is Z-up, so using
#    the wrong axis sends UVs outside [0,1] and the texture tiles.
if A.get("source"):
    img = bpy.data.images.load(A["source"])
    mat = bpy.data.materials.new("prop"); mat.use_nodes = True
    nt = mat.node_tree
    tex = nt.nodes.new("ShaderNodeTexImage"); tex.image = img; tex.extension = "EXTEND"
    xs = [v.co.x for v in obj.data.vertices]; zs = [v.co.z for v in obj.data.vertices]
    sx = 1.0 / (max(xs) - min(xs)); sz = 1.0 / (max(zs) - min(zs))
    co = nt.nodes.new("ShaderNodeTexCoord"); sep = nt.nodes.new("ShaderNodeSeparateXYZ")
    mx = nt.nodes.new("ShaderNodeMath"); mx.operation = "MULTIPLY_ADD"
    mx.inputs[1].default_value = sx; mx.inputs[2].default_value = -min(xs) * sx
    mz = nt.nodes.new("ShaderNodeMath"); mz.operation = "MULTIPLY_ADD"
    mz.inputs[1].default_value = sz; mz.inputs[2].default_value = -min(zs) * sz
    cb = nt.nodes.new("ShaderNodeCombineXYZ")
    nt.links.new(co.outputs["Object"], sep.inputs["Vector"])
    nt.links.new(sep.outputs["X"], mx.inputs[0]); nt.links.new(sep.outputs["Z"], mz.inputs[0])
    nt.links.new(mx.outputs["Value"], cb.inputs["X"]); nt.links.new(mz.outputs["Value"], cb.inputs["Y"])
    nt.links.new(cb.outputs["Vector"], tex.inputs["Vector"])
    nt.links.new(tex.outputs["Color"], nt.nodes["Principled BSDF"].inputs["Base Color"])
    nt.nodes["Principled BSDF"].inputs["Roughness"].default_value = 0.85
    obj.data.materials.clear(); obj.data.materials.append(mat)
    bpy.ops.object.mode_set(mode="EDIT"); bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=math.radians(66))
    bpy.ops.object.mode_set(mode="OBJECT")

bpy.ops.export_scene.gltf(filepath=A["out"], export_format="GLB", use_selection=True)
print("PROPKIT_REPORT=" + json.dumps(rep))
'''

CHECK_PY = r'''
import bpy, bmesh, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=A["glb"])
obj = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
me = obj.data
# Topology must be measured on a WELDED copy. glTF cannot share a vertex between
# two UVs or two normals, so export duplicates vertices along every seam; a raw
# re-import of a perfectly watertight mesh reports hundreds of components and
# thousands of boundary edges that exist only in the file format. Counts here are
# about the SURFACE, so weld first — but report tris/verts as they ship.
bm = bmesh.new(); bm.from_mesh(me)
bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=1e-5)
boundary = sum(1 for e in bm.edges if len(e.link_faces) == 1)
nonman = sum(1 for e in bm.edges if len(e.link_faces) > 2)
seen, comps = set(), 0
for f in bm.faces:
    if f in seen: continue
    comps += 1; stack = [f]
    while stack:
        c = stack.pop()
        if c in seen: continue
        seen.add(c)
        for e in c.edges:
            for nf in e.link_faces:
                if nf not in seen: stack.append(nf)
worst = 0.0
for f in bm.faces:
    ls = [e.calc_length() for e in f.edges]
    if min(ls) > 0: worst = max(worst, max(ls) / min(ls))
bm.free()

def effective_colour(m):
    """What this material will actually render as."""
    if not (m and m.use_nodes and "Principled BSDF" in m.node_tree.nodes):
        return None
    bsdf = m.node_tree.nodes["Principled BSDF"]
    inp = bsdf.inputs["Base Color"]
    if inp.is_linked:
        node = inp.links[0].from_node
        # Walk one hop through common passthroughs.
        while node.type in {"MIX_RGB", "MIX", "GAMMA", "BRIGHTCONTRAST", "HUE_SAT"}:
            linked = [i for i in node.inputs if i.is_linked]
            if not linked: return None
            node = linked[0].links[0].from_node
        if node.type == "TEX_IMAGE" and node.image:
            px = list(node.image.pixels)
            if not px: return None
            n = len(px) // 4
            step = max(1, n // 4096)          # sample, do not walk 1M pixels
            acc, cnt = [0.0, 0.0, 0.0], 0
            for i in range(0, n, step):
                a_ = px[i * 4 + 3]
                if a_ <= 0.5: continue
                for c in range(3): acc[c] += px[i * 4 + c]
                cnt += 1
            if not cnt: return None
            return [round(min(1.0, acc[c] / cnt) ** (1 / 2.2) * 255) for c in range(3)]
        return None
    return [round(c * 255) for c in inp.default_value[:3]]

eff = [effective_colour(m) for m in me.materials]
vs = [v.co for v in me.vertices]
dims = [max(v[i] for v in vs) - min(v[i] for v in vs) for i in range(3)]
print("PROPKIT_REPORT=" + json.dumps({
    "tris": len(me.polygons), "verts": len(me.vertices),
    "components": comps, "boundary_edges": boundary, "non_manifold_edges": nonman,
    "worst_aspect": round(worst, 2),
    "dims": [round(d, 4) for d in dims],
    "min_z": round(min(v.z for v in vs), 5),
    "centre_xy": [round((min(v.x for v in vs) + max(v.x for v in vs)) / 2, 5),
                  round((min(v.y for v in vs) + max(v.y for v in vs)) / 2, 5)],
    "has_uv": bool(me.uv_layers), "materials": len(me.materials),
    # Base colours of every material, so `check` can tell a coloured prop from a
    # grey one. A colourless blob passed every structural criterion once; silhouette
    # IoU cannot see colour, so the contract has to look at it directly.
    "material_colors": [
        [round(c * 255) for c in (m.node_tree.nodes["Principled BSDF"]
                                   .inputs["Base Color"].default_value[:3])]
        if (m and m.use_nodes and "Principled BSDF" in m.node_tree.nodes) else None
        for m in me.materials
    ],
    "effective_colors": eff,
}))
'''


def run_blender(script: str, payload: dict) -> dict:
    if not os.path.isfile(BLENDER):
        die(f"blender not found at {BLENDER} — set PROPKIT_BLENDER")
    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
        f.write(script); path = f.name
    try:
        r = subprocess.run(
            [BLENDER, "--background", "--factory-startup", "--python", path,
             "--", json.dumps(payload)],
            capture_output=True, text=True, timeout=1800)
    finally:
        os.unlink(path)
    for line in r.stdout.splitlines():
        if line.startswith("PROPKIT_REPORT="):
            return json.loads(line.split("=", 1)[1])
    die(f"blender produced no report.\nstdout tail:\n{r.stdout[-1200:]}\nstderr:\n{r.stderr[-600:]}")


def cmd_clean(a) -> None:
    t = tier_of(a.game, a.tier)
    rep = run_blender(CLEAN_PY, {
        "raw": os.path.abspath(a.raw), "out": os.path.abspath(a.out),
        "weld": WELD_DIST, "voxel": a.voxel, "passes": COLLAPSE_PASSES,
        "budget": a.budget or t["budget"], "height": t["height"],
        "source": os.path.abspath(a.source) if a.source else None,
        "repair": a.repair, "remesh": not a.no_remesh,
    })
    rep["tier"] = t
    print(json.dumps(rep, indent=2))


def cmd_check(a) -> None:
    t = tier_of(a.game, a.tier)
    rep = run_blender(CHECK_PY, {"glb": os.path.abspath(a.glb)})
    budget = a.budget or t["budget"]
    # Blender converts glTF Y-up to Z-up on import, so height is the Z extent.
    height = rep["dims"][2]
    fails = []
    if rep["tris"] > budget:
        fails.append(f"tris {rep['tris']} over budget {budget}")
    if rep["components"] != 1:
        fails.append(f"{rep['components']} components (want 1)")
    if rep["boundary_edges"]:
        fails.append(f"{rep['boundary_edges']} boundary edges (not watertight)")
    if rep["non_manifold_edges"]:
        fails.append(f"{rep['non_manifold_edges']} non-manifold edges")
    if rep["worst_aspect"] > ASPECT_LIMIT:
        fails.append(f"worst aspect {rep['worst_aspect']} over {ASPECT_LIMIT}")
    if abs(height - t["height"]) > 0.01:
        fails.append(f"height {height} != authored {t['height']} tiles")
    if abs(rep["min_z"]) > 1e-3:
        fails.append(f"origin not at base (min axis {rep['min_z']})")
    fails += colour_check(a, rep)
    if a.source:
        fails += silhouette_check(a, rep)
    rep["tier"] = t
    rep.update(_link(a.glb, "prop"))
    rep["failures"] = fails
    rep["ok"] = not fails
    print(json.dumps(rep, indent=2))
    if fails:
        raise SystemExit(1)


def colour_check(a, rep: dict) -> list:
    """Does the prop actually carry the source art's colour?

    A delivered prop once passed every structural criterion while being a grey blob:
    `author` had assigned five palette materials and a later `clean` without
    `--source` replaced them with nothing. Structure and silhouette are both blind
    to that, so assert it.
    """
    cols = [c for c in (rep.get("effective_colors") or []) if c]
    if not cols:
        return ["no material resolves to a colour — the prop would render untextured "
                "grey. A texture NODE is not enough; it must be linked to Base Color "
                "with a real image (did `clean` run without --source?)"]
    out = []
    # Grey means every channel within a few points of the others.
    grey = [c for c in cols if max(c) - min(c) <= 12]
    if len(grey) == len(cols):
        out.append(f"every material is greyscale {cols} — colour was lost somewhere "
                   f"after the art was read")
    if not a.source:
        return out
    # And the colours should come from the art, not be invented.
    from PIL import Image
    import numpy as np
    src = Image.open(a.source).convert("RGBA")
    arr = np.asarray(src)
    px = arr[..., :3][arr[..., 3] > 128].astype(int)
    if len(px) == 0:
        return out
    far = []
    for c in cols:
        d = int(np.abs(px - np.array(c)).sum(axis=1).min())
        if d > a.max_colour_dist:
            far.append((c, d))
    if far and len(far) == len(cols):
        out.append(f"no material colour appears in the source art (closest "
                   f"distances {[d for _, d in far]}, limit {a.max_colour_dist})")
    rep["colour"] = {"materials": cols, "greyscale": len(grey), "checked_against_source": True}
    return out


def silhouette_check(a, rep: dict) -> list:
    """Front silhouette IoU against the isolated source. The source's own alpha is
    the ground truth: no mesh ships yet, so there is nothing else to compare to."""
    from PIL import Image
    import numpy as np
    src = Image.open(a.source)
    if src.mode != "RGBA":
        return ["--source has no alpha; run `propkit isolate` first"]
    ssil = np.asarray(src)[..., 3] > 128
    # Reuse the renderer we already have rather than adding a GL dependency.
    png = os.path.splitext(a.glb)[0] + "-silhouette.png"
    run_blender(RENDER_PY, {"glb": os.path.abspath(a.glb), "out": os.path.abspath(png)})
    msil = np.asarray(Image.open(png).convert("RGBA"))[..., 3] > 128

    def norm(m, n=512):
        ys, xs = np.where(m)
        c = Image.fromarray((m * 255).astype("uint8")).crop((xs.min(), ys.min(), xs.max() + 1, ys.max() + 1))
        return np.asarray(c.resize((n, n), Image.NEAREST)) > 127

    A_, B_ = norm(msil), norm(ssil)
    inter, union = (A_ & B_).sum(), (A_ | B_).sum()
    rep["silhouette"] = {
        "iou": round(float(inter / union), 4),
        "mesh_only_pct": round(float((A_ & ~B_).sum() / union) * 100, 2),
        "source_only_pct": round(float((B_ & ~A_).sum() / union) * 100, 2),
        "render": png,
    }
    return [] if rep["silhouette"]["iou"] >= a.min_iou else [
        f"silhouette IoU {rep['silhouette']['iou']} below --min-iou {a.min_iou}"]


RENDER_PY = r'''
import bpy, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=A["glb"])
obj = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
vs = [v.co for v in obj.data.vertices]
w = max(v.x for v in vs) - min(v.x for v in vs)
h = max(v.z for v in vs) - min(v.z for v in vs)
cx = (min(v.x for v in vs) + max(v.x for v in vs)) / 2
cz = (min(v.z for v in vs) + max(v.z for v in vs)) / 2
cd = bpy.data.cameras.new("c"); cd.type = "ORTHO"; cd.ortho_scale = max(w, h) * 1.05
cam = bpy.data.objects.new("c", cd); bpy.context.scene.collection.objects.link(cam)
bpy.context.scene.camera = cam
cam.location = (cx, -5.0, cz); cam.rotation_euler = (math.radians(90), 0, 0)
s = bpy.context.scene
s.render.engine = "BLENDER_WORKBENCH"   # silhouette only; no lighting needed
s.render.film_transparent = True
s.render.resolution_x = s.render.resolution_y = 1024
s.render.filepath = A["out"]
bpy.ops.render.render(write_still=True)
print("PROPKIT_REPORT=" + json.dumps({"out": A["out"]}))
'''


# --- atlas ---------------------------------------------------------------------
# The game's flora art has no PNG on disk: it is painted procedurally into a
# Uint8ClampedArray and only handed to a canvas at the very end. An agent asked for
# "a prop from 2D art" will otherwise stand up a dev server and drive headless
# Chrome to recover it — measured at most of a run. No browser is needed.
#
# `paintFloraAtlas` is module-private, reached only through the exported
# `makeTerrainTextures()`, which also paints sky/water/vista using the full canvas
# 2D API (arc, gradients, fillRect). So the shim below implements createImageData /
# putImageData / getImageData FAITHFULLY — that is the flora path — and stubs the
# rest just well enough not to throw. Those other textures come out wrong and are
# discarded unread.
ATLAS_ENTRY = r"""
import * as T from "GAME_TEXTURES";
const out = T.makeTerrainTextures();
const c = out.flora.image;
process.stdout.write(JSON.stringify({
  w: c.width, h: c.height, data: Array.from(c.__pix),
}));
"""

ATLAS_SHIM = r"""
class Ctx2D {
  constructor(cv){ this.cv = cv; }
  createImageData(w,h){ return { width:w, height:h, data:new Uint8ClampedArray(w*h*4) }; }
  putImageData(img,x,y){
    const cv=this.cv; cv.__pix = cv.__pix || new Uint8ClampedArray(cv.width*cv.height*4);
    for (let row=0; row<img.height; row++){
      const src=row*img.width*4, dst=((y+row)*cv.width + x)*4;
      cv.__pix.set(img.data.subarray(src, src+img.width*4), dst);
    }
  }
  getImageData(x,y,w,h){
    const cv=this.cv, d=new Uint8ClampedArray(w*h*4);
    if (cv.__pix) for (let row=0; row<h; row++){
      const src=((y+row)*cv.width + x)*4; d.set(cv.__pix.subarray(src, src+w*4), row*w*4);
    }
    return { width:w, height:h, data:d };
  }
  // Stubs: only the flora path's pixels are ever read back.
  beginPath(){} closePath(){} moveTo(){} lineTo(){} arc(){} rect(){} fill(){} stroke(){}
  fillRect(){} clearRect(){} strokeRect(){} save(){} restore(){} translate(){} scale(){}
  rotate(){} clip(){} drawImage(){} fillText(){} strokeText(){} setTransform(){}
  createLinearGradient(){ return { addColorStop(){} }; }
  createRadialGradient(){ return { addColorStop(){} }; }
  createPattern(){ return {}; }
}
class Canvas {
  constructor(){ this.width=0; this.height=0; this.__pix=null; this.style={}; }
  getContext(){ return this._c || (this._c = new Ctx2D(this)); }
  toDataURL(){ return "data:,"; }
}
globalThis.document = {
  createElement: (t) => (t === "canvas" ? new Canvas() : { style:{}, appendChild(){} }),
  createElementNS: () => new Canvas(),
  body: { appendChild(){} },
};
globalThis.window = globalThis;
globalThis.HTMLCanvasElement = Canvas;
globalThis.ImageData = class { constructor(d,w,h){ this.data=d; this.width=w; this.height=h; } };
globalThis.self = globalThis;
"""


def cmd_atlas(a) -> None:
    from PIL import Image
    import numpy as np

    t = read_tiers(a.game)
    if a.kind and a.kind not in t["slots"]:
        die(f"unknown --kind {a.kind}; the game defines: {', '.join(sorted(t['slots']))}")
    esbuild = os.path.join(a.game, "node_modules/.bin/esbuild")
    if not os.path.isfile(esbuild):
        die(f"esbuild not found at {esbuild} — run the game's package install first")

    work = tempfile.mkdtemp(prefix="propkit-atlas-")
    try:
        tex = os.path.join(a.game, "src/render/textures.ts")
        entry = os.path.join(work, "entry.mjs")
        open(entry, "w").write(ATLAS_ENTRY.replace("GAME_TEXTURES", tex))
        shim = os.path.join(work, "shim.mjs")
        open(shim, "w").write(ATLAS_SHIM)
        bundle = os.path.join(work, "bundle.mjs")
        r = subprocess.run([esbuild, entry, "--bundle", "--platform=node", "--format=esm",
                            f"--inject:{shim}", f"--outfile={bundle}"],
                           capture_output=True, text=True, cwd=a.game, timeout=300)
        if r.returncode:
            die(f"esbuild failed:\n{r.stderr[-900:]}")
        r = subprocess.run(["node", bundle], capture_output=True, text=True,
                           cwd=a.game, timeout=300)
        if r.returncode or not r.stdout.strip():
            die(f"atlas render failed:\n{r.stderr[-900:]}")
        got = json.loads(r.stdout)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    arr = np.array(got["data"], dtype=np.uint8).reshape(got["h"], got["w"], 4)
    im = Image.fromarray(arr, "RGBA")
    if a.kind:
        s = t["slots"][a.kind]
        im = im.crop((s["x"], s["y"], s["x"] + s["w"], s["y"] + s["h"]))
    if a.scale and a.scale != 1:
        im = im.resize((im.width * a.scale, im.height * a.scale), Image.NEAREST)
    im.save(a.out)
    print(json.dumps({"out": a.out, "atlas": [got["w"], got["h"]],
                      "kind": a.kind, "size": [im.width, im.height],
                      "opaque_pct": round(float((np.asarray(im)[..., 3] > 128).mean()) * 100, 2),
                      **_link(a.out, f"{a.kind or 'atlas'} source art")}, indent=2))

PREVIEW_PY = r'''
import bpy, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=A["glb"])
obj = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
vs = [v.co for v in obj.data.vertices]
w = max(v.x for v in vs) - min(v.x for v in vs)
h = max(v.z for v in vs) - min(v.z for v in vs)
cz = (min(v.z for v in vs) + max(v.z for v in vs)) / 2
R = max(w, h) * 2.2
cd = bpy.data.cameras.new("c"); cam = bpy.data.objects.new("c", cd)
bpy.context.scene.collection.objects.link(cam); bpy.context.scene.camera = cam
l = bpy.data.lights.new("l", "SUN"); l.energy = 4
lo = bpy.data.objects.new("l", l); bpy.context.scene.collection.objects.link(lo)
lo.rotation_euler = (math.radians(52), math.radians(15), math.radians(35))
wd = bpy.data.worlds.new("w"); wd.use_nodes = True
wd.node_tree.nodes["Background"].inputs[0].default_value = (0.16, 0.17, 0.20, 1)
bpy.context.scene.world = wd
s = bpy.context.scene; s.render.engine = "BLENDER_EEVEE"
s.render.resolution_x = s.render.resolution_y = A["res"]
outs = []
for yaw in A["yaws"]:
    t = math.radians(yaw)
    cam.location = (math.sin(t) * R, -math.cos(t) * R, cz + h * 0.35)
    cam.rotation_euler = (math.radians(78), 0, t)
    p = A["out"] + "/preview-yaw%d.png" % yaw
    s.render.filepath = p
    bpy.ops.render.render(write_still=True)
    outs.append(p)
print("PROPKIT_REPORT=" + json.dumps({"views": outs}))
'''


def cmd_preview(a) -> None:
    """Renders the orbit views the skill demands ("measure, then LOOK").

    Existed only as hand-written Blender code before: 10 of 32 ad-hoc calls in an
    audited run were renders, which is a deterministic step sitting outside the tool.
    """
    from PIL import Image
    os.makedirs(a.out, exist_ok=True)
    yaws = [int(x) for x in a.yaws.split(",")]
    rep = run_blender(PREVIEW_PY, {"glb": os.path.abspath(a.glb), "out": os.path.abspath(a.out),
                                   "yaws": yaws, "res": a.res})
    ims = [Image.open(p) for p in rep["views"]]
    sheet = Image.new("RGB", (sum(i.width for i in ims), ims[0].height))
    x = 0
    for i in ims:
        sheet.paste(i.convert("RGB"), (x, 0)); x += i.width
    contact = os.path.join(a.out, "contact-sheet.png")
    sheet.save(contact)
    rep["contact_sheet"] = contact
    rep.update(_link(contact, "orbit views"))
    print(json.dumps(rep, indent=2))


def cmd_sweep(a) -> None:
    """Clean at several budgets and report the trade, because the achievable floor
    is not knowable in advance: collapse shatters a remeshed manifold below some
    ratio and the tool reverts, so 'what budget is actually reachable' needs data."""
    rows = []
    for b in [int(x) for x in a.budgets.split(",")]:
        out = f"{os.path.splitext(a.out)[0]}-b{b}.glb"
        rep = run_blender(CLEAN_PY, {
            "raw": os.path.abspath(a.raw), "out": os.path.abspath(out),
            "weld": WELD_DIST, "voxel": a.voxel, "passes": COLLAPSE_PASSES,
            "budget": b, "height": tier_of(a.game, a.tier)["height"],
            "source": os.path.abspath(a.source) if a.source else None,
            "repair": a.repair,
        })
        rows.append({"budget": b, "out": out, "final_tris": rep["final_tris"],
                     "unreachable": rep.get("budget_unreachable", False),
                     "repaired": rep.get("repaired")})
    print(json.dumps({"sweep": rows}, indent=2))


# --- author --------------------------------------------------------------------
# The primitive-assembly route. A clean run took image-to-3D through four-yaw
# inspection, REJECTED it ("flat top/base sheets and torn single-view colour made
# it visibly worse than the billboard") and hand-authored a volumetric oak from 17
# intersecting primitives instead — which measured better on every structural
# criterion. Single-view reconstruction has no back to infer for a canopy; a
# silhouette-guided assembly does not need one.
#
# So this is the DEFAULT route for foliage. Image-to-3D stays the route for rock
# and built form, where a boulder scored 0.93 silhouette IoU.
AUTHOR_PY = r"""
import bpy, bmesh, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)

def add_ico(loc, r, subdiv=2):
    bpy.ops.mesh.primitive_ico_sphere_add(subdivisions=subdiv, radius=r, location=loc)
    return bpy.context.active_object

def add_cyl(loc, r, depth, verts=8):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=depth, location=loc)
    return bpy.context.active_object

parts = []
# Canopy: spheres placed on the blobs the 2D silhouette actually has, so the
# result reads as the authored shape rather than a generic tree.
for b in A["canopy"]:
    # The 2D art fixes x and z. Depth is unobservable from one view, so mirror the
    # crown's width into y — a canopy is about as deep as it is wide, and leaving
    # y at 0 produces the flat slab this route exists to avoid.
    for dy in b.get("ys", [0.0]):
        parts.append(add_ico((b["x"], dy * A["depth_scale"], b["z"]), b["r"]))
# Trunk, then a root flare — the flare is what stops a prop looking like a pole
# stuck in the ground at map scale.
t = A["trunk"]
parts.append(add_cyl((0.0, 0.0, t["z"]), t["r"], t["h"]))
for i in range(A["roots"]):
    a = 2 * math.pi * i / A["roots"]
    parts.append(add_cyl((math.cos(a) * t["r"] * 1.15, math.sin(a) * t["r"] * 1.15,
                          t["z"] - t["h"] * 0.42), t["r"] * 0.42, t["h"] * 0.30))

for o in bpy.context.scene.objects: o.select_set(o in parts)
bpy.context.view_layer.objects.active = parts[0]
bpy.ops.object.join()
obj = bpy.context.active_object

# Voxel union: turns intersecting primitives into ONE watertight shell. Without
# it the pieces stay separate objects and every downstream check fails.
r = obj.modifiers.new("rm", "REMESH"); r.mode = "VOXEL"; r.voxel_size = A["voxel"]
bpy.ops.object.modifier_apply(modifier="rm")
rep = {"after_union_tris": len(obj.data.polygons), "parts": len(parts)}

def topology():
    b = bmesh.new(); b.from_mesh(obj.data)
    bad = sum(1 for e in b.edges if len(e.link_faces) != 2)
    seen, comps = set(), 0
    for f in b.faces:
        if f in seen: continue
        comps += 1; st = [f]
        while st:
            c = st.pop()
            if c in seen: continue
            seen.add(c)
            for e in c.edges:
                for nf in e.link_faces:
                    if nf not in seen: st.append(nf)
    b.free(); return comps, bad

best = obj.data.copy()
for i in range(5):
    n = len(obj.data.polygons)
    if n <= A["budget"]: break
    d = obj.modifiers.new(f"d{i}", "DECIMATE"); d.decimate_type = "COLLAPSE"
    d.ratio = min(1.0, A["budget"] / n)
    bpy.ops.object.modifier_apply(modifier=f"d{i}")
    c, bad = topology()
    if c != 1 or bad:
        obj.data = best; rep["budget_unreachable"] = True; break
    best = obj.data.copy()
rep["final_tris"] = len(obj.data.polygons)

# Fit to the authored height AND to the shipped sprite's aspect, so the prop
# occupies the same footprint the game already balances around.
vs = [v.co for v in obj.data.vertices]
h = max(v.z for v in vs) - min(v.z for v in vs)
w = max(v.x for v in vs) - min(v.x for v in vs)
sz = A["height"] / h
sx = (A["width"] / w) if A.get("width") else sz
obj.scale = (sx, sx, sz)
bpy.ops.object.transform_apply(scale=True)
vs = [v.co for v in obj.data.vertices]
minz = min(v.z for v in vs)
cx = (min(v.x for v in vs) + max(v.x for v in vs)) / 2
cy = (min(v.y for v in vs) + max(v.y for v in vs)) / 2
for v in obj.data.vertices: v.co.x -= cx; v.co.y -= cy; v.co.z -= minz

# Solid palette materials sampled from the 2D art, assigned by height. This is
# why the authored route beats projection: every view is coloured correctly,
# where a single-view projection stretches and tears on the sides and back.
leaf = A["leaf"]; bark = A["bark"]
mats = []
for name, rgb in [(f"Leaf{i}", c) for i, c in enumerate(leaf)] + \
                 [(f"Bark{i}", c) for i, c in enumerate(bark)]:
    m = bpy.data.materials.new(name); m.use_nodes = True
    m.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (
        rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1)
    m.node_tree.nodes["Principled BSDF"].inputs["Roughness"].default_value = 0.9
    obj.data.materials.append(m); mats.append(name)
canopy_lo = A["canopy_lo"] * A["height"]
nleaf = len(leaf)
trunk_rad = A["trunk"]["r"] * A["height"] * 2.2
for poly in obj.data.polygons:
    vs_ = [obj.data.vertices[i].co for i in poly.vertices]
    z = sum(v.z for v in vs_) / len(vs_)
    rad = max(math.hypot(v.x, v.y) for v in vs_)
    # Bark only where the face is BOTH low and close to the axis. Height alone
    # painted the canopy's underside as bark.
    if z < canopy_lo and rad < trunk_rad:
        poly.material_index = nleaf + (0 if z < canopy_lo * 0.45 else min(len(bark) - 1, 1))
    else:
        f = (z - canopy_lo) / max(A["height"] - canopy_lo, 1e-6)
        poly.material_index = min(nleaf - 1, max(0, int(f * nleaf)))

try: bpy.ops.object.shade_auto_smooth(angle=math.radians(28))
except Exception: bpy.ops.object.shade_flat()
bpy.ops.export_scene.gltf(filepath=A["out"], export_format="GLB", use_selection=True)
rep["materials"] = mats
print("PROPKIT_REPORT=" + json.dumps(rep))
"""


def _silhouette_plan(path: str, roots: int):
    """Derive canopy blobs, trunk and palette from the 2D art's own alpha.

    The point of the authored route is that it follows the SHIPPED silhouette, so
    every number here comes from the sprite rather than from a generic tree recipe.
    """
    from PIL import Image
    import numpy as np
    im = Image.open(path).convert("RGBA")
    a = np.asarray(im)
    mask = a[..., 3] > 128
    if not mask.any():
        die(f"{path} has no opaque pixels — run `propkit isolate` first")
    ys, xs = np.where(mask)
    y0, y1, x0, x1 = ys.min(), ys.max(), xs.min(), xs.max()
    H = y1 - y0 + 1
    W = x1 - x0 + 1
    widths = np.array([mask[y, x0:x1 + 1].sum() for y in range(y0, y1 + 1)], dtype=float)
    # The trunk is the narrow bottom run; the canopy is everything above it.
    thin = widths < max(widths.max() * 0.34, 2)
    split = H - 1
    while split > 0 and thin[split]:
        split -= 1
    trunk_rows = H - 1 - split
    if trunk_rows < 2:                       # bush/rock-like: no trunk
        split = H - 1
        trunk_rows = 0

    # Canopy blobs: sample rows across the canopy and put a sphere at each side's
    # local half-width, which reproduces a lobed crown rather than one ball.
    blobs = []
    rows = [r for r in range(0, split, max(1, split // 5))][:5]
    for r in rows:
        w = widths[r]
        if w < 2: continue
        row = mask[y0 + r, x0:x1 + 1]
        idx = np.where(row)[0]
        cx = (idx.mean() - W / 2) / H          # normalised, height==1
        half = (w / 2) / H
        z = 1.0 - (r / H)
        # ys spreads the same lobe through depth so the crown has volume.
        blobs.append({"x": round(float(cx) * 2.0, 4), "z": round(float(z), 4),
                      "r": round(float(max(half, 0.08)) * 0.92, 4),
                      "ys": [0.0, -half * 0.62, half * 0.62]})
        if w > W * 0.55:                       # wide row: flanking lobes
            for sgn in (-1, 1):
                blobs.append({"x": round((float(cx) + sgn * half * 0.70) * 2.0, 4),
                              "z": round(float(z) - half * 0.22, 4),
                              "r": round(float(half) * 0.58, 4),
                              "ys": [0.0, sgn * half * 0.45]})
    if not blobs:
        blobs = [{"x": 0.0, "z": 0.65, "r": 0.35}]

    trunk_h = max(trunk_rows / H, 0.18)
    trunk_r = max(float(widths[split:].min() if trunk_rows else widths.min()) / 2 / H, 0.035)
    # The measured trunk half-width is the sprite's narrowest run, which reads as a
    # stick in 3D; widen it so it carries the crown visually.
    trunk = {"z": round(trunk_h * 0.55, 4), "h": round(trunk_h * 1.45, 4),
             "r": round(max(trunk_r * 1.6, 0.055), 4)}

    # Palette from the art: greens/leaf above the split, browns/bark below.
    rgb = a[..., :3].astype(int)
    def shades(region, n):
        px = rgb[region & mask]
        if len(px) == 0: return [(90, 120, 70)] * n
        lum = px.sum(axis=1)
        order = np.argsort(lum)
        picks = [px[order[int((i + 0.5) / n * len(order))]] for i in range(n)]
        return [tuple(int(v) for v in p) for p in picks]
    band = np.zeros_like(mask)
    band[y0:y0 + split, :] = True
    leaf = shades(band, 3)
    lower = np.zeros_like(mask)
    lower[y0 + split:y1 + 1, :] = True
    bark = shades(lower, 2) if trunk_rows else leaf[:2]
    return {"canopy": blobs, "trunk": trunk, "roots": roots,
            "leaf": leaf, "bark": bark,
            "canopy_lo": round(float(trunk_h) * 0.92, 4),
            "aspect": round(float(W) / float(H), 4)}


def cmd_author(a) -> None:
    _guard_source_size(a.art, "author")
    t = tier_of(a.game, a.tier)
    plan = _silhouette_plan(a.art, a.roots)
    payload = {
        "out": os.path.abspath(a.out), "voxel": a.voxel,
        "depth_scale": a.depth,
        "budget": a.budget or t["budget"], "height": t["height"],
        # Width from the shipped slot's aspect, so the footprint matches.
        "width": round(t["height"] * plan["aspect"], 4) if not a.free_width else None,
        **{k: plan[k] for k in ("canopy", "trunk", "roots", "leaf", "bark", "canopy_lo")},
    }
    rep = run_blender(AUTHOR_PY, payload)
    rep["plan"] = {"canopy_blobs": len(plan["canopy"]), "aspect": plan["aspect"],
                   "leaf": plan["leaf"], "bark": plan["bark"]}
    rep["tier"] = t
    print(json.dumps(rep, indent=2))


COMPARE_RENDER_PY = r'''
import bpy, json, math, sys
A = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=A["glb"])
obj = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
vs = [v.co for v in obj.data.vertices]
w = max(v.x for v in vs) - min(v.x for v in vs)
h = max(v.z for v in vs) - min(v.z for v in vs)
cx = (min(v.x for v in vs) + max(v.x for v in vs)) / 2
cz = (min(v.z for v in vs) + max(v.z for v in vs)) / 2
cd = bpy.data.cameras.new("c"); cd.type = "ORTHO"; cd.ortho_scale = max(w, h) * 1.05
cam = bpy.data.objects.new("c", cd)
bpy.context.scene.collection.objects.link(cam); bpy.context.scene.camera = cam
cam.location = (cx, -5.0, cz); cam.rotation_euler = (math.radians(90), 0, 0)
l = bpy.data.lights.new("l", "SUN"); l.energy = 3.5
lo = bpy.data.objects.new("l", l); bpy.context.scene.collection.objects.link(lo)
lo.rotation_euler = (math.radians(50), math.radians(15), math.radians(30))
wd = bpy.data.worlds.new("w"); wd.use_nodes = True
wd.node_tree.nodes["Background"].inputs[0].default_value = (0.6, 0.6, 0.6, 1)
wd.node_tree.nodes["Background"].inputs[1].default_value = 0.6
bpy.context.scene.world = wd
s = bpy.context.scene
s.render.engine = "BLENDER_EEVEE"      # materials MUST show; workbench hides them
s.render.film_transparent = True
s.render.resolution_x = s.render.resolution_y = A["res"]
s.render.filepath = A["out"]
bpy.ops.render.render(write_still=True)
print("PROPKIT_REPORT=" + json.dumps({"out": A["out"]}))
'''

# --- compare -------------------------------------------------------------------
def cmd_compare(a) -> None:
    """Source art beside the prop, at the size the game draws it.

    Nine hand-written python calls in an audited run were composing exactly this.
    The skill's judging rule ("put it beside the art it replaces, at map scale")
    cannot be followed without it, so it must be a command, not a snippet.
    """
    from PIL import Image, ImageDraw
    t = tier_of(a.game, a.tier)
    png = os.path.splitext(a.glb)[0] + "-compare-render.png"
    run_blender(COMPARE_RENDER_PY, {"glb": os.path.abspath(a.glb),
                                    "out": os.path.abspath(png), "res": 512})
    prop = Image.open(png).convert("RGBA")
    src = Image.open(a.source).convert("RGBA")

    # Map scale: the game draws this prop `tiles * pxPerUnit` pixels tall.
    px = int(round(t["height"] * (t["pxPerUnit"] or 26)))
    def fit(im, h):
        s = h / im.height
        return im.resize((max(1, int(im.width * s)), h), Image.NEAREST)
    pairs = [("SHIPPED 2D", fit(src, px)), ("3D PROP", fit(prop, px))]
    if a.zoom > 1:
        pairs = [(n, i.resize((i.width * a.zoom, i.height * a.zoom), Image.NEAREST))
                 for n, i in pairs]
    pad, label = 12, 16
    W = sum(i.width for _, i in pairs) + pad * (len(pairs) + 1)
    H = max(i.height for _, i in pairs) + pad * 2 + label
    sheet = Image.new("RGB", (W, H), (30, 32, 38))
    d = ImageDraw.Draw(sheet)
    x = pad
    for name, im in pairs:
        sheet.paste(im, (x, pad + label), im)
        d.text((x, 2), name, fill=(210, 214, 220))
        x += im.width + pad
    sheet.save(a.out)
    print(json.dumps({"out": a.out, "map_px_height": px,
                      "note": "nearest-neighbour at the game's own pixel density",
                      **_link(a.out, "source vs prop at map scale")}, indent=2))


# --- upres ---------------------------------------------------------------------
# THE STAGE THAT WAS MISSING. A shipped sprite slot is tiny — an oak is 52x64 — and
# a small model handed that straight to `author` derived its whole silhouette plan
# and palette from 64 rows of chunky pixels. The result passed every structural
# check and was a colourless blob.
#
# So: the tiny sprite is never the source. It is up-resed into a clean image FIRST,
# and that image is what every later stage reads. Two ways, both validated here:
#   ESRGAN x4 alone      — faithful, no invention, keeps the pixel palette exactly.
#   ESRGAN + Z-Image     — the official 2K recipe: a light 0.33-denoise pass that
#                          adds form a 52px sprite cannot contain.
MIN_SOURCE_PX = 384   # below this, a prop's plan is being derived from noise


def cmd_upres(a) -> None:
    from PIL import Image
    src = Image.open(a.image)
    if a.refine:
        # ESRGAN is x4 and the official recipe then halves it — a net x2, which is
        # right for a 1024 plate and useless for a 52px sprite. Pre-scale with
        # nearest so that x4 lands on the target, keeping the pixel edges crisp for
        # ESRGAN to work from rather than asking it to invent 20x of detail.
        # Pad first: a subject touching the frame edge gets clipped by the diffusion
        # pass and leaves the reconstructor no silhouette to work from.
        if a.pad > 0:
            pw, ph = src.width, src.height
            m = int(max(pw, ph) * a.pad)
            padded = Image.new("RGBA", (pw + m * 2, ph + m * 2), (0, 0, 0, 0))
            padded.paste(src.convert("RGBA"), (m, m))
            src = padded
        # Pre-scale computed AFTER padding: sizing off the unpadded sprite made the
        # x4 land short of --target (asked 1024, got 528).
        pre_target = max(1, a.target // 4)
        f = max(1, round(pre_target / max(src.width, src.height)))
        base = "propkit-upres-in.png"
        dest = os.path.join(os.path.expanduser("~/ComfyUI/input"), base)
        big = src.convert("RGBA").resize((src.width * f, src.height * f), Image.NEAREST)
        # Composite onto MID-GREY, never black. ComfyUI needs RGB, and flattening
        # alpha to black poisons the next stage: `isolate` treats dark pixels as
        # subject, so a black background is keyed IN — measured once as 79% coverage
        # and a full-frame bbox, after which the reconstructor was asked to model a
        # black rectangle. Mid-grey is neither saturated nor dark, so it keys out.
        plate = Image.new("RGB", big.size, (128, 128, 128))
        plate.paste(big, (0, 0), big)
        plate.save(dest)
        print(f"  pre-scaled x{f} to {src.width * f}x{src.height * f} before ESRGAN",
              file=sys.stderr)
        g = {
          "1": {"class_type": "LoadImage", "inputs": {"image": base}},
          "2": {"class_type": "UpscaleModelLoader",
                "inputs": {"model_name": "RealESRGAN_x4plus.safetensors"}},
          "3": {"class_type": "ImageUpscaleWithModel",
                "inputs": {"upscale_model": ["2", 0], "image": ["1", 0]}},
          "4": {"class_type": "ImageScaleBy",
                "inputs": {"image": ["3", 0], "upscale_method": "lanczos", "scale_by": a.post_scale}},
          "10": {"class_type": "UNETLoader",
                 "inputs": {"unet_name": "z_image_turbo_int8_convrot.safetensors",
                            "weight_dtype": "default"}},
          "11": {"class_type": "CLIPLoader",
                 "inputs": {"clip_name": "qwen_3_4b.safetensors", "type": "lumina2"}},
          "12": {"class_type": "VAELoader", "inputs": {"vae_name": "z_image_ae.safetensors"}},
          "13": {"class_type": "ModelSamplingAuraFlow", "inputs": {"model": ["10", 0], "shift": 3.0}},
          "14": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["11", 0], "text": a.prompt}},
          "15": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["11", 0], "text": ""}},
          "16": {"class_type": "VAEEncode", "inputs": {"pixels": ["4", 0], "vae": ["12", 0]}},
          # denoise 0.33: enough to add form, little enough to keep the silhouette.
          "17": {"class_type": "KSampler",
                 "inputs": {"model": ["13", 0], "seed": a.seed, "steps": 5, "cfg": 1.0,
                            "sampler_name": "dpmpp_2m_sde", "scheduler": "beta",
                            "positive": ["14", 0], "negative": ["15", 0],
                            "latent_image": ["16", 0], "denoise": a.denoise}},
          "18": {"class_type": "VAEDecode", "inputs": {"samples": ["17", 0], "vae": ["12", 0]}},
          "19": {"class_type": "SaveImage",
                 "inputs": {"images": ["18", 0], "filename_prefix": "propkit/upres"}},
        }
        base_url = a.comfy.rstrip("/")
        try:
            pid = comfy_post("/prompt", {"prompt": g}, base_url)["prompt_id"]
        except urllib.error.HTTPError as e:  # noqa: F821
            die(f"ComfyUI rejected the upres graph: {e.read().decode()[:400]}")
        deadline = time.time() + a.timeout
        while time.time() < deadline:
            h = json.load(urllib.request.urlopen(f"{base_url}/history/{pid}", timeout=30))
            if h:
                st = h[pid]["status"]
                if st.get("status_str") != "success":
                    die(f"upres failed: {json.dumps(st.get('messages', []))[:400]}")
                for o in h[pid]["outputs"].values():
                    for im in o.get("images", []):
                        p2 = os.path.join(os.path.expanduser("~/ComfyUI/output"),
                                          im.get("subfolder", ""), im["filename"])
                        shutil.copyfile(p2, a.out)
                        out = Image.open(a.out)
                        print(json.dumps({"out": a.out, "from": list(src.size),
                                          "to": list(out.size), "method": "ESRGAN x4 + Z-Image refine",
                                          "denoise": a.denoise, "seed": a.seed,
                                          **_link(a.out, "up-resed")}, indent=2))
                        return
                die("upres produced no image")
            time.sleep(6)
        die("upres timed out")

    # Faithful path: integer nearest-neighbour, which preserves the pixel palette
    # exactly. Correct when the prop must keep the shipped art's colours.
    f = max(1, -(-a.target // max(src.width, src.height)))
    out = src.convert("RGBA").resize((src.width * f, src.height * f), Image.NEAREST)
    out.save(a.out)
    print(json.dumps({"out": a.out, "from": list(src.size), "to": list(out.size),
                      "method": f"nearest x{f} (faithful, palette preserved)",
                      **_link(a.out, "up-resed")}, indent=2))


def _guard_source_size(path: str, what: str) -> None:
    """Refuse to plan a prop from a thumbnail.

    A 52x64 sprite went straight into `author` and produced a colourless blob that
    still passed every structural check. The tool has to say no.
    """
    from PIL import Image
    w, h = Image.open(path).size
    if max(w, h) < MIN_SOURCE_PX:
        die(f"{what} source is only {w}x{h}. A prop's silhouette and palette cannot "
            f"come from a thumbnail — run `propkit upres` first (want >= {MIN_SOURCE_PX}px "
            f"on the long edge), then isolate, then retry.")


# --- texture --------------------------------------------------------------------
# Real baked texturing, via the Hunyuan3D paint model in ComfyUI-Hunyuan3DWrapper.
#
# This replaces a front-projection hack that painted colour by shining the source
# image at the mesh like a slide projector: it coloured whatever faced the camera
# and left the sides and back grey. The paint model renders the mesh's own normals
# and positions from six cameras, paints each view, and bakes them into one UV
# texture — so every side is coloured correctly.
#
# Requires the wrapper pack AND its `custom_rasterizer` extension. Building that on
# a CUDA-13 torch needs matched `nvidia-cuda-nvcc` / `nvidia-nvvm` /
# `nvidia-cuda-crt` wheels; mismatched versions emit PTX the assembler rejects.
def cmd_texture(a) -> None:
    base = a.comfy.rstrip("/")
    ref = os.path.basename(a.ref)
    dest = os.path.join(os.path.expanduser("~/ComfyUI/input"), ref)
    if os.path.abspath(a.ref) != os.path.abspath(dest):
        shutil.copyfile(a.ref, dest)
    prefix = "propkit/textured"
    g = {
      # Six cameras: four around the equator plus top and bottom, front weighted
      # heaviest because that is the view the source art was authored from.
      "1": {"class_type": "Hy3DCameraConfig", "inputs": {
             "camera_azimuths": a.azimuths, "camera_elevations": a.elevations,
             "view_weights": a.weights, "camera_distance": 1.45, "ortho_scale": 1.2}},
      "2": {"class_type": "Hy3DLoadMesh", "inputs": {"glb_path": os.path.abspath(a.glb)}},
      # UV unwrap first: a baked texture needs somewhere to live, and the shape
      # models emit no UVs at all.
      "3": {"class_type": "Hy3DMeshUVWrap", "inputs": {"trimesh": ["2", 0]}},
      "4": {"class_type": "Hy3DRenderMultiView", "inputs": {
             "trimesh": ["3", 0], "render_size": a.render_size,
             "texture_size": a.texture_size, "camera_config": ["1", 0],
             "normal_space": "world"}},
      "5": {"class_type": "DownloadAndLoadHy3DPaintModel", "inputs": {"model": a.paint_model}},
      "6": {"class_type": "LoadImage", "inputs": {"image": ref}},
      "7": {"class_type": "Hy3DSampleMultiView", "inputs": {
             "pipeline": ["5", 0], "ref_image": ["6", 0],
             "normal_maps": ["4", 0], "position_maps": ["4", 1],
             "view_size": a.view_size, "steps": a.steps, "seed": a.seed,
             "camera_config": ["1", 0]}},
      "8": {"class_type": "Hy3DBakeFromMultiview", "inputs": {
             "images": ["7", 0], "renderer": ["4", 2], "camera_config": ["1", 0]}},
      "9": {"class_type": "Hy3DApplyTexture", "inputs": {"texture": ["8", 0], "renderer": ["8", 2]}},
      "10": {"class_type": "Hy3DExportMesh", "inputs": {
              "trimesh": ["9", 0], "filename_prefix": prefix,
              "file_format": "glb", "save_file": True}},
    }
    try:
        pid = comfy_post("/prompt", {"prompt": g}, base)["prompt_id"]
    except urllib.error.HTTPError as e:  # noqa: F821
        die(f"ComfyUI rejected the texture graph:\n{e.read().decode()[:600]}")

    out_dir = os.path.join(os.path.expanduser("~/ComfyUI/output"), "propkit")
    before = set(glob.glob(os.path.join(out_dir, "textured*.glb")))
    deadline = time.time() + a.timeout
    while time.time() < deadline:
        h = json.load(urllib.request.urlopen(f"{base}/history/{pid}", timeout=30))
        if h:
            st = h[pid]["status"]
            if st.get("status_str") != "success":
                msgs = [m for m in st.get("messages", []) if "error" in json.dumps(m).lower()]
                die(f"texturing failed: {json.dumps(msgs)[:700]}")
            # Hy3DExportMesh writes the file but reports nothing in the history
            # payload, so the new file has to be found on disk.
            new = sorted(set(glob.glob(os.path.join(out_dir, "textured*.glb"))) - before,
                         key=os.path.getmtime)
            if not new:
                new = sorted(glob.glob(os.path.join(out_dir, "textured*.glb")),
                             key=os.path.getmtime)
            if not new:
                die(f"texturing reported success but no glb appeared in {out_dir}")
            shutil.copyfile(new[-1], a.out)
            print(json.dumps({"out": a.out, "prompt_id": pid,
                              "paint_model": a.paint_model, "seed": a.seed,
                              "views": len(a.azimuths.split(",")),
                              **_link(a.out, "textured prop")}, indent=2))
            return
        time.sleep(10)
    die(f"texturing did not finish within {a.timeout}s")


# --- views ----------------------------------------------------------------------
# Generate the side/back views that single-view reconstruction cannot infer.
#
# A canopy has no back in the source art, so a single-view model represents it as
# flat sheets — measured on two different reconstructors. Supplying four views fixed
# it: silhouette IoU went 0.57 -> 0.93 on the same subject.
#
# The views come from a SCAFFOLD mesh used purely as a camera rig: render it from
# each yaw, then restyle each render through Canny + the ControlNet so it looks like
# the game's art rather than grey clay. The scaffold sets where the cameras stand;
# the diffusion supplies appearance.
VIEW_STYLE = ("stylised painted {kind} game asset, solid volumetric form, "
              "soft ambient occlusion, plain flat light grey background, "
              "single object centred, whole object in frame, no text, no watermark, "
              "no ground shadow")


def cmd_views(a) -> None:
    from PIL import Image
    base = a.comfy.rstrip("/")
    os.makedirs(a.out, exist_ok=True)
    yaws = [int(x) for x in a.yaws.split(",")]

    # 1. Render the scaffold from each yaw — geometry only, no appearance.
    rep = run_blender(PREVIEW_PY, {"glb": os.path.abspath(a.scaffold),
                                   "out": os.path.abspath(a.out), "yaws": yaws,
                                   "res": a.res})
    ids = {}
    for yaw, png in zip(yaws, rep["views"]):
        name = f"propkit-view-{yaw}.png"
        shutil.copyfile(png, os.path.join(os.path.expanduser("~/ComfyUI/input"), name))
        side = {0: "FRONT", 90: "LEFT side", 180: "BACK", 270: "RIGHT side"}.get(yaw, f"{yaw} degree")
        prompt = f"The {side} view of a {a.kind}. " + VIEW_STYLE.format(kind=a.kind)
        g = {
          "1": {"class_type": "UNETLoader", "inputs": {"unet_name": a.unet, "weight_dtype": "default"}},
          "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": a.clip, "type": "lumina2"}},
          "3": {"class_type": "VAELoader", "inputs": {"vae_name": a.vae}},
          "20": {"class_type": "ModelPatchLoader", "inputs": {"name": a.controlnet}},
          "11": {"class_type": "LoadImage", "inputs": {"image": name}},
          # Canny thresholds from the official ControlNet template.
          "12": {"class_type": "Canny", "inputs": {"image": ["11", 0],
                  "low_threshold": 0.1, "high_threshold": 0.32}},
          "21": {"class_type": "ZImageFunControlnet", "inputs": {
                  "model": ["1", 0], "model_patch": ["20", 0], "vae": ["3", 0],
                  "strength": a.strength, "image": ["12", 0]}},
          "7": {"class_type": "ModelSamplingAuraFlow", "inputs": {"model": ["21", 0], "shift": 3.0}},
          "4": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": prompt}},
          "5": {"class_type": "ConditioningZeroOut", "inputs": {"conditioning": ["4", 0]}},
          "6": {"class_type": "EmptySD3LatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
          "8": {"class_type": "KSampler", "inputs": {
                 "model": ["7", 0], "seed": a.seed, "steps": 8, "cfg": 1.0,
                 "sampler_name": "res_multistep", "scheduler": "simple",
                 "positive": ["4", 0], "negative": ["5", 0],
                 "latent_image": ["6", 0], "denoise": 1.0}},
          "9": {"class_type": "VAEDecode", "inputs": {"samples": ["8", 0], "vae": ["3", 0]}},
          "10": {"class_type": "SaveImage", "inputs": {"images": ["9", 0],
                  "filename_prefix": f"propkit/styled-{yaw}"}},
        }
        try:
            ids[yaw] = comfy_post("/prompt", {"prompt": g}, base)["prompt_id"]
        except urllib.error.HTTPError as e:  # noqa: F821
            die(f"view {yaw} rejected: {e.read().decode()[:300]}")

    made = {}
    deadline = time.time() + a.timeout
    pend = dict(ids)
    while pend and time.time() < deadline:
        for yaw, pid in list(pend.items()):
            h = json.load(urllib.request.urlopen(f"{base}/history/{pid}", timeout=30))
            if not h:
                continue
            if h[pid]["status"].get("status_str") != "success":
                die(f"view {yaw} failed: {json.dumps(h[pid]['status'].get('messages'))[:400]}")
            for o in h[pid]["outputs"].values():
                for im in o.get("images", []):
                    src = os.path.join(os.path.expanduser("~/ComfyUI/output"),
                                      im.get("subfolder", ""), im["filename"])
                    made[yaw] = os.path.join(a.out, f"view-{yaw}.png")
                    shutil.copyfile(src, made[yaw])
            del pend[yaw]
        if pend:
            time.sleep(6)
    if pend:
        die("view generation timed out")

    # 2. Isolate AND flatten each view. Flattening is mandatory here: a
    #    reconstruction model reads RGB, so a surviving background gets modelled
    #    as a box around the subject.
    finals = {}
    for yaw, png in sorted(made.items()):
        cut = os.path.join(a.out, f"cut-{yaw}.png")
        ns = argparse.Namespace(image=png, out=cut, sat=40.0, dark=100.0,
                                refine=True, bg_dist=40.0, flatten="white")
        cmd_isolate(ns)
        finals[yaw] = cut
    sheet = os.path.join(a.out, "contact-sheet.png")
    print(json.dumps({"out": a.out, "views": finals,
                      "note": "isolated AND flattened; ready for `meshviews`",
                      **(_link(sheet, "generated views") if os.path.exists(sheet) else {})},
                     indent=2))

def cmd_meshviews(a) -> None:
    """Multi-view reconstruction with hunyuan3d-dit-v2-mv.

    The multi-view checkpoint is a different file from the single-view one and lives
    in models/checkpoints (ImageOnlyCheckpointLoader reads there, not
    diffusion_models). Views must already be isolated AND flattened.
    """
    base = a.comfy.rstrip("/")
    order = ["front", "left", "back", "right"]
    yaws = [int(x) for x in a.yaws.split(",")]
    g = {"1": {"class_type": "ImageOnlyCheckpointLoader", "inputs": {"ckpt_name": a.ckpt}}}
    cond = {}
    for i, (slot, yaw) in enumerate(zip(order, yaws)):
        src = os.path.join(a.views, f"cut-{yaw}.png")
        if not os.path.isfile(src):
            die(f"missing view: {src} — run `propkit views` first")
        # Unique filenames: ComfyUI caches an execution by prompt JSON, so reusing a
        # name after overwriting the file returns the PREVIOUS mesh.
        name = f"propkit-mv-{yaw}-{int(os.path.getmtime(src))}.png"
        shutil.copyfile(src, os.path.join(os.path.expanduser("~/ComfyUI/input"), name))
        g[f"l{i}"] = {"class_type": "LoadImage", "inputs": {"image": name}}
        g[f"e{i}"] = {"class_type": "CLIPVisionEncode",
                      "inputs": {"clip_vision": ["1", 1], "image": [f"l{i}", 0], "crop": "none"}}
        cond[slot] = [f"e{i}", 0]
    g.update({
      "4": {"class_type": "Hunyuan3Dv2ConditioningMultiView", "inputs": cond},
      "5": {"class_type": "EmptyLatentHunyuan3Dv2", "inputs": {"resolution": 3072, "batch_size": 1}},
      "6": {"class_type": "ModelSamplingAuraFlow", "inputs": {"model": ["1", 0], "shift": 1.0}},
      "7": {"class_type": "KSampler", "inputs": {
             "model": ["6", 0], "seed": a.seed, "steps": 20, "cfg": 8.0,
             "sampler_name": "euler", "scheduler": "normal",
             "positive": ["4", 0], "negative": ["4", 1],
             "latent_image": ["5", 0], "denoise": 1.0}},
      "8": {"class_type": "VAEDecodeHunyuan3D", "inputs": {
             "samples": ["7", 0], "vae": ["1", 2], "num_chunks": 8000, "octree_resolution": 256}},
      "9": {"class_type": "VoxelToMesh", "inputs": {"voxel": ["8", 0],
             "algorithm": "basic", "threshold": 0.6}},
      "10": {"class_type": "SaveGLB", "inputs": {"mesh": ["9", 0], "filename_prefix": "propkit/mvmesh"}},
    })
    try:
        pid = comfy_post("/prompt", {"prompt": g}, base)["prompt_id"]
    except urllib.error.HTTPError as e:  # noqa: F821
        die(f"ComfyUI rejected the multi-view graph:\n{e.read().decode()[:600]}")
    deadline = time.time() + a.timeout
    while time.time() < deadline:
        h = json.load(urllib.request.urlopen(f"{base}/history/{pid}", timeout=30))
        if h:
            st = h[pid]["status"]
            if st.get("status_str") != "success":
                die(f"reconstruction failed: {json.dumps(st.get('messages'))[:600]}")
            for o in h[pid]["outputs"].values():
                for f in (o.get("3d") or []) + (o.get("images") or []):
                    if str(f.get("filename", "")).endswith(".glb"):
                        src = os.path.join(os.path.expanduser("~/ComfyUI/output"),
                                          f.get("subfolder", ""), f["filename"])
                        shutil.copyfile(src, a.out)
                        print(json.dumps({"out": a.out, "prompt_id": pid,
                                          "views": len(yaws), "ckpt": a.ckpt,
                                          **_link(a.out, "multi-view mesh")}, indent=2))
                        return
            die("reconstruction reported success but produced no glb")
        time.sleep(8)
    die("reconstruction timed out")


def cmd_tiers(a) -> None:
    t = read_tiers(a.game)
    rows = [{"kind": k, "tiles": v, "slot": t["slots"].get(k), "budget": tier_of(a.game, k)["budget"]}
            for k, v in sorted(t["tiles"].items(), key=lambda kv: -kv[1])]
    print(json.dumps({"pxPerUnit": t["pxPerUnit"], "tiers": rows}, indent=2))


def main() -> None:
    p = argparse.ArgumentParser(prog="propkit.py", description=__doc__,
                               formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    q = sub.add_parser("tiers", help="print the prop tiers read from the game")
    q.add_argument("--game", required=True); q.set_defaults(fn=cmd_tiers)

    q = sub.add_parser("isolate", help="remove the background (real removal, not white)")
    q.add_argument("image"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--sat", type=float, default=40.0)
    q.add_argument("--dark", type=float, default=100.0)
    q.add_argument("--refine", action="store_true",
                   help="also key on border-colour distance and gate the ground shadow")
    q.add_argument("--bg-dist", type=float, default=40.0)
    q.add_argument("--flatten", default="white",
                   choices=["white", "grey", "magenta", "black", "keep"],
                   help="also fill the RGB background. Alpha alone is NOT enough: "
                        "ComfyUI's LoadImage routes alpha to a separate MASK output, so a "
                        "conditioning model still sees the old background and reconstructs it")
    q.set_defaults(fn=cmd_isolate)

    q = sub.add_parser("upres", help="turn a tiny sprite into a usable source image (DO THIS FIRST)")
    q.add_argument("image"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--target", type=int, default=1024)
    q.add_argument("--refine", action="store_true",
                   help="ESRGAN + a light Z-Image pass, which adds form a tiny sprite cannot hold")
    q.add_argument("--prompt", default="clean high-resolution game asset, crisp detail")
    q.add_argument("--denoise", type=float, default=0.33)
    q.add_argument("--post-scale", type=float, default=0.5)
    q.add_argument("--pad", type=float, default=0.12,
                   help="pad the subject away from the frame edge before refining")
    q.add_argument("--seed", type=int, default=42)
    q.add_argument("--comfy", default=COMFY); q.add_argument("--timeout", type=int, default=600)
    q.set_defaults(fn=cmd_upres)

    q = sub.add_parser("views", help="generate the side/back views a single view cannot infer")
    q.add_argument("scaffold", help="a rough mesh used ONLY as a camera rig")
    q.add_argument("--kind", required=True, help="what the subject is, e.g. 'conifer tree'")
    q.add_argument("-o", "--out", required=True)
    q.add_argument("--yaws", default="0,90,180,270")
    q.add_argument("--res", type=int, default=1024)
    q.add_argument("--strength", type=float, default=0.85)
    q.add_argument("--seed", type=int, default=1234)
    q.add_argument("--unet", default="z_image_turbo_int8_convrot.safetensors")
    q.add_argument("--clip", default="qwen_3_4b.safetensors")
    q.add_argument("--vae", default="z_image_ae.safetensors")
    q.add_argument("--controlnet", default="Z-Image-Turbo-Fun-Controlnet-Union-2.1-lite-2602-8steps.safetensors")
    q.add_argument("--comfy", default=COMFY); q.add_argument("--timeout", type=int, default=1200)
    q.set_defaults(fn=cmd_views)

    q = sub.add_parser("meshviews", help="multi-view reconstruction (best for foliage)")
    q.add_argument("views", help="directory produced by `propkit views`")
    q.add_argument("-o", "--out", required=True)
    q.add_argument("--yaws", default="0,90,180,270")
    q.add_argument("--ckpt", default="hunyuan3d-dit-v2-mv-fp16.safetensors")
    q.add_argument("--seed", type=int, default=42)
    q.add_argument("--comfy", default=COMFY); q.add_argument("--timeout", type=int, default=1500)
    q.set_defaults(fn=cmd_meshviews)

    q = sub.add_parser("texture", help="bake a real texture with the Hunyuan3D paint model")
    q.add_argument("glb"); q.add_argument("--ref", required=True,
                   help="reference art the paint model matches")
    q.add_argument("-o", "--out", required=True)
    q.add_argument("--paint-model", default="hunyuan3d-paint-v2-0-turbo",
                   choices=["hunyuan3d-paint-v2-0", "hunyuan3d-paint-v2-0-turbo"])
    q.add_argument("--azimuths", default="0, 90, 180, 270, 0, 180")
    q.add_argument("--elevations", default="0, 0, 0, 0, 90, -90")
    q.add_argument("--weights", default="1, 0.1, 0.5, 0.1, 0.05, 0.05")
    q.add_argument("--render-size", type=int, default=1024)
    q.add_argument("--texture-size", type=int, default=1024)
    q.add_argument("--view-size", type=int, default=512)
    q.add_argument("--steps", type=int, default=20)
    q.add_argument("--seed", type=int, default=42)
    q.add_argument("--comfy", default=COMFY); q.add_argument("--timeout", type=int, default=1500)
    q.set_defaults(fn=cmd_texture)

    q = sub.add_parser("compare", help="prop beside the source art at map scale")
    q.add_argument("glb"); q.add_argument("--source", required=True)
    q.add_argument("--tier", required=True); q.add_argument("--game", required=True)
    q.add_argument("-o", "--out", required=True); q.add_argument("--zoom", type=int, default=3)
    q.set_defaults(fn=cmd_compare)

    q = sub.add_parser("mesh", help="image to raw GLBs via Hunyuan3D (both extractors)")
    q.add_argument("image"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--seed", type=int, default=42)
    q.add_argument("--comfy", default=COMFY)
    q.add_argument("--timeout", type=int, default=900)
    q.set_defaults(fn=cmd_mesh)

    q = sub.add_parser("clean", help="raw GLB to a contract-compliant prop")
    q.add_argument("raw"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--tier", required=True); q.add_argument("--game", required=True)
    q.add_argument("--source", help="isolated RGBA source, for colour projection")
    q.add_argument("--voxel", type=float, default=VOXEL_SIZE)
    q.add_argument("--budget", type=int)
    q.add_argument("--repair", action="store_true",
                   help="weld + hole-fill + re-verify when collapse leaves bad edges")
    q.add_argument("--no-remesh", action="store_true",
                   help="skip the voxel pass, preserving UVs and textures (use when the "
                        "reconstructor already emitted them)")
    q.set_defaults(fn=cmd_clean)

    q = sub.add_parser("sweep", help="clean at several budgets and report the trade")
    q.add_argument("raw"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--tier", required=True); q.add_argument("--game", required=True)
    q.add_argument("--budgets", default="600,1200,2400")
    q.add_argument("--source"); q.add_argument("--voxel", type=float, default=VOXEL_SIZE)
    q.add_argument("--repair", action="store_true")
    q.set_defaults(fn=cmd_sweep)

    q = sub.add_parser("preview", help="orbit renders + contact sheet, for looking at it")
    q.add_argument("glb"); q.add_argument("-o", "--out", required=True)
    q.add_argument("--yaws", default="0,90,180,270")
    q.add_argument("--res", type=int, default=512)
    q.set_defaults(fn=cmd_preview)

    q = sub.add_parser("author", help="build a prop from silhouette-guided primitives (best for foliage)")
    q.add_argument("art", help="the 2D art, isolated (RGBA)")
    q.add_argument("-o", "--out", required=True)
    q.add_argument("--tier", required=True); q.add_argument("--game", required=True)
    q.add_argument("--voxel", type=float, default=0.055)
    q.add_argument("--depth", type=float, default=1.0,
                   help="how deep the crown is relative to its width (1.0 = round)")
    q.add_argument("--budget", type=int); q.add_argument("--roots", type=int, default=5)
    q.add_argument("--free-width", action="store_true",
                   help="do not fit X to the shipped sprite's aspect")
    q.set_defaults(fn=cmd_author)

    q = sub.add_parser("atlas", help="extract the game's procedurally-painted sprite art")
    q.add_argument("--game", required=True); q.add_argument("-o", "--out", required=True)
    q.add_argument("--kind", help="FloraKind to crop; omit for the whole atlas")
    q.add_argument("--scale", type=int, default=1)
    q.set_defaults(fn=cmd_atlas)

    q = sub.add_parser("check", help="verify a prop against the game's contract")
    q.add_argument("glb"); q.add_argument("--tier", required=True); q.add_argument("--game", required=True)
    q.add_argument("--source", help="isolated RGBA source, enables the silhouette check")
    q.add_argument("--budget", type=int)
    q.add_argument("--min-iou", type=float, default=0.85)
    q.add_argument("--max-colour-dist", type=float, default=150.0,
                   help="how far a material colour may sit from the nearest source pixel")
    q.set_defaults(fn=cmd_check)

    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
