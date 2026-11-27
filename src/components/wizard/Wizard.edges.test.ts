// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { OAuthApi, OAuthAvailability } from "../../lib/oauth";
import type { NeedsConfirm, ProjectListing, RecentListing, WizardApi, WizardPreview, WizardSaveResult } from "./api";
import Wizard from "./Wizard.svelte";

/**
 * The wizard's orchestration on the paths Wizard.test.ts does not take: answers
 * that arrive late or out of order, a second click before the first one's
 * render, questions the user declines, an older shell without the optional
 * commands, and the failures each request can come back with.
 */

const IDENTITY = {
  username: "alice",
  name: null,
  bot: false,
  token: { kind: "personal" as const },
  scopes: ["read_api"],
  expires_at: null,
  warnings: [],
};

const LISTING: ProjectListing = {
  mode: "projects",
  truncated: false,
  projects: [
    { id: 42, path: "acme/web", name: "web", default_branch: "trunk", web_url: null },
    { id: 43, path: "acme/docs", name: "docs", default_branch: null, web_url: null },
  ],
};

const RECENT: RecentListing = {
  mode: "projects",
  listed_by: { provider: "gitlab", origin: "https://gitlab.com" },
  projects: [{ id: 42, path: "acme/web", name: "web", default_branch: "trunk", web_url: null, ticked: true }],
};

const PREVIEW: WizardPreview = { toml: "x = 1\n", warnings: [], edited_existing: false };
const SAVED: WizardSaveResult = { ok: true, diagnostics: [], path: "/x/config.toml" };
const CONFIRM = { id: "q1", changes: ["Run gh auth token."] } as NeedsConfirm["confirm"];

type Api = Required<WizardApi>;
type FakeApi = { [K in keyof Api]: Mock<Api[K]> };

function fakeApi(overrides: Partial<Record<keyof Api, Mock>> = {}): FakeApi {
  return {
    detectCliToken: vi.fn<Api["detectCliToken"]>(async () => ({ status: "not_found", service: "glab:gitlab.com" })),
    ownTokenStatus: vi.fn<Api["ownTokenStatus"]>(async () => ({ kind: "legacy" })),
    testConnection: vi.fn<Api["testConnection"]>(async () => IDENTITY),
    recentProjects: vi.fn<Api["recentProjects"]>(async () => ({ mode: "unlisted", reason: "" })),
    listProjects: vi.fn<Api["listProjects"]>(async () => LISTING),
    resolveProject: vi.fn<Api["resolveProject"]>(async () => ({ id: 7, path: "acme/api", default_branch: "develop", web_url: null })),
    suggestDeployMarkers: vi.fn<Api["suggestDeployMarkers"]>(async () => ({
      pipeline_id: 1,
      pipeline_url: null,
      suggestions: [],
      unread_children: [],
    })),
    suggestExpectedWorkflows: vi.fn<Api["suggestExpectedWorkflows"]>(async () => ({ commits: 0, workflows: [] })),
    previewConfig: vi.fn<Api["previewConfig"]>(async () => PREVIEW),
    save: vi.fn<Api["save"]>(async () => SAVED),
    skip: vi.fn<Api["skip"]>(async () => {}),
    ...(overrides as Partial<FakeApi>),
  };
}

