import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { OAuthApi, OAuthAvailability, SignedIn, StartedSignIn } from "../../lib/oauth";
import type { Identity, WizardApi } from "./api";
import Wizard from "./Wizard.svelte";

/**
 * "Sign in with GitHub / GitLab" on the wizard's account step, against a fake
 * `OAuthApi`. Nothing here reaches a provider or a keychain: every answer is
 * the fake's, and "signed in" means the fake's `wait` resolved.
 */

const IDENTITY: Identity = {
  username: "octocat",
  name: null,
  bot: false,
  token: { kind: "unknown" },
  scopes: [],
  expires_at: null,
  warnings: [],
};

const OFF: OAuthAvailability = {
  available: false,
  builtin: false,
  needs_client_id: false,
  host: "github.com",
  install_url: null,
};
const BUILT_IN: OAuthAvailability = {
  available: true,
  builtin: true,
  needs_client_id: false,
  host: "github.com",
  install_url: "https://github.com/apps/bridgewatch-ci/installations/new",
};
const STARTED: StartedSignIn = {
  id: "flow-1",
  user_code: "WDJB-MJHT",
  verification_uri: "https://github.com/login/device",
  expires_in: 900,
  host: "github.com",
};
const SIGNED: SignedIn = { login: "octocat", scopes: [], expires_at: 1_800_000_000 };

type FakeOAuth = { [K in keyof Required<OAuthApi>]: Mock };

function fakeOAuth(availability: (clientId: string | null | undefined) => OAuthAvailability = () => OFF): FakeOAuth {
  return {
    availability: vi.fn(async (_p: string, _b: string, clientId?: string | null) => availability(clientId)),
    start: vi.fn(async () => STARTED),
    wait: vi.fn(async () => SIGNED),
    cancel: vi.fn(async () => {}),
    openVerification: vi.fn(async () => {}),
    openInstall: vi.fn(async () => {}),
    status: vi.fn(async () => null),
    signOut: vi.fn(async () => {}),
    copy: vi.fn(async () => true),
    onProgress: vi.fn(async () => () => {}),
  };
}

