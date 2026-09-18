import { describe, expect, it } from "vitest";

import {
  bridgeTone,
  bridgeWord,
  configErrorLines,
  countdown,
  errorLines,
  isFirstRun,
  deployTone,
  deployWord,
  jobTone,
  millis,
  relativeAge,
  shortPath,
  sourceBadge,
  stateTone,
  stateWord,
  statusTone,
} from "./format";
import type { DiagnosticView, IconState } from "./types";

/** The eight names `IconState::as_str()` produces. */
const STATES: IconState[] = [
  "unknown",
  "failed",
  "deployed_with_failure",
  "deployed",
  "running",
  "canceled",
  "parked_gate",
  "succeeded_no_deploy",
];

/**
 * The phrase each state is shown as.
 *
 * ⚠ Spelled out here rather than derived, and that is the whole point: both
 * `stateTone` and `stateWord` FALL BACK (to grey, and to the raw identifier)
 * rather than throwing, so "is it defined" and "does it contain no underscore"
 * are both true of the fallback for five of the eight states. A deleted entry
 * passed those checks. An explicit table is the only assertion a deletion
 * fails.
 */
const WORDS: Record<IconState, string> = {
  unknown: "unknown",
  failed: "failed",
  deployed_with_failure: "deployed with failure",
  deployed: "deployed",
  running: "running",
  canceled: "canceled",
  parked_gate: "parked at a gate",
  succeeded_no_deploy: "green, no deploy",
};

/** The two states that are grey ON PURPOSE. Every other one must carry colour. */
const GREY_BY_DESIGN: IconState[] = ["unknown", "canceled"];

describe("state formatting", () => {
  it("has a tone and a phrase for every icon state", () => {
    // ⛔ A state the core can produce and the frontend cannot name would render
    // as the raw identifier, in grey, and read as a bug in the verdict engine.
    for (const state of STATES) {
      expect(stateWord(state), state).toBe(WORDS[state]);
      const tone = stateTone(state);
      expect(["red", "amber", "green", "blue", "grey"], state).toContain(tone);
      // A state dropped from the tone table falls through to grey, which is a
      // legible colour and the wrong one.
      if (!GREY_BY_DESIGN.includes(state)) expect(tone, state).not.toBe("grey");
    }
  });

  it("does not colour a deploy-with-failure the same as a clean deploy", () => {
    // The distinction the whole program exists to make.
    expect(stateTone("deployed")).toBe("green");
    expect(stateTone("deployed_with_failure")).toBe("amber");
    expect(stateTone("failed")).toBe("red");
  });

  it("falls back to grey and the raw name rather than throwing", () => {
    expect(stateTone(null)).toBe("grey");
    expect(stateWord(undefined)).toBe("unknown");
  });
});

describe("bridge and deploy words", () => {
  it("treats a dead bridge as red", () => {
    // A trigger job that produced no child pipeline. Every other monitor
    // reports the parent green.
    expect(bridgeTone("dead")).toBe("red");
    expect(bridgeWord("dead")).toBe("dead");
  });

  it("spells the two-word verdicts out", () => {
    expect(bridgeWord("passed_with_warnings")).toBe("passed, with warnings");
    expect(bridgeWord("awaiting_gate")).toBe("awaiting a gate");
  });

  it("names every deploy outcome the core can emit", () => {
    // ⚠ Each phrase is named. `deployWord` returns its argument for anything it
    // does not know, so "is it not undefined" was true of every outcome
    // including one whose case had been deleted.
    const words: Record<string, string> = {
      absent: "no deploy",
      in_progress: "deploying",
      live: "deployed",
      failed: "deploy failed",
      dead: "deploy never ran",
      canceled: "deploy canceled",
    };
    for (const [outcome, word] of Object.entries(words)) {
      expect(deployWord(outcome), outcome).toBe(word);
      expect(deployTone(outcome), outcome).toBeTruthy();
    }
    // The fallback is reachable and returns the identifier, which is how a
    // missing case above would have rendered.
    expect(deployWord("something_gitlab_added_later")).toBe("something_gitlab_added_later");
  });

  it("maps a job class to a tone", () => {
    expect(jobTone("blocking_failure")).toBe("red");
    expect(jobTone("warning_failure")).toBe("amber");
    expect(jobTone("gate")).toBe("amber");
    expect(jobTone("passed")).toBe("green");
    expect(jobTone("not_built")).toBe("grey");
  });
});

