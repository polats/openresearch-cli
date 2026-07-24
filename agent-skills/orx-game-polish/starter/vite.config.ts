import { defineConfig } from 'vite';

// base:'./' is REQUIRED — the play build is served under /play/<expId>/, so every
// asset reference must be relative or it 404s. Do not change this.
export default defineConfig({
  base: './',
  build: {
    target: 'es2020',
    // one self-contained bundle; keep assets inlined where cheap
    assetsInlineLimit: 4096,
  },
});
