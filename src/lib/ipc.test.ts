import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import Popover from "../components/Popover.svelte";
import fixture from "./__fixtures__/ca41ab28.json" with { type: "json" };
import type { Snapshot } from "./types";

/**
 * M11 / M12: the webview never opens a URL itself.
 *
 * A GitLab instance chooses every `web_url` the popover renders, so a hostile
 * one can hand back `file://` or `javascript:`. The shell's `open_link` is the
 * one door, and it opens only URLs on a configured account's scheme, host and
 * port. These tests hold the frontend half: every click goes to `open_link`,
 * and no source file has a second way out.
 */

const invoked: { cmd: string; args: unknown }[] = [];

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    invoked.push({ cmd, args });
    return Promise.resolve(null);
  },
}));

beforeEach(() => {
  invoked.length = 0;
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("openExternal", () => {
  it("hands the URL to the shell's open_link and nothing else", async () => {
    const { openExternal } = await import("./ipc");
    await openExternal("https://gitlab.com/x/-/jobs/1");
    expect(invoked).toEqual([{ cmd: "open_link", args: { url: "https://gitlab.com/x/-/jobs/1" } }]);
  });

  it("does not decide on the scheme itself: the shell refuses, the webview only asks", async () => {
    // ⚠ Deliberately passed through. A second, weaker check here would read as
    // the guard and invite someone to loosen the real one in Rust.
    const { openExternal } = await import("./ipc");
    await openExternal("file:///etc/passwd");
    expect(invoked.map((i) => i.cmd)).toEqual(["open_link"]);
  });

  it("sends nothing for a missing URL", async () => {
    const { openExternal } = await import("./ipc");
    await openExternal(null);
    await openExternal("");
    expect(invoked).toEqual([]);
  });
});

describe("Link", () => {
  it("cancels the webview's own navigation and asks the shell instead", async () => {
    const { default: Link } = await import("../components/Link.svelte");
    const host = document.createElement("div");
    document.body.append(host);
    const component = mount(Link, { target: host, props: { href: "https://gitlab.com/p/-/pipelines/1" } });
    flushSync();
    try {
      const a = host.querySelector("a")!;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      a.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
      await Promise.resolve();
      expect(invoked).toEqual([
        { cmd: "open_link", args: { url: "https://gitlab.com/p/-/pipelines/1" } },
      ]);
    } finally {
      void unmount(component);
      host.remove();
    }
  });
});

describe("the job view's links", () => {
  it("open a job through open_link, from inside a bridge", async () => {
    const snapshot = fixture as unknown as Snapshot;
    const host = document.createElement("div");
    document.body.append(host);
    const component = mount(Popover, { target: host, props: { snapshot, now: 0 } });
    flushSync();
    try {
      const link = [...host.querySelectorAll("a")].find((a) => a.textContent?.trim() === "verify:android")!;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      link.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
      await Promise.resolve();
      expect(invoked).toEqual([{ cmd: "open_link", args: { url: link.getAttribute("href") } }]);
    } finally {
      void unmount(component);
      host.remove();
    }
  });
});

describe("no second way out of the webview", () => {
  // Vite reads the tree at transform time: no node:fs, which this project has
  // no types for. Keys are root-relative ("/src/components/Link.svelte").
  const tree = import.meta.glob(["/src/**/*.ts", "/src/**/*.svelte", "!/src/**/*.test.ts", "!/src/**/__fixtures__/**"], {
    query: "?raw",
    import: "default",
    eager: true,
  }) as Record<string, string>;
  const files = Object.keys(tree);
  const name = (f: string) => f.replace(/^\/src\//, "");

  it("walks the real tree", () => {
    // Guards the assertions below against a walker that finds nothing.
    expect(files.some((f) => f.endsWith("lib/ipc.ts"))).toBe(true);
    expect(files.some((f) => f.endsWith(".test.ts"))).toBe(false);
    expect(files.some((f) => f.endsWith("components/Link.svelte"))).toBe(true);
  });

  it("imports no opener plugin and calls no window.open", () => {
    const offenders = files.filter((f) => {
      const text = tree[f];
      return /plugin-opener|window\.open\s*\(|location\.(href|assign|replace)\s*[=(]/.test(text);
    });
    expect(offenders.map((f) => name(f))).toEqual([]);
  });

  it("sends open_link from ipc.ts only", () => {
    const offenders = files.filter(
      (f) => !f.endsWith("lib/ipc.ts") && tree[f].includes('"open_link"'),
    );
    expect(offenders.map((f) => name(f))).toEqual([]);
  });

  it("gives every anchor a click handler that can cancel the navigation", () => {
    // An <a href> without one is a GitLab page loaded INTO the popover.
    const bare: string[] = [];
    // The vendored ui kit's Button renders an <a> when handed `href`; it is
    // held by the next assertion instead (nothing may hand it one).
    const app = files.filter((f) => f.endsWith(".svelte") && !f.includes("/lib/components/ui/"));
    for (const f of app) {
      const text = tree[f];
      for (const match of text.matchAll(/<a\b[^>]*>/gs)) {
        if (!/\bonclick=/.test(match[0])) bare.push(`${name(f)}: ${match[0].slice(0, 60)}`);
      }
    }
    expect(bare).toEqual([]);
  });

  it("hands no ui-kit component an href, which would render a bare anchor", () => {
    const offenders = files
      .filter((f) => f.endsWith(".svelte") && !f.includes("/lib/components/ui/"))
      .filter((f) => /<(Button|[A-Z]\w*\.\w+)\b[^>]*\bhref=/s.test(tree[f]));
    expect(offenders.map((f) => name(f))).toEqual([]);
  });
});
