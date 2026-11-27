// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

/**
 * The one node the coverage report leaves out: the `?? ''` the Svelte compiler
 * writes around every `{expression}` in a template, so that `null` renders as
 * nothing (`{step.label}` compiles to `` `${step.label ?? ''}` ``). Its right
 * side runs only when a typed value is null, which no caller can produce, so it
 * is a branch in the compiled output and not in the source.
 *
 * The match is exact: a `??` whose right side is the empty string written with
 * SINGLE quotes, which is how the compiler prints it. Hand-written code in src/
 * uses double quotes, and src/lib/svelte-generated.test.ts walks every script
 * in src/ to prove this matches none of it, and compiles components to prove it
 * matches what the compiler writes.
 */

/** The fields of an ESTree node the match reads. */
interface Node {
  type: string;
  operator?: unknown;
  right?: { type?: unknown; value?: unknown; raw?: unknown };
}

export function isSvelteTemplateFallback(node: object, type: "function" | "statement" | "branch"): boolean {
  const n = node as Node;
  return (
    type === "branch" &&
    n.type === "LogicalExpression" &&
    n.operator === "??" &&
    n.right?.type === "Literal" &&
    n.right.value === "" &&
    n.right.raw === "''"
  );
}
