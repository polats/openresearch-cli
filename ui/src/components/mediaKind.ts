// One media-type map, shared by the files browser and the file viewer.
//
// Both need the same question answered — "can this be shown, and how?" — and the
// two answers must not drift: a file the browser previews as video and the viewer
// dumps into a <pre> is the kind of split that only shows up as garbage on screen.
//
// Generated art arrives here as PNG frames, MP4 turnarounds and GLB meshes, so the
// list is deliberately wider than the text-first tooling that preceded it.

export const IMAGE_RE = /\.(png|jpe?g|gif|webp|svg|avif|bmp)$/i;
export const VIDEO_RE = /\.(mp4|webm|mov|m4v)$/i;
export const AUDIO_RE = /\.(mp3|ogg|wav|flac|m4a)$/i;
/** Meshes the 3D viewer can orbit. Anything else 3D lists and downloads only. */
export const MODEL_RE = /\.(glb|gltf)$/i;
/** Gaussian splats, which orbit in their own viewer — a splat has no scene graph
 *  or materials, so it cannot go through the mesh loader. `.ply` is ambiguous in
 *  general (it is also a plain point/mesh format) but every .ply this pipeline
 *  produces is a splat, and the splat loader reports a clear error on one that
 *  isn't. */
export const SPLAT_RE = /\.(ply|spz|splat|ksplat)$/i;
export const PDF_RE = /\.pdf$/i;
export const MD_RE = /\.(md|mdx|markdown)$/i;

export type MediaKind = "image" | "video" | "audio" | "model" | "splat" | "pdf";

/** The media kind a filename renders as, or null when it is text/unknown. */
export function mediaKind(name: string): MediaKind | null {
  if (IMAGE_RE.test(name)) return "image";
  if (VIDEO_RE.test(name)) return "video";
  if (AUDIO_RE.test(name)) return "audio";
  if (MODEL_RE.test(name)) return "model";
  if (SPLAT_RE.test(name)) return "splat";
  if (PDF_RE.test(name)) return "pdf";
  return null;
}

/** True when a filename is media, i.e. must never be fetched as text. */
export function isMedia(name: string): boolean {
  return mediaKind(name) !== null;
}
