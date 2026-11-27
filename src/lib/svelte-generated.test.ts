// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { compile, parse } from "svelte/compiler";
import { describe, expect, it } from "vitest";

/**
 * vitest.config.ts leaves exactly one node out of the coverage report: the
 * `?? ''` the Svelte compiler writes around a template `{expression}` (see
 * vitest.svelte-generated.ts). These tests hold that match to the compiler's
 * output and nothing else: it matches no node anywhere in the hand-written
 * source, markup included, and it matches every fallback the compiler writes.
 */

// The matcher lives beside vitest.config.ts, in the node tsconfig project; a
// computed specifier keeps this project's tsc from type-checking it a second time.
const matcherPath = "../../vitest.svelte-generated.ts";
const { isSvelteTemplateFallback } = (await import(/* @vite-ignore */ matcherPath)) as {
  isSvelteTemplateFallback: (node: object, type: "branch") => boolean;
};

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

/** The AST of plain JavaScript, through the same parser (acorn is not a direct dependency). */
function parseJs(code: string): unknown {
  return parse(`<script>${code}</script>`, { modern: true });
}

/** The AST of a source file, markup and both script blocks for a component. */
function astOf(file: string, source: string): unknown {
  // A .ts file goes through the same TypeScript-aware parser as a component's script.
  return parse(file.endsWith(".svelte") ? source : `<script lang="ts">${source}</script>`, { modern: true });
}

describe("the coverage report's one ignored node", () => {
  const files = Object.keys(tree);

  it("walks the real tree", () => {
    expect(files.some((f) => f.endsWith("components/wizard/Wizard.svelte"))).toBe(true);
    expect(files.some((f) => f.endsWith("lib/ipc.ts"))).toBe(true);
    // A walk that reaches the nodes a hand-written fallback would be.
    const ast = astOf("x.svelte", `<script lang="ts">const a = b ?? "";</script>{c ?? ""}`);
    expect(nodes(ast).filter((n) => (n as { type: string }).type === "LogicalExpression")).toHaveLength(2);
  });

  it("matches no node in the hand-written source, markup and scripts alike", () => {
    const matched = files.flatMap((file) =>
      nodes(astOf(file, tree[file]))
        .filter((node) => isSvelteTemplateFallback(node, "branch"))
        .map(() => file),
    );
    expect(matched).toEqual([]);
  });

  it("would match a hand-written fallback in single quotes, which is why the test above walks the source", () => {
    const ast = astOf("x.svelte", `<script lang="ts">const a = b ?? '';</script>`);
    expect(nodes(ast).filter((node) => isSvelteTemplateFallback(node, "branch"))).toHaveLength(1);
  });

  it("matches every fallback the compiler writes around a template expression", () => {
    const { js } = compile(
      `<script lang="ts">let { label, steps }: { label: string; steps: string[] } = $props();</script>
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
});
