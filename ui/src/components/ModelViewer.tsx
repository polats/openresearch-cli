// Orbitable preview for a 3D artifact (.glb / .gltf), after tris-bot's
// web/components/ModelViewer.tsx.
//
// That one builds an iframe with an importmap because it loads models proxied from
// HuggingFace; crux serves its own files, so this is a plain React component and the
// indirection buys nothing. What is worth keeping is its scene recipe: hemisphere +
// key light so an untextured mesh still reads, and auto-fit so a model of any scale
// lands in view (a UniRig output and a Hunyuan3D mesh differ by orders of magnitude).
//
// The pipeline's GLBs frequently CARRY ANIMATION — a rigged walk cycle is the whole
// point of the artifact — so clips are detected and played, which a static viewer
// would silently hide.
//
// One WebGL context per mount, disposed on unmount: browsers cap live contexts at
// around 16, so a viewer per row in a file list would start evicting them. Mount one
// for the selected file.

import { Pause, Play, RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

/** Frame the object: centre it on the origin and scale it into a 1.6-unit box. */
function fitToView(obj: THREE.Object3D, controls: OrbitControls): void {
  const box = new THREE.Box3().setFromObject(obj);
  const size = box.getSize(new THREE.Vector3());
  const center = box.getCenter(new THREE.Vector3());
  obj.position.sub(center);
  const max = Math.max(size.x, size.y, size.z) || 1;
  obj.scale.multiplyScalar(1.6 / max);
  controls.target.set(0, 0, 0);
  controls.update();
}

export function ModelViewer({ src, name }: { src: string; name?: string }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const mixerRef = useRef<THREE.AnimationMixer | null>(null);
  const resetRef = useRef<(() => void) | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [clips, setClips] = useState<string[]>([]);
  const [playing, setPlaying] = useState(true);
  const playingRef = useRef(true);

  useEffect(() => {
    playingRef.current = playing;
  }, [playing]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let raf = 0;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x0b0e18);
    const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 1000);
    camera.position.set(0, 0.6, 2.4);
    const renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    host.appendChild(renderer.domElement);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    // A mesh with no material still has to be legible, so light it from two sides
    // rather than relying on whatever the exporter wrote.
    scene.add(new THREE.HemisphereLight(0xffffff, 0x333333, 2.4));
    const key = new THREE.DirectionalLight(0xffffff, 1.8);
    key.position.set(3, 4, 5);
    scene.add(key);

    const home = camera.position.clone();
    resetRef.current = () => {
      camera.position.copy(home);
      controls.target.set(0, 0, 0);
      controls.update();
    };

    // The canvas has no intrinsic size; follow the container instead of the window
    // so the viewer works in a pane as well as full width.
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

    new GLTFLoader().load(
      src,
      (gltf) => {
        if (disposed) return;
        scene.add(gltf.scene);
        fitToView(gltf.scene, controls);
        if (gltf.animations.length > 0) {
          const mixer = new THREE.AnimationMixer(gltf.scene);
          for (const clip of gltf.animations) mixer.clipAction(clip).play();
          mixerRef.current = mixer;
          setClips(gltf.animations.map((c, i) => c.name || `clip ${i + 1}`));
        }
        setLoading(false);
      },
      undefined,
      (e) => {
        if (disposed) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );

    const clock = new THREE.Clock();
    const tick = () => {
      raf = requestAnimationFrame(tick);
      const dt = clock.getDelta();
      if (playingRef.current) mixerRef.current?.update(dt);
      controls.update();
      renderer.render(scene, camera);
    };
    raf = requestAnimationFrame(tick);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      ro.disconnect();
      controls.dispose();
      mixerRef.current?.stopAllAction();
      mixerRef.current = null;
      // Release GPU memory explicitly: a dropped canvas alone leaves the context
      // and its buffers alive until the browser decides to reclaim them.
      scene.traverse((o) => {
        const mesh = o as THREE.Mesh;
        mesh.geometry?.dispose?.();
        const mat = mesh.material;
        if (Array.isArray(mat)) mat.forEach((m) => m.dispose());
        else mat?.dispose?.();
      });
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
            ? `Could not load ${name ?? "model"}: ${error}`
            : loading
              ? "Loading…"
              : "drag to orbit · wheel to zoom"}
        </span>
        {clips.length > 0 && (
          <>
            <span className="model-view-clips">
              {clips.length === 1 ? clips[0] : `${clips.length} clips`}
            </span>
            <button
              className="icon-btn"
              data-tip={playing ? "Pause animation" : "Play animation"}
              aria-label={playing ? "Pause animation" : "Play animation"}
              onClick={() => setPlaying((p) => !p)}
            >
              {playing ? <Pause size={13} /> : <Play size={13} />}
            </button>
          </>
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
