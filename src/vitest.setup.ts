/**
 * Runs before every test file (vitest.config.ts `setupFiles`).
 *
 * ⛔ A Svelte `derived_inert` warning fails the test it is printed in. It means
 * something read a `$derived` whose owner had already been destroyed, which
 * returns a stale value. The suite once passed while printing 52 of them, and
 * nobody can see a new one in that much noise.
 */
import { afterEach, vi } from "vitest";

/**
 * ⚠ jsdom has no `Element.getAnimations`, and bits-ui's `AnimationsComplete`
 * takes a different path without it: it runs the open/close completion on the
 * next microtask with no check that the component still exists, so a
 * Collapsible unmounted right after a toggle read its own destroyed `open` box
 * (all of the derived_inert warnings this suite used to print). The webview
 * has the method, and there bits-ui waits for the element's animations and
 * cancels the wait on destroy. An element with no animations running is what
 * the tests render, so the stub says exactly that.
 */
if (typeof Element !== "undefined" && typeof Element.prototype.getAnimations !== "function") {
  Element.prototype.getAnimations = () => [];
}

const inert: string[] = [];
const warn = console.warn.bind(console);

vi.spyOn(console, "warn").mockImplementation((...args: unknown[]) => {
  if (args.some((a) => String(a).includes("derived_inert"))) {
    inert.push(new Error("derived_inert").stack ?? "derived_inert");
  }
  warn(...args);
});

afterEach(() => {
  if (inert.length === 0) return;
  const stacks = inert.splice(0);
  throw new Error(
    `Svelte warned derived_inert ${stacks.length} time(s): a $derived was read after its owner was destroyed.\n` +
      `First read:\n${stacks[0]}`,
  );
});
