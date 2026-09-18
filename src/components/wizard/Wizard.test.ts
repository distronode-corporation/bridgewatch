import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type {
  Identity,
  MarkerSuggestions,
  ProjectListing,
  ResolvedProject,
  WizardApi,
  WizardFailure,
  WizardPreview,
  WizardSaveResult,
} from "./api";
import Wizard from "./Wizard.svelte";

/**
 * The wizard mounted against a fake `WizardApi`. Every assertion about a
 * request reads the fake's recorded calls, so "nothing was written" means
 * `save` was never called, not that the UI looked finished.
 */

const IDENTITY: Identity = {
  username: "alice",
  name: "Alice Doe",
  bot: false,
  token: { kind: "personal" },
  scopes: ["read_api"],
  expires_at: null,
  warnings: [],
};

const PROJECT_TOKEN: Identity = {
  username: "project_42_bot_1",
  name: null,
  bot: true,
  token: { kind: "project", project_id: 42 },
  scopes: ["read_api"],
  expires_at: "2026-12-01",
  warnings: ["This is a project access token: it can see project 42 only."],
};

const LISTING: ProjectListing = {
  mode: "projects",
  truncated: false,
  projects: [
    { id: 42, path: "acme/web", name: "web", default_branch: "trunk", web_url: null },
    { id: 43, path: "acme/docs", name: "docs", default_branch: "main", web_url: null },
  ],
};

const RESOLVED: ResolvedProject = { id: 7, path: "acme/api", default_branch: "develop", web_url: null };

const SUGGESTIONS: MarkerSuggestions = {
  pipeline_id: 900,
  pipeline_url: "https://gitlab.com/acme/web/-/pipelines/900",
  suggestions: [
    {
      name: "deploy:prod",
      stage: "deploy",
      pipeline: "parent",
      status: "success",
      score: 90,
      reasons: ["the name says deploy", "it runs in the last stage"],
    },
  ],
  unread_children: [],
};

const PREVIEW: WizardPreview = {
  toml: '[accounts.gitlab]\nbase_url = "https://gitlab.com"\n\n[[watches]]\nid = "web-trunk"\n',
  warnings: [],
  edited_existing: false,
};

const SAVED: WizardSaveResult = { ok: true, diagnostics: [], path: "/home/u/.config/bridgewatch/config.toml" };

type Api = Required<WizardApi>;
type FakeApi = { [K in keyof Api]: Mock<Api[K]> };

