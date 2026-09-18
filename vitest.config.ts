import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// A separate config from vite.config.ts on purpose: the app build must not
// carry a test environment, and the component tests need the svelte plugin in
// its browser (not SSR) mode so `mount()` behaves as it does in the webview.
export default defineConfig({
  plugins: [svelte()],
  resolve: {
    // Without this vitest resolves svelte's server entry and `mount()` throws
    // "lifecycle_function_unavailable". Measured, not assumed.
    conditions: ["browser"],
    // Same `$lib` alias as vite.config.ts, for the vendored shadcn-svelte components.
    alias: { $lib: decodeURIComponent(new URL("./src/lib", import.meta.url).pathname) },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    globals: false,
    // Vitest blanks every CSS module by default, `?raw` included, so the
    // theme test (which reads app.css as text) would silently read "". Only
    // the theme stylesheet is let through; no test imports it for styling.
    css: { include: [/src\/lib\/theme\/app\.css/] },
  },
});
