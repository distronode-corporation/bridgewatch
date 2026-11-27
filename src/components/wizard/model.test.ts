// SPDX-FileCopyrightText: Distronode Corporation
//
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import {
  answersOf,
  cliKeyringService,
  draftFromAnswers,
  emptyDraft,
  findTokenPrefix,
  followOwnToken,
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
  it("a commit group sends group and expect and no workflow; run mode and GitLab send neither", () => {
    expect("group" in answersOf(emptyDraft())).toBe(false);
    const draft = emptyDraft("github");
    draft.workflow = "ci.yml";
    expect("group" in answersOf(draft)).toBe(false);
    expect("expect" in answersOf(draft)).toBe(false);

    draft.group = "commit";
    draft.expect = [" ci.yml ", "", "release.yml"];
    const answers = answersOf(draft);
    expect(answers.group).toBe("commit");
    expect(answers.expect).toEqual(["ci.yml", "release.yml"]);
    expect(answers.workflow).toBeNull();

    // A re-run on a commit group comes back as one, and switching to GitLab drops it.
    const back = draftFromAnswers(answers);
    expect(back.group).toBe("commit");
    expect(back.expect).toEqual(["ci.yml", "release.yml"]);
    expect(draftFromAnswers({ ...answers, provider: undefined }).group).toBe("run");
  });

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
    // As the core's `oauth::host_of` normalises it: no default port, no userinfo.
    expect(cliKeyringService("gitlab", "https://GitLab.com:443/")).toBe("glab:gitlab.com:token");
    expect(cliKeyringService("gitlab", "http://gitlab.internal:80")).toBe("glab:gitlab.internal:token");
    expect(cliKeyringService("gitlab", "https://gitlab.com?x=1")).toBe("glab:gitlab.com:token");
    expect(cliKeyringService("gitlab", "https://me@gitlab.com")).toBeNull();
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

describe("the model on hostile or unusual input", () => {
  it("has no gh keyring item for a host that is only `api.`", () => {
    expect(cliKeyringService("github", "https://api./")).toBeNull();
    expect(cliKeyringService("github", "https://api.github.com")).toBe("gh:github.com");
  });

  it("finds no GitHub prefix in text that carries none", () => {
    expect(findTokenPrefix("just words", "github")).toBeNull();
    expect(findTokenPrefix("ghp_x", "gitlab")).toBeNull();
  });

  it("leaves the draft alone when switching to the provider it already has", () => {
    const draft = emptyDraft("github");
    draft.account = "typed";
    switchProvider(draft, "github");
    expect(draft).toEqual({ ...emptyDraft("github"), account: "typed" });
  });

  it("falls back to push when no source survives a provider switch", () => {
    const draft = emptyDraft("gitlab");
    draft.sources = ["merge_request_event"];
    switchProvider(draft, "github");
    expect(draft.sources).toEqual(["push"]);
  });

  it("reads a sign-in without an account, a not-yet-stored own token, and a commit group without expect", () => {
    const oauth = draftFromAnswers({ base_url: "https://gitlab.com", token: { oauth: true } });
    expect(oauth.tokenMode).toBe("oauth");
    expect(oauth.oauthAccount).toBeNull();
    const own = draftFromAnswers({ base_url: "https://gitlab.com", token: { own: false } });
    expect(own.ownStored).toBe(false);
    expect(own.ownStoredFor).toBeNull();
    const group = draftFromAnswers({ provider: "github", base_url: "https://api.github.com", group: "commit" });
    expect(group.group).toBe("commit");
    expect(group.expect).toEqual([]);
  });

  it("keeps primary from the answers", () => {
    expect(draftFromAnswers({ primary: true }).primary).toBe(true);
    expect(draftFromAnswers({}).primary).toBe(false);
  });

  it("suggests the provider's name for a URL with no usable host", () => {
    expect(suggestAccountName("ftp://files.example.com")).toBe("gitlab");
    expect(suggestAccountName("not a url", "github")).toBe("github");
    expect(suggestAccountName("https://.example.com")).toBe("gitlab");
    expect(suggestAccountName("file:///tmp/x")).toBe("gitlab");
  });

  it("suggests a watch id from a path with no slash", () => {
    expect(suggestWatchId("monorepo", "main")).toBe("monorepo-main");
  });

  it("keeps a backslash that ends a double-quoted word", () => {
    expect(parseCommand('echo "a\\')).toEqual(["echo", "a\\"]);
    expect(parseCommand('echo "a\\"b"')).toEqual(["echo", 'a"b']);
  });

  it("has nothing to check on the review step", () => {
    expect(validateStep("review", emptyDraft())).toEqual({});
  });

  it("keeps a typed account name across a provider switch", () => {
    const draft = emptyDraft("gitlab");
    draft.account = "work";
    draft.accountEdited = true;
    switchProvider(draft, "github");
    expect(draft.account).toBe("work");
  });

  it("does not take a stored token for an instance URL that does not parse", () => {
    const draft = emptyDraft("gitlab");
    draft.instance = "self-managed";
    draft.selfManagedUrl = "gitlab dot example";
    draft.ownStoredFor = "https://gitlab.example.com";
    followOwnToken(draft);
    expect(draft.ownStored).toBe(false);
  });

  it("asks for the instance address when the self-managed URL is empty, naming the provider", () => {
    const draft = emptyDraft("gitlab");
    draft.instance = "self-managed";
    expect(validateStep("account", draft).base_url).toBe("Enter your GitLab's address, e.g. https://gitlab.example.com.");
    switchProvider(draft, "github");
    draft.instance = "self-managed";
    expect(validateStep("account", draft).base_url).toBe(
      "Enter your GitHub Enterprise address, e.g. https://github.example.com.",
    );
  });
});
