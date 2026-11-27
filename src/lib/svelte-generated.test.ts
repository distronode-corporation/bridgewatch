// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { compile, parse } from "svelte/compiler";
import { describe, expect, it } from "vitest";

/**
 * vitest.config.ts leaves three shapes of Svelte compiler output out of the
 * coverage report (see vitest.svelte-generated.ts): the `?? ''` around a
 * template `{expression}`, the dev-mode `$$ownership_validator.binding` check,
 * and the `x !== (x = value)` attribute cache. These tests hold each match to
 * the compiler's output and nothing else: none matches a node anywhere in the
 * hand-written source, markup included, and each matches what the compiler writes.
 */

type Kind = "function" | "statement" | "branch";

// The matchers live beside vitest.config.ts, in the node tsconfig project; a
// computed specifier keeps this project's tsc from type-checking them a second time.
const matcherPath = "../../vitest.svelte-generated.ts";
const { isSvelteTemplateFallback, isSvelteOwnershipBinding, isSvelteAttributeCache, ignoreSvelteGenerated } =
  (await import(/* @vite-ignore */ matcherPath)) as {
    isSvelteTemplateFallback: (node: object, type: Kind) => boolean;
    isSvelteOwnershipBinding: (node: object) => boolean;
    isSvelteAttributeCache: (node: object, type: Kind) => boolean;
    ignoreSvelteGenerated: (node: object, type: Kind) => boolean | "ignore-this-and-nested-nodes";
  };

/** Whether any of the three matches the node, as the coverage report asks it, for any kind. */
const anyMatch = (node: object) => (["function", "statement", "branch"] as const).some((kind) => ignoreSvelteGenerated(node, kind));

