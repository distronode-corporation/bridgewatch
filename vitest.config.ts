import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

import { ignoreSvelteGenerated } from "./vitest.svelte-generated";

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
    // Fails a test that makes Svelte warn `derived_inert`; see the file.
    setupFiles: ["./src/vitest.setup.ts"],
    // `npx vitest run --coverage` (CI's `frontend` job) fails under these floors.
    // Every file under src/ is measured, loaded by a test or not, the vendored
    // shadcn-svelte components included; there is no exclusion list, only the
    // compiler-generated nodes below. The floors are the measured level
    // rounded down, and only go up: a change that raises coverage raises them in
    // the same pull request.
    coverage: {
      provider: "v8",
      include: ["src/**"],
      reporter: ["text-summary"],
      // The nodes left out of the report, the way Vitest already leaves out
      // Vite's own generated SSR and CommonJS-interop code: three shapes the
      // Svelte compiler writes (the `?? ''` around a template expression, the
      // dev-mode `$$ownership_validator.binding` check, and the `x !== (x = v)`
      // attribute cache). No test could reach their other side without misusing
      // a component. Each match is exact, and src/lib/svelte-generated.test.ts
      // proves none of them matches a hand-written node (vitest.svelte-generated.ts).
      ignoreNode: ignoreSvelteGenerated,
      thresholds: { statements: 99, branches: 99, functions: 99, lines: 99 },
    },
  },
});