function fakeApi(overrides: Partial<{ [K in keyof Api]: Mock<(...args: never[]) => unknown> }> = {}): FakeApi {
  return {
    detectGlab: vi.fn<Api["detectGlab"]>(async () => ({ status: "not_found", service: "glab:gitlab.com" })),
    testConnection: vi.fn<Api["testConnection"]>(async () => IDENTITY),
    listProjects: vi.fn<Api["listProjects"]>(async () => LISTING),
    resolveProject: vi.fn<Api["resolveProject"]>(async () => RESOLVED),
    suggestDeployMarkers: vi.fn<Api["suggestDeployMarkers"]>(async () => SUGGESTIONS),
    previewConfig: vi.fn<Api["previewConfig"]>(async () => PREVIEW),
    save: vi.fn<Api["save"]>(async () => SAVED),
    skip: vi.fn<Api["skip"]>(async () => {}),
    ...(overrides as Partial<FakeApi>),
  };
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

const next = () => click('[data-action="next"]');
const fieldErrors = () => [...host.querySelectorAll('[data-slot="field-error"]')].map((e) => e.textContent?.trim());

/** Account (pasted token) → project (picked from the list) → watch. */
async function toWatch() {
  await type("#wizard-secret", "pasted-secret-value");
  await next();
  await click('input[name="project"][value="42"]');
  await next();
}

async function toReview() {
  await toWatch();
  await next(); // watch → deploy
  await click('input[name="marker-mode"][value="none"]');
  await next(); // deploy → preferences
  await next(); // preferences → review
}

describe("navigation and per-step validation", () => {
  it("refuses to leave a step with an invalid field, names it, and focuses it", async () => {
    await start(fakeApi());
    expect(stepId()).toBe("account");
    await next();
    expect(stepId()).toBe("account");
    expect(fieldErrors()).toEqual(["Paste a personal, project or group access token."]);
    expect(document.activeElement?.id).toBe("wizard-secret");

    await type("#wizard-secret", "pasted-secret-value");
    await next();
    expect(stepId()).toBe("project");
    expect(document.activeElement?.id).toBe("wizard-heading");

    await next();
    expect(stepId()).toBe("project");
    expect(fieldErrors()[0]).toMatch(/Pick a project/);
  });

  it("validates a self-managed URL and an env var name", async () => {
    await start(fakeApi());
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "not a url");
    await click('input[name="token-mode"][value="env"]');
    await type("#wizard-env", "glpat-abcdef");
    await next();
    expect(stepId()).toBe("account");
    const errs = fieldErrors();
    expect(errs.some((e) => e?.includes("is not an instance URL"))).toBe(true);
    expect(errs.some((e) => e?.includes("looks like a token itself"))).toBe(true);
  });

  it("checks the watch step (branch, preflight glob) and the deploy step (marker or none)", async () => {
    const api = fakeApi();
    await start(api);
    await toWatch();
    expect(stepId()).toBe("watch");
    await type("#wizard-ref", "");
    await click("#wizard-preflight");
    await next();
    expect(stepId()).toBe("watch");
    expect(fieldErrors()).toEqual([
      "Name the branch to watch.",
      "Enter a branch pattern, e.g. pf/*, or turn preflight watching off.",
    ]);
    await type("#wizard-ref", "trunk");
    await type("#wizard-preflight-ref", "pf/*");
    await next();
    expect(stepId()).toBe("deploy");

    await next();
    expect(stepId()).toBe("deploy");
    expect(fieldErrors()[0]).toMatch(/No deploy marker/);
    await click('input[name="marker-mode"][value="none"]');
    await next();
    expect(stepId()).toBe("preferences");
  });

  it("goes back without validating and keeps what was typed", async () => {
    await start(fakeApi());
    await toWatch();
    await click('[data-action="back"]');
    expect(stepId()).toBe("project");
    await click('[data-action="back"]');
    expect(stepId()).toBe("account");
    expect(q<HTMLInputElement>("#wizard-secret")?.value).toBe("pasted-secret-value");
    expect(q<HTMLButtonElement>('[data-action="back"]')?.disabled).toBe(true);
  });

  it("is driven by native buttons, and the stepper jumps back to a completed step", async () => {
    await start(fakeApi());
    await toWatch();
    for (const action of ["skip", "back", "next"]) {
      expect(q(`[data-action="${action}"]`)?.tagName).toBe("BUTTON");
    }
    expect(q('[data-action="next"]')?.getAttribute("type")).toBe("submit");
    const done = [...host.querySelectorAll<HTMLButtonElement>('[data-slot="stepper"] button')];
    expect(done.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Account, completed. Go back to this step",
      "Project, completed. Go back to this step",
    ]);
    done[0].click();
    await settle();
    expect(stepId()).toBe("account");
    expect(document.activeElement?.id).toBe("wizard-heading");
  });
});

describe("skip", () => {
  it("writes nothing from any step", async () => {
    const api = fakeApi();
    const { onSkip, onFinish } = await start(api);
    await toWatch();
    await click('[data-action="skip"]');
    expect(api.skip).toHaveBeenCalledTimes(1);
    expect(onSkip).toHaveBeenCalledTimes(1);
    expect(api.save).not.toHaveBeenCalled();
    expect(api.previewConfig).not.toHaveBeenCalled();
    expect(onFinish).not.toHaveBeenCalled();
  });

  it("from the review step too, after the preview was built", async () => {
    const api = fakeApi();
    const { onSkip } = await start(api);
    await toReview();
    expect(api.previewConfig).toHaveBeenCalledTimes(1);
    await click('[data-action="skip"]');
    expect(onSkip).toHaveBeenCalledTimes(1);
    expect(api.save).not.toHaveBeenCalled();
  });
});

