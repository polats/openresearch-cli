// Orbitable preview for a 3D gaussian splat (.ply / .spz / .splat / .ksplat),
// sibling of ModelViewer.tsx.
//
// Split from that one rather than folded into it because a splat is not a mesh:
// there is no GLTF scene graph, no material to light, and no animation clips. What
// IS shared is the container contract — one WebGL context per mount, disposed on
// unmount, sized from the host element rather than the window — so the two read
// the same on purpose.
//
// Spark renders splats through its own SparkRenderer, which needs the THREE
// renderer handed to it so it can do sorting work outside the normal render loop.
// Two deliberate departures from ModelViewer's recipe:
//
//   * antialias: false. Spark's own guidance — MSAA does nothing for splats and
//     costs a lot of fill rate.
//   * no lights. Splats carry their own baked colour; a HemisphereLight would be
//     ignored, so adding one would only imply it mattered.
//
// TripoSplat's .ply comes out Y-down (the 3DGS convention), so it is flipped 180°
// about X on load. Without that the subject hangs upside down, which looks like a
// broken file rather than a wrong axis.

import { RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { SparkRenderer, SplatMesh } from "@sparkjsdev/spark";

export function SplatViewer({ src, name }: { src: string; name?: string }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const resetRef = useRef<(() => void) | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [count, setCount] = useState<number | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let raf = 0;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x0b0e18);
    const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 1000);
    camera.position.set(0, 0.4, 2.4);
    const renderer = new THREE.WebGLRenderer({ antialias: false });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    host.appendChild(renderer.domElement);

    const spark = new SparkRenderer({ renderer });
    scene.add(spark);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;

    const home = camera.position.clone();
    resetRef.current = () => {
      camera.position.copy(home);
      controls.target.set(0, 0, 0);
      controls.update();
    };

    const resize = () => {
      const w = host.clientWidth || 1;
      const h = host.clientHeight || 1;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(host);

    let mesh: SplatMesh | null = null;
    try {
      mesh = new SplatMesh({
        url: src,
        onLoad: (loaded) => {
          if (disposed) return;
          // 3DGS files are Y-down; three.js is Y-up.
          loaded.rotation.x = Math.PI;
          // Centre and scale into view. A TripoSplat output is already about one
          // unit across, but a splat from anywhere else can be metres or
          // millimetres, and an off-screen splat is indistinguishable from a
          // failed load.
          try {
            const box = loaded.getBoundingBox(true);
            if (!box.isEmpty()) {
              const size = box.getSize(new THREE.Vector3());
              const centre = box.getCenter(new THREE.Vector3());
              const max = Math.max(size.x, size.y, size.z) || 1;
              const s = 1.6 / max;
              loaded.scale.setScalar(s);
              // Undo the centre offset in the parent frame, so the flip above
              // does not also move the subject off the pivot.
              loaded.position.set(-centre.x * s, centre.y * s, -centre.z * s);
            }
          } catch {
            // Bounds are a nicety; a splat that cannot report them still draws.
          }
          const n = (loaded as unknown as { numSplats?: number }).numSplats;
          if (typeof n === "number" && n > 0) setCount(n);
          setLoading(false);
        },
      });
      scene.add(mesh);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setLoading(false);
    }

    const tick = () => {
      raf = requestAnimationFrame(tick);
      controls.update();
      renderer.render(scene, camera);
    };
    raf = requestAnimationFrame(tick);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      ro.disconnect();
      controls.dispose();
      // Spark allocates its own textures and worker buffers, so the scene walk
      // ModelViewer does is not enough on its own.
      mesh?.dispose?.();
      spark.dispose?.();
      renderer.dispose();
      renderer.forceContextLoss();
      renderer.domElement.remove();
    };
  }, [src]);

  return (
    <div className="model-view">
      <div className="model-view-canvas" ref={hostRef} />
      <div className="model-view-bar">
        <span className="model-view-hint">
          {error
            ? `Could not load ${name ?? "splat"}: ${error}`
            : loading
              ? "Loading splat…"
              : "drag to orbit · wheel to zoom"}
        </span>
        {count != null && (
          <span className="model-view-clips">{count.toLocaleString()} gaussians</span>
        )}
        <button
          className="icon-btn"
          data-tip="Reset view"
          aria-label="Reset view"
          onClick={() => resetRef.current?.()}
        >
          <RotateCcw size={13} />
        </button>
      </div>
    </div>
  );
}
