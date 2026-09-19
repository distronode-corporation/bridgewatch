import { describe, expect, it } from "vitest";

import {
  answersOf,
  cliKeyringService,
  draftFromAnswers,
  emptyDraft,
  findTokenPrefix,
  formatCommand,
  parseCommand,
  projectRefFor,
  suggestAccountName,
  suggestWatchId,
  switchProvider,
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

describe("GitHub", () => {
  it("answers carry provider and workflow only for GitHub, so a GitLab payload is unchanged", () => {
    const gitlab = answersOf(emptyDraft());
    expect("provider" in gitlab).toBe(false);
    expect("workflow" in gitlab).toBe(false);

    const draft = emptyDraft("github");
    draft.workflow = " ci.yml ";
    draft.preflightEnabled = true;
    draft.preflightRef = "pf/*";
    const answers = answersOf(draft);
    expect(answers.provider).toBe("github");
    expect(answers.base_url).toBe("https://api.github.com");
    expect(answers.account).toBe("github");
    expect(answers.workflow).toBe("ci.yml");
    expect(answers.live_secs).toBe(30);
    // A preflight watch is refused for GitHub by the core, so none is sent.
    expect(answers.preflight_ref).toBeNull();
    draft.workflow = "";
    expect(answersOf(draft).workflow).toBeNull();
  });

  it("round-trips a GitHub re-run, and an Enterprise host stays self-managed", () => {
    const given = {
      provider: "github" as const,
      workflow: "deploy.yml",
      account: "ghe",
      base_url: "https://ghe.acme.com",
      token: { keyring: { service: "gh:ghe.acme.com", user: "" } },
      project: "acme/web",
      watch_id: "web-main",
      ref_name: "main",
      sources: ["push", "workflow_dispatch"],
      deploy_markers: ["publish"],
      schedule_watch: true,
      preflight_ref: null,
      notify: { deployed: true, blocking_failure: true, finished: false },
      launch_at_login: false,
      live_secs: 30,
    };
    const draft = draftFromAnswers(given);
    expect(draft.instance).toBe("self-managed");
    expect(draft.tokenMode).toBe("cli");
    expect(answersOf(draft)).toEqual(given);
    expect(draftFromAnswers({ provider: "github", base_url: "https://api.github.com" }).instance).toBe("hosted");
  });

  it("switching provider resets what cannot carry over and keeps what can", () => {
    const draft = emptyDraft();
    draft.sources = ["push", "merge_request_event"];
    draft.preflightEnabled = true;
    draft.project = { ref: 42, path: "acme/web", default_branch: "main" };
    draft.tokenMode = "cli";
    draft.cliSource = { keyring: { service: "glab:gitlab.com:token", user: "" } };
    switchProvider(draft, "github");
    expect(draft.provider).toBe("github");
    expect(draft.account).toBe("github");
    expect(draft.sources).toEqual(["push"]);
    expect(draft.preflightEnabled).toBe(false);
    expect(draft.project).toBeNull();
    expect(draft.tokenMode).toBe("paste");
    expect(draft.cliSource).toBeNull();
    expect(draft.liveSecs).toBe(30);

    // A speed the user chose is theirs; only the old default follows.
    draft.liveSecs = 20;
    switchProvider(draft, "gitlab");
    expect(draft.liveSecs).toBe(20);
    expect(draft.account).toBe("gitlab");
  });

  it("mirrors the core's account name, keyring service and token-shape rules", () => {
    expect(suggestAccountName("https://api.github.com", "github")).toBe("github");
    expect(suggestAccountName("https://ghe.acme.com", "github")).toBe("ghe");
    expect(suggestAccountName("https://api.acme.ghe.com", "github")).toBe("acme");
    expect(cliKeyringService("github", "https://api.github.com")).toBe("gh:github.com");
    expect(cliKeyringService("github", "https://ghe.acme.com")).toBe("gh:ghe.acme.com");
    expect(cliKeyringService("gitlab", "https://GitLab.Example.com:8443/")).toBe("glab:gitlab.example.com:8443:token");
    expect(cliKeyringService("github", "github.com")).toBeNull();
    // GitHub's prefixes are refused for GitHub only, as in the core.
    expect(findTokenPrefix("ghp_abc", "github")).toBe("ghp_");
    expect(findTokenPrefix("github_pat_abc", "github")).toBe("github_pat_");
    expect(findTokenPrefix("ghp_abc")).toBeNull();
    expect(projectRefFor("github", { id: 7, path: "acme/web" })).toBe("acme/web");
    expect(projectRefFor("gitlab", { id: 7, path: "acme/web" })).toBe(7);
  });

  it("validates in GitHub's words", () => {
    const draft = emptyDraft("github");
    expect(validateStep("account", draft).token).toMatch(/classic or fine-grained/);
    expect(validateStep("project", draft).project).toMatch(/owner\/repo/);
    draft.sources = [];
    expect(validateStep("watch", draft).sources).toMatch(/event/);
    draft.tokenMode = "cli";
    expect(validateStep("account", draft).token).toMatch(/^gh's token/);
  });
});
