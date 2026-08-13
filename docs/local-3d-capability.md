# Local 3D capability: what this machine can run, why the gaps exist, how to close them

**Date: 8 August 2026.** Findings are specific to this machine and will drift as
ComfyUI and torch move. Re-verify before acting on it.

Companion to [sprite-animation-research.md](./sprite-animation-research.md),
which establishes *why* the 3D-proxy route is the one worth pursuing. This
document covers whether we can actually run it here.

---

## 1. The machine

| | |
|---|---|
| GPU | RTX 3090, 25.3 GB VRAM (3.9 GB free — ComfyUI cache holds the rest) |
| ComfyUI | 0.30.0 at `/home/paul/ComfyUI` |
| torch | **2.13.0+cu130**, CXX11 ABI on |
| Python | 3.12.13 (ComfyUI venv at `/home/paul/ComfyUI/venv`) |
| Blender | **5.2.0 LTS**, build 2026-07-14, MCP add-on live on port 9876 |
| CUDA toolkit | **nvcc 12.4** (driver 580.173.02) |
| Compilers | gcc 15.2 and **gcc-13** both present |
| Docker | 29.6.2, daemon reachable, **nvidia-ctk present** |
| Disk | 91 GB free |

---

## 2. What works today

- **ComfyUI core: 798 nodes.** Image generation confirmed working end to end.
- **Hunyuan3D v2 nodes** (`EmptyLatentHunyuan3Dv2`, `Hunyuan3Dv2Conditioning`,
  `Hunyuan3Dv2ConditioningMultiView`, `VAEDecodeHunyuan3D`) — these are **ComfyUI
  core, pure Python, no compiled extensions**, so they are unaffected by the ABI
  problem below. Models are not installed.
- **Mesh handling**: `Load3D`, `Load3DAdvanced`, `Preview3D`, `VoxelToMesh`.
- **`nvdiffrast` imports cleanly** — the one compiled 3D dependency that works.
- **Blender via MCP** — scene building, orthographic rendering and PNG output with
  alpha all verified working during the sprite experiments.
- **`pixel.mjs`** (the target game, imported read-only) enforces the sprite contract:
  `keyBackground`, `trim`, `planFit`, `renderFit`, `remapToPalette`,
  `alphaThreshold`, `selOutRepair`.

## 3. What does not work, and exactly why

### 3.1 Trellis2 — 0 nodes registered

```
ImportError: .../site-packages/cumesh/_C.cpython-312-x86_64-linux-gnu.so:
undefined symbol: _ZN3c104impl3cow23materialize_cow_storageERNS_11StorageImplE
```

Demangled: `c10::impl::cow::materialize_cow_storage(c10::StorageImpl&)`.

**Root cause: upstream PyTorch renamed the function, and the wheel's metadata
lies about its own compatibility.**

Confirmed by inspecting the installed torch:

```
libc10.so:      0 symbols matching materialize_cow_storage
libtorch_cpu.so: 0 symbols matching materialize_cow_storage
# what torch 2.13 actually exports:
c10::impl::cow::materialize_cow(c10::StorageImpl*)     <- pointer, renamed
c10::impl::cow::cow_deleter(void*)
c10::impl::cow::is_cow_data_ptr(c10::DataPtr const&)
```

So old torch took a **reference** and was called `materialize_cow_storage`; torch
2.13 takes a **pointer** and is called `materialize_cow`. `cumesh 1.0` is a
prebuilt binary compiled against the old ABI, and its metadata declares
`Requires-Dist: torch >=2.4.0` **with no upper bound** — so pip installed it
against a torch that cannot satisfy it. This is a packaging bug upstream, not a
misconfiguration here.

**It is not one package.** `flash_attn` fails with the same class of ABI error,
and four more dependencies are missing outright:

| dependency | state |
|---|---|
| `cumesh` | installed, **ABI-broken** |
| `flash_attn` | installed, **ABI-broken** |
| `nvdiffrast` | OK |
| `diffoctreerast` | **missing** |
| `spconv` | **missing** |
| `utils3d` | **missing** |
| `kaolin` | **missing** |

Fixing `cumesh` alone just advances to the next failure.

### 3.2 Why we cannot simply rebuild in place

- torch is built for **CUDA 13.0**; the installed toolkit is **nvcc 12.4**.
  Building torch extensions against a different CUDA major version than torch
  itself is not supported.
- Default `gcc` is **15.2**, beyond what CUDA 12.4 accepts (max 13). `gcc-13`
  exists, so *that* half is solvable — but the CUDA-version mismatch is not.
- Several sources may still call the old torch API, so even a correct toolchain
  could need source patches (`materialize_cow_storage(x)` → `materialize_cow(&x)`).

