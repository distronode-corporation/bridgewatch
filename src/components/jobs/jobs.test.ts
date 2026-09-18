import { describe, expect, it } from "vitest";

import fixture from "../../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { Snapshot } from "../../lib/types";
import { PARENT_JOBS, RUNNING_JOB, T0, bridge, job } from "./__fixtures__/pipeline";
import {
  NO_STAGE,
  bridgeVisible,
  createExpansionStore,
  elapsedSeconds,
  filterJobs,
  formatDuration,
  groupByStage,
  updatedAgo,
} from "./jobs";

const snapshot = fixture as unknown as Snapshot;
const REAL = snapshot.watches[0].rows[0];

describe("groupByStage", () => {
  it("orders stages by pipeline order, not by the order the API listed them", () => {
    // PARENT_JOBS arrive newest id first: deploy, test, test, build, build.
    const groups = groupByStage(PARENT_JOBS);
    expect(groups.map((g) => g.stage)).toEqual(["build", "test", "deploy"]);
    // Within a stage the incoming order is kept.
    expect(groups[0].jobs.map((j) => j.name)).toEqual(["build:web", "lint"]);
  });

  it("gets a real recording's child stages in pipeline order", () => {
    const voice = REAL.bridges.find((b) => b.name === "trigger:voice_agent")!;
    expect(groupByStage(voice.jobs).map((g) => g.stage)).toEqual([
      "verify",
      "build",
      "mirror",
      "security",
      "deploy",
    ]);
    expect(groupByStage(REAL.parent_jobs).map((g) => g.stage)).toEqual([".pre", "test", "scan-verify"]);
  });

  it("lets an explicit stage order win, and appends stages it does not list", () => {
    const groups = groupByStage(PARENT_JOBS, ["deploy", "build"]);
    expect(groups.map((g) => g.stage)).toEqual(["deploy", "build", "test"]);
  });

  it("puts jobs without a stage last", () => {
    const groups = groupByStage([job({ id: 1, name: "a", stage: null }), job({ id: 9, name: "b", stage: "z" })]);
    expect(groups.map((g) => g.stage)).toEqual(["z", NO_STAGE]);
  });

  it("survives a retry: the stage's lowest id still places it", () => {
    // `lint` was retried (new id 110); `build:web` keeps id 102, still below test.
    const jobs = [job({ id: 110, name: "lint", stage: "build" }), ...PARENT_JOBS.filter((j) => j.id !== 101)];
    expect(groupByStage(jobs).map((g) => g.stage)).toEqual(["build", "test", "deploy"]);
  });
});

describe("failures mode", () => {
  it("keeps blocking failures, tolerated failures and gates only", () => {
    const gate = job({ id: 7, name: "approve", class: "gate", status: "manual" });
    expect(filterJobs([...PARENT_JOBS, gate], "failures").map((j) => j.name)).toEqual(["e2e", "lint", "approve"]);
    expect(filterJobs(PARENT_JOBS, "all")).toHaveLength(PARENT_JOBS.length);
  });

  it("hides a quiet bridge but keeps a dead one that has no jobs at all", () => {
    expect(bridgeVisible(bridge({ name: "ok" }), "failures")).toBe(false);
    expect(bridgeVisible(bridge({ name: "ok" }), "all")).toBe(true);
    expect(bridgeVisible(bridge({ name: "gone", verdict: "dead", dived: false, child_id: null }), "failures")).toBe(
      true,
    );
  });
});

describe("durations", () => {
  it("ticks a running job from started_at, never negative", () => {
    expect(elapsedSeconds(RUNNING_JOB, T0)).toBe(65);
    expect(elapsedSeconds(RUNNING_JOB, T0 + 3_000)).toBe(68);
    expect(elapsedSeconds(RUNNING_JOB, T0 - 120_000)).toBe(0);
  });

  it("uses GitLab's duration for a finished job, else the timestamps, else nothing", () => {
    expect(elapsedSeconds(job({ id: 1, name: "a", duration: 42.4 }), T0)).toBe(42);
    expect(
      elapsedSeconds(
        job({ id: 1, name: "a", started_at: "2026-09-18T10:00:00Z", finished_at: "2026-09-18T10:02:30Z" }),
        T0,
      ),
    ).toBe(150);
    expect(elapsedSeconds(job({ id: 1, name: "a", class: "not_built" }), T0)).toBeNull();
  });

  it("formats", () => {
    expect(formatDuration(0)).toBe("0s");
    expect(formatDuration(59)).toBe("59s");
    expect(formatDuration(65)).toBe("1m 05s");
    expect(formatDuration(3_725)).toBe("1h 02m");
    expect(formatDuration(null)).toBe("");
  });

  it("says how long ago the last poll was", () => {
    expect(updatedAgo(new Date(T0 - 4_000).toISOString(), T0)).toBe("updated 4s ago");
    expect(updatedAgo(T0 - 125_000, T0)).toBe("updated 2m ago");
    expect(updatedAgo(null, T0)).toBe("not updated yet");
  });
});

describe("expansion store", () => {
  it("returns the fallback until the user chooses, then the choice", () => {
    const store = createExpansionStore();
    expect(store.isOpen(1, "trigger:a", true)).toBe(true);
    expect(store.toggle(1, "trigger:a", true)).toBe(false);
    // A later fallback (the pipeline settled) does not override the choice.
    expect(store.isOpen(1, "trigger:a", false)).toBe(false);
    expect(store.isOpen(2, "trigger:a", true)).toBe(true);
  });

  it("prunes pipelines that left the screen", () => {
    const store = createExpansionStore();
    store.set(1, "a", true);
    store.set(2, "a", true);
    store.prune([2]);
    expect(store.has(1, "a")).toBe(false);
    expect(store.has(2, "a")).toBe(true);
  });
});
