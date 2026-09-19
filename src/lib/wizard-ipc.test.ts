import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { WizardAnswers } from "../components/wizard/api";
import WizardApp from "../WizardApp.svelte";

/**
 * The wizard's wiring to the shell: `wizardApi()` is one command per method,
 * the pasted token only ever leaves as `secret`, the core's span-based
 * diagnostics reach the wizard as `DiagnosticView`, and a re-run pre-fills
 * from the configuration without a token in sight.
 */

const invoked: { cmd: string; args: Record<string, unknown> | undefined }[] = [];
const answers = new Map<string, unknown>();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => {
    invoked.push({ cmd, args });
    return Promise.resolve(answers.get(cmd) ?? null);
  },
}));

beforeEach(() => {
  invoked.length = 0;
  answers.clear();
});

const ANSWERS: WizardAnswers = {
  account: "gitlab.com",
  base_url: "https://gitlab.com",
  token: { own: true },
  project: 82468124,
  watch_id: "main-push",
  ref_name: "main",
  sources: ["push"],
  deploy_markers: ["deploy:origins"],
  schedule_watch: false,
  preflight_ref: null,
  notify: null,
  launch_at_login: null,
  live_secs: null,
};

describe("wizardApi", () => {
  it("maps every method onto one wizard command", async () => {
    const { wizardApi } = await import("./ipc");
    const api = wizardApi();
    const conn = { base_url: "https://gitlab.com", token: { own: true } as const, secret: "glpat-x" };
    await api.detectCliToken!("https://gitlab.com", "gitlab");
    await api.testConnection(conn);
    await api.listProjects(conn);
    await api.resolveProject(conn, "group/project");
    await api.suggestDeployMarkers(conn, 5, "main");
    await api.previewConfig(ANSWERS).catch(() => {});
    await api.skip();
    expect(invoked.map((i) => i.cmd)).toEqual([
      "wizard_detect_cli_token",
      "wizard_test_connection",
      "wizard_list_projects",
      "wizard_resolve_project",
      "wizard_suggest_deploy_markers",
      "wizard_preview_config",
      "wizard_skip",
    ]);
    expect(invoked[0].args).toEqual({ baseUrl: "https://gitlab.com", provider: "gitlab" });
    expect(invoked[2].args).toEqual({ connection: conn, search: null });
    expect(invoked[3].args).toEqual({ connection: conn, idOrPath: "group/project" });
    expect(invoked[4].args).toEqual({ connection: conn, project: 5, refName: "main" });
  });

  it("sends a workflow only when there is one, and asks for gh's item by provider", async () => {
    const { wizardApi } = await import("./ipc");
    const api = wizardApi();
    const conn = { provider: "github" as const, base_url: "https://api.github.com", token: { own: true } as const };
    await api.detectCliToken!("https://api.github.com", "github");
    await api.suggestDeployMarkers(conn, "acme/web", "main", "ci.yml");
    await api.suggestDeployMarkers(conn, "acme/web", "main");
    expect(invoked[0]).toEqual({
      cmd: "wizard_detect_cli_token",
      args: { baseUrl: "https://api.github.com", provider: "github" },
    });
    expect(invoked[1].args).toEqual({ connection: conn, project: "acme/web", refName: "main", workflow: "ci.yml" });
    expect(invoked[2].args).toEqual({ connection: conn, project: "acme/web", refName: "main" });
  });

  it("names the account an own-token source belongs to", async () => {
    // The shell reads `{own: true}` from `bridgewatch:<account>`; without the
    // account it finds nothing. The wizard's own value wins over the fallback.
    const { wizardApi } = await import("./ipc");
    const own = { base_url: "https://gitlab.example.com", token: { own: true } as const };
    await wizardApi("work").testConnection(own);
    await wizardApi("work").listProjects({ ...own, account: "mine" } as typeof own);
    await wizardApi().testConnection(own);
    expect((invoked[0].args as { connection: { account?: string } }).connection.account).toBe("work");
    expect((invoked[1].args as { connection: { account?: string } }).connection.account).toBe("mine");
    expect((invoked[2].args as { connection: { account?: string } }).connection.account).toBeUndefined();
  });

  it("sends the pasted token as secret only, never inside the answers", async () => {
    answers.set("wizard_save", { ok: true, diagnostics: [], path: "/tmp/c.toml" });
    const { wizardApi } = await import("./ipc");
    const result = await wizardApi().save(ANSWERS, { secret: "glpat-secret" });
    expect(invoked).toEqual([
      { cmd: "wizard_save", args: { answers: ANSWERS, secret: "glpat-secret", confirm: null } },
    ]);
    expect(JSON.stringify((invoked[0].args as { answers: unknown }).answers)).not.toContain("glpat");
    expect(result.path).toBe("/tmp/c.toml");
  });

  it("reports the account a sign-in was made under only when the wizard says so", async () => {
    answers.set("wizard_save", { ok: true, diagnostics: [] });
    const { wizardApi } = await import("./ipc");
    const signing = { ...ANSWERS, token: { oauth: true } as const };
    await wizardApi().save(signing, { oauthAccount: "github" });
    await wizardApi().save(signing, {});
    expect(invoked[0].args).toEqual({ answers: signing, secret: null, confirm: null, oauthAccount: "github" });
    expect(invoked[1].args).toEqual({ answers: signing, secret: null, confirm: null });
  });

  it("passes a confirmation id back on save", async () => {
    answers.set("wizard_save", { ok: true, diagnostics: [] });
    const { wizardApi } = await import("./ipc");
    await wizardApi().save(ANSWERS, { confirm: "c-7" });
    expect(invoked[0].args).toEqual({ answers: ANSWERS, secret: null, confirm: "c-7" });
  });

  it("turns the core's span diagnostics into the view the wizard renders", async () => {
    answers.set("wizard_preview_config", {
      toml: "x = 1\n",
      edited_existing: false,
      warnings: [{ severity: "warning", path: "watches[0]", message: "slow", span: { start: 0, end: 3 } }],
    });
    const { wizardApi } = await import("./ipc");
    const preview = await wizardApi().previewConfig(ANSWERS);
    expect(preview.warnings).toEqual([
      { severity: "warning", path: "watches[0]", message: "slow", line: null, col: null },
    ]);
  });

  it("lets a WizardFailure rejection through untouched, issues and all", async () => {
    const core = await import("@tauri-apps/api/core");
    const failure = {
      kind: "answers",
      message: "bad",
      issues: [{ step: "project", field: "project", message: "required" }],
      diagnostics: [],
    };
    const spy = vi.spyOn(core, "invoke").mockRejectedValueOnce(failure);
    const { wizardApi } = await import("./ipc");
    await expect(wizardApi().previewConfig(ANSWERS)).rejects.toBe(failure);
    spy.mockRestore();
  });
});