**Conclusion: an in-place rebuild is the worst option.** It requires installing a
CUDA 13.0 toolkit system-wide, rebuilding four-plus CUDA extensions, and possibly
patching sources — all against the venv that currently runs ComfyUI perfectly.

### 3.3 Other gaps

- **No ControlNet models** (`ControlNetLoader` empty). Note: depth/canny
  ControlNet needs no pose estimation, so the earlier objection to ControlNet was
  wrong — see the research doc §7.
- **No checkpoints, no CLIP-vision** (`CheckpointLoaderSimple`,
  `ImageOnlyCheckpointLoader`, `CLIPVisionLoader` all empty). Hunyuan3D v2 needs
  a DiT checkpoint and a CLIP-vision encoder.

> **Correction (same day).** An earlier revision said ComfyUI's `models/`
> directories were empty and the install must be loading from paths elsewhere.
> That was wrong: **there are two ComfyUI checkouts on this machine** —
> `/home/paul/projects/ComfyUI` (a modelless checkout, the one reachable as
> `../ComfyUI` from crux) and `/home/paul/ComfyUI` (**the running server**, cwd
> confirmed from its pid, holding the models under
> `models/diffusion_models/`). Always disambiguate by the running process's cwd.
> crux's `install_path()` defaults to `~/ComfyUI`, so it points at the right one;
> `COMFYUI_PATH` overrides it if that ever stops being true.
- **Only two diffusion models**: `krea2_turbo_fp8_scaled` (guidance-distilled,
  runs at cfg 1 — editing instructions have no guidance channel) and
  `minimax_h3` (video).
- **Tripo (16), Meshy (7), Rodin (7) nodes are cloud API** — keys and credits, not
  local compute. Includes `MeshyRigModelNode` and `MeshyAnimateModelNode`.
- **Kimodo Bridge** (Blender add-on for NVIDIA's motion model) advertises Blender
  **3.6–4.x**; this machine runs **5.2 LTS**, so it likely will not load. Kimodo
  itself is a standalone local server (Apache-2.0, <3 GB VRAM with
  `TEXT_ENCODER_DEVICE=cpu`, tested on RTX 3090) and remains reachable regardless.

---

## 4. How to close the gaps, ranked by effort against risk

### Option 1 — Docker with pinned torch (recommended for any compiled 3D stack)

Docker 29.6.2 and `nvidia-ctk` are both present, so this is available now. Run
mesh generation in a container with torch pinned to the version its prebuilt
wheels expect (~2.4–2.6 + cu124), driven as a standalone step that writes a mesh
file to disk. Blender and ComfyUI then consume the file.

- **The working ComfyUI install is never touched** — that is the main argument
- No system CUDA toolkit installation, no gcc juggling
- Reproducible and disposable; a failed attempt costs nothing
- Cost: container build time, ~10–15 GB image, and 3D generation stops being a
  ComfyUI node and becomes a pipeline step (which for batch sprite work is
  arguably better anyway)

### Option 2 — Hunyuan3D v2 through ComfyUI core nodes (zero build risk)

The nodes are already registered and are pure Python. This needs **model files
only** — a DiT checkpoint plus a CLIP-vision encoder, roughly 3–5 GB against
91 GB free. No compilation, no ABI exposure, no container.

This is the only local image-to-3D path that requires **no build step at all**,
which on a torch-2.13/cu130 install is worth a great deal. Open question, for the
research to answer: whether Hunyuan3D v2 meshes are clean enough to rig.

### Option 3 — Cloud mesh generation (Tripo / Meshy / Rodin)

Nodes are already registered; needs an API key and credits. Zero install risk,
and Meshy additionally offers rigging and animation. Deviates from the "local"
premise and has a per-asset cost, but for 25 characters generated once, that cost
may well be trivial next to the engineering time of options 1 and 2.

### Option 4 — Rebuild everything in place

Install CUDA 13.0 toolkit, pin gcc-13, rebuild `cumesh`, `flash_attn`,
`diffoctreerast`, `spconv`, `utils3d`, `kaolin` from source, patch any that use
the removed torch API. **Not recommended** — highest effort, most fragile, and it
puts the one working ComfyUI install at risk to gain what options 1–3 already
provide.

---

## 5. What this rules in and out for the plan

- **Blender is not a constraint.** 5.2 LTS works, the MCP bridge works, ortho
  rendering with alpha works, and the render → `pixel.mjs` → contract chain is
  already proven end to end (six frames passed on the first attempt).
- **The constraint is the mesh**, not the renderer, the rig, or the animation.
- **Any plan depending on compiled-CUDA custom nodes inside the ComfyUI venv is
  unrealistic here** and should be rejected on that basis alone.
- **Kimodo is viable but optional** — it addresses motion quality, which has not
  been our failure mode. Hand-authored joint angles already produced a readable
  side-view stride; the boxes were the problem.
