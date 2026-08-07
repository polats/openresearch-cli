---
name: orx-blender
description: "Author 3D assets in the user's running Blender through the Blender MCP tools: inspect the scene before editing, drive bpy in small verified steps, render and read the image back to check the result, and export glTF to the files dir. Use when making or fixing a 3D asset, inspecting a .blend, or when a Blender tool reports it cannot reach Blender."
---

You author 3D assets in **the Blender window the user has open**, through the
Blender MCP server crux manages. Everything here assumes that live session: your
edits land in their workspace, and their unsaved work is at stake.

The MCP server sends its own instructions on `bpy` itself — operators versus the
data API, mode and selection pitfalls, `execute_blender_code` as a last resort,
and where the bundled API/manual RST docs live. **This module does not repeat
them.** Read them; they are correct and current. What follows is what the server
cannot know: how this fits crux, and how not to wreck someone's file.

## Before anything: is Blender there?

Every scene tool needs Blender running with the MCP add-on enabled and its
server started. When one reports it cannot connect:

1. Say so plainly and **stop**. Do not retry in a loop.
2. Point the user at **Settings → Generative AI → Blender** in the dashboard —
   it reports the server, the add-on socket, and what to fix.
3. Do not work around it. There is no substitute path: writing `.blend` files
   blind or shelling out to `blender --background` is not this tool.

The add-on auto-starts with Blender by default, so "closed and reopened" fixes
itself — the tools reconnect on the next call with nothing to restart.

## Inspect before you touch

The scene is not a default cube. Read it first:

- **`get_objects_summary`** — the whole scene: collections, objects, types, what
  is selected, what is active, the current mode. Your first call, always.
- **`get_object_detail_summary`** — one object in depth: transforms, modifiers,
  materials, mesh statistics.
- **`get_blendfile_summary_*`** — datablocks, path info, linked libraries, and
  **missing files**. The missing-files summary is the fastest way to explain a
  scene that renders pink or empty.
- **`get_screenshot_of_window_as_image`** / `..._as_json`,
  **`get_screenshot_of_area_as_image`** — what the user is actually looking at.
  Useful when their description and the data disagree.
- **`jump_to_tab_by_name`** / `jump_to_tab_by_space_type`,
  **`jump_to_view3d_object_by_name`** / `..._object_data_by_name` — move *their*
  viewport so you and the user are looking at the same thing. Courteous during a
  conversation, and the fastest way to make a question concrete.

A wrong assumption here costs the user's work, and one summary call prevents it.

## Build in small verified steps

Operators mutate selection, mode, and the active object **as a side effect**. A
five-step sequence written in one go will be operating on the wrong object by
step three. So: one meaningful change, re-inspect, next change. It feels slower
and is much faster than debugging a scene you no longer understand.

Keep the user's conventions — object and material names, collection structure,
units, up-axis. If you must rename something, say why.

## Verify by looking — do not skip this

`get_objects_summary` will happily report a perfectly correct mesh that is
inside-out, forty metres tall, or unreadable at thumbnail size. Numbers do not
show you a silhouette.

- **`render_thumbnail_to_path`** — small, fast; the right check for "does this
  read at the size it will actually be seen".
- **`render_viewport_to_path`** — what the viewport shows, including the current
  shading mode. Faster than a full render for a shape check.

Both write a file. **Open that file with your image-reading tool and look at it.**
If your harness cannot read images, say so and hand the path to the user rather
than asserting a visual result you have not seen — claiming an asset is correct
sight-unseen is the one failure this module exists to prevent.

Check, at minimum: silhouette reads at target size; scale plausible against a
1.8 m reference; normals outward; origin somewhere useful (usually base-centre
for props); no stray objects included in the export.

## Never clobber the user's file

- **No `bpy.ops.wm.save_mainfile`, no `wm.open_mainfile`, no `wm.read_homefile`.**
  Their unsaved work dies with any of them.
- Additive edits — a new object, a new collection — need no permission.
- Anything that **deletes, replaces, or re-topologises existing geometry** needs
  the user's say-so. Ask with concrete options and end your turn.
- Export to a **new file**; never overwrite the `.blend`.

## Deliver

Export **glTF binary (`.glb`)**: one self-contained file including materials, and
the format the house Three.js stack loads natively.

```py
bpy.ops.export_scene.gltf(
    filepath="<files>/tree.glb",
    export_format="GLB",
    use_selection=True,   # export the asset, not the whole scene
)
```

Where it goes:

- **`{files}` (the project's files dir)** for anything the user should see in the
  dashboard's Files tab — deliverables, reference renders. This is the default.
- **Into the repo** only when the game loads it directly from there; then commit
  it on the right branch (the **`orx-git`** skill) and keep it small.

Then say, in one message: what you made, the **absolute path**, the **scale and
up-axis**, the **origin point**, poly count, and the material story. That handful
of facts is what the game designer needs and cannot recover from the file alone.

Budget for a mobile-first portrait build: keep props in the low thousands of
triangles and textures at 512–1024 px unless the user asks otherwise. Say the
numbers when you deliver so the budget stays visible.

## Procedural is still the default

The house style for playables is procedural, zero-binary assets. A baked asset is
the exception, justified when the shape cannot be authored in code — organic
silhouettes, sculpted characters, forms whose appeal is their irregularity.

State the reason when you deliver one, and when a procedural version would do,
say that instead. "Easier in Blender" is not a reason.

## Hand off

Your output is a file, not a playable. Finish by suggesting a **game-designer**
pass to load the asset in, sized and placed — passing along the path, scale,
up-axis, and origin. Don't wire it into the build yourself.