describe("wizardInitial", () => {
  it("pre-fills the first account and its primary watch, with no token", async () => {
    const { wizardInitial } = await import("./ipc");
    const initial = wizardInitial({
      accounts: { "gitlab.com": { base_url: "https://gitlab.com", token: { own: true } } },
      watches: [
        { id: "hourly", role: "secondary", account: "gitlab.com", project: 1, ref: "main" },
        {
          id: "main-push",
          role: "primary",
          account: "gitlab.com",
          project: 82468124,
          ref: "main",
          sources: ["push"],
          deploy_markers: ["deploy:origins"],
        },
      ],
      ui: { launch_at_login: true },
    });
    expect(initial).toEqual({
      account: "gitlab.com",
      base_url: "https://gitlab.com",
      token: { own: true },
      project: 82468124,
      watch_id: "main-push",
      ref_name: "main",
      sources: ["push"],
      deploy_markers: ["deploy:origins"],
      launch_at_login: true,
    });
  });

  it("pre-fills a GitHub account's provider and workflow, and nothing extra for GitLab", async () => {
    const { wizardInitial } = await import("./ipc");
    const initial = wizardInitial({
      accounts: { hub: { provider: "github", base_url: "https://api.github.com", token: { own: true } } },
      watches: [{ id: "web-main", role: "primary", account: "hub", project: "acme/web", ref: "main", workflow: "ci.yml" }],
    });
    expect(initial).toMatchObject({ provider: "github", project: "acme/web", workflow: "ci.yml" });
    const gitlab = wizardInitial({
      accounts: { lab: { provider: "gitlab", base_url: "https://gitlab.com", token: { own: true } } },
      watches: [{ id: "w", role: "primary", account: "lab", project: 1, ref: "main", workflow: "stray.yml" }],
    });
    expect(gitlab && "provider" in gitlab).toBe(false);
    expect(gitlab && "workflow" in gitlab).toBe(false);
  });

  it("has nothing to pre-fill without an account", async () => {
    const { wizardInitial } = await import("./ipc");
    expect(wizardInitial(null)).toBeUndefined();
    expect(wizardInitial({ accounts: {}, watches: [] })).toBeUndefined();
  });
});

