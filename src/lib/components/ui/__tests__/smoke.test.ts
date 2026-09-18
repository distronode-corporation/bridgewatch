import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cn } from "$lib/components/ui/utils.js";
import Smoke from "./Smoke.svelte";

/**
 * Smoke render of the vendored shadcn-svelte (Rhea) components under jsdom:
 * each mounts through its public barrel, emits its `data-slot`, and the
 * interactive ones answer a click. Not a test of bits-ui itself; a test that
 * the vendoring, the `$lib` alias and the Svelte 5 / bits-ui pairing hold.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  // jsdom has no ResizeObserver; both webviews do. bits-ui's scroll-area
  // measures through it, so a no-op stands in.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host.remove();
  vi.unstubAllGlobals();
});

const slot = (name: string, root: ParentNode = host) => root.querySelector<HTMLElement>(`[data-slot="${name}"]`);

describe("vendored ui components", () => {
  it("render through their barrels with Rhea's markup", () => {
    component = mount(Smoke, { target: host });
    flushSync();

    for (const name of [
      "card",
      "card-title",
      "button",
      "badge",
      "separator",
      "label",
      "input",
      "switch",
      "checkbox",
      "progress",
      "stepper",
      "tabs",
      "tabs-list",
      "collapsible",
      "accordion",
      "scroll-area",
      "alert",
    ]) {
      expect(slot(name), name).not.toBeNull();
    }

    // Rhea's compact density: 32px buttons with the 1.8x-radius corner.
    expect(slot("button")!.className).toContain("h-8");
    expect(slot("button")!.className).toContain("rounded-2xl");
    expect(slot("badge")!.textContent).toBe("failed");
    expect(slot("input")).toBeInstanceOf(HTMLInputElement);
    expect((slot("input") as HTMLInputElement).value).toBe("https://gitlab.com");
    expect(slot("label")!.getAttribute("for")).toBe("smoke-url");
    expect(slot("alert")!.getAttribute("role")).toBe("alert");
    expect(slot("checkbox")!.getAttribute("data-state")).toBe("checked");

    // Only the active tab's panel is shown (bits-ui keeps the other mounted, hidden).
    const panels = [...host.querySelectorAll<HTMLElement>('[data-slot="tabs-content"]')];
    expect(panels.map((p) => [p.textContent?.trim(), p.hidden])).toEqual([
      ["every job", false],
      ["failures only", true],
    ]);

    // Stepper: step 2 of 3 is current, step 1 is complete.
    const current = host.querySelector('[aria-current="step"]');
    expect(current?.textContent).toContain("Project");
    expect(host.querySelectorAll('[data-slot="stepper-item"][data-state="complete"]')).toHaveLength(1);
  });

  it("wires events and bindings", () => {
    const onClick = vi.fn();
    component = mount(Smoke, { target: host, props: { onClick } });
    flushSync();

    slot("button")!.click();
    expect(onClick).toHaveBeenCalledTimes(1);

    const toggle = slot("switch")!;
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    toggle.click();
    flushSync();
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("portals an open dialog into the document", () => {
    component = mount(Smoke, { target: host, props: { dialogOpen: true } });
    flushSync();
    const dialog = slot("dialog-content", document);
    expect(dialog).not.toBeNull();
    expect(dialog!.getAttribute("role")).toBe("dialog");
    expect(slot("dialog-title", document)?.textContent).toBe("Write config.toml?");
  });

  // buttonVariants is not imported here on purpose: a .ts file importing a ui
  // barrel pulls it into plain tsc, which cannot read `<script module>` exports
  // (see tsconfig.tsc.json). The rendered class above covers the variants.
  it("exposes cn, merging Tailwind classes", () => {
    expect(cn("px-2 h-8", "px-3")).toBe("h-8 px-3");
  });
});