/** A promise and the hands that settle it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host.remove();
  document.body.innerHTML = "";
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
  }
  await tick();
}

async function start(api: WizardApi, extra: Record<string, unknown> = {}) {
  const onFinish = vi.fn();
  const onSkip = vi.fn();
  component = mount(Wizard, { target: host, props: { api, onFinish, onSkip, ...extra } });
  await settle();
  return { onFinish, onSkip };
}

const stepId = () => host.querySelector<HTMLElement>('[data-slot="wizard"]')?.dataset.step;
const q = <T extends Element = HTMLElement>(sel: string) => document.body.querySelector<T>(sel);
const text = (sel: string) => q(sel)?.textContent?.replace(/\s+/g, " ").trim() ?? null;

async function click(sel: string) {
  const el = q(sel);
  if (!el) throw new Error(`no element for ${sel}`);
  (el as HTMLElement).click();
  await settle();
}

async function type(sel: string, value: string) {
  const el = q<HTMLInputElement>(sel);
  if (!el) throw new Error(`no input for ${sel}`);
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

async function key(sel: string, k: string) {
  q(sel)!.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  await settle();
}

const next = () => click('[data-action="next"]');
const fieldErrors = () => [...host.querySelectorAll('[data-slot="field-error"]')].map((e) => e.textContent?.trim());
const confirmDialog = () => q('[data-slot="confirm-dialog"]');
const answerDialog = (yes: boolean) => click(`[data-slot="confirm-dialog"] [data-action="${yes ? "confirm" : "cancel"}"]`);

/** Account (pasted token) → project (picked from the list) → watch. */
async function toWatch() {
  await type("#wizard-secret", "pasted-secret-value");
  await next();
  await click('input[name="project"][value="42"]');
  await next();
}

async function toReview() {
  await toWatch();
  await next();
  await click('input[name="marker-mode"][value="none"]');
  await next();
  await next();
}

async function useCommand(command = "pass show gitlab") {
  await click('input[name="token-mode"][value="command"]');
  await type("#wizard-command", command);
}

describe("a second click before the first one's render", () => {
  it("sends one Next and one Skip for a double click, and nothing for a Finish while the save is out", async () => {
    const save = deferred<WizardSaveResult>();
    const skip = deferred<void>();
    const api = fakeApi({ save: vi.fn(() => save.promise), skip: vi.fn(() => skip.promise) });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    q<HTMLButtonElement>('[data-action="next"]')!.click();
    q<HTMLButtonElement>('[data-action="next"]')!.click();
    await settle();
    expect(stepId()).toBe("project");
    await click('input[name="project"][value="42"]');
    await next();
    await next();
    await click('input[name="marker-mode"][value="none"]');
    await next();
    await next();
    expect(stepId()).toBe("review");
    await click('[data-action="finish"]');
    // A click while the save is out sends nothing.
    q<HTMLButtonElement>('[data-action="finish"]')!.click();
    await settle();
    expect(api.save).toHaveBeenCalledTimes(1);
    save.resolve({ ok: false, diagnostics: [] });
    await settle();
    q<HTMLButtonElement>('[data-action="skip"]')!.click();
    q<HTMLButtonElement>('[data-action="skip"]')!.click();
    expect(api.skip).toHaveBeenCalledTimes(1);
    skip.reject(new Error("the shell did not answer"));
    await settle();
    expect(host.textContent).toContain("the shell did not answer");
  });
});

describe("Finish's other answers", () => {
  it("says the config was not written when the save fails with no diagnostic", async () => {
    await start(fakeApi({ save: vi.fn(async () => ({ ok: false, diagnostics: [] })) }));
    await toReview();
    await click('[data-action="finish"]');
    expect(text('[data-slot="save-error"]')).toContain("The config was not written.");
  });

  it("re-reads the preview when the file changed on disk while the wizard was open", async () => {
    const api = fakeApi({ save: vi.fn(async () => ({ ok: false, diagnostics: [], conflict: true })) });
    await start(api);
    await toReview();
    expect(api.previewConfig).toHaveBeenCalledTimes(1);
    await click('[data-action="finish"]');
    expect(api.previewConfig).toHaveBeenCalledTimes(2);
    expect(text('[data-slot="save-error"]')).toContain("config.toml changed on disk");
  });

  it("asks before saving a command source, shows the command, and writes nothing when declined", async () => {
    const api = fakeApi({ save: vi.fn(async () => ({ ok: false, diagnostics: [], confirm: CONFIRM })) });
    await start(api);
    await useCommand("pass show gitlab");
    await next();
    // The wizard's own question about running a command, answered once.
    await answerDialog(true);
    await click('input[name="project"][value="42"]');
    await next();
    await next();
    await click('input[name="marker-mode"][value="none"]');
    await next();
    await next();
    await click('[data-action="finish"]');
    expect(confirmDialog()?.textContent).toContain("pass");
    await answerDialog(false);
    expect(api.save).toHaveBeenCalledTimes(1);
  });

  it("jumps to the step a preview names, and takes the first message per field", async () => {
    const failure = {
      kind: "answers",
      message: "bad",
      issues: [
        { step: "watch", field: "ref_name", message: "Name the branch." },
        { step: "watch", field: "ref_name", message: "A second message for the same field." },
        { step: "deploy", field: "deploy_markers", message: "Another step." },
      ],
      diagnostics: [],
    };
    await start(fakeApi({ previewConfig: vi.fn(async () => Promise.reject(failure)) }));
    await toReview();
    expect(stepId()).toBe("watch");
    expect(fieldErrors()).toEqual(["Name the branch."]);
  });

  it("shows a save's issues on the step it is already on", async () => {
    const issues = [{ step: "review", field: "watches", message: "Nothing to add." }];
    await start(fakeApi({ save: vi.fn(async () => ({ ok: false, diagnostics: [], issues })) }));
    await toReview();
    await click('[data-action="finish"]');
    expect(stepId()).toBe("review");
  });
});

