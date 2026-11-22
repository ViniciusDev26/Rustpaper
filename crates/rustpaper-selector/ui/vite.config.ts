/// <reference types="vitest/config" />
// The selector's UI: React + Tailwind, bundled into ui/dist, which
// tauri.conf.json embeds into the rustpaper-selector binary at compile time
// (build.rs runs `npm run build` before that).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // relative asset URLs: Tauri serves dist/ from its own scheme
  base: "./",
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  test: { environment: "node" },
});