describe("relativeAge", () => {
  const now = Date.parse("2026-09-17T12:00:00Z");

  it("renders seconds, minutes, hours and days", () => {
    expect(relativeAge("2026-09-17T11:59:50Z", now)).toBe("10s");
    expect(relativeAge("2026-09-17T11:55:00Z", now)).toBe("5m");
    expect(relativeAge("2026-09-17T09:00:00Z", now)).toBe("3h");
    expect(relativeAge("2026-09-15T12:00:00Z", now)).toBe("2d");
  });

  it("moves to the next unit rather than printing 60m or 24h", () => {
    // One second short of the boundary: rounding the SECONDS produced a unit
    // the next branch exists to avoid.
    expect(relativeAge("2026-09-17T11:00:01Z", now)).toBe("1h");
    expect(relativeAge("2026-09-16T12:00:01Z", now)).toBe("1d");
    // ...and the units below the boundary still round normally.
    expect(relativeAge("2026-09-17T11:10:00Z", now)).toBe("50m");
    expect(relativeAge("2026-09-16T13:00:00Z", now)).toBe("23h");
  });

  it("never reports a future time, because clocks disagree", () => {
    // GitLab's clock and this machine's differ by a second or two routinely.
    // "in 3s" on a pipeline that already exists reads as a bug.
    expect(relativeAge("2026-09-17T12:00:05Z", now)).toBe("0s");
  });

  it("returns an empty string for a missing or unparseable timestamp", () => {
    expect(relativeAge(null, now)).toBe("");
    expect(relativeAge(undefined, now)).toBe("");
    expect(relativeAge("not a date", now)).toBe("");
  });
});

describe("debug pane formatting", () => {
  it("counts down, and says now at zero", () => {
    expect(countdown(12)).toBe("in 12s");
    expect(countdown(0)).toBe("now");
    expect(countdown(-4)).toBe("now");
    expect(countdown(null)).toBe("");
  });

  it("switches from milliseconds to seconds", () => {
    expect(millis(536)).toBe("536ms");
    expect(millis(1201)).toBe("1.20s");
  });

  it("keeps the TAIL of a long path, where the query string is", () => {
    const path = "/projects/82468124/pipelines?ref=main&per_page=20&order_by=id";
    const short = shortPath(path, 24);
    expect(short.length).toBe(24);
    expect(short.startsWith("…")).toBe(true);
    expect(short.endsWith("order_by=id")).toBe(true);
  });

  it("colours a transport failure, which has no status at all, red", () => {
    expect(statusTone(null)).toBe("red");
    expect(statusTone(200)).toBe("green");
    expect(statusTone(429)).toBe("amber");
    expect(statusTone(500)).toBe("red");
  });

  it("passes an unrecognised pipeline source through rather than guessing", () => {
    expect(sourceBadge("merge_request_event")).toBe("MR");
    expect(sourceBadge("schedule")).toBe("sched");
    expect(sourceBadge("something_gitlab_added_later")).toBe("something_gitlab_added_later");
    expect(sourceBadge(null)).toBe("");
  });
});

describe("the error strip's lines (H9)", () => {
  const diag = (d: Partial<DiagnosticView>): DiagnosticView => ({
    severity: "error",
    path: "",
    message: "boom",
    line: null,
    col: null,
    ...d,
  });

  it("spells a diagnostic exactly as the shell's poller does, so the two dedupe", () => {
    // Mirrors `poller::config_errors`: path + line, line alone, path alone, bare.
    expect(
      configErrorLines({
        configOk: false,
        diagnostics: [
          diag({ path: "watches[0].project", line: 12, message: "expected `=`" }),
          diag({ line: 3, message: "bad" }),
          diag({ path: "ui.jobs", message: "unknown variant" }),
          diag({ message: "bare" }),
          diag({ severity: "warning", message: "only a warning" }),
        ],
      }),
    ).toEqual([
      "watches[0].project (line 12): expected `=`",
      "line 3: bad",
      "ui.jobs: unknown variant",
      "bare",
    ]);
  });

  it("still says something when a broken file came with no error diagnostics", () => {
    expect(configErrorLines({ configOk: false, diagnostics: [] })).toEqual([
      "the configuration file does not load; open Settings to fix it",
    ]);
  });

  it("says nothing for a file that loads, or before the status arrives", () => {
    expect(configErrorLines({ configOk: true, diagnostics: [diag({})] })).toEqual([]);
    expect(configErrorLines(null)).toEqual([]);
  });

  it("puts the file's complaints first and never shows a line twice", () => {
    const status = { configOk: false, diagnostics: [diag({ path: "ui.jobs", message: "nope" })] };
    expect(errorLines(["ui.jobs: nope", "main-push: 429"], status)).toEqual([
      "ui.jobs: nope",
      "main-push: 429",
    ]);
    // ⛔ The case the review found: a tick that forgot the file's errors.
    expect(errorLines(["main-push: 429"], status)).toEqual(["ui.jobs: nope", "main-push: 429"]);
  });
});

describe("isFirstRun", () => {
  const diag = (message: string): DiagnosticView => ({ severity: "error", path: "", message, line: null, col: null });

  it("recognises the shell's first-run diagnostic, and only that", () => {
    expect(isFirstRun({ configOk: false, diagnostics: [diag("No configuration yet. Finish the setup wizard")] })).toBe(true);
    expect(isFirstRun({ configOk: false, diagnostics: [diag("expected `=`")] })).toBe(false);
    expect(isFirstRun(null)).toBe(false);
  });

  it("prefers the shell's own flag when it is sent", () => {
    expect(isFirstRun({ configOk: false, diagnostics: [diag("expected `=`")], firstRun: true })).toBe(true);
    expect(isFirstRun({ configOk: false, diagnostics: [diag("No configuration yet.")], firstRun: false })).toBe(false);
  });
});