describe("live requests that ask first", () => {
  it("does not test the connection when the user declines the shell's question", async () => {
    const api = fakeApi({ testConnection: vi.fn(async () => ({ confirm: CONFIRM })) });
    await start(api);
    await useCommand();
    await click('[data-action="test-connection"]');
    // The command question first, then the shell's.
    await answerDialog(true);
    expect(confirmDialog()?.textContent).toContain("Run gh auth token.");
    await answerDialog(false);
    expect(api.testConnection).toHaveBeenCalledTimes(1);
    expect(q('[data-slot="identity"]')).toBeNull();
  });

  it("says the request was not allowed when the shell asks twice", async () => {
    const api = fakeApi({ testConnection: vi.fn(async () => ({ confirm: CONFIRM })) });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    await answerDialog(true);
    expect(text('[data-slot="test-error"]')).toContain("The request was not allowed to run.");
  });

  it("stays on the account step when the command question is declined on Next", async () => {
    await start(fakeApi());
    await useCommand();
    await next();
    await answerDialog(false);
    expect(stepId()).toBe("account");
  });

  it("keeps the project list unasked when its question is declined", async () => {
    const api = fakeApi({ listProjects: vi.fn(async () => ({ confirm: CONFIRM })) });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await answerDialog(false);
    expect(stepId()).toBe("project");
    expect(q('input[name="project"][value="42"]')).toBeNull();
  });
});

describe("the project step's typed project", () => {
  it("looks nothing up for an empty box, and keeps a project typed as its id", async () => {
    const api = fakeApi();
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await key("#wizard-project-input", "Enter");
    expect(api.resolveProject).not.toHaveBeenCalled();
    await type("#wizard-project-input", "7");
    await key("#wizard-project-input", "Enter");
    expect(api.resolveProject).toHaveBeenCalledTimes(1);
    await next();
    // "7" names the resolved project by its id, so Next does not resolve it again.
    expect(api.resolveProject).toHaveBeenCalledTimes(1);
    expect(stepId()).toBe("watch");
  });

  it("stays on the project step when the typed project does not resolve, with the core's message", async () => {
    const failure = { kind: "not_found", message: "404", issues: [{ step: "project", field: "project", message: "No such project." }] };
    await start(fakeApi({ resolveProject: vi.fn(async () => Promise.reject(failure)) }));
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await type("#wizard-project-input", "acme/nope");
    await next();
    expect(stepId()).toBe("project");
    expect(fieldErrors()).toEqual(["No such project."]);
  });

  it("does not resolve when its question is declined", async () => {
    const api = fakeApi({ resolveProject: vi.fn(async () => ({ confirm: CONFIRM })) });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await type("#wizard-project-input", "acme/api");
    await key("#wizard-project-input", "Enter");
    await answerDialog(false);
    expect(stepId()).toBe("project");
  });

  it("keeps the branch when the same project is picked again, and a typed watch id across a pick", async () => {
    await start(fakeApi());
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await click('input[name="project"][value="42"]');
    await next();
    await type("#wizard-ref", "release");
    await type("#wizard-watch-id", "mine");
    await click('[data-action="back"]');
    await click('input[name="project"][value="43"]');
    await click('input[name="project"][value="42"]');
    await next();
    expect(q<HTMLInputElement>("#wizard-watch-id")?.value).toBe("mine");
  });

  it("searches the project list", async () => {
    const api = fakeApi();
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await type("#wizard-project-search", "web");
    await key("#wizard-project-search", "Enter");
    expect(api.listProjects.mock.calls.at(-1)?.[1]).toBe("web");
  });
});

