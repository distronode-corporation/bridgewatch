import { describe, expect, it } from "vitest";

// Vite `?raw`: the core's source as committed, read at transform time.
import wizardRs from "../../../crates/bridgewatch-core/src/wizard.rs?raw";
import table from "./__fixtures__/validation-cases.json";
import type { WizardAnswers } from "./api";
import { GITHUB_TOKEN_PREFIXES, TOKEN_PREFIXES, draftFromAnswers, validateStep, type Draft, type StepId } from "./model";

/**
 * The webview's copies of rules the core owns, held to the core.
 *
 * ⛔ Each of these was typed twice, in Rust and here, with no test reading
 * both. The watch-id rule had already drifted: the wizard refused ids the core
 * and Settings accept, so a re-run on such a file could not pass its own
 * Watch step.
 */

describe("the token-prefix refusal lists", () => {
  /** A `pub const NAME: &[&str] = &[...]` from wizard.rs, as written. */
  function rustList(name: string): string[] {
    const match = new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`).exec(wizardRs);
    expect(match, `${name} not found in wizard.rs`).not.toBeNull();
    return [...match![1].matchAll(/"([^"]*)"/g)].map((m) => m[1]);
  }

  it("GitLab's are the core's TOKEN_PREFIXES, in order", () => {
    const rust = rustList("TOKEN_PREFIXES");
    // Guards against a pattern that matched an empty list.
    expect(rust).toContain("glpat-");
    expect([...TOKEN_PREFIXES]).toEqual(rust);
  });

  it("GitHub's are the core's GITHUB_TOKEN_PREFIXES, in order", () => {
    const rust = rustList("GITHUB_TOKEN_PREFIXES");
    expect(rust).toContain("ghp_");
    expect([...GITHUB_TOKEN_PREFIXES]).toEqual(rust);
  });
});

/**
 * The same table `crates/bridgewatch-core/tests/wizard_validation_parity.rs`
 * feeds to `validate_answers`.
 */
describe("the wizard's validation, against the shared cases", () => {
  interface Case {
    name: string;
    step: StepId;
    answers: Record<string, unknown>;
    fields: string[];
  }
  const cases = table.cases as Case[];

  /**
   * `draftFromAnswers` is a PREFILL: it keeps the draft's own default where an
   * answer is empty, so on its own it could never show an empty id or a zero
   * interval to `validateStep`. The fields a case can empty are set here as
   * given, which is what a user clearing the input produces.
   */
  function draftFor(answers: Record<string, unknown>): Draft {
    const merged = { ...table.base, ...answers } as Partial<WizardAnswers>;
    const draft = draftFromAnswers(merged);
    if ("account" in answers) draft.account = String(answers.account);
    if ("watch_id" in answers) draft.watchId = String(answers.watch_id);
    if ("ref_name" in answers) draft.refName = String(answers.ref_name);
    if ("live_secs" in answers) draft.liveSecs = Number(answers.live_secs);
    return draft;
  }

  it("has cases", () => {
    expect(cases.length).toBeGreaterThanOrEqual(10);
  });

  it.each(cases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    expect(Object.keys(validateStep(c.step, draftFor(c.answers))).sort()).toEqual(c.fields);
  });

  it("the base answers pass every step", () => {
    const draft = draftFor({});
    for (const step of ["account", "project", "watch", "deploy", "preferences"] as const) {
      expect(validateStep(step, draft), step).toEqual({});
    }
  });
});
