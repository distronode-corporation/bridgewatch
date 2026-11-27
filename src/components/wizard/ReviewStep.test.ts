// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import type { WizardPreview } from "./api";
import ReviewStep from "./ReviewStep.svelte";

/** The review step on its own: the build error, the quick setup's counts, and where each warning is. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

const PREVIEW: WizardPreview = { toml: "x = 1\n", warnings: [], edited_existing: false };

function render(props: Partial<ComponentProps<typeof ReviewStep>>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(ReviewStep, {
    target: host,
    props: { preview: PREVIEW, loading: false, error: null, saveDiagnostics: [], saveError: null, tokenNote: null, ...props },
  });
  flushSync();
  return host;
}

const slot = (name: string) => host!.querySelector(`[data-slot="${name}"]`)?.textContent?.replace(/\s+/g, " ").trim();

describe("ReviewStep", () => {
  it("says the config could not be built, and why", () => {
    render({ preview: null, error: "the core refused the answers" });
    expect(slot("preview-error")).toBe("Could not build the config the core refused the answers");
  });

  it("counts what a quick setup adds: none, one, or several with the first primary", () => {
    render({ quick: true, preview: { ...PREVIEW, already_watched: ["acme/web"] } });
    expect(slot("quick-added")).toBe("Adds no watch: every one you ticked is already watched.");
    expect(slot("quick-already")).toBe("Already watched, left as they are: acme/web.");
    void unmount(component!);
    host!.remove();
    render({ quick: true, preview: { ...PREVIEW, added: ["web-main"] } });
    expect(slot("quick-added")).toBe("Adds 1 watch: web-main.");
    expect(slot("quick-already")).toBeUndefined();
    void unmount(component!);
    host!.remove();
    render({ quick: true, preview: { ...PREVIEW, added: ["web-main", "api-main"] } });
    expect(slot("quick-added")).toMatch(/^Adds 2 watches: web-main, api-main\. The first drives the tray icon/);
  });

  it("places each warning by its line, else its path, else nowhere", () => {
    render({
      preview: {
        ...PREVIEW,
        warnings: [
          { severity: "warning", path: "watches[0]", message: "a", line: 4, col: 1 },
          { severity: "warning", path: "watches[1]", message: "b", line: null, col: null },
          { severity: "warning", path: "", message: "c", line: null, col: null },
        ],
      },
    });
    const lines = [...host!.querySelectorAll('[data-slot="preview-warning"]')].map((p) => p.textContent);
    expect(lines).toEqual(["line 4: a", "watches[1]: b", "c"]);
  });
});