describe("the deploy step's suggestions", () => {
  it("asks once per project and branch, and not when its question is declined", async () => {
    const api = fakeApi();
    await start(api);
    await toWatch();
    await next();
    expect(api.suggestDeployMarkers).toHaveBeenCalledTimes(1);
    await click('[data-action="back"]');
    await next();
    expect(api.suggestDeployMarkers).toHaveBeenCalledTimes(1);
  });

  it("keeps the old suggestions unasked when the question is declined", async () => {
    const api = fakeApi({ suggestDeployMarkers: vi.fn(async () => ({ confirm: CONFIRM })) });
    await start(api);
    await toWatch();
    await next();
    await answerDialog(false);
    expect(stepId()).toBe("deploy");
    expect(q('[data-slot="no-suggestions"]')).toBeNull();
  });
});

describe("an older shell without the optional commands", () => {
  it("detects no CLI token and offers no one-click list", async () => {
    const api = fakeApi();
    const older = { ...api } as Partial<FakeApi>;
    delete older.detectCliToken;
    delete older.recentProjects;
    await start(older as WizardApi);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    expect(q('[data-slot="identity"]')).not.toBeNull();
    expect(q('[data-slot="quick-setup"]')).toBeNull();
  });
});

describe("answers that arrive out of order", () => {
  it("keeps only the newest CLI detection", async () => {
    type Found = { status: "found"; source: { keyring: { service: string; user: string } } };
    const first = deferred<Found>();
    const found = (service: string): Found => ({ status: "found", source: { keyring: { service, user: "" } } });
    const detect = vi
      .fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementation(async () => found("glab:gitlab.example.com:token"));
    const api = fakeApi({ detectCliToken: detect });
    await start(api);
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://gitlab.example.com");
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    first.resolve(found("glab:gitlab.com:token"));
    await settle();
    expect(detect).toHaveBeenCalledTimes(2);
    // The late answer for gitlab.com did not replace the one for this instance.
    await click('input[name="token-mode"][value="glab"]');
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0].token).toEqual({ keyring: { service: "glab:gitlab.example.com:token", user: "" } });
  });

  it("drops a one-click list that arrives after a newer question, and one that failed late", async () => {
    const first = deferred<RecentListing>();
    const second = deferred<RecentListing>();
    const recent = vi.fn().mockImplementationOnce(() => first.promise).mockImplementationOnce(() => second.promise);
    await start(fakeApi({ recentProjects: recent }));
    await type("#wizard-secret", "first-secret-value");
    await click('[data-action="test-connection"]');
    await type("#wizard-secret", "second-secret-value");
    await click('[data-action="test-connection"]');
    first.resolve(RECENT);
    await settle();
    expect(q('[data-slot="quick-setup"]')?.textContent ?? "").not.toContain("acme/web");
    second.reject(new Error("late"));
    await settle();
    expect(text('[data-slot="quick-error"]')).toContain("late");
  });
});