describe("command token source", () => {
  async function commandMode(api: FakeApi) {
    await start(api);
    await click('input[name="token-mode"][value="command"]');
    await type("#wizard-command", "pass show 'gitlab/my pat'");
  }

  it("shows the exact argv and runs nothing until the user confirms", async () => {
    const api = fakeApi();
    await commandMode(api);
    await click('[data-action="test-connection"]');
    expect(q('[data-slot="confirm-dialog"]')).not.toBeNull();
    const args = [...document.body.querySelectorAll('[aria-label="command and arguments"] li')].map((li) =>
      li.textContent?.replace(/^(run|arg) /, ""),
    );
    expect(args).toEqual(["pass", "show", "gitlab/my pat"]);
    expect(api.testConnection).not.toHaveBeenCalled();

    await click('[data-slot="confirm-dialog"] [data-action="cancel"]');
    expect(q('[data-slot="confirm-dialog"]')).toBeNull();
    expect(api.testConnection).not.toHaveBeenCalled();

    await click('[data-action="test-connection"]');
    await click('[data-slot="confirm-dialog"] [data-action="confirm"]');
    expect(api.testConnection).toHaveBeenCalledTimes(1);
    expect(api.testConnection.mock.calls[0][0]).toEqual({
      base_url: "https://gitlab.com",
      token: { command: ["pass", "show", "gitlab/my pat"] },
    });
  });

  it("Next asks too; once confirmed it is not asked again, and editing the command asks again", async () => {
    const api = fakeApi();
    await commandMode(api);
    await next();
    expect(stepId()).toBe("account");
    expect(q('[data-slot="confirm-dialog"]')).not.toBeNull();
    await click('[data-slot="confirm-dialog"] [data-action="confirm"]');
    await settle();
    expect(stepId()).toBe("project");
    // listProjects ran with the acknowledged command and no second dialog.
    expect(q('[data-slot="confirm-dialog"]')).toBeNull();
    expect(api.listProjects).toHaveBeenCalledTimes(1);

    await click('[data-action="back"]');
    await type("#wizard-command", "pass show other");
    await click('[data-action="test-connection"]');
    expect(q('[data-slot="confirm-dialog"]')).not.toBeNull();
    expect(api.testConnection).not.toHaveBeenCalled();
  });

  it("repeats the request with the id when the shell answers NeedsConfirm", async () => {
    const api = fakeApi({
      testConnection: vi
        .fn()
        .mockResolvedValueOnce({ confirm: { id: "c-1", changes: ["Run pass show gitlab/my pat to read the token."] } })
        .mockResolvedValueOnce(IDENTITY),
    });
    await commandMode(api);
    await click('[data-action="test-connection"]');
    await click('[data-slot="confirm-dialog"] [data-action="confirm"]'); // ours
    expect(text('[data-slot="confirm-dialog"]')).toContain("Run pass show gitlab/my pat to read the token.");
    await click('[data-slot="confirm-dialog"] [data-action="confirm"]'); // the shell's
    const calls = api.testConnection.mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][0].confirm).toBeUndefined();
    expect(calls[1][0].confirm).toBe("c-1");
    expect(text('[data-slot="identity"]')).toContain("@alice");
  });
});

describe("test connection", () => {
  it("names the identity on success", async () => {
    const api = fakeApi();
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0]).toEqual({
      base_url: "https://gitlab.com",
      token: { own: true },
      secret: "pasted-secret-value",
    });
    const shown = text('[data-slot="identity"]');
    expect(shown).toContain("@alice");
    expect(shown).toContain("Alice Doe");
    expect(shown).toContain("personal token");
    expect(q('[data-slot="test-error"]')).toBeNull();
  });

  it("shows an auth error, and never the token", async () => {
    const api = fakeApi({
      testConnection: vi.fn(async () => {
        throw new Error("GitLab rejected the token (401). Check it has not expired or been revoked.");
      }),
    });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    expect(text('[data-slot="test-error"]')).toContain("rejected the token (401)");
    expect(q('[data-slot="identity"]')).toBeNull();
    expect(document.body.textContent).not.toContain("pasted-secret-value");
  });

  it("warns about a project token, and forgets the result when the token changes", async () => {
    const api = fakeApi({ testConnection: vi.fn(async () => PROJECT_TOKEN) });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    const shown = text('[data-slot="identity"]');
    expect(shown).toContain("project token");
    expect(shown).toContain("A project token cannot list projects");
    expect(shown).toContain("it can see project 42 only");

    await type("#wizard-secret", "a-different-token");
    expect(q('[data-slot="identity"]')).toBeNull();
  });

  it("offers glab's token when detection finds it, and uses its keyring source", async () => {
    const source = { keyring: { service: "glab:gitlab.com", user: "" } };
    const api = fakeApi({ detectGlab: vi.fn(async () => ({ status: "found", source })) });
    await start(api);
    expect(q<HTMLInputElement>('input[name="token-mode"][value="glab"]')?.checked).toBe(true);
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0]).toEqual({ base_url: "https://gitlab.com", token: source });
  });
});

