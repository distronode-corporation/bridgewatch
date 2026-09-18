import { describe, expect, it } from "vitest";

import {
  answersOf,
  draftFromAnswers,
  emptyDraft,
  formatCommand,
  parseCommand,
  suggestAccountName,
  suggestWatchId,
  validateStep,
} from "./model";

describe("parseCommand / formatCommand", () => {
  it("splits on whitespace, groups quotes, expands nothing", () => {
    expect(parseCommand(`pass show "gitlab/my pat"`)).toEqual(["pass", "show", "gitlab/my pat"]);
    expect(parseCommand(`sh -c '$HOME; rm x'`)).toEqual(["sh", "-c", "$HOME; rm x"]);
    expect(parseCommand("a\\ b ''")).toEqual(["a b", ""]);
    expect(parseCommand("   ")).toEqual([]);
  });

  it("round-trips", () => {
    const argv = ["op", "read", "op://vault/it's here/token", ""];
    expect(parseCommand(formatCommand(argv))).toEqual(argv);
  });
});

describe("suggestions", () => {
  it("mirror the core's account and watch-id rules", () => {
    expect(suggestAccountName("https://gitlab.com")).toBe("gitlab");
    expect(suggestAccountName("https://git.example.org:8443")).toBe("git");
    expect(suggestWatchId("acme/My Web", "release/1.x")).toBe("my-web-release-1-x");
  });
});

describe("answersOf", () => {
  it("never carries the pasted token, and 'none' clears markers", () => {
    const draft = emptyDraft();
    draft.secret = "glpat-should-not-appear";
    draft.markers = ["deploy"];
    draft.noMarker = true;
    const answers = answersOf(draft);
    expect(JSON.stringify(answers)).not.toContain("glpat");
    expect(answers.token).toEqual({ own: true });
    expect(answers.deploy_markers).toEqual([]);
    expect(answers.preflight_ref).toBeNull();
  });

  it("draftFromAnswers → answersOf keeps what it was given", () => {
    const given = {
      account: "work",
      base_url: "https://gitlab.example.com",
      token: { command: ["pass", "show", "gl"] },
      project: 42,
      watch_id: "web-main",
      ref_name: "main",
      sources: ["push", "web"],
      deploy_markers: ["deploy:prod"],
      schedule_watch: true,
      preflight_ref: "pf/*",
      notify: { deployed: true, blocking_failure: false, finished: true },
      launch_at_login: true,
      live_secs: 10,
    };
    expect(answersOf(draftFromAnswers(given))).toEqual(given);
  });
});

describe("validateStep", () => {
  it("a stored own token needs no new paste on a re-run", () => {
    const fresh = emptyDraft();
    expect(validateStep("account", fresh).token).toBeDefined();
    const rerun = draftFromAnswers({ token: { own: true } });
    expect(validateStep("account", rerun).token).toBeUndefined();
  });

  it("rejects a live interval outside 1..60 and no sources", () => {
    const draft = emptyDraft();
    draft.liveSecs = 0;
    draft.sources = [];
    expect(validateStep("preferences", draft).live_secs).toBeDefined();
    expect(validateStep("watch", draft).sources).toBeDefined();
  });
});
