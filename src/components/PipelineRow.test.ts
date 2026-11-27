// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import fixture from "../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { PipelineView, Snapshot } from "../lib/types";
import { createExpansionStore } from "./jobs";
import PipelineRow from "./PipelineRow.svelte";

/** One pipeline row on its own, from the recorded ca41ab28 snapshot with one field changed per test. */

const snapshot = fixture as unknown as Snapshot;
const ROW = snapshot.watches[0].rows[0];

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(row: PipelineView): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(PipelineRow, {
    target: host,
    props: { row, now: Date.parse("2026-09-17T15:00:00Z"), expansion: createExpansionStore() },
  });
  flushSync();
  return host;
}

describe("PipelineRow", () => {
  it("links a sibling failure that is a parent job to that job's page", () => {
    const failed = ROW.parent_jobs[0];
    const root = render({ ...ROW, sibling_failures: [failed.name] });
    const link = [...root.querySelectorAll(".notes a")].find((a) => a.textContent === failed.name);
    expect(link?.getAttribute("href")).toBe(failed.web_url);
  });

  it("shows no notes line when nothing failed beside or after the deploy", () => {
    expect(render({ ...ROW, sibling_failures: [], post_deploy_failures: [] }).querySelector(".notes")).toBeNull();
  });

  it("names the deploy marker's own failures beside it", () => {
    const root = render({ ...ROW, deploy_failures: ["deploy:origins", "deploy:cdn"] });
    expect(root.querySelector(".marker .text-tone-red")?.textContent).toBe("deploy:origins, deploy:cdn");
  });

  it("leaves out the source when the pipeline has none, and the age title when it has no creation time", () => {
    const root = render({ ...ROW, source: null, created_at: null });
    expect(root.querySelector(".source")).toBeNull();
    expect(root.querySelector(".age")?.getAttribute("title")).toBe("");
  });
});
