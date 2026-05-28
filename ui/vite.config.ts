/// <reference types="vitest" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Vite config tuned for Tauri 2:
// - fixed dev port (matches `devUrl` in tauri.conf.json)
// - HMR over a stable port so Tauri can reach it
// - source maps for the dev build
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    hmr: {
      protocol: "ws",
      host: "127.0.0.1",
      port: 1421,
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    sourcemap: true,
    minify: "esbuild",
  },
  // Vitest configuration. Svelte 5 ships separate browser /
  // server entries; without an explicit `browser` condition
  // here the test runner picks the SSR build and `mount()`
  // throws `lifecycle_function_unavailable` (the SSR build only
  // exposes `render()`). The condition is scoped to the test
  // mode so the production / dev builds keep their default
  // resolution.
  test: {
    environment: "jsdom",
    globals: false,
    include: ["src/**/*.{test,spec}.ts"],
  },
  resolve: {
    // Svelte 5 ships separate browser / server entries (see
    // `package.json` exports map). Without an explicit `browser`
    // condition the production build resolves the SSR entry,
    // which makes `mount()` throw `lifecycle_function_unavailable`
    // at runtime and leaves the WebView with an empty body.
    // Pin the browser condition for every Vite mode (dev, build,
    // test) so the resolution is identical across environments.
    conditions: ["browser"],
  },
});