// Vite reads the tree at transform time (no node:fs here; see ipc.test.ts).
const tree = import.meta.glob(["/src/**/*.ts", "/src/**/*.svelte", "!/src/**/__fixtures__/**"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Every object in an AST that has a `type`, the tree's own nodes and nothing else. */
function nodes(root: unknown): object[] {
  const out: object[] = [];
  const seen = new Set<unknown>();
  const visit = (value: unknown) => {
    if (value === null || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    if (Array.isArray(value)) {
      value.forEach(visit);
      return;
    }
    if (typeof (value as { type?: unknown }).type === "string") out.push(value);
    for (const [key, child] of Object.entries(value)) {
      // Parent links and source positions, not children.
      if (key !== "parent" && key !== "loc") visit(child);
    }
  };
  visit(root);
  return out;
}

// ⚠ Every closing tag below is written `<\/script>`: the walk parses files as script blocks, and Svelte ends a
// script at the first literal closing tag, even inside a string. Vite leaves this file out of its own glob.

/** The AST of plain JavaScript, through the same parser (acorn is not a direct dependency). */
function parseJs(code: string): unknown {
  return parse(`<script>${code}<\/script>`, { modern: true });
}

/** The AST of a source file, markup and both script blocks for a component. */
function astOf(file: string, source: string): unknown {
  // A .ts file goes through the same TypeScript-aware parser as a component's script.
  return parse(file.endsWith(".svelte") ? source : `<script lang="ts">${source}<\/script>`, { modern: true });
}

describe("the coverage report's ignored compiler output", () => {
  const files = Object.keys(tree);

  it("walks the real tree", () => {
    expect(files.some((f) => f.endsWith("components/wizard/Wizard.svelte"))).toBe(true);
    expect(files.some((f) => f.endsWith("lib/ipc.ts"))).toBe(true);
    // Test files are walked too, all but this one, which Vite's glob leaves out of its own results.
    expect(files.some((f) => f.endsWith("lib/ipc.test.ts"))).toBe(true);
    expect(files.some((f) => f.endsWith("lib/svelte-generated.test.ts"))).toBe(false);
    // A walk that reaches the nodes a hand-written fallback would be.
    const ast = astOf("x.svelte", `<script lang="ts">const a = b ?? "";<\/script>{c ?? ""}`);
    expect(nodes(ast).filter((n) => (n as { type: string }).type === "LogicalExpression")).toHaveLength(2);
  });

  it("matches no hand-written node with any of the three, markup and scripts alike", () => {
    const matched = files.flatMap((file) =>
      nodes(astOf(file, tree[file]))
        .filter(anyMatch)
        .map(() => file),
    );
    expect(matched).toEqual([]);
  });

  it("would match a hand-written fallback in single quotes, which is why the test above walks the source", () => {
    const ast = astOf("x.svelte", `<script lang="ts">const a = b ?? '';<\/script>`);
    expect(nodes(ast).filter((node) => isSvelteTemplateFallback(node, "branch"))).toHaveLength(1);
  });

  it("matches every fallback the compiler writes around a template expression", () => {
    const { js } = compile(
      `<script lang="ts">let { label, steps }: { label: string; steps: string[] } = $props();<\/script>
       <span>{label}</span><p>Step 1 of {steps.length}: {steps[0]}</p>`,
      { filename: "Probe.svelte", generate: "client" },
    );
    const written = js.code.split("?? ''").length - 1;
    expect(written).toBeGreaterThanOrEqual(2);
    const ast = parseJs(js.code);
    expect(nodes(ast).filter((node) => isSvelteTemplateFallback(node, "branch"))).toHaveLength(written);
  });

  it("only ever names branches, so no statement or function is left out", () => {
    const ast = parseJs("x ?? ''");
    const [fallback] = nodes(ast).filter((node) => isSvelteTemplateFallback(node, "branch"));
    expect(fallback).toBeDefined();
    const asAny = isSvelteTemplateFallback as (node: object, type: string) => boolean;
    expect(asAny(fallback, "statement")).toBe(false);
    expect(asAny(fallback, "function")).toBe(false);
    // And a double-quoted or non-empty fallback is code, not the compiler's.
    for (const code of [`x ?? ""`, `x ?? 'a'`, `x || ''`]) {
      const [node] = nodes(parseJs(code)).filter(
        (n) => (n as { type: string }).type === "LogicalExpression",
      );
      expect(isSvelteTemplateFallback(node, "branch"), code).toBe(false);
    }
  });

  it("matches the ownership check the compiler writes for a binding into a child, and only that", () => {
    const { js } = compile(
      `<script lang="ts">import Child from "./Child.svelte"; let { draft = $bindable() }: { draft: { x: string } } = $props();<\/script>
       <Child bind:value={draft.x} />`,
      { filename: "Probe.svelte", generate: "client", dev: true },
    );
    const written = js.code.split("$$ownership_validator.binding(").length - 1;
    expect(written).toBe(1);
    const matched = nodes(parseJs(js.code)).filter((node) => isSvelteOwnershipBinding(node));
    expect(matched).toHaveLength(written);
    // The mutation call wraps the real assignment, so it stays measured.
    expect(js.code).toContain("$$ownership_validator.mutation(");
    const mutation = nodes(parseJs("$$ownership_validator.mutation('draft', [], draft(x = 1, true), 1, 1);"));
    expect(mutation.filter((node) => isSvelteOwnershipBinding(node))).toEqual([]);
    expect(ignoreSvelteGenerated(matched[0], "statement")).toBe("ignore-this-and-nested-nodes");
  });

  it("matches the attribute cache the compiler writes for an option's value, as a branch only", () => {
    const { js } = compile(
      `<script lang="ts">let { values }: { values: string[] } = $props();<\/script>
       <select>{#each values as v (v)}<option value={v}>{v}</option>{/each}</select>`,
      { filename: "Probe.svelte", generate: "client" },
    );
    const written = (js.code.match(/if \((\w+) !== \(\1 = /g) ?? []).length;
    expect(written).toBeGreaterThanOrEqual(1);
    const caches = nodes(parseJs(js.code)).filter((node) => isSvelteAttributeCache(node, "branch"));
    expect(caches).toHaveLength(written);
    expect(isSvelteAttributeCache(caches[0], "statement")).toBe(false);
    // Hand-written look-alikes: another variable, or no assignment.
    for (const code of ["if (a !== (b = c)) d();", "if (a !== b) d();", "if (a === (a = c)) d();"]) {
      expect(nodes(parseJs(code)).filter((node) => isSvelteAttributeCache(node, "branch")), code).toEqual([]);
    }
  });
});
