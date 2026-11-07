import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { OAuthApi, OAuthAvailability } from "../../lib/oauth";
import type { Identity, MarkerSuggestions, RecentListing, RecentProject, WizardApi } from "./api";
import { activeLabel, answersOf, emptyDraft } from "./model";
import Wizard from "./Wizard.svelte";

/**
 * One-click setup: after a sign-in or a passed Test connection, the account's
 * recent projects with the top three ticked, and one button that turns the
 * ticked ones into watches written at Finish. Every request is the fake's,
 * and the fixtures are written by hand in the shapes the core serialises
 * (`RecentListing`, `RecentProject`), never pasted from a real answer.
 */

const NOW = Date.parse("2026-11-07T12:00:00Z");
const hoursAgo = (h: number) => new Date(NOW - h * 3_600_000).toISOString();

const IDENTITY: Identity = {
  username: "alice",
  name: null,
  bot: false,
  token: { kind: "personal" },
  scopes: ["read_api"],
  expires_at: null,
  warnings: [],
};

function row(id: number, ticked: boolean, active: string | null, branch: string | null = "main"): RecentProject {
  return {
    id,
    path: `acme/p${id}`,
    name: `p${id}`,
    default_branch: branch,
    web_url: null,
    last_activity_at: active,
    ticked,
  };
}

/** Ten rows, newest first, as `rank_recent` answers: the top three ticked, row 10 with no timestamp. */
const TEN: RecentListing = {
  mode: "projects",
  listed_by: { provider: "gitlab", origin: "https://gitlab.com" },
  projects: Array.from({ length: 10 }, (_, i) => row(i + 1, i < 3, i === 9 ? null : hoursAgo(i + 1))),
};

const EMPTY: RecentListing = { mode: "projects", listed_by: { provider: "gitlab", origin: "https://gitlab.com" }, projects: [] };

const MARKERS = (name: string | null): MarkerSuggestions => ({
  pipeline_id: 1,
  pipeline_url: null,
  suggestions: name
    ? [{ name, stage: "deploy", pipeline: "parent", status: "success", score: 9, reasons: [] }]
    : [],
  unread_children: [],
});

type Api = Required<WizardApi>;
type FakeApi = { [K in keyof Api]: Mock<Api[K]> };

function fakeApi(listing: RecentListing = TEN): FakeApi {
  return {
    detectCliToken: vi.fn<Api["detectCliToken"]>(async () => ({ status: "not_found", service: "glab:gitlab.com:token" })),
    ownTokenStatus: vi.fn<Api["ownTokenStatus"]>(async () => ({ kind: "none" })),
    testConnection: vi.fn<Api["testConnection"]>(async () => IDENTITY),
    recentProjects: vi.fn<Api["recentProjects"]>(async () => listing),
    listProjects: vi.fn<Api["listProjects"]>(async () => ({ mode: "projects", truncated: false, projects: [] })),
    resolveProject: vi.fn<Api["resolveProject"]>(async () => ({ id: 1, path: "acme/p1", default_branch: "main", web_url: null })),
    // p1 has a deploy job, p2 none; any other project's read fails.
    suggestDeployMarkers: vi.fn<Api["suggestDeployMarkers"]>(async (_c, project) => {
      if (project === 1) return MARKERS("deploy:production");
      if (project === 2) return MARKERS(null);
      throw { kind: "network", message: "timed out", issues: [], diagnostics: [] };
    }),
    previewConfig: vi.fn<Api["previewConfig"]>(async () => ({
      toml: "[accounts.gitlab]\n",
      warnings: [],
      edited_existing: false,
      added: ["p1-main", "p2-main"],
    })),
    save: vi.fn<Api["save"]>(async () => ({ ok: true, diagnostics: [], path: "config.toml" })),
    skip: vi.fn<Api["skip"]>(async () => {}),
  };
}

const BUILT_IN: OAuthAvailability = {
  available: true,
  builtin: true,
  needs_client_id: false,
  host: "gitlab.com",
  install_url: null,
};

