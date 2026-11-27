// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

/**
 * The nodes the coverage report leaves out: three shapes the Svelte compiler
 * writes into a component's output, none of which is in the source and none of
 * which a test could reach without misusing a component.
 *
 * 1. The `?? ''` around every template `{expression}`, so that `null` renders
 *    as nothing (`{step.label}` compiles to `` `${step.label ?? ''}` ``). Its
 *    right side runs only when a typed value is null.
 * 2. In dev mode, the ownership check `$$ownership_validator.binding(...)`
 *    written for `bind:value={draft.x}` on a child component. Its callback runs
 *    only when a parent hands a child `$state` without binding it, which Svelte
 *    warns about. `$$` names are the compiler's own.
 * 3. The attribute cache `if (x !== (x = value))` written for `<option value>`
 *    and a select's `value`. Its else side runs only when the effect re-runs
 *    with the same value, which Svelte never does.
 *
 * Each match is exact. src/lib/svelte-generated.test.ts walks every script and
 * template in src/ to prove none matches a hand-written node, and compiles
 * components to prove each matches what the compiler writes.
 */

type Kind = "function" | "statement" | "branch";

/** The fields of ESTree nodes the matches read. */
interface Node {
  type: string;
  operator?: unknown;
  name?: unknown;
  left?: Node;
  right?: Node & { value?: unknown; raw?: unknown };
  test?: Node;
  expression?: Node;
  callee?: Node;
  object?: Node;
  property?: Node;
}

/** Shape 1: the `?? ''` template fallback. */
export function isSvelteTemplateFallback(node: object, type: Kind): boolean {
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

/** Shape 2: the dev-mode `$$ownership_validator.binding(...)` statement, with everything in it. */
export function isSvelteOwnershipBinding(node: object): boolean {
  const n = node as Node;
  const callee = n.type === "ExpressionStatement" && n.expression?.type === "CallExpression" ? n.expression.callee : undefined;
  return (
    callee?.type === "MemberExpression" &&
    callee.object?.type === "Identifier" &&
    callee.object.name === "$$ownership_validator" &&
    callee.property?.type === "Identifier" &&
    callee.property.name === "binding"
  );
}

/** Shape 3: the `if (x !== (x = value))` attribute cache, as a branch only (its body is still measured). */
export function isSvelteAttributeCache(node: object, type: Kind): boolean {
  const n = node as Node;
  const test = n.type === "IfStatement" ? n.test : undefined;
  return (
    type === "branch" &&
    test?.type === "BinaryExpression" &&
    test.operator === "!==" &&
    test.left?.type === "Identifier" &&
    test.right?.type === "AssignmentExpression" &&
    test.right.operator === "=" &&
    test.right.left?.type === "Identifier" &&
    test.right.left.name === test.left.name
  );
}

/** vitest's `coverage.ignoreNode`: the three shapes above, and nothing else. */
export function ignoreSvelteGenerated(node: object, type: Kind): boolean | "ignore-this-and-nested-nodes" {
  if (isSvelteOwnershipBinding(node)) return "ignore-this-and-nested-nodes";
  return isSvelteTemplateFallback(node, type) || isSvelteAttributeCache(node, type);
}
