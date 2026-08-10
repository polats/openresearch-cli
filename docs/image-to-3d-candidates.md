# Image-to-3D candidates for the sprite pipeline

**Date: 8 August 2026.** Fast-moving area; re-check before acting on this.

Stage 2 of the [sprite pipeline](./sprite-pipeline-build.md) turns a T-posed
character image into a mesh. This is the shortlist and the criterion.

## The criterion

**Not "which mesh looks best."** The mesh is consumed by UniRig and then rendered
at 96×96 with 20 colours, so surface detail and PBR textures are thrown away.
What matters, in order:

1. **Does UniRig produce a clean rig from it?** Topology at the joints, not
   fidelity. Quad or quad-ish beats triangle soup.
2. **Are limbs separated rather than fused?** Fused limbs are unrecoverable
   without manual surgery.
3. **Is the held weapon a separate part**, or one continuous shell with the hand?
4. **Licence** — this ships in a game.
5. **Does it run here** (torch 2.13.0+cu130, RTX 3090 24 GB, Docker available).

Note that 1–3 are all *topology*, and topology is exactly where every current
generator is weakest.

## Candidates

| Model | Licence | Runs here? | Why it's on the list |
|---|---|---|---|
| **[Pixal3D](https://github.com/TencentARC/Pixal3D)** (TencentARC) | **MIT** | **Docker** — needs the TRELLIS.2 base env + `natten` (no cu130/torch2.13 wheel; source build needs CUDA 13.0) | SIGGRAPH 2026, code May 2026. Pixel-aligned back-projection → direct pixel↔geometry correspondence. Cleanest licence of the lot. **Three community ComfyUI integrations exist** ([Saganaki22](https://github.com/Saganaki22/Pixal3D-ComfyUI), [dreamrec](https://github.com/dreamrec/ComfyUI-Pixal3D), [RH](https://github.com/RH-RunningHub/ComfyUI_RH_Pixal3D)); ≥16 GB VRAM, 24 GB recommended, low-VRAM mode available |
| **[TRELLIS.2-4B](https://huggingface.co/microsoft/TRELLIS.2-4B)** (Microsoft) | ⚠️ **no LICENSE file** | Docker (ABI-dead in-place) | Rated best overall open image-to-3D on HF 2026. O-Voxel, open surfaces, non-manifold, PBR. **Licence gap is a shipping blocker until resolved** |
| **Hunyuan3D v2** (kijai wrapper) | Tencent community licence | **Yes, build-free** — core requirements are pure Python | The only zero-build local option. Not the frontier; chosen originally for deployability, which is a different criterion from quality |
| **Hunyuan3D 2.5 / 3.0** | — | No | 2.5 tops the leaderboard (1325 vs TRELLIS 1290) but **is not the open-weights release**; 3.0 in ComfyUI is a **cloud Partner Node on Tencent's API** — sends art off-box |
| **Hi3DGen** | — | Unevaluated | Rated best *geometric* quality in 2026 roundups |
| **[QuadGPT](https://arxiv.org/html/2509.21420)** | Research | Unverified packaging | **Native quad topology** — attacks criterion 1 directly, and explicitly *"robust across soft-surface models (e.g. human characters)"* |
| **[PartCrafter](https://arxiv.org/pdf/2506.05573)** | Research | Unverified packaging | Compositional generation → **part separation**, i.e. criterion 3, the axe-fused-to-hand problem |
| **[Rigel3D](https://arxiv.org/html/2605.13129)** | Paper only | **No code or weights** | Generates geometry **and rig jointly** — would collapse stages 2+3. Built on TRELLIS, 9.7% FD-DINOv2 improvement. Watch, don't build |
| **[Mesh-Pro](https://arxiv.org/pdf/2603.00526)** | Research | Unverified | Artist-style quad mesh generation |
| Tripo / Meshy / Rodin | Commercial API | Cloud only | Nodes already registered in ComfyUI. Meshy also rigs and animates. Rejected here only because the brief is local |
| InstantMesh, Unique3D, CraftsMan, TripoSR, Stable Fast 3D | various | — | Strictly dominated by the above; listed so nobody re-proposes them |

## Notes on sources

Search results for these terms are dominated by SEO affiliate content — Meshy,
3D AI Studio, TripoSR and trellis2.app blogs all rank for each other's names and
review each other favourably. Only primary sources (arXiv, GitHub, HF model cards)
are cited above. The one leaderboard used
([Pixazo](https://www.pixazo.ai/leaderboard/ai-3d-model-generation)) should be
treated as soft; separately, 3D Arena's own analysis found **textured models gain
+144 ELO regardless of geometry**, i.e. these rankings measure "looks nice", not
"deforms well" — which is the opposite of our criterion.

## Plan

**Bake off on `warrior`'s T-pose**, judged on whether UniRig produces a clean rig
— not on how the mesh looks:

1. **Pixal3D in Docker** — best licence, newest, ComfyUI path exists
2. **Hunyuan3D v2 build-free** — the baseline; if it ties, it wins on simplicity
3. **TRELLIS.2 in Docker** — only if the licence question resolves

Half a day, and it's the difference between choosing a model and inheriting one.

## Standing correction

The first version of the build plan named Hunyuan3D v2 as *the* stage-2 model.
That was a **deployability** choice dressed up as a quality choice. Once we accept
a container for any stage, "build-free" stops being decisive and the model should
be picked on topology and licence instead.