function fakeOAuth(): { [K in keyof Required<OAuthApi>]: Mock } {
  return {
    availability: vi.fn(async () => BUILT_IN),
    start: vi.fn(async () => ({
      id: "flow-1",
      user_code: "ABCD-EFGH",
      verification_uri: "https://gitlab.com/oauth/device",
      expires_in: 900,
      host: "gitlab.com",
    })),
    wait: vi.fn(async () => ({ login: "alice", scopes: [], expires_at: null })),
    cancel: vi.fn(async () => {}),
    openVerification: vi.fn(async () => {}),
    openInstall: vi.fn(async () => {}),
    status: vi.fn(async () => null),
    signOut: vi.fn(async () => {}),
    copy: vi.fn(async () => true),
    onProgress: vi.fn(async () => () => {}),
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
  for (let i = 0; i < 8; i++) {
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
  }
  await tick();
}

async function start(api: WizardApi, oauth?: OAuthApi) {
  const onFinish = vi.fn();
  component = mount(Wizard, { target: host, props: { api, oauth, onFinish, now: () => NOW } });
  await settle();
  return onFinish;
}

const q = <T extends Element = HTMLElement>(sel: string) => document.body.querySelector<T>(sel);
const all = <T extends Element = HTMLElement>(sel: string) => [...document.body.querySelectorAll<T>(sel)];
const text = (sel: string) => q(sel)?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const stepId = () => q<HTMLElement>('[data-slot="wizard"]')?.dataset.step;

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

const rows = () => all('[data-slot="quick-row"]');
const checkedIds = () =>
  rows()
    .filter((r) => r.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked)
    .map((r) => Number(r.dataset.id));
const box = (id: number) => q<HTMLInputElement>(`[data-slot="quick-row"][data-id="${id}"] input[type="checkbox"]`)!;

/** The token path: paste, Test connection. */
async function pasteAndTest(api: FakeApi) {
  const onFinish = await start(api);
  await type("#wizard-secret", "pasted-secret-value");
  await click('[data-action="test-connection"]');
  return onFinish;
}

describe("the one-click list", () => {
  it("is offered once Test connection succeeds on a token: ten rows, the top three ticked", async () => {
    const api = fakeApi();
    await start(api);
    expect(q('[data-slot="quick-setup"]')).toBeNull();
    expect(api.recentProjects).not.toHaveBeenCalled();

    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    expect(api.recentProjects).toHaveBeenCalledTimes(1);
    expect(api.recentProjects.mock.calls[0][0]).toMatchObject({ base_url: "https://gitlab.com", secret: "pasted-secret-value" });
    expect(rows().map((r) => Number(r.dataset.id))).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(checkedIds()).toEqual([1, 2, 3]);
    expect(text('[data-action="quick-setup"]')).toBe("Watch 3 projects");
  });

  it("is offered right after a sign-in, with no Test connection", async () => {
    const api = fakeApi();
    await start(api, fakeOAuth());
    await click('[data-action="oauth-sign-in"]');
    expect(text('[data-slot="oauth-signed-in"]')).toBe("Signed in as @alice.");
    expect(api.recentProjects).toHaveBeenCalledTimes(1);
    expect(api.recentProjects.mock.calls[0][0]).toMatchObject({ token: { oauth: true }, account: "gitlab" });
    expect(checkedIds()).toEqual([1, 2, 3]);
  });

  it("labels a row 'active … ago' from its timestamp, and shows nothing where there is none", async () => {
    await pasteAndTest(fakeApi());
    const meta = (id: number) => text(`[data-slot="quick-row"][data-id="${id}"] [data-slot="quick-meta"]`);
    expect(meta(1)).toBe("main · active 1h ago");
    expect(meta(9)).toBe("main · active 9h ago");
    expect(meta(10)).toBe("main");
    expect(q('[data-slot="quick-row"][data-id="10"] [data-slot="quick-active"]')).toBeNull();
    expect(activeLabel({ last_activity_at: null }, NOW)).toBe("");
    expect(activeLabel({ last_activity_at: undefined }, NOW)).toBe("");
    expect(activeLabel({ last_activity_at: "garbage" }, NOW)).toBe("");
  });

  it("asks once per account: a second Test connection sends no list call, another token does", async () => {
    const api = fakeApi();
    await pasteAndTest(api);
    await click('[data-action="test-connection"]');
    expect(api.testConnection).toHaveBeenCalledTimes(2);
    expect(api.recentProjects).toHaveBeenCalledTimes(1);

    await type("#wizard-secret", "another-token");
    expect(q('[data-slot="quick-setup"]')).toBeNull();
    await click('[data-action="test-connection"]');
    expect(api.recentProjects).toHaveBeenCalledTimes(2);
  });

  it("says so for an account with no projects, and offers no button", async () => {
    await pasteAndTest(fakeApi(EMPTY));
    expect(text('[data-slot="quick-empty"]')).toContain("This account has no projects yet");
    expect(rows()).toHaveLength(0);
    expect(q('[data-action="quick-setup"]')).toBeNull();
  });

  it("names a repository on GitHub, and gives an unlisted token its reason", async () => {
    const api = fakeApi({ mode: "unlisted", reason: "This token can read only its own repository." });
    await start(api);
    await click('input[name="provider"][value="github"]');
    await type("#wizard-secret", "pasted-secret-value");
    await click('[data-action="test-connection"]');
    expect(text("#wizard-quick-heading")).toBe("Watch your recent repositories");
    expect(text('[data-slot="quick-unlisted"]')).toBe("This token can read only its own repository.");
  });

  it("never ticks or offers an empty repository", async () => {
    const listing: RecentListing = { ...TEN, projects: [row(1, true, hoursAgo(1)), row(2, false, hoursAgo(2), null)] };
    await pasteAndTest(fakeApi(listing));
    expect(box(2).disabled).toBe(true);
    expect(text('[data-slot="quick-row"][data-id="2"] [data-slot="quick-meta"]')).toBe("empty · active 2h ago");
    expect(text('[data-action="quick-setup"]')).toBe("Watch 1 project");
  });
});

describe("Watch N projects", () => {
  it("adds every ticked row, with its best deploy marker, in one save at Finish", async () => {
    const api = fakeApi();
    const onFinish = await pasteAndTest(api);

    // Untick p3 and tick p4: p4's deploy read fails, so it gets no marker.
    box(3).click();
    await settle();
    expect(checkedIds()).toEqual([1, 2]);
    expect(text('[data-action="quick-setup"]')).toBe("Watch 2 projects");
    box(4).click();
    await settle();
    await click('[data-action="quick-setup"]');

    expect(api.suggestDeployMarkers.mock.calls.map((c) => [c[1], c[2]])).toEqual([
      [1, "main"],
      [2, "main"],
      [4, "main"],
    ]);
    expect(api.listProjects).not.toHaveBeenCalled();
    expect(stepId()).toBe("preferences");

    await click('[data-action="next"]');
    expect(stepId()).toBe("review");
    const listed_by = { provider: "gitlab", origin: "https://gitlab.com" };
    const watches = [
      { project: 1, path: "acme/p1", ref_name: "main", deploy_markers: ["deploy:production"], listed_by },
      { project: 2, path: "acme/p2", ref_name: "main", deploy_markers: [], listed_by },
      { project: 4, path: "acme/p4", ref_name: "main", deploy_markers: [], listed_by },
    ];
    expect(api.previewConfig.mock.calls.at(-1)![0].watches).toEqual(watches);
    expect(text('[data-slot="quick-added"]')).toContain("Adds 2 watches: p1-main, p2-main.");

    await click('[data-action="finish"]');
    expect(api.save).toHaveBeenCalledTimes(1);
    const [answers, options] = api.save.mock.calls[0];
    expect(answers.watches).toEqual(watches);
    expect(answers.account).toBe("gitlab");
    expect(options.secret).toBe("pasted-secret-value");
    expect(JSON.stringify(answers)).not.toContain("pasted-secret-value");
    expect(onFinish).toHaveBeenCalledTimes(1);
  });

  it("skips the project, watch and deploy steps both ways, and Next on the account step is the step-by-step path", async () => {
    const api = fakeApi();
    await pasteAndTest(api);
    await click('[data-action="quick-setup"]');
    expect(stepId()).toBe("preferences");
    await click('[data-action="back"]');
    expect(stepId()).toBe("account");

    await click('[data-action="next"]');
    expect(stepId()).toBe("project");
    await click('[data-action="next"]');
    // The one-click choice is gone: the project step asks for its project.
    expect(stepId()).toBe("project");
  });

  it("re-shows a binding refusal from the core on the account step, under the list", async () => {
    const api = fakeApi();
    api.save.mockResolvedValueOnce({
      ok: false,
      diagnostics: [],
      issues: [
        {
          step: "account",
          field: "watches",
          message: "acme/p1 was listed by gitlab on https://gitlab.com, not by this account; list the projects again",
        },
      ],
    });
    await pasteAndTest(api);
    await click('[data-action="quick-setup"]');
    await click('[data-action="next"]');
    await click('[data-action="finish"]');
    expect(stepId()).toBe("account");
    expect(text("#wizard-quick-msg")).toContain("not by this account");
  });

  it("sends no watches in a step-by-step payload", () => {
    expect(answersOf(emptyDraft())).not.toHaveProperty("watches");
    expect(answersOf({ ...emptyDraft(), quick: [] })).not.toHaveProperty("watches");
  });
});
