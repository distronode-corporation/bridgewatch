import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

// Tauri drives this: `beforeDevCommand` runs `npm run dev` and points the
// webview at `devUrl`, `beforeBuildCommand` runs `npm run build` and the shell
// loads `frontendDist`. The port is therefore fixed and `strictPort` is on — a
// silent fallback to 1421 would leave the webview staring at nothing.
export default defineConfig({
  // Tailwind v4 compiles the shadcn-svelte (Rhea) utilities at build time; the
  // output is one static stylesheet in the bundle, nothing is fetched at runtime.
  plugins: [tailwindcss(), svelte()],
  resolve: {
    // `$lib` is SvelteKit's alias, which the vendored shadcn-svelte components
    // import through. Plain Vite has no such alias, so it is declared here, in
    // vitest.config.ts and in tsconfig.json `paths` (all three must agree).
    alias: { $lib: decodeURIComponent(new URL("./src/lib", import.meta.url).pathname) },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // The Rust side has its own watcher; letting vite walk target/ costs a
      // few thousand file handles for no benefit.
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  build: {
    // Matches `macOS.minimumSystemVersion` 10.15 / WebKitGTK on Ubuntu 22.04.
    target: "es2022",
    sourcemap: false,
    emptyOutDir: true,
  },
});