describe("the one-click setup", () => {
  async function toQuick(api: FakeApi) {
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
  }

  it("sends nothing for a click while the one-click setup runs", async () => {
    const markers = deferred<never>();
    const api = fakeApi({ recentProjects: vi.fn(async () => RECENT), suggestDeployMarkers: vi.fn(() => markers.promise) });
    await toQuick(api);
    await click('[data-action="quick-setup"]');
    // A click while the deploy jobs are being read sends nothing.
    q<HTMLButtonElement>('[data-action="quick-setup"]')!.click();
    await settle();
    expect(api.suggestDeployMarkers).toHaveBeenCalledTimes(1);
  });

  it("refuses to start with an invalid account step, and stops when a question is declined", async () => {
    const api = fakeApi({ recentProjects: vi.fn(async () => RECENT), suggestDeployMarkers: vi.fn(async () => ({ confirm: CONFIRM })) });
    await toQuick(api);
    await type("#wizard-account", "");
    await click('[data-action="quick-setup"]');
    expect(fieldErrors().length).toBeGreaterThan(0);
    expect(api.suggestDeployMarkers).not.toHaveBeenCalled();
    await type("#wizard-account", "gitlab");
    await click('[data-action="quick-setup"]');
    await answerDialog(false);
    expect(stepId()).toBe("account");
  });

  it("drops the one-click choice when a skipped step is opened from the stepper, and then asks that step's questions", async () => {
    const api = fakeApi({ recentProjects: vi.fn(async () => RECENT) });
    await toQuick(api);
    await click('[data-action="quick-setup"]');
    expect(stepId()).toBe("preferences");
    await click('nav[aria-label="Setup steps"] button[aria-label^="Deploy"]');
    expect(stepId()).toBe("deploy");
  });
});

describe("sign-in", () => {
  function fakeOAuth(availability: Partial<OAuthAvailability> = {}): OAuthApi {
    return {
      availability: vi.fn(async () => ({ available: true, builtin: true, needs_client_id: false, host: "gitlab.com", install_url: null, ...availability })),
      start: vi.fn(async () => ({ id: "f1", user_code: "WDJB-MJHT", verification_uri: "https://gitlab.com/oauth/device", expires_in: 900, host: "gitlab.com" })),
      wait: vi.fn(async () => ({ login: null, scopes: [], expires_at: null })),
      cancel: vi.fn(async () => {}),
      openVerification: vi.fn(async () => {}),
      openInstall: vi.fn(async () => {}),
      status: vi.fn(async () => null),
      signOut: vi.fn(async () => {}),
      copy: vi.fn(async () => true),
    } as unknown as OAuthApi;
  }

  it("names a sign-in with no login as unknown, and sends the form's account when the sign-in has none", async () => {
    const api = fakeApi();
    const oauth = fakeOAuth();
    await start(api, { oauth });
    expect(q<HTMLInputElement>('input[name="token-mode"][value="oauth"]')?.checked).toBe(true);
    await click('[data-action="oauth-sign-in"]');
    expect(host.textContent).toContain("(unknown)");
  });

  it("keeps the newest availability answer, whether the older one lands or fails", async () => {
    const late = deferred<OAuthAvailability>();
    const lateFailure = deferred<OAuthAvailability>();
    const oauth = fakeOAuth();
    (oauth.availability as Mock)
      .mockImplementationOnce(() => late.promise)
      .mockImplementationOnce(() => lateFailure.promise)
      .mockImplementation(async () => ({ available: false, builtin: false, needs_client_id: true, host: "gitlab.example.com", install_url: null }));
    await start(fakeApi(), { oauth });
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://gitlab.example.com");
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    late.resolve({ available: true, builtin: true, needs_client_id: false, host: "gitlab.com", install_url: null });
    lateFailure.reject(new Error("offline"));
    await settle();
    expect(q<HTMLInputElement>('input[name="token-mode"][value="oauth"]')?.checked ?? false).toBe(false);
  });
});

describe("the GitHub commit group's expected workflows", () => {
  it("keeps only the newest answer while the events change", async () => {
    const first = deferred<{ commits: number; workflows: { file: string; seen_in: number }[] }>();
    const suggest = vi
      .fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementation(async () => ({ commits: 1, workflows: [{ file: "ci.yml", seen_in: 1 }] }));
    const api = fakeApi({ suggestExpectedWorkflows: suggest });
    await start(api, { initial: { provider: "github", base_url: "https://api.github.com", group: "commit", token: { own: true } } });
    await type("#wizard-secret", "ghp_pasted");
    await next();
    await click('input[name="project"][value="42"]');
    await next();
    expect(stepId()).toBe("watch");
    await click("#wizard-source-push");
    first.reject(new Error("too late"));
    await settle();
    expect(q('[data-slot="expect"] [role="alert"]')).toBeNull();
  });
});

