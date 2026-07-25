// Toon look: a banded gradient-ramp MeshToonMaterial + an inverted-hull outline.
// This is the cheap, consistent "crafted" 3D style — flat cel shading with a
// clean ink edge, no textures. Light the scene with one hemi + one directional.

import * as THREE from 'three';

let sharedRamp: THREE.DataTexture | null = null;

/** A 4-step gradient ramp shared by every toon material (hard cel bands). */
function ramp(): THREE.DataTexture {
  if (sharedRamp) return sharedRamp;
  const steps = new Uint8Array([70, 130, 200, 255]);
  const tex = new THREE.DataTexture(steps, steps.length, 1, THREE.RedFormat);
  tex.minFilter = THREE.NearestFilter;
  tex.magFilter = THREE.NearestFilter;
  tex.needsUpdate = true;
  sharedRamp = tex;
  return tex;
}

export function toonMaterial(color: THREE.ColorRepresentation): THREE.MeshToonMaterial {
  return new THREE.MeshToonMaterial({ color, gradientMap: ramp() });
}

/**
 * Add an inverted-hull outline: a back-face-culled copy of the mesh, slightly
 * fattened along normals, in the ink colour. Parent it to the mesh so it
 * follows transforms. thickness is in local units (~0.03 for hand-size props).
 */
export function addOutline(mesh: THREE.Mesh, thickness = 0.03, ink: THREE.ColorRepresentation = 0x171019) {
  const mat = new THREE.MeshBasicMaterial({ color: ink, side: THREE.BackSide });
  const outline = new THREE.Mesh(mesh.geometry, mat);
  outline.scale.multiplyScalar(1 + thickness);
  mesh.add(outline);
  return outline;
}

/** One-call standard toon lighting (hemi fill + key directional). */
export function toonLights(scene: THREE.Scene) {
  const hemi = new THREE.HemisphereLight(0xffffff, 0x404050, 0.9);
  const key = new THREE.DirectionalLight(0xfff2d8, 1.1);
  key.position.set(4, 8, 5);
  scene.add(hemi, key);
  return { hemi, key };
}