describe("project step", () => {
  it("picks from the token's list and pre-fills the default branch and watch id", async () => {
    const api = fakeApi();
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    expect(api.listProjects).toHaveBeenCalledTimes(1);
    const paths = [...host.querySelectorAll('input[name="project"][type="radio"]')].map((r) =>
      r.closest("label")?.textContent?.replace(/\s+/g, " ").trim(),
    );
    expect(paths).toEqual(["acme/web trunk", "acme/docs main"]);
    await click('input[name="project"][value="42"]');
    expect(text('[data-slot="chosen-project"]')).toContain("acme/web");
    await next();
    expect(api.resolveProject).not.toHaveBeenCalled();
    expect(q<HTMLInputElement>("#wizard-ref")?.value).toBe("trunk");
    expect(q<HTMLInputElement>("#wizard-watch-id")?.value).toBe("web-trunk");
  });

  it("with a project token, types the id: the reason is shown, the suggestion pre-filled, Next resolves it", async () => {
    const api = fakeApi({
      listProjects: vi.fn(async () => ({
        mode: "type_id_or_path",
        reason: "A project token cannot list projects; type the project's id or path.",
        suggestion: 7,
      })),
    });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    expect(text('[data-slot="type-reason"]')).toContain("cannot list projects");
    expect(host.querySelectorAll('input[name="project"][type="radio"]')).toHaveLength(0);
    expect(q<HTMLInputElement>("#wizard-project-input")?.value).toBe("7");
    await next();
    expect(api.resolveProject).toHaveBeenCalledTimes(1);
    expect(api.resolveProject.mock.calls[0][1]).toBe("7");
    expect(stepId()).toBe("watch");
    expect(q<HTMLInputElement>("#wizard-ref")?.value).toBe("develop");
  });

  it("a typed path that does not resolve stays on the step with the reason", async () => {
    const api = fakeApi({
      resolveProject: vi.fn(async () => {
        throw new Error("No project acme/nope, or this token cannot see it.");
      }),
    });
    await start(api);
    await type("#wizard-secret", "pasted-secret-value");
    await next();
    await type("#wizard-project-input", "acme/nope");
    await click('[data-action="resolve-project"]');
    expect(stepId()).toBe("project");
    expect(fieldErrors()).toEqual(["No project acme/nope, or this token cannot see it."]);
  });
});

describe("deploy detection", () => {
  it("lists suggestions with their reasons and the one-line explanation", async () => {
    const api = fakeApi();
    await start(api);
    await toWatch();
    await next();
    expect(api.suggestDeployMarkers.mock.calls[0].slice(1)).toEqual([42, "trunk"]);
    expect(text('[data-slot="marker-explainer"]')).toMatch(/job whose success means/);
    expect(text('[data-marker="deploy:prod"] [data-slot="marker-reason"]')).toBe(
      "the name says deploy; it runs in the last stage",
    );
    await click('[data-marker="deploy:prod"] button[role="checkbox"]');
    await next();
    expect(stepId()).toBe("preferences");
  });
});