function fakeApi(): { [K in keyof Required<WizardApi>]: Mock } {
  return {
    detectCliToken: vi.fn(async () => ({ status: "not_found", service: "gh:github.com" })),
    testConnection: vi.fn(async () => IDENTITY),
    listProjects: vi.fn(async () => ({ mode: "projects", truncated: false, projects: [] })),
    resolveProject: vi.fn(async () => ({ id: 1, path: "acme/web", project: "acme/web", default_branch: "main", web_url: null })),
    suggestDeployMarkers: vi.fn(async () => ({ pipeline_id: null, pipeline_url: null, suggestions: [], unread_children: [] })),
    previewConfig: vi.fn(async () => ({ toml: "", warnings: [], edited_existing: false })),
    save: vi.fn(async () => ({ ok: true, diagnostics: [], path: "/tmp/config.toml" })),
    skip: vi.fn(async () => {}),
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

async function start(api: WizardApi, oauth?: OAuthApi, extra: Record<string, unknown> = {}) {
  component = mount(Wizard, { target: host, props: { api, oauth, ...extra } });
  await settle();
}

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
const tokenModes = () =>
  [...document.body.querySelectorAll<HTMLInputElement>('input[name="token-mode"]')].map((r) => r.value);
const checkedMode = () => q<HTMLInputElement>('input[name="token-mode"]:checked')?.value;
const stepId = () => q<HTMLElement>('[data-slot="wizard"]')?.dataset.step;

async function github(api: WizardApi, oauth?: OAuthApi) {
  await start(api, oauth);
  await click('input[name="provider"][value="github"]');
}

describe("sign-in, while this build has no built-in application", () => {
  it("is not offered, and the other token options are exactly as before", async () => {
    const oauth = fakeOAuth();
    await github(fakeApi(), oauth);
    expect(tokenModes()).toEqual(["paste", "env", "command"]);
    expect(oauth.availability).toHaveBeenLastCalledWith("github", "https://api.github.com", null);
  });

  it("is offered, first, once the user types a client id of their own", async () => {
    const oauth = fakeOAuth((clientId) => (clientId ? { ...OFF, available: true } : OFF));
    await github(fakeApi(), oauth);
    await type("#wizard-oauth-client-id", "Iv23liMINE");
    expect(oauth.availability).toHaveBeenLastCalledWith("github", "https://api.github.com", "Iv23liMINE");
    expect(tokenModes()).toEqual(["oauth", "paste", "env", "command"]);
    expect(text('label:has(input[value="oauth"])')).toContain("Sign in with GitHub");
    expect(text('label:has(input[value="oauth"])')).toContain("Recommended");
  });

  it("is not offered at all without the shell's sign-in commands (a browser, or an older shell)", async () => {
    await github(fakeApi());
    expect(tokenModes()).toEqual(["paste", "env", "command"]);
    expect(q("#wizard-oauth-client-id")).toBe(null);
  });
});

describe("sign-in with a built-in application", () => {
  it("comes first, recommended and chosen, beside the unchanged options", async () => {
    await github(fakeApi(), fakeOAuth(() => BUILT_IN));
    expect(tokenModes()).toEqual(["oauth", "paste", "env", "command"]);
    expect(checkedMode()).toBe("oauth");
    expect(text('[data-action="oauth-sign-in"]')).toBe("Sign in with GitHub");
  });

  it("puts the sign-in button right under its option, before the alternatives, under the Access legend", async () => {
    await github(fakeApi(), fakeOAuth(() => BUILT_IN));
    const legend = [...document.body.querySelectorAll("legend")].map((l) => l.textContent?.trim());
    expect(legend).toContain("Access");
    expect(legend).not.toContain("Token");
    const order = [
      ...document.body.querySelectorAll<HTMLElement>('input[name="token-mode"], [data-action="oauth-sign-in"]'),
    ].map((el) => (el instanceof HTMLInputElement ? el.value : "BUTTON"));
    expect(order).toEqual(["oauth", "BUTTON", "paste", "env", "command"]);
  });

  it("says what a token needs only while a token is chosen, never for sign-in", async () => {
    await github(fakeApi(), fakeOAuth(() => BUILT_IN));
    expect(q('[data-slot="token-needs"]')).toBeNull();
    const fieldset = q('fieldset[aria-describedby="wizard-token-msg"]');
    expect(fieldset).not.toBeNull();
    for (const mode of ["paste", "env", "command"]) {
      await click(`input[name="token-mode"][value="${mode}"]`);
      expect(text('[data-slot="token-needs"]')).toContain("Actions: read");
    }
    await click('input[name="token-mode"][value="oauth"]');
    expect(q('[data-slot="token-needs"]')).toBeNull();
  });

  it("shows the code large with copy and open, waits, then names who signed in", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    let approve: (s: SignedIn) => void = () => {};
    oauth.wait.mockImplementation(() => new Promise<SignedIn>((resolve) => (approve = resolve)));
    const api = fakeApi();
    await github(api, oauth);

    await click('[data-action="oauth-sign-in"]');
    expect(oauth.start).toHaveBeenCalledWith({
      account: "github",
      provider: "github",
      base_url: "https://api.github.com",
      client_id: null,
    });
    expect(text('[data-slot="user-code"]')).toBe("WDJB-MJHT");
    expect(text('[data-action="open-verification"]')).toBe("Open github.com/login/device");
    expect(text('[data-slot="sign-in-waiting"]')).toContain("Waiting for you to approve it on github.com");

    await click('[data-action="copy-code"]');
    expect(oauth.copy).toHaveBeenCalledWith("WDJB-MJHT");
    expect(text('[data-action="copy-code"]')).toBe("Copied");
    await click('[data-action="open-verification"]');
    expect(oauth.openVerification).toHaveBeenCalledWith("flow-1");

    approve(SIGNED);
    await settle();
    expect(text('[data-slot="oauth-signed-in"]')).toBe("Signed in as @octocat.");
    expect(q('[data-slot="user-code"]')).toBe(null);

    // The live steps carry the sign-in's source and the account it is kept under.
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0]).toMatchObject({
      provider: "github",
      token: { oauth: true },
      account: "github",
    });
    expect(api.testConnection.mock.calls[0][0]).not.toHaveProperty("secret");
  });

  it("will not leave the step before signing in, and says why", async () => {
    await github(fakeApi(), fakeOAuth(() => BUILT_IN));
    await click('[data-action="next"]');
    expect(stepId()).toBe("account");
    expect(text('[data-slot="field-error"]')).toBe("Sign in with GitHub first, or choose another source.");
  });

  it("says an expired code plainly, and starts again", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    oauth.wait.mockRejectedValueOnce({ kind: "expired", message: "the code expired before it was entered" });
    await github(fakeApi(), oauth);
    await click('[data-action="oauth-sign-in"]');
    expect(text('[data-slot="sign-in-error"]')).toBe("The code expired before it was entered. Start again.");
    expect(text('[data-action="oauth-sign-in"]')).toBe("Start again");
    await click('[data-action="oauth-sign-in"]');
    expect(oauth.start).toHaveBeenCalledTimes(2);
    expect(text('[data-slot="oauth-signed-in"]')).toBe("Signed in as @octocat.");
  });

  it("says a refusal on the provider's page plainly", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    oauth.wait.mockRejectedValueOnce({ kind: "denied", message: "the sign-in was cancelled on github.com" });
    await github(fakeApi(), oauth);
    await click('[data-action="oauth-sign-in"]');
    expect(text('[data-slot="sign-in-error"]')).toBe("You cancelled the sign-in on github.com.");
  });

  it("stops the shell's wait when the user cancels", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    oauth.wait.mockImplementation(() => new Promise<SignedIn>(() => {}));
    await github(fakeApi(), oauth);
    await click('[data-action="oauth-sign-in"]');
    await click('[data-action="cancel-sign-in"]');
    expect(oauth.cancel).toHaveBeenCalledWith("flow-1");
    expect(q('[data-slot="user-code"]')).toBe(null);
    expect(text('[data-action="oauth-sign-in"]')).toBe("Sign in with GitHub");
  });

  it("on the project step, says a GitHub App sees only where it is installed, and links the install page", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    await github(fakeApi(), oauth);
    await click('[data-action="oauth-sign-in"]');
    await click('[data-action="next"]');
    expect(stepId()).toBe("project");
    expect(text('[data-slot="install-app"]')).toContain("sees only repositories on accounts it is installed on");
    await click('[data-action="install-app"]');
    expect(oauth.openInstall).toHaveBeenCalledWith("github", "https://api.github.com");
  });

  it("moves the sign-in to the account name that is saved, when it was renamed after signing in", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    const api = fakeApi();
    await github(api, oauth);
    await click('[data-action="oauth-sign-in"]');
    await type("#wizard-account", "work");
    await click('[data-action="next"]');
    await type("#wizard-project-input", "acme/web");
    await click('[data-action="resolve-project"]');
    await click('[data-action="next"]'); // watch
    await click('[data-action="next"]'); // deploy
    await click('input[name="marker-mode"][value="none"]');
    await click('[data-action="next"]'); // preferences
    await click('[data-action="next"]'); // review
    await click('[data-action="finish"]');
    expect(api.save).toHaveBeenCalledTimes(1);
    const [answers, options] = api.save.mock.calls[0];
    expect(answers).toMatchObject({ account: "work", token: { oauth: true } });
    expect(options).toEqual({ secret: undefined, oauthAccount: "github" });
  });

  it("a GitLab sign-in says GitLab, and a self-managed one writes the typed client id", async () => {
    const oauth = fakeOAuth((clientId) =>
      clientId ? { ...OFF, available: true, needs_client_id: true, host: "gitlab.example.com" } : { ...OFF, needs_client_id: true },
    );
    const api = fakeApi();
    await start(api, oauth);
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://gitlab.example.com");
    q<HTMLInputElement>("#wizard-base-url")!.dispatchEvent(new Event("blur"));
    await settle();
    expect(q<HTMLDetailsElement>('[data-slot="oauth-own-app"]')?.open).toBe(true);
    await type("#wizard-oauth-client-id", "gl-app-self");
    await click('input[name="token-mode"][value="oauth"]');
    expect(text('[data-action="oauth-sign-in"]')).toBe("Sign in with GitLab");
    await click('[data-action="oauth-sign-in"]');
    expect(oauth.start.mock.calls[0][0]).toMatchObject({
      provider: "gitlab",
      base_url: "https://gitlab.example.com",
      client_id: "gl-app-self",
    });
    await click('[data-action="test-connection"]');
    expect(api.testConnection.mock.calls[0][0].token).toEqual({ oauth: { client_id: "gl-app-self" } });
  });
});

