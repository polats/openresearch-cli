---
name: orx-comfyui
description: "Generate images on the local ComfyUI through its MCP tools: check installed models first, build a graph by adapting one from execution history rather than composing blind, validate before enqueuing, free VRAM, then fetch the render and look at it. Use when making or iterating on a generated asset, when a workflow fails to submit, or when a render runs out of memory."
---

You drive the ComfyUI running on this machine through the `comfyui-mcp` tools
crux manages. Every tool here has a long `action`-driven description of its own —
read those for syntax. **This module does not repeat them.** What follows is the
path that actually works, and the traps that cost a render each.

## Orient before you compose

1. **Health and inventory.** One call gets you ComfyUI's version, the GPU, free
   VRAM and the model inventory. Do it first, every session — VRAM and installed
   models both change under you.
2. **List local models** by folder. What matters is not just *whether* a model
   exists but *which folder* it is in, because that decides the loader:
   - `checkpoints/` → a single-file checkpoint, loaded by `CheckpointLoaderSimple`
   - `diffusion_models/` + `text_encoders/` + `vae/` → a **split** model, needing
     `UNETLoader` + `CLIPLoader` + `VAELoader` separately
   This distinction is the single most common cause of a graph that won't run.

## The high-level generator only fits checkpoints

There is a convenience action that builds a txt2img graph for you and
auto-selects a checkpoint. It is the right first choice **when
`models/checkpoints/` has something in it**.

On a split-model install (`checkpoints/` empty, weights in `diffusion_models/`)
it has nothing to select and will tell you so. That is not a bug to work around —
it is the signal to use the explicit route: author the graph, validate, enqueue.

## Build from a graph that worked

The node catalog is hundreds of classes and over a megabyte; a wrong
`class_type` or a mis-wired input is only discovered at submit. So:

**First choice — execution history.** Past successful runs on this machine hold
complete, known-good graphs referencing models that are actually installed. Read
one, copy its shape, change the prompt and seed. This is by far the cheapest way
to a working graph and it is what you should reach for before anything else.

**Second — a saved workflow** from the user's library.

**Third — ask about specific nodes.** When you need to know a node's inputs or
whether an enum accepts a value, query that node. Never fetch the entire catalog
to read it; it is enormous and answers nothing you couldn't have asked directly.

**Then validate.** A validation pass costs nothing and catches missing nodes,
bad enum values and unwired inputs before they cost you a render.

### Shipped templates are editor documents, not API graphs

The workflow templates that come with ComfyUI (and with custom-node packs) are
**UI documents**: `{"nodes": [...], "links": [...]}`, frequently wrapping the real
pipeline inside a **subgraph** definition. The queue wants the API prompt format:
`{"<id>": {"class_type": ..., "inputs": {...}}}`. A template cannot be submitted
as-is — the subgraph has to be expanded first. Read a template to learn the model
names and sampler settings it uses, then build the API graph yourself.

Templates also carry **feature switches** — booleans gating optional branches like
a style LoRA or an LLM prompt-enhancer. A node being present and unmuted does not
mean it is active. And where a template's saved widget values disagree with its own
documentation, trust the documentation: shipped templates carry stale defaults.

## VRAM

Free ComfyUI's model cache before a large render. It is the one pool you may
reclaim freely and it reloads on next use; expect to recover several gigabytes.

Then read the **real** numbers before saying anything about them. Free VRAM
reported by ComfyUI minus its own cache does not tell you who holds the rest —
other processes (a local LLM server, a game, a browser) do, and naming a phantom
"external process" sends the user hunting. If you cannot enumerate the holders,
say you cannot.

Never kill another process to free memory. Report the contention and let the user
choose. Some installs stream weights and will render fine with far less free VRAM
than the model's file size suggests — try before declaring it impossible, and if
it does fail for memory, offer the concrete options: lower resolution, smaller
batch, lighter model.

## Render, then look

Enqueue and **wait for the job** rather than polling in a tight loop. When it
finishes, the history entry gives you the status, the duration and the output
filenames.

Then **fetch the image and look at it.** A `success` status means the graph
executed — nothing more. It does not tell you the composition works, the subject
is intact, the style landed, or that it reads at its intended size. If your
harness cannot display images, say so and hand over the path; never assert a
visual result you have not seen.

## Deliver

Renders land in ComfyUI's own `output/`. Copy what you are delivering into the
project's files dir so it appears in the dashboard's Files tab, and commit into
the repo only when the build loads it from there (the **`orx-git`** skill).

Report, in one message: what you made, the **absolute path**, dimensions, and the
**exact settings** — model files, steps, cfg, sampler, scheduler, seed, and any
LoRA with its strength and trigger words. Those settings are what make the result
reproducible or extendable; without them a good image is a lucky accident.

## Iterating

Keep the seed **fixed** while you change one thing — prompt wording, a LoRA
strength, a sampler. Changing the seed and the prompt together tells you nothing
about which mattered. Randomize the seed only when you want variety from settings
you have already accepted.

Distilled or "turbo" models want low step counts and cfg near 1; a normal model's
20–30 steps at cfg 7 will look wrong on them. Take the numbers from a working
graph for that model rather than from habit.

## Hand off

Your output is a file, not a playable. Finish by suggesting a **game-designer**
pass to wire the asset in — passing the path, dimensions, tiling and transparency,
and the settings. If what's needed is geometry rather than a texture, hand to a
**blender** agent instead.