describe("WizardApp", () => {
  let host: HTMLDivElement;
  let component: Record<string, unknown> | null = null;

  afterEach(() => {
    if (component) void unmount(component);
    component = null;
    host?.remove();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("mounts the wizard pre-filled from the configuration, and skips through the shell", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    answers.set("get_config_json", {
      config: {
        accounts: { work: { base_url: "https://gitlab.example.com", token: { own: true } } },
        watches: [{ id: "main-push", role: "primary", account: "work", project: 7, ref: "main" }],
      },
      job_order: [[]],
    });
    answers.set("wizard_detect_cli_token", { status: "not_found", service: "glab:gitlab.example.com" });
    let skipped = 0;
    host = document.createElement("div");
    document.body.append(host);
    // No api injected: the window builds its own over the shell's commands.
    component = mount(WizardApp, { target: host, props: { onskip: () => skipped++ } });
    for (let i = 0; i < 10; i++) await Promise.resolve();
    flushSync();
    expect(invoked.map((i) => i.cmd)).toContain("get_config_json");
    // The wizard is on screen, seeded from the file: a self-managed URL is
    // nothing the wizard would have typed for itself.
    const inputs = [...host.querySelectorAll<HTMLInputElement>("input")].map((i) => i.value);
    expect(inputs).toContain("https://gitlab.example.com");
    // Skip: the wizard calls the shell's skip (which hides the window and
    // opens Settings on the text tab); nothing else is sent.
    const skip = [...host.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
      /edit config\.toml/i.test(b.textContent ?? ""),
    );
    expect(skip, "no skip button").toBeTruthy();
    skip!.click();
    for (let i = 0; i < 10; i++) await Promise.resolve();
    flushSync();
    expect(invoked.map((i) => i.cmd)).toContain("wizard_skip");
    expect(invoked.map((i) => i.cmd)).not.toContain("wizard_save");
    expect(skipped).toBe(1);
  });
});

describe("oauthApi", () => {
  it("maps every method onto one oauth command, and no token goes either way", async () => {
    answers.set("oauth_start", {
      id: "f1",
      user_code: "WDJB-MJHT",
      verification_uri: "https://github.com/login/device",
      expires_in: 900,
      host: "github.com",
    });
    const { oauthApi } = await import("./ipc");
    const api = oauthApi();
    await api.availability("github", "https://api.github.com");
    await api.availability("gitlab", "https://gitlab.example.com", "gl-app");
    const request = { account: "gh", provider: "github" as const, base_url: "https://api.github.com", client_id: null };
    const started = await api.start(request);
    await api.wait(started.id);
    await api.cancel(started.id);
    await api.openVerification(started.id);
    await api.openInstall("github", "https://api.github.com");
    await api.status("gh");
    await api.signOut("gh");
    expect(invoked).toEqual([
      { cmd: "oauth_availability", args: { provider: "github", baseUrl: "https://api.github.com", clientId: null } },
      { cmd: "oauth_availability", args: { provider: "gitlab", baseUrl: "https://gitlab.example.com", clientId: "gl-app" } },
      { cmd: "oauth_start", args: { request } },
      { cmd: "oauth_wait", args: { id: "f1" } },
      { cmd: "oauth_cancel", args: { id: "f1" } },
      { cmd: "oauth_open_verification", args: { id: "f1" } },
      { cmd: "oauth_open_install", args: { provider: "github", baseUrl: "https://api.github.com" } },
      { cmd: "oauth_status", args: { account: "gh" } },
      { cmd: "oauth_sign_out", args: { account: "gh" } },
    ]);
  });
});