describe("review and finish", () => {
  it("shows the TOML preview built from the answers", async () => {
    const api = fakeApi();
    await start(api);
    await toReview();
    expect(stepId()).toBe("review");
    expect(q('[data-slot="toml-preview"]')?.textContent).toBe(PREVIEW.toml);
    const answers = api.previewConfig.mock.calls[0][0];
    expect(answers).toMatchObject({
      account: "gitlab",
      base_url: "https://gitlab.com",
      token: { own: true },
      project: 42,
      watch_id: "web-trunk",
      ref_name: "trunk",
      deploy_markers: [],
    });
    expect(JSON.stringify(answers)).not.toContain("pasted-secret-value");
    expect(text('[data-slot="token-note"]')).toContain("bridgewatch's own keychain entry");
  });

  it("Finish calls save with the answers, the pasted token only as an option, then reports the path", async () => {
    const api = fakeApi();
    const { onFinish, onSkip } = await start(api);
    await toReview();
    await click('[data-action="finish"]');
    expect(api.save).toHaveBeenCalledTimes(1);
    const [answers, options] = api.save.mock.calls[0];
    expect(answers).toEqual(api.previewConfig.mock.calls[0][0]);
    expect(options).toEqual({ secret: "pasted-secret-value" });
    expect(onFinish).toHaveBeenCalledWith({ path: SAVED.path });
    expect(onSkip).not.toHaveBeenCalled();
    expect(api.skip).not.toHaveBeenCalled();
  });

  it("a save the shell parks for confirmation is repeated with the id after the user agrees", async () => {
    const api = fakeApi({
      save: vi
        .fn()
        .mockResolvedValueOnce({ ok: true, diagnostics: [], confirm: { id: "s-1", changes: ["Store the token in the keychain."] } })
        .mockResolvedValueOnce(SAVED),
    });
    const { onFinish } = await start(api);
    await toReview();
    await click('[data-action="finish"]');
    expect(onFinish).not.toHaveBeenCalled();
    expect(text('[data-slot="confirm-dialog"]')).toContain("Store the token in the keychain.");
    await click('[data-slot="confirm-dialog"] [data-action="confirm"]');
    const calls = api.save.mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1][1]).toEqual({ secret: "pasted-secret-value", confirm: "s-1" });
    expect(onFinish).toHaveBeenCalledTimes(1);
  });

  it("issues the core reports send the user to the step at fault", async () => {
    const api = fakeApi({
      save: vi.fn(async () => ({
        ok: false,
        diagnostics: [],
        issues: [{ step: "watch", field: "ref_name", message: "unclosed [ in branch pattern" }],
      })),
    });
    const { onFinish } = await start(api);
    await toReview();
    await click('[data-action="finish"]');
    expect(stepId()).toBe("watch");
    expect(fieldErrors()).toEqual(["unclosed [ in branch pattern"]);
    expect(onFinish).not.toHaveBeenCalled();
  });

  it("a WizardFailure of kind answers, from the preview, lands on the step it names", async () => {
    const failure: WizardFailure = {
      kind: "answers",
      message: "some answers need fixing",
      issues: [{ step: "preferences", field: "live_secs", message: "the live poll interval is between 1 and 60 seconds" }],
      diagnostics: [],
    };
    const api = fakeApi({ previewConfig: vi.fn(async () => Promise.reject(failure)) });
    await start(api);
    await toReview();
    expect(stepId()).toBe("preferences");
    expect(fieldErrors()).toEqual(["the live poll interval is between 1 and 60 seconds"]);
    expect(document.activeElement?.id).toBe("wizard-live-secs");
  });

  it("a WizardFailure of kind invalid, from save, lists its diagnostics on review", async () => {
    const failure: WizardFailure = {
      kind: "invalid",
      message: "the config would not load",
      issues: [],
      diagnostics: [{ severity: "error", path: "watches.0.ref", message: "unclosed [", span: { start: 3, end: 9 } }],
    };
    const api = fakeApi({ save: vi.fn(async () => Promise.reject(failure)) });
    const { onFinish } = await start(api);
    await toReview();
    await click('[data-action="finish"]');
    expect(stepId()).toBe("review");
    const shown = text('[data-slot="save-error"]');
    expect(shown).toContain("the config would not load");
    expect(shown).toContain("watches.0.ref: unclosed [");
    expect(onFinish).not.toHaveBeenCalled();
  });

  it("a refused save stays on review with the diagnostics", async () => {
    const api = fakeApi({
      save: vi.fn(async () => ({
        ok: false,
        diagnostics: [{ severity: "error", path: "watches[0].ref", message: "bad ref", line: 9, col: 1 }],
      })),
    });
    const { onFinish } = await start(api);
    await toReview();
    await click('[data-action="finish"]');
    expect(stepId()).toBe("review");
    expect(text('[data-slot="save-error"]')).toContain("line 9: bad ref");
    expect(onFinish).not.toHaveBeenCalled();
  });
});

describe("re-running on an existing config", () => {
  it("pre-fills from the answers and keeps a stored own token without a new paste", async () => {
    const api = fakeApi();
    await start(api, {
      initial: {
        account: "work",
        base_url: "https://gitlab.example.com",
        token: { own: true },
        project: 42,
        ref_name: "main",
        watch_id: "web-main",
      },
    });
    expect(q<HTMLInputElement>("#wizard-base-url")?.value).toBe("https://gitlab.example.com");
    expect(q<HTMLInputElement>("#wizard-account")?.value).toBe("work");
    await next();
    expect(stepId()).toBe("project");
    await next();
    expect(stepId()).toBe("watch");
    expect(q<HTMLInputElement>("#wizard-watch-id")?.value).toBe("web-main");
  });
});
