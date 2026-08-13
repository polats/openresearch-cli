#!/usr/bin/env python3
"""splatkit — one 2D image to a 3D gaussian splat, as commands rather than prose.

Third sibling of spritekit.mjs and propkit.py, and the same bet: the deterministic
parts of a pipeline belong in a tool, not in a paragraph a model re-interprets.
What is left to judgement — whether a splat reads better than the sprite it came
from, and whether to keep the splat or push on to a mesh — stays in the orx-splat
skill.

    splatkit.py models                                  # are the weights installed?
    splatkit.py models --install                        # ~3.4 GB, then re-verify
    splatkit.py splat  02-cut.png -o 03-splat.ply       # + .spz, turntable, front views
    splatkit.py check  03-splat.ply --source 02-cut.png
    splatkit.py compare 03-splat.ply --source 02-cut.png -o 06-compare.png
    splatkit.py space  02-cut.png -o 03-splat.ply       # remote fallback (see below)

Runs on the LOCAL ComfyUI (default http://127.0.0.1:8188) using its built-in
TripoSplat nodes. Nothing leaves the machine on the default route.

Measured facts this encodes, each of which cost a run to learn:

* One view is enough here, and that is the whole reason this route exists. The
  same isolated oak plate that gave propkit's single-view MESH reconstruction
  0.57 silhouette IoU (flat sheets, no back) gives 0.9099 as a splat, against
  0.93 for propkit's four-view route — which needs a camera rig, four restyled
  renders and a reconstructor. No scaffold, no views, no yaw sweep.
* The whole thing is ONE submit: 25 nodes, 57 s wall clock on an RTX 3090 for
  splat + spz + mesh + 75-frame turntable + front stills. Splitting it into
  per-stage submits would reload 3.4 GB of weights each time.
* A saved splat cannot be cheaply reloaded. `Load3D` requires a LOAD_3D viewport
  blob and `SaveGaussianSplat` wants the same, so there is no clean headless path
  from a .ply back to a SPLAT. Generation is deterministic on (seed, decode_seed),
  so re-run instead of reloading — that is why `splat` emits every derived
  artifact in the one pass rather than offering `render`/`mesh` subcommands.
* `SplatToMesh` output is NOT a prop. 262k gaussians meshed at resolution 384
  came out a 79 MB GLB; propkit's tiers budget hundreds to a few thousand
  triangles. The mesh is a handoff to `propkit clean`, never a deliverable.
* ComfyUI's DynamicCombo inputs (CreateCameraInfo.mode, SaveVideo.codec) do not
  serialise as nested dicts in the /prompt API. The field holds the option KEY
  and the option's own inputs are flat siblings named "field.nested" — so orbit
  is {"mode": "orbit", "mode.yaw": 35.0, ...}. See _expand_schema_for_dynamic in
  comfy_api/latest/_io.py.
* LoadImage's MASK output is 1-alpha, the opposite polarity to what
  TripoSplatPreprocessImage wants. It must be inverted; RemoveBackground's mask
  must not be. Getting this backwards reconstructs the background instead.
* A downloader's exit status is not evidence. `hf download` can exit 0 having
  written nothing (a broken launcher shebang does it), which is why `models`
  verifies against /object_info rather than the filesystem or a return code.

The remote route (`space`) exists for when the local GPU is unavailable. It drives
the VAST-AI/TripoSplat HF Space over HTTP, reading its contract from the Space's
own agents.md rather than from endpoints written down here — see
https://huggingface.co/blog/mishig/spaces-agents-md. It UPLOADS THE IMAGE to a
third-party service, needs $HF_TOKEN, and is subject to that Space's queue and
quota, so it is opt-in and never the default.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

COMFY_DEFAULT = "http://127.0.0.1:8188"
SPACE_DEFAULT = "VAST-AI/TripoSplat"

# The four files the graph loads, and where each comes from. flux2-vae is listed
# because the graph needs it, but it ships with other workflows too.
WEIGHTS = [
    ("diffusion_models", "triposplat_fp16.safetensors", "VAST-AI/TripoSplat", "UNETLoader", "unet_name"),
    ("clip_vision", "dino_v3_vit_h.safetensors", "VAST-AI/TripoSplat", "CLIPVisionLoader", "clip_name"),
    ("vae", "triposplat_vae_decoder_fp16.safetensors", "VAST-AI/TripoSplat", "VAELoader", "vae_name"),
    ("vae", "flux2-vae.safetensors", "VAST-AI/TripoSplat", "VAELoader", "vae_name"),
    ("background_removal", "birefnet.safetensors", "Comfy-Org/BiRefNet", "LoadBackgroundRemovalModel", "bg_removal_name"),
]

PLY_PROPS = ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2",
             "opacity", "scale_0", "scale_1", "scale_2",
             "rot_0", "rot_1", "rot_2", "rot_3"]


# --- plumbing ---------------------------------------------------------------

def die(msg: str, code: int = 2):
    print(f"splatkit: {msg}", file=sys.stderr)
    sys.exit(code)


def api_get(base: str, path: str, timeout: int = 120):
    try:
        with urllib.request.urlopen(base + path, timeout=timeout) as r:
            return json.loads(r.read())
    except urllib.error.URLError as e:
        die(f"cannot reach ComfyUI at {base}{path}: {e}\n"
            f"        Is it running? Check Settings -> Generative AI.")


def api_post(base: str, path: str, payload: dict, timeout: int = 120):
    req = urllib.request.Request(
        base + path, data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        die(f"ComfyUI rejected {path} (HTTP {e.code}): {e.read().decode()[:800]}")
    except urllib.error.URLError as e:
        die(f"cannot reach ComfyUI at {base}{path}: {e}")


def comfy_root(base: str) -> str:
    """The ComfyUI checkout we are talking to. Derived from the running process,
    because two checkouts on one machine is the normal case (~/ComfyUI and
    ~/projects/ComfyUI here) and staging an image into the wrong one fails
    silently — the graph just cannot find the file.

    /system_stats reports argv but not the root, and argv[0] is usually the bare
    relative "main.py", so it must be resolved against THAT process's cwd rather
    than ours. Getting this wrong is how the first version of this function
    pointed at the scratch dir.
    """
    root = os.environ.get("COMFYUI_ROOT")
    if root:
        return os.path.abspath(root)
    for pid in os.listdir("/proc") if os.path.isdir("/proc") else []:
        if not pid.isdigit():
            continue
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as f:
                argv = f.read().decode(errors="replace").split("\0")
            argv = [x for x in argv if x]
            if not argv or not any(x.endswith("main.py") for x in argv):
                continue
            if "python" not in argv[0]:
                continue
            script = next(x for x in argv if x.endswith("main.py"))
            if os.path.isabs(script):
                return os.path.dirname(script)
            return os.path.realpath(f"/proc/{pid}/cwd")
        except (OSError, StopIteration):
            continue
    return os.path.expanduser("~/ComfyUI")


def comfy_dirs(base: str) -> tuple[str, str]:
    """(input_dir, output_dir) of the ComfyUI we are talking to."""
    root = comfy_root(base)
    return os.path.join(root, "input"), os.path.join(root, "output")


def link_line(kind: str, path: str, extra: str = ""):
    """Print a ready-to-paste markdown link relative to the files dir, the way
    propkit does — the chat viewer resolves against that dir, so a path in prose
    renders as text while this renders as the artifact."""
    files = os.environ.get("ORX_FILES_DIR") or os.environ.get("FILES") or ""
    p = os.path.abspath(path)
    rel = os.path.relpath(p, os.path.abspath(files)) if files and p.startswith(os.path.abspath(files)) else p
    print(f"  {kind}: {path}{('  ' + extra) if extra else ''}")
    print(f"  link: [{kind}]({rel})")


# --- weights ----------------------------------------------------------------

def installed_options(base: str) -> dict:
    oi = api_get(base, "/object_info")
    out = {}
    for _, fname, _, node, field in WEIGHTS:
        spec = None
        n = oi.get(node) or {}
        for grp in ("required", "optional"):
            g = (n.get("input", {}) or {}).get(grp, {}) or {}
            if field in g:
                spec = g[field]
                break
        opts = []
        if spec:
            t = spec[0]
            if isinstance(t, list):
                opts = t
            elif len(spec) > 1 and isinstance(spec[1], dict):
                opts = spec[1].get("options") or []
        out[fname] = opts
    return out


def cmd_models(a) -> None:
    if a.install:
        inp, _ = comfy_dirs(a.comfy)
        models = os.path.join(os.path.dirname(inp), "models")
        py = os.path.join(os.path.dirname(inp), "venv", "bin", "python")
        if not os.path.exists(py):
            py = sys.executable
        for sub, fname, repo, _, _ in WEIGHTS:
            dest = os.path.join(models, sub, fname)
            if os.path.exists(dest):
                print(f"  have  {sub}/{fname}")
                continue
            print(f"  fetch {sub}/{fname}  <- {repo}", flush=True)
            r = subprocess.run([py, "-m", "huggingface_hub.cli.hf", "download",
                                repo, f"{sub}/{fname}", "--local-dir", models])
            if r.returncode != 0:
                print(f"  FAILED {sub}/{fname} (rc={r.returncode})", file=sys.stderr)

    opts = installed_options(a.comfy)
    missing = [f for _, f, _, _, _ in WEIGHTS if f not in opts.get(f, [])]
    for sub, fname, repo, node, field in WEIGHTS:
        ok = fname in (opts.get(fname) or [])
        print(f"  [{'ok' if ok else '--'}] {node}.{field}: {sub}/{fname}")
    if missing:
        print(f"\n{len(missing)} missing: {', '.join(missing)}")
        print("Run `splatkit.py models --install`. ComfyUI picks new files up without a restart.")
        sys.exit(1)
    print("\nAll TripoSplat weights present and visible to ComfyUI.")


# --- the graph --------------------------------------------------------------

def build_graph(image_name: str, prefix: str, a) -> dict:
    g = {}
    g["1"] = {"class_type": "LoadImage", "inputs": {"image": image_name}}

    if a.remove_bg:
        g["2a"] = {"class_type": "LoadBackgroundRemovalModel",
                   "inputs": {"bg_removal_name": "birefnet.safetensors"}}
        g["2b"] = {"class_type": "RemoveBackground",
                   "inputs": {"bg_removal_model": ["2a", 0], "image": ["1", 0]}}
        mask = ["2b", 0]
    else:
        # LoadImage's MASK is 1-alpha; invert it so the subject reads as 1, the
        # polarity RemoveBackground emits and TripoSplatPreprocessImage expects.
        g["2"] = {"class_type": "InvertMask", "inputs": {"mask": ["1", 1]}}
        mask = ["2", 0]

    g["3"] = {"class_type": "TripoSplatPreprocessImage",
              "inputs": {"image": ["1", 0], "mask": mask,
                         "erode_radius": a.erode, "size": 1024}}
    g["4"] = {"class_type": "CLIPVisionLoader",
              "inputs": {"clip_name": "dino_v3_vit_h.safetensors"}}
    g["5"] = {"class_type": "VAELoader", "inputs": {"vae_name": "flux2-vae.safetensors"}}
    g["6"] = {"class_type": "VAELoader",
              "inputs": {"vae_name": "triposplat_vae_decoder_fp16.safetensors"}}
    g["7"] = {"class_type": "TripoSplatConditioning",
              "inputs": {"clip_vision": ["4", 0], "vae": ["5", 0], "image": ["3", 0]}}
    g["8"] = {"class_type": "UNETLoader",
              "inputs": {"unet_name": "triposplat_fp16.safetensors",
                         "weight_dtype": "default"}}
    g["9"] = {"class_type": "KSampler",
              "inputs": {"model": ["8", 0], "seed": a.seed, "steps": a.steps,
                         "cfg": a.cfg, "sampler_name": "dpmpp_2m",
                         "scheduler": "simple", "positive": ["7", 0],
                         "negative": ["7", 1], "latent_image": ["7", 2],
                         "denoise": 1.0}}
    g["10"] = {"class_type": "VAEDecodeTripoSplat",
               "inputs": {"samples": ["9", 0], "vae": ["6", 0],
                          "num_gaussians": a.gaussians, "seed": a.decode_seed}}
    splat = ["10", 0]

    # .ply is the artifact `check` reads and the viewer loads; .spz is the compact
    # one to ship (5.8x smaller on the measured oak).
    g["11"] = {"class_type": "SplatToFile3D", "inputs": {"splat": splat, "format": "ply"}}
    g["12"] = {"class_type": "SaveGLB", "inputs": {"mesh": ["11", 0],
                                                   "filename_prefix": prefix + "-ply"}}
    if a.spz:
        g["13"] = {"class_type": "SplatToFile3D", "inputs": {"splat": splat, "format": "spz"}}
        g["14"] = {"class_type": "SaveGLB", "inputs": {"mesh": ["13", 0],
                                                       "filename_prefix": prefix + "-spz"}}
    if a.mesh:
        g["15"] = {"class_type": "SplatToMesh",
                   "inputs": {"splat": splat, "resolution": a.mesh_resolution,
                              "kernel": 5, "smooth": 0, "level": a.mesh_level,
                              "min_component": 500, "min_opacity": 0.02,
                              "color_sharpen": 2.0}}
        g["16"] = {"class_type": "SaveGLB", "inputs": {"mesh": ["15", 0],
                                                       "filename_prefix": prefix + "-mesh"}}

    if a.frames > 1:
        g["17"] = {"class_type": "CreateCameraInfo",
                   "inputs": {"mode": "orbit", "mode.yaw": 35.0, "mode.pitch": 30.0,
                              "mode.distance": a.distance, "target_x": 0.0,
                              "target_y": 0.0, "target_z": 0.0, "roll": 0.0,
                              "fov": 35.0, "zoom": 1.0, "camera_type": "perspective"}}
        g["18"] = {"class_type": "RenderSplat",
                   "inputs": {"splat": splat, "width": a.size, "height": a.size,
                              "frames": a.frames, "splat_scale": 1.0, "sharpen": 2.0,
                              "headlight_shading": 0.0, "opacity_threshold": 0.0,
                              "render_style": "color", "background": a.background,
                              "camera_info": ["17", 0]}}
        g["19"] = {"class_type": "CreateVideo", "inputs": {"images": ["18", 0], "fps": a.fps}}
        g["20"] = {"class_type": "SaveVideo",
                   "inputs": {"video": ["19", 0], "filename_prefix": prefix + "-turntable",
                              "format": "auto", "codec": "auto"}}

    # Front orthographic still + mask. Orthographic because the source sprite was
    # drawn without perspective, and IoU against a perspective render measures the
    # lens as much as the shape.
    g["21"] = {"class_type": "CreateCameraInfo",
               "inputs": {"mode": "orbit", "mode.yaw": 0.0, "mode.pitch": 0.0,
                          "mode.distance": a.distance, "target_x": 0.0, "target_y": 0.0,
                          "target_z": 0.0, "roll": 0.0, "fov": 35.0, "zoom": 1.0,
                          "camera_type": "orthographic"}}
    g["22"] = {"class_type": "RenderSplat",
               "inputs": {"splat": splat, "width": a.size, "height": a.size, "frames": 1,
                          "splat_scale": 1.0, "sharpen": 2.0, "headlight_shading": 0.0,
                          "opacity_threshold": 0.0, "render_style": "color",
                          "background": "#000000", "camera_info": ["21", 0]}}
    g["23"] = {"class_type": "SaveImage",
               "inputs": {"images": ["22", 0], "filename_prefix": prefix + "-front"}}
    g["24"] = {"class_type": "MaskToImage", "inputs": {"mask": ["22", 1]}}
    g["25"] = {"class_type": "SaveImage",
               "inputs": {"images": ["24", 0], "filename_prefix": prefix + "-frontmask"}}
    return g


def submit_and_wait(base: str, graph: dict, timeout: int, quiet: bool) -> dict:
    r = api_post(base, "/prompt", {"prompt": graph, "client_id": str(uuid.uuid4())})
    if r.get("node_errors"):
        die("ComfyUI rejected the graph:\n" + json.dumps(r["node_errors"], indent=2)[:2000])
    pid = r["prompt_id"]
    t0 = time.time()
    if not quiet:
        print(f"  submitted {pid} ({len(graph)} nodes)", flush=True)
    while True:
        time.sleep(2)
        h = api_get(base, f"/history/{pid}")
        if pid in h:
            entry = h[pid]
            st = entry.get("status", {})
            if st.get("status_str") == "error":
                for m in st.get("messages", []):
                    if m[0] == "execution_error":
                        d = m[1]
                        die(f"node {d.get('node_type')} failed: {d.get('exception_message')}", 1)
                die("execution failed", 1)
            if not quiet:
                print(f"  done in {time.time() - t0:.1f}s", flush=True)
            return entry
        if time.time() - t0 > timeout:
            die(f"timed out after {timeout}s waiting for {pid}", 1)


def collect(entry: dict, out_dir: str) -> dict:
    """Map filename_prefix stem -> absolute path, for every saved artifact."""
    found = {}
    for _, out in (entry.get("outputs") or {}).items():
        for _, items in out.items():
            if not isinstance(items, list):
                continue
            for it in items:
                if not isinstance(it, dict) or not it.get("filename"):
                    continue
                p = os.path.join(out_dir, it.get("subfolder") or "", it["filename"])
                stem = it["filename"]
                for tag in ("-ply_", "-spz_", "-mesh_", "-turntable_", "-frontmask_", "-front_"):
                    if tag in stem:
                        found[tag.strip("-_")] = p
                        break
    return found


def cmd_splat(a) -> None:
    if not os.path.exists(a.image):
        die(f"no such image: {a.image}")
    inp, out_dir = comfy_dirs(a.comfy)
    if not os.path.isdir(inp):
        die(f"ComfyUI input dir not found: {inp} (set COMFYUI_ROOT)")

    # Refuse a thumbnail for the same reason propkit does: every downstream
    # measurement reads the silhouette and palette from this image.
    try:
        from PIL import Image
        im = Image.open(a.image)
        w, h = im.size
        has_alpha = im.mode in ("RGBA", "LA") or "transparency" in im.info
    except Exception as e:
        die(f"cannot read {a.image}: {e}")
    if min(w, h) < 384 and not a.allow_small:
        die(f"{a.image} is {w}x{h}. A splat planned from a shipped sprite comes out a "
            f"blob that still passes every structural check.\n"
            f"        Up-res first: propkit.py upres {a.image} -o big.png --refine\n"
            f"        (--allow-small overrides.)")
    if not has_alpha and not a.remove_bg:
        print("  note: source has no alpha and --remove-bg was not passed; the "
              "background will be reconstructed as geometry.", file=sys.stderr)

    stem = os.path.splitext(os.path.basename(a.output))[0]
    staged = f"splatkit-{stem}-{int(time.time())}.png"
    shutil.copy(a.image, os.path.join(inp, staged))

    prefix = f"splatkit/{stem}"
    graph = build_graph(staged, prefix, a)
    entry = submit_and_wait(a.comfy, graph, a.timeout, a.quiet)
    got = collect(entry, out_dir)
    if "ply" not in got:
        die("run finished but produced no .ply", 1)

    base = os.path.splitext(a.output)[0]
    os.makedirs(os.path.dirname(os.path.abspath(a.output)) or ".", exist_ok=True)
    dests = {
        "ply": a.output,
        "spz": base + ".spz",
        "mesh": base + "-mesh.glb",
        "turntable": base + "-turntable.mp4",
        "front": base + "-front.png",
        "frontmask": base + "-frontmask.png",
    }
    written = {}
    for tag, dest in dests.items():
        if tag in got:
            shutil.copy(got[tag], dest)
            written[tag] = dest

    stats = ply_stats(a.output)
    print(f"\n{stats['count']} gaussians, "
          f"extent {stats['extent'][0]:.2f}x{stats['extent'][1]:.2f}x{stats['extent'][2]:.2f}")
    for tag in ("ply", "spz", "mesh", "turntable", "front"):
        if tag in written:
            sz = os.path.getsize(written[tag]) / 1e6
            link_line(tag, written[tag], f"({sz:.1f} MB)")
    if "mesh" in written:
        print("\n  The mesh is a handoff, not a prop: run `propkit.py clean` on it "
              "before anything else.")
    manifest = {
        "source": os.path.abspath(a.image),
        "route": "local-comfyui-triposplat",
        "seed": a.seed, "decode_seed": a.decode_seed, "steps": a.steps, "cfg": a.cfg,
        "num_gaussians": a.gaussians, "remove_bg": a.remove_bg, "erode": a.erode,
        "stats": stats,
        "artifacts": {k: os.path.abspath(v) for k, v in written.items()},
    }
    with open(base + "-manifest.json", "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"  manifest: {base}-manifest.json")


# --- measurement ------------------------------------------------------------

def ply_stats(path: str) -> dict:
    """Count, bounding box and degenerate fraction, read straight from the file.
    No ComfyUI needed, so `check` works on a delivered artifact months later."""
    with open(path, "rb") as f:
        hdr = b""
        while b"end_header" not in hdr:
            chunk = f.read(256)
            if not chunk:
                die(f"{path} is not a PLY (no end_header)")
            hdr += chunk
        text = hdr.split(b"end_header")[0].decode("ascii", "replace")
        offset = hdr.find(b"end_header") + len(b"end_header\n")
        count = 0
        props = []
        for line in text.splitlines():
            if line.startswith("element vertex"):
                count = int(line.split()[-1])
            elif line.startswith("property float"):
                props.append(line.split()[-1])
        if "binary_little_endian" not in text:
            die(f"{path}: only binary_little_endian PLY is supported")
        stride = len(props) * 4
        f.seek(offset)
        raw = f.read(count * stride)
    if len(raw) < count * stride:
        die(f"{path}: truncated ({len(raw)} of {count * stride} bytes)")

    try:
        import numpy as np
    except ImportError:
        return {"count": count, "properties": props, "extent": [0, 0, 0]}
    arr = np.frombuffer(raw, dtype="<f4").reshape(count, len(props))
    idx = {p: i for i, p in enumerate(props)}
    xyz = arr[:, [idx["x"], idx["y"], idx["z"]]]
    lo, hi = xyz.min(0), xyz.max(0)
    op = 1.0 / (1.0 + np.exp(-arr[:, idx["opacity"]]))  # stored as logit
    scale = np.exp(arr[:, [idx["scale_0"], idx["scale_1"], idx["scale_2"]]])
    return {
        "count": int(count),
        "properties": props,
        "bbox_min": [round(float(v), 4) for v in lo],
        "bbox_max": [round(float(v), 4) for v in hi],
        "extent": [round(float(v), 4) for v in (hi - lo)],
        "centre": [round(float(v), 4) for v in ((hi + lo) / 2)],
        "opacity_mean": round(float(op.mean()), 4),
        "near_transparent_pct": round(float((op < 0.05).mean()) * 100, 2),
        "scale_median": round(float(np.median(scale)), 6),
        "huge_splat_pct": round(float((scale.max(1) > 0.5).mean()) * 100, 3),
    }


def norm_mask(m, n: int = 512):
    """propkit's normalisation, reproduced exactly so the IoU numbers from the two
    tools are comparable: crop to the bbox, then resize to a square."""
    from PIL import Image
    import numpy as np
    ys, xs = np.where(m)
    if len(xs) == 0:
        return None
    c = Image.fromarray((m * 255).astype("uint8")).crop(
        (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1))
    return np.asarray(c.resize((n, n), Image.NEAREST)) > 127


def cmd_check(a) -> None:
    from PIL import Image
    import numpy as np

    stats = ply_stats(a.splat)
    fails = []
    print(f"  gaussians          {stats['count']}")
    print(f"  extent             {stats['extent']}")
    print(f"  centre             {stats.get('centre')}")
    print(f"  opacity mean       {stats.get('opacity_mean')}")
    print(f"  near-transparent   {stats.get('near_transparent_pct')}%")
    print(f"  huge splats        {stats.get('huge_splat_pct')}%")

    if stats["count"] < a.min_gaussians:
        fails.append(f"only {stats['count']} gaussians (min {a.min_gaussians})")
    if stats.get("near_transparent_pct", 0) > a.max_transparent:
        fails.append(f"{stats['near_transparent_pct']}% near-transparent "
                     f"(max {a.max_transparent}%) — the decode mostly produced nothing")
    ext = stats.get("extent") or [0, 0, 0]
    if max(ext) > 0 and min(ext) / max(ext) < a.min_aspect:
        fails.append(f"extent {ext} is degenerate: thinnest axis is "
                     f"{min(ext) / max(ext):.3f} of the longest (min {a.min_aspect}) "
                     f"— this is a flat sheet, not a volume")

    mask_path = a.front_mask or os.path.splitext(a.splat)[0] + "-frontmask.png"
    if a.source:
        if not os.path.exists(mask_path):
            die(f"no front mask at {mask_path}; pass --front-mask, or re-run "
                f"`splatkit splat` which writes it")
        src = Image.open(a.source)
        if src.mode != "RGBA":
            die("--source has no alpha; run `propkit.py isolate` first")
        ssil = np.asarray(src)[..., 3] > 128
        msil = np.asarray(Image.open(mask_path).convert("L")) > 128
        A_, B_ = norm_mask(msil), norm_mask(ssil)
        if A_ is None:
            fails.append("front render is empty — nothing was reconstructed")
        else:
            inter, union = (A_ & B_).sum(), (A_ | B_).sum()
            iou = round(float(inter / union), 4)
            print(f"  silhouette IoU     {iou}  "
                  f"(splat-only {round(float((A_ & ~B_).sum() / union) * 100, 2)}%, "
                  f"source-only {round(float((B_ & ~A_).sum() / union) * 100, 2)}%)")
            if iou < a.min_iou:
                fails.append(f"silhouette IoU {iou} below --min-iou {a.min_iou}")

    if fails:
        print("\nFAIL")
        for f in fails:
            print(f"  - {f}")
        sys.exit(1)
    print("\nPASS")


def cmd_compare(a) -> None:
    """Source beside the splat, plus an orbit strip. The only stage that answers
    'is this better than the art we already have?'"""
    from PIL import Image
    base = os.path.splitext(a.splat)[0]
    front = a.front or base + "-front.png"
    turntable = a.turntable or base + "-turntable.mp4"
    if not os.path.exists(front):
        die(f"no front render at {front}; pass --front")

    cell = a.cell
    tiles = []
    src = Image.open(a.source).convert("RGBA")
    src.thumbnail((cell, cell))
    tiles.append(("source", src))
    fr = Image.open(front).convert("RGBA").resize((cell, cell))
    tiles.append(("splat front", fr))

    strip = []
    if os.path.exists(turntable) and shutil.which("ffmpeg"):
        tmp = tempfile.mkdtemp()
        n = a.orbit
        # Read the real frame count: --frames is a flag, so assuming the default
        # 75 samples the same handful of frames from a 40-frame orbit.
        total = 0
        if shutil.which("ffprobe"):
            pr = subprocess.run(
                ["ffprobe", "-v", "error", "-select_streams", "v:0",
                 "-count_frames", "-show_entries", "stream=nb_read_frames",
                 "-of", "default=nw=1:nk=1", turntable],
                capture_output=True, text=True)
            total = int(pr.stdout.strip() or 0)
        step = max(1, total // n) if total else 12
        # No `thumbnail` filter here: it collapses the stream to one
        # representative frame, which silently yields a 1-view "strip".
        subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", turntable,
                        "-vf", f"select='not(mod(n\\,{step}))'", "-vsync", "vfr",
                        "-frames:v", str(n), os.path.join(tmp, "f%02d.png")],
                       check=False)
        for f in sorted(os.listdir(tmp))[:n]:
            im = Image.open(os.path.join(tmp, f)).convert("RGBA").resize((cell, cell))
            strip.append(im)
        if len(strip) < n:
            print(f"  note: only {len(strip)} of {n} orbit views extracted from "
                  f"{total or 'unknown'} frames", file=sys.stderr)

    total = len(tiles) + len(strip)
    canvas = Image.new("RGBA", (cell * total, cell), (24, 26, 32, 255))
    x = 0
    for _, im in tiles:
        canvas.paste(im, (x + (cell - im.width) // 2, (cell - im.height) // 2), im)
        x += cell
    for im in strip:
        canvas.paste(im, (x, 0), im)
        x += cell
    canvas.convert("RGB").save(a.output)
    print(f"  {total} panels: source, splat front, {len(strip)} orbit views")
    link_line("compare", a.output)
    print("\n  Look at it. A splat that passes `check` can still read worse than "
          "the sprite it came from; keeping the sprite is a legitimate outcome.")


# --- remote fallback --------------------------------------------------------

def cmd_space(a) -> None:
    """Drive the HF Space, reading its contract from its own agents.md.

    Deliberately does not hardcode the endpoint shape: the point of agents.md is
    that the Space tells you how to call it, so a change there does not silently
    break this. See https://huggingface.co/blog/mishig/spaces-agents-md.
    """
    token = os.environ.get("HF_TOKEN")
    if not token:
        die("HF_TOKEN is not set. The local route needs no token — prefer it. "
            "For this one, add a token in the orx up settings (Hugging Face) or "
            "run `hf auth login`.")
    print(f"  WARNING: this uploads {a.image} to the {a.space} Space, a "
          f"third-party service.", file=sys.stderr)

    md_url = f"https://huggingface.co/spaces/{a.space}/agents.md"
    try:
        with urllib.request.urlopen(md_url, timeout=60) as r:
            agents_md = r.read().decode()
    except Exception as e:
        die(f"cannot read {md_url}: {e}")
    print("  --- agents.md ---")
    for line in agents_md.strip().splitlines():
        print(f"  {line}")

    host = None
    for line in agents_md.splitlines():
        if "hf.space" in line:
            for tok in line.replace(",", " ").split():
                if "hf.space" in tok:
                    host = tok.split("/gradio_api")[0].rstrip("/")
                    break
        if host:
            break
    if not host:
        die("agents.md did not advertise an hf.space base URL; read it above and "
            "drive the Space by hand rather than guessing")

    auth = {"Authorization": f"Bearer {token}"}
    boundary = uuid.uuid4().hex
    with open(a.image, "rb") as f:
        blob = f.read()
    name = os.path.basename(a.image)
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; "
            f"filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").encode() + blob + \
           f"\r\n--{boundary}--\r\n".encode()
    req = urllib.request.Request(f"{host}/gradio_api/upload", data=body, headers={
        **auth, "Content-Type": f"multipart/form-data; boundary={boundary}"})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            uploaded = json.loads(r.read())[0]
    except urllib.error.HTTPError as e:
        die(f"upload failed (HTTP {e.code}): {e.read().decode()[:400]}")
    print(f"  uploaded -> {uploaded}")

    payload = {"image": {"path": uploaded, "meta": {"_type": "gradio.FileData"},
                         "orig_name": name},
               "seed": a.seed, "steps": a.steps, "guidance_scale": a.cfg,
               "num_gaussians": a.gaussians, "output_format": "ply"}
    req = urllib.request.Request(f"{host}/gradio_api/call/generate",
                                 data=json.dumps({"data": list(payload.values())}).encode(),
                                 headers={**auth, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            event_id = json.loads(r.read())["event_id"]
    except urllib.error.HTTPError as e:
        die(f"call failed (HTTP {e.code}): {e.read().decode()[:400]}\n"
            f"        Re-read the schema: {host}/gradio_api/info")
    print(f"  event {event_id}, polling…")

    req = urllib.request.Request(f"{host}/gradio_api/call/generate/{event_id}", headers=auth)
    result = None
    with urllib.request.urlopen(req, timeout=a.timeout) as r:
        for raw in r:
            line = raw.decode().strip()
            if line.startswith("data:"):
                try:
                    result = json.loads(line[5:].strip())
                except Exception:
                    continue
            if line.startswith("event: complete"):
                break
    if not result:
        die("Space returned no result; it may be queued or rate-limited. The local "
            "route has no quota — prefer it.", 1)

    url = None
    def walk(o):
        nonlocal url
        if isinstance(o, dict):
            if o.get("url"):
                url = url or o["url"]
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)
    walk(result)
    if not url:
        die(f"no file URL in the Space's response: {json.dumps(result)[:400]}", 1)
    with urllib.request.urlopen(urllib.request.Request(url, headers=auth), timeout=600) as r:
        with open(a.output, "wb") as f:
            shutil.copyfileobj(r, f)
    link_line("splat", a.output, f"({os.path.getsize(a.output) / 1e6:.1f} MB, remote)")
    print("  No turntable or front mask: the remote route returns the splat only, "
          "so `check --source` cannot measure IoU on it.")


# --- cli --------------------------------------------------------------------

def main() -> None:
    p = argparse.ArgumentParser(
        prog="splatkit.py", description="2D image to 3D gaussian splat, locally.")
    p.add_argument("--comfy", default=os.environ.get("COMFYUI_URL", COMFY_DEFAULT),
                   help=f"ComfyUI base URL (default {COMFY_DEFAULT})")
    sub = p.add_subparsers(dest="cmd", required=True)

    q = sub.add_parser("models", help="verify the TripoSplat weights via /object_info")
    q.add_argument("--install", action="store_true", help="download what is missing (~3.4 GB)")
    q.set_defaults(fn=cmd_models)

    q = sub.add_parser("splat", help="image -> splat, plus turntable and front views")
    q.add_argument("image")
    q.add_argument("-o", "--output", required=True, help="the .ply to write")
    q.add_argument("--gaussians", type=int, default=262144,
                   help="rounded to x32; higher oversamples, it does not add detail")
    q.add_argument("--seed", type=int, default=46)
    q.add_argument("--decode-seed", type=int, default=790219963981395)
    q.add_argument("--steps", type=int, default=20)
    q.add_argument("--cfg", type=float, default=3.0)
    q.add_argument("--erode", type=int, default=1)
    q.add_argument("--remove-bg", action="store_true",
                   help="run BiRefNet first; skip it when the source already has alpha")
    q.add_argument("--no-spz", dest="spz", action="store_false", help="skip the compact .spz")
    q.add_argument("--mesh", action="store_true",
                   help="also emit a mesh GLB — a handoff to `propkit clean`, not a prop")
    q.add_argument("--mesh-resolution", type=int, default=384)
    q.add_argument("--mesh-level", type=float, default=0.6)
    q.add_argument("--frames", type=int, default=75, help="turntable frames; 1 disables it")
    q.add_argument("--fps", type=float, default=25.0)
    q.add_argument("--size", type=int, default=1024)
    q.add_argument("--distance", type=float, default=2.5)
    q.add_argument("--background", default="#848484")
    q.add_argument("--allow-small", action="store_true", help="override the 384px floor")
    q.add_argument("--timeout", type=int, default=1800)
    q.add_argument("--quiet", action="store_true")
    q.set_defaults(fn=cmd_splat, spz=True)

    q = sub.add_parser("check", help="is this a real, usable splat?")
    q.add_argument("splat")
    q.add_argument("--source", help="the isolated source image, for silhouette IoU")
    q.add_argument("--front-mask", help="defaults to <splat>-frontmask.png")
    q.add_argument("--min-iou", type=float, default=0.85)
    q.add_argument("--min-gaussians", type=int, default=1000)
    q.add_argument("--max-transparent", type=float, default=60.0)
    q.add_argument("--min-aspect", type=float, default=0.05)
    q.set_defaults(fn=cmd_check)

    q = sub.add_parser("compare", help="source beside the splat, at a size you can judge")
    q.add_argument("splat")
    q.add_argument("--source", required=True)
    q.add_argument("-o", "--output", required=True)
    q.add_argument("--front", help="defaults to <splat>-front.png")
    q.add_argument("--turntable", help="defaults to <splat>-turntable.mp4")
    q.add_argument("--orbit", type=int, default=4, help="orbit views to include")
    q.add_argument("--cell", type=int, default=384)
    q.set_defaults(fn=cmd_compare)

    q = sub.add_parser("space", help="remote HF Space fallback (uploads your image)")
    q.add_argument("image")
    q.add_argument("-o", "--output", required=True)
    q.add_argument("--space", default=SPACE_DEFAULT)
    q.add_argument("--gaussians", type=int, default=262144)
    q.add_argument("--seed", type=int, default=42)
    q.add_argument("--steps", type=int, default=20)
    q.add_argument("--cfg", type=float, default=3.0)
    q.add_argument("--timeout", type=int, default=1800)
    q.set_defaults(fn=cmd_space)

    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