describe("re-running on a config that already signs in", () => {
  it("keeps the stored sign-in without asking for a new one", async () => {
    const oauth = fakeOAuth(() => BUILT_IN);
    await start(fakeApi(), oauth, {
      initial: { provider: "github", account: "gh", base_url: "https://api.github.com", token: { oauth: true } },
    });
    expect(checkedMode()).toBe("oauth");
    expect(text('[data-slot="oauth-stored"]')).toContain("already signed in");
    expect(text('[data-action="oauth-sign-in"]')).toBe("Sign in again");
    await click('[data-action="next"]');
    expect(stepId()).toBe("project");
  });
});

describe("a fresh start where gh's token is found too", () => {
  const GH_FOUND = { status: "found", source: { keyring: { service: "gh:github.com", user: "" } } } as const;

  /** A promise the test resolves when it chooses, to order the two answers. */
  function later<T>() {
    let resolve: (value: T) => void = () => {};
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
  }

  /** Both answers held back, so the test decides which lands first. */
  function held(availability: OAuthAvailability) {
    const detection = later<typeof GH_FOUND>();
    const signIn = later<OAuthAvailability>();
    const api = fakeApi();
    api.detectCliToken.mockImplementation(() => detection.promise);
    const oauth = fakeOAuth();
    oauth.availability.mockImplementation(() => signIn.promise);
    return {
      api,
      oauth,
      detect: async () => {
        detection.resolve(GH_FOUND);
        await settle();
      },
      answer: async () => {
        signIn.resolve(availability);
        await settle();
      },
    };
  }

  const startGithub = (api: WizardApi, oauth: OAuthApi) => start(api, oauth, { initial: { provider: "github" } });

  it("preselects Sign in when detection answers first, with gh's token listed below it", async () => {
    const h = held(BUILT_IN);
    await startGithub(h.api, h.oauth);
    await h.detect();
    expect(checkedMode()).toBe("gh");
    await h.answer();
    expect(checkedMode()).toBe("oauth");
    expect(tokenModes()).toEqual(["oauth", "gh", "paste", "env", "command"]);
  });

  it("preselects Sign in when availability answers first, and detection does not take it back", async () => {
    const h = held(BUILT_IN);
    await startGithub(h.api, h.oauth);
    await h.answer();
    expect(checkedMode()).toBe("oauth");
    await h.detect();
    expect(checkedMode()).toBe("oauth");
    expect(tokenModes()).toEqual(["oauth", "gh", "paste", "env", "command"]);
  });

  it("keeps what the user picked before availability answered", async () => {
    const h = held(BUILT_IN);
    await startGithub(h.api, h.oauth);
    await h.detect();
    await click('input[name="token-mode"][value="paste"]');
    await h.answer();
    expect(checkedMode()).toBe("paste");
  });

  it("keeps gh's token when the user picked it, even though it was already preselected", async () => {
    const h = held(BUILT_IN);
    await startGithub(h.api, h.oauth);
    await h.detect();
    await click('input[name="token-mode"][value="env"]');
    await click('input[name="token-mode"][value="gh"]');
    await h.answer();
    expect(checkedMode()).toBe("gh");
  });

  it("preselects gh's token when there is no built-in sign-in, in either order", async () => {
    let h = held(OFF);
    await startGithub(h.api, h.oauth);
    await h.detect();
    await h.answer();
    expect(checkedMode()).toBe("gh");
    expect(tokenModes()).toEqual(["gh", "paste", "env", "command"]);
    if (component) void unmount(component);
    component = null;
    document.body.innerHTML = "";
    host = document.createElement("div");
    document.body.append(host);

    h = held(OFF);
    await startGithub(h.api, h.oauth);
    await h.answer();
    await h.detect();
    expect(checkedMode()).toBe("gh");
  });

  it("falls back to gh's token on a GitHub Enterprise host with no client id", async () => {
    const api = fakeApi();
    api.detectCliToken.mockImplementation(async (base: string) =>
      base === "https://api.github.com"
        ? GH_FOUND
        : { status: "found", source: { keyring: { service: "gh:ghe.acme.com", user: "" } } },
    );
    const oauth = fakeOAuth();
    oauth.availability.mockImplementation(async (_p: string, base: string) =>
      base === "https://api.github.com" ? BUILT_IN : { ...OFF, needs_client_id: true, host: "ghe.acme.com" },
    );
    await startGithub(api, oauth);
    expect(checkedMode()).toBe("oauth");
    await click('input[name="instance"][value="self-managed"]');
    await type("#wizard-base-url", "https://ghe.acme.com");
    q<HTMLInputElement>("#wizard-base-url")!.dispatchEvent(new Event("blur"));
    await settle();
    expect(checkedMode()).toBe("gh");
  });
});
