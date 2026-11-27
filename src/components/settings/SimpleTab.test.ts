// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import SimpleTab from "./SimpleTab.svelte";

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  host?.remove();
  component = null;
  host = null;
});

describe("SimpleTab", () => {
  const config = { icon: { theme: "builtin", states: { failed: "octagon", retries: 3 } } };
  let edits: unknown[] = [];

  function render(): HTMLDivElement {
    edits = [];
    host = document.createElement("div");
    document.body.append(host);
    component = mount(SimpleTab, {
      target: host,
      props: { tab: "icon", title: "Icon", config, onedit: (e: unknown[]) => void edits.push(...e) },
    });
    flushSync();
    return host;
  }

  function change(input: HTMLInputElement, value: string) {
    input.value = value;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
  }

  it("lists a map table's rows, each value as text", () => {
    const cells = [...render().querySelectorAll<HTMLInputElement>("input")].map((i) => i.value);
    expect(cells).toEqual(expect.arrayContaining(["failed", "octagon", "retries", "3"]));
  });

  it("writes a map cell under the table's own path", () => {
    const glyph = [...render().querySelectorAll<HTMLInputElement>("input")].find((i) => i.value === "octagon")!;
    change(glyph, "check");
    // A table key is always quoted, the encoding the core's split_path decodes.
    expect(edits).toEqual([{ op: "set", path: 'icon.states."failed"', value: { string: "check" } }]);
  });

  it("writes a plain field under its registry path", () => {
    const theme = [...render().querySelectorAll<HTMLInputElement>("input")].find((i) => i.value === "builtin")!;
    change(theme, "/icons");
    expect(edits).toEqual([{ op: "set", path: "icon.theme", value: { string: "/icons" } }]);
  });

  it("lists no rows for a table the file does not have", () => {
    host = document.createElement("div");
    document.body.append(host);
    component = mount(SimpleTab, { target: host, props: { tab: "icon", title: "Icon", config: {}, onedit: () => {} } });
    flushSync();
    expect([...host.querySelectorAll<HTMLInputElement>("input")].map((i) => i.value)).toEqual([""]);
  });
});