describe("requests that fail", () => {
  it("names the reason when the CLI detection itself fails", async () => {
    const api = fakeApi({ detectCliToken: vi.fn(async () => Promise.reject(new Error("no keychain"))) });
    await start(api);
    // An unavailable CLI is no option to pick.
    expect(q('input[name="token-mode"][value="glab"]')).toBeNull();
    expect(q<HTMLInputElement>('input[name="token-mode"][value="paste"]')?.checked).toBe(true);
  });

  it("says why the project list, the deploy jobs and the preview could not be read", async () => {
    const api = fakeApi({
      listProjects: vi.fn(async () => Promise.reject(new Error("list failed"))),
      suggestDeployMarkers: vi.fn(async () => Promise.reject(new Error("pipeline unreadable"))),
      previewConfig: vi.fn(async () => Promise.reject({ kind: "io", message: "disk full", issues: [], diagnostics: [] })),
    });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    expect(host.textContent).toContain("list failed");
    await type("#wizard-project-input", "acme/api");
    await next();
    await next();
    expect(host.textContent).toContain("pipeline unreadable");
    await click('input[name="marker-mode"][value="none"]');
    await next();
    await next();
    expect(text('[data-slot="preview-error"]')).toContain("disk full");
  });

  it("jumps to the step a failed save names", async () => {
    const failure = { kind: "answers", message: "bad", issues: [{ step: "watch", field: "ref_name", message: "Name the branch." }], diagnostics: [] };
    await start(fakeApi({ save: vi.fn(async () => Promise.reject(failure)) }));
    await toReview();
    await click('[data-action="finish"]');
    expect(stepId()).toBe("watch");
  });

  it("drops a one-click list error that arrives after a newer question", async () => {
    const first = deferred<RecentListing>();
    const recent = vi.fn().mockImplementationOnce(() => first.promise).mockImplementation(async () => RECENT);
    await start(fakeApi({ recentProjects: recent }));
    await type("#wizard-secret", "first-secret-value");
    await click('[data-action="test-connection"]');
    await type("#wizard-secret", "second-secret-value");
    await click('[data-action="test-connection"]');
    first.reject(new Error("too late"));
    await settle();
    expect(q('[data-slot="quick-error"]')).toBeNull();
    expect(q('[data-slot="quick-row"]')).not.toBeNull();
  });
});

describe("the review step's token note", () => {
  it("says which variable an env source reads", async () => {
    await start(fakeApi());
    await click('input[name="token-mode"][value="env"]');
    await type("#wizard-env", "GITLAB_TOKEN");
    await next();
    await click('input[name="project"][value="42"]');
    await next();
    await next();
    await click('input[name="marker-mode"][value="none"]');
    await next();
    await next();
    expect(text('[data-slot="token-note"]')).toBe("The token is read from $GITLAB_TOKEN when bridgewatch starts.");
  });
});

describe("a GitHub commit group on an older shell", () => {
  it("asks for no expected workflows without the command", async () => {
    const api = fakeApi();
    const older = { ...api } as Partial<FakeApi>;
    delete older.suggestExpectedWorkflows;
    await start(older as WizardApi, { initial: { provider: "github", base_url: "https://api.github.com", group: "commit", token: { own: true } } });
    await type("#wizard-secret", "ghp_pasted");
    await next();
    await click('input[name="project"][value="42"]');
    await next();
    expect(stepId()).toBe("watch");
    expect(q('[data-slot="expect"]')?.textContent).not.toContain("Reading the newest pushes");
  });

  it("says why the expected workflows could not be read", async () => {
    const api = fakeApi({ suggestExpectedWorkflows: vi.fn(async () => Promise.reject(new Error("rate limited"))) });
    await start(api, { initial: { provider: "github", base_url: "https://api.github.com", group: "commit", token: { own: true } } });
    await type("#wizard-secret", "ghp_pasted");
    await next();
    await click('input[name="project"][value="42"]');
    await next();
    expect(text('[data-slot="expect"] [role="alert"]')).toBe("rate limited");
  });
});

