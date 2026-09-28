#!/usr/bin/env node
// The licence texts of every npm package that ends up in the shipped frontend.
//
//   node scripts/third-party-licenses-npm.mjs <output-file>
//
// ⛔ WHAT SHIPS IS WHAT THE BUNDLER KEPT, NOT WHAT package-lock.json NAMES. Of the
// ~200 packages in the lockfile, only two are runtime `dependencies`, and Svelte,
// bits-ui, Lucide and the rest of what Vite compiles into the app are
// devDependencies next to Vite, TypeScript and vitest, which ship nothing. So the
// list comes from the build itself: this runs the same `vite build` as
// `npm run build` (the same vite.config.ts, with `write: false`, so dist/ is not
// touched) and reads the module ids of every output chunk. A package is listed
// when at least one of its files is in the bundle.
//
// Each package's declared licence must be one that deny.toml's `[licenses].allow`
// admits for the Rust side; anything else fails the run, so a new dependency under
// another licence is a decision rather than an accident. The shadcn-svelte
// components vendored under src/lib/components/ui/ are source files of this
// repository, not an npm package, and their MIT notice is in NOTICE.
//
// No dependencies beyond Vite itself, which is already installed, and nothing is
// fetched: every text is read from node_modules/.

import { readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// Keep in step with deny.toml `[licenses].allow` and about.toml `accepted`.
const ALLOWED = new Set([
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Zlib",
  "Unicode-3.0",
  "Unlicense",
  "CC0-1.0",
  "MPL-2.0",
  "0BSD",
]);

const LICENCE_FILE = /^(licen[cs]e|copying|notice)([-._].*)?$/i;

// Packages that reach the bundle through a CSS `@import` in src/lib/theme/app.css
// rather than as a module: @tailwindcss/vite inlines them into the stylesheet, so
// no chunk names them. Checked against app.css on every run, both ways.
const CSS_IMPORTS = ["tailwindcss", "tw-animate-css", "shadcn-svelte"];
const APP_CSS = join(ROOT, "src", "lib", "theme", "app.css");

// Packages whose package.json declares no licence, with the licence their shipped
// licence file states. Each entry is checked against that file's first line, so a
// release that changes the licence fails here instead of being listed under the old
// name.
const CLARIFY = {
  "svelte-toolbelt": { licence: "MIT", file: "LICENSE", firstLine: "MIT License" },
};

/** The package directory a bundled module id belongs to, or null. */
function packageDirOf(id) {
  const clean = id.replace(/^\0/, "").split("?")[0];
  const marker = `${sep}node_modules${sep}`;
  const at = clean.lastIndexOf(marker);
  if (at < 0) return null;
  const rest = clean.slice(at + marker.length).split(sep);
  const depth = rest[0].startsWith("@") ? 2 : 1;
  if (rest.length <= depth) return null;
  return clean.slice(0, at + marker.length) + rest.slice(0, depth).join(sep);
}

/** Whether an SPDX expression is admitted by ALLOWED. */
function admitted(expression) {
  const text = String(expression ?? "").trim().replace(/^\(|\)$/g, "");
  if (!text) return false;
  if (ALLOWED.has(text)) return true;
  if (/\sAND\s/.test(text)) {
    return text.split(/\s+AND\s+/).every((part) => admitted(part));
  }
  return text.split(/\s+OR\s+/).some((part) => ALLOWED.has(part.trim().replace(/^\(|\)$/g, "")));
}

async function bundledPackageDirs() {
  const output = await build({
    root: ROOT,
    configFile: join(ROOT, "vite.config.ts"),
    logLevel: "warn",
    build: { write: false },
  });
  const outputs = (Array.isArray(output) ? output : [output]).flatMap((o) => o.output ?? []);
  const dirs = new Set();
  for (const item of outputs) {
    if (item.type !== "chunk") continue;
    for (const id of item.moduleIds ?? Object.keys(item.modules ?? {})) {
      const dir = packageDirOf(id);
      if (dir) dirs.add(dir);
    }
  }
  const imported = [...readFileSync(APP_CSS, "utf8").matchAll(/^@import\s+"([^"]+)"/gm)]
    .map((m) => m[1])
    .filter((spec) => !spec.startsWith(".") && !spec.startsWith("/"))
    .map((spec) => (spec.startsWith("@") ? spec.split("/").slice(0, 2) : spec.split("/").slice(0, 1)).join("/"));
  const listed = [...CSS_IMPORTS].sort().join(", ");
  const found = [...new Set(imported)].sort().join(", ");
  if (listed !== found) {
    throw new Error(`app.css imports [${found}] but CSS_IMPORTS lists [${listed}]; update CSS_IMPORTS`);
  }
  for (const name of CSS_IMPORTS) dirs.add(join(ROOT, "node_modules", ...name.split("/")));
  return [...dirs];
}

function describe(dir) {
  const manifest = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  let licence =
    typeof manifest.license === "string"
      ? manifest.license
      : (manifest.license?.type ?? (manifest.licenses ?? []).map((l) => l.type).join(" OR "));
  const clarified = CLARIFY[manifest.name];
  if (!licence && clarified) {
    const first = readFileSync(join(dir, clarified.file), "utf8").split("\n")[0].trim();
    if (first !== clarified.firstLine) {
      throw new Error(
        `${manifest.name}: expected ${clarified.file} to start ${JSON.stringify(clarified.firstLine)}, ` +
          `found ${JSON.stringify(first)}; re-check its licence and update CLARIFY`,
      );
    }
    licence = clarified.licence;
  }
  const files = readdirSync(dir)
    .filter((f) => LICENCE_FILE.test(f))
    .sort();
  return {
    name: manifest.name,
    version: manifest.version,
    licence,
    homepage: manifest.homepage ?? manifest.repository?.url ?? manifest.repository ?? "",
    texts: files.map((f) => ({ file: f, text: readFileSync(join(dir, f), "utf8").trim() })),
  };
}

async function main() {
  const out = process.argv[2];
  if (!out) {
    console.error("usage: node scripts/third-party-licenses-npm.mjs <output-file>");
    process.exit(64);
  }
  const packages = (await bundledPackageDirs())
    .filter((dir) => existsSync(join(dir, "package.json")))
    .map(describe)
    // The same package can be reached through two paths; list it once.
    .filter((p, i, all) => all.findIndex((q) => q.name === p.name && q.version === p.version) === i)
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

  const refused = packages.filter((p) => !admitted(p.licence));
  if (refused.length > 0) {
    for (const p of refused) {
      console.error(`${p.name}@${p.version}: licence ${JSON.stringify(p.licence)} is not on the allow-list`);
    }
    process.exit(1);
  }
  if (packages.length === 0) {
    // A build whose chunks named no package would write an empty list and look fine.
    console.error("the build reported no bundled npm package; refusing to write an empty list");
    process.exit(1);
  }

  const lines = [
    "Third-party npm packages in bridgewatch",
    "=======================================",
    "",
    "The bridgewatch frontend bundles code from the npm packages listed below. Each",
    "keeps its own licence; the text each package ships is reproduced after the list.",
    "Generated by scripts/third-party-licenses-npm.mjs from the production build; do",
    "not edit by hand.",
    "",
    ...packages.map((p) => `  - ${p.name} ${p.version} (${p.licence})`),
    "",
  ];
  for (const p of packages) {
    lines.push("-".repeat(78), `${p.name} ${p.version}`, `Licence: ${p.licence}`);
    if (p.homepage) lines.push(`Source: ${p.homepage}`);
    lines.push("");
    if (p.texts.length === 0) {
      lines.push(`(The package ships no licence file; its package.json declares ${p.licence}.)`, "");
    }
    for (const t of p.texts) {
      lines.push(`[${t.file}]`, "", t.text, "");
    }
  }
  writeFileSync(out, lines.join("\n"));
  console.log(`${relative(process.cwd(), out)}: ${packages.length} package(s)`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
