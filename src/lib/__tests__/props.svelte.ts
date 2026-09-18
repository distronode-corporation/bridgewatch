/**
 * Reactive props for the component tests.
 *
 * ⛔ A plain object handed to `mount()` is a SNAPSHOT: assigning to it after
 * the component is mounted changes nothing, so a test written that way proves
 * the first render and silently skips every re-render. Several of the defects
 * these tests hold — a reload wiping an uncommitted row, an open panel
 * following a position rather than a watch — only exist on the SECOND render,
 * so they need props the component actually subscribes to.
 *
 * `$state` is a rune, and a rune outside a `.svelte` file needs the
 * `.svelte.ts` extension; that is the only reason this module has one. It is
 * imported by tests only.
 */

/** A props object whose top-level keys are reactive. Assign to one, then `flushSync()`. */
export function reactive<T extends Record<string, unknown>>(initial: T): T {
  const props = $state({ ...initial });
  return props as T;
}