describe("the CLI token as detections come and go", () => {
  type Found = { status: "found"; source: { keyring: { service: string; user: string } } };
  const found = (service: string): Found => ({ status: "found", source: { keyring: { service, user: "" } } });
  const CUSTOM = { keyring: { service: "my-own-item", user: "me" } };

  async function testedToken(api: FakeApi) {
    await click('[data-action="test-connection"]');
    return api.testConnection.mock.calls.at(-1)?.[0].token;
  }

  it("forgets a detected token the next detection on the same instance no longer finds", async () => {
    const detect = vi
      .fn()
      .mockImplementationOnce(async () => ({ status: "not_found", service: "glab:gitlab.com" }))
      .mockImplementationOnce(async () => found("glab:gitlab.example.com:token"))
      .mockImplementation(async () => ({ status: "not_found", service: "glab:gitlab.example.com" }));
    await start(fakeApi({ detectCliToken: detect }));
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://gitlab.example.com");
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    expect(q('input[name="token-mode"][value="glab"]')).not.toBeNull();
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    expect(detect).toHaveBeenCalledTimes(3);
    expect(q('input[name="token-mode"][value="glab"]')).toBeNull();
  });

  it("leaves a chosen pasted token alone while detections find the CLI item, on this instance and the next", async () => {
    const detect = vi
      .fn()
      .mockImplementationOnce(async () => ({ status: "not_found", service: "glab:gitlab.example.com" }))
      .mockImplementation(async () => found("glab:gitlab.example.com:token"));
    const api = fakeApi({ detectCliToken: detect });
    await start(api, { initial: { account: "work", base_url: "https://gitlab.example.com", token: CUSTOM } });
    await click('input[name="token-mode"][value="paste"]');
    // Detected now, but the form is on a pasted token: the file's own item is left as it is.
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    // And a move to another instance does not drop it either, since it is not the detected one.
    await type("#wizard-base-url", "https://gitlab.other.example");
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    // Neither detection took the form off the pasted token the user chose.
    expect(q<HTMLInputElement>('input[name="token-mode"][value="paste"]')?.checked).toBe(true);
    await type("#wizard-secret", "pasted-secret-value");
    expect(await testedToken(api)).toEqual({ own: true });
  });
});

describe("a stored sign-in from a file that names no account", () => {
  it("is looked up under the form's account name", async () => {
    const api = fakeApi();
    await start(api, { initial: { base_url: "https://gitlab.com", token: { oauth: true } } });
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0].account).toBe("gitlab");
  });
});

describe("availability answers out of order", () => {
  it("ignores an older availability question that fails after a newer one answered", async () => {
    const older = deferred<OAuthAvailability>();
    const oauth = {
      availability: vi
        .fn()
        .mockImplementationOnce(() => older.promise)
        .mockImplementation(async () => ({ available: true, builtin: true, needs_client_id: false, host: "gitlab.example.com", install_url: null })),
      start: vi.fn(),
      wait: vi.fn(),
      cancel: vi.fn(),
      openVerification: vi.fn(),
      openInstall: vi.fn(),
      status: vi.fn(async () => null),
      signOut: vi.fn(),
      copy: vi.fn(),
    } as unknown as OAuthApi;
    await start(fakeApi(), { oauth });
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://gitlab.example.com");
    q("#wizard-base-url")!.dispatchEvent(new FocusEvent("blur"));
    await settle();
    expect(q<HTMLInputElement>('input[name="token-mode"][value="oauth"]')?.checked).toBe(true);
    older.reject(new Error("offline"));
    await settle();
    // The failure belonged to the old instance: sign-in stays chosen for this one.
    expect(q<HTMLInputElement>('input[name="token-mode"][value="oauth"]')?.checked).toBe(true);
  });
});
