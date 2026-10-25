import { defineConfig } from "vite";

// `npm run mockup`: the Live Watch and plot layout against a simulated gimbal, built with Preact
export default defineConfig({
  root: import.meta.dirname,
  oxc: { jsx: { runtime: "automatic", importSource: "preact" } },
  build: { outDir: "dist", emptyOutDir: true },
});
