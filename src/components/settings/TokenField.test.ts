import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { OAuthApi, SignInStatus } from "../../lib/oauth";
import type { OwnTokenStatus } from "../../lib/own-token";
import type { Edit } from "../../lib/types";
import { formatCommand, parseCommand } from "../wizard/model";
import TokenField from "./TokenField.svelte";

/** The two credential-store writes, scripted; everything else is the real module. */
const store = vi.hoisted(() => ({
  setOwnToken: vi.fn((_account: string, _token: string) => Promise.resolve()),
  clearOwnToken: vi.fn((_account: string) => Promise.resolve()),
}));

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/ipc")>()),
  setOwnToken: store.setOwnToken,
  clearOwnToken: store.clearOwnToken,
}));

/**
 * The token source: four mutually exclusive variants of one enum, edited by
 * radio buttons.
 *
 * ⚠ Nothing here touches a real credential. `set_own_token` is a Tauri command
 * and `inTauri()` is false under jsdom, so the paste field is deliberately not
 * exercised: what this holds is the part that writes the CONFIG FILE, which is
 * where every defect in this component was.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  store.setOwnToken.mockReset();
  store.clearOwnToken.mockReset();
});

function render(account: string, value: unknown) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const props = reactive({
    account,
    value,
    onedit: (e: Edit[]) => {
      edits.push(e);
      return Promise.resolve();
    },
  }) as ComponentProps<typeof TokenField>;
  component = mount(TokenField, { target: host, props });
  flushSync();
  return {
    props,
    edits,
    /** What the parent does after any save anywhere: it re-reads the document. */
    reload: (next?: unknown) => {
      // A fresh object with the same contents, which is exactly what
      // `get_config_json` returns on every round trip: the value is equal and
      // the identity is new, so every `$derived` downstream of it recomputes.
      props.value = JSON.parse(JSON.stringify(next ?? props.value)) as unknown;
      flushSync();
    },
    pick: (kind: string) => {
      const radio = host.querySelector<HTMLInputElement>(`input[type=radio][value="${kind}"]`)!;
      radio.checked = true;
      radio.dispatchEvent(new Event("change", { bubbles: true }));
      flushSync();
    },
    field: (placeholder: string) =>
      host.querySelector<HTMLInputElement>(`input[placeholder^="${placeholder}"]`)!,
    type: (placeholder: string, text: string) => {
      const input = host.querySelector<HTMLInputElement>(`input[placeholder^="${placeholder}"]`)!;
      input.value = text;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      flushSync();
    },
    apply: () => {
      host.querySelector<HTMLButtonElement>("button.apply")!.click();
      flushSync();
    },
    ask: () => host.querySelector('[role="dialog"]'),
    confirmAsk: () => {
      host.querySelector<HTMLButtonElement>('[role="dialog"] .confirm')!.click();
      flushSync();
    },
    cancelAsk: () => {
      host.querySelector<HTMLButtonElement>('[role="dialog"] .cancel')!.click();
      flushSync();
    },
  };
}

const KEYRING = { keyring: { service: "glab:gitlab.com", user: "" } };

describe("TokenField, the path it writes to", () => {
  it("quotes an account name that has a dot in it", () => {
    // ⛔ An account named after its host, `gitlab.com`, has a dot in it. An
    // unquoted template string sent the core into `accounts` → `gitlab` →
    // `com`, so it wrote a nested table nobody asked for and the token source
    // of an account named that way could not be changed from this window.
    const h = render("gitlab.com", { own: true });
    h.pick("env");
    h.type("BRIDGEWATCH_TOKEN", "BRIDGEWATCH_TOKEN_GITLAB");
    h.apply();
    expect(h.edits[0]).toContainEqual({
      op: "set",
      path: 'accounts."gitlab.com".token.env',
      value: { string: "BRIDGEWATCH_TOKEN_GITLAB" },
    });
  });

  it("leaves a plain account name alone", () => {
    const h = render("work", { own: true });
    h.pick("env");
    h.type("BRIDGEWATCH_TOKEN", "T");
    h.apply();
    expect(h.edits[0]).toContainEqual({
      op: "set",
      path: "accounts.work.token.env",
      value: { string: "T" },
    });
  });
});

describe("TokenField, switching variant", () => {
  it("unsets the other variants and never the token table itself", () => {
    // ⛔ `unset accounts.x.token` followed by a set is what moved the key: a
    // table `toml_edit` has never seen is APPENDED, so `token` sank below
    // `timeout_secs` and the backoff block every time the source changed.
    // Unsetting only the variants that no longer apply leaves it where the user
    // put it.
    const h = render("work", KEYRING);
    h.pick("env");
    h.type("BRIDGEWATCH_TOKEN", "T");
    h.apply();
    expect(h.edits).toEqual([
      [
        { op: "unset", path: "accounts.work.token.keyring" },
        { op: "unset", path: "accounts.work.token.command" },
        { op: "unset", path: "accounts.work.token.own" },
        // A fifth variant since sign-in: switching away from it must remove it
        // too, or the file names two sources and refuses to load.
        { op: "unset", path: "accounts.work.token.oauth" },
        { op: "set", path: "accounts.work.token.env", value: { string: "T" } },
      ],
    ]);
    expect(h.edits[0].some((e) => "path" in e && e.path === "accounts.work.token")).toBe(false);
  });

  it("does not unset the variant it is about to write", () => {
    const h = render("work", KEYRING);
    h.type("service", "glab:gitlab.example");
    h.apply();
    expect(h.edits[0].map((e) => ("path" in e ? e.path : e.op))).toEqual([
      "accounts.work.token.env",
      "accounts.work.token.command",
      "accounts.work.token.own",
      "accounts.work.token.oauth",
      "accounts.work.token.keyring.service",
      "accounts.work.token.keyring.user",
    ]);
  });
});

describe("TokenField, the command source", () => {
  it("writes the command edit and leaves the question to the shell", () => {
    // Lo13: the confirmation moved into Rust, which answers the write with a
    // request and writes nothing until it comes back. A second question here
    // would ask the same thing twice.
    const h = render("work", { own: true });
    h.pick("command");
    h.type("pass gitlab", "pass gitlab/pat");
    h.apply();
    expect(h.ask()).toBe(null);
    expect(h.edits[0]).toContainEqual({
      op: "set",
      path: "accounts.work.token.command",
      value: { array: [{ string: "pass" }, { string: "gitlab/pat" }] },
    });
  });

  it("refuses an empty command without writing", () => {
    const h = render("work", { own: true });
    h.pick("command");
    h.apply();
    expect(h.edits).toEqual([]);
    expect(host.textContent).toContain("needs a program");
  });

  it("keeps an argument with a space in it whole, as the wizard wrote it", () => {
    // ⛔ The field showed `argv.join(" ")` and wrote back `split(/\s+/)`, so
    // "Use this source" on an untouched `sh -c '...'` command broke the quoted
    // argument into words and the token stopped resolving.
    const argv = parseCommand("sh -c 'pass show gitlab/pat'");
    const h = render("work", { command: argv });
    expect(h.field("pass gitlab").value).toBe(formatCommand(argv));
    h.apply();
    expect(h.edits[0]).toContainEqual({
      op: "set",
      path: "accounts.work.token.command",
      value: { array: argv.map((a) => ({ string: a })) },
    });
  });

  it("says so on the form for as long as the file says it, not just once", () => {
    // The dialog is a moment; a program that runs on every token refresh is a
    // standing fact, and the core warns about it on every load for the same
    // reason.
    render("work", { command: ["pass", "gitlab/pat"] });
    expect(host.querySelector(".warning")?.textContent).toContain("pass gitlab/pat");
    const plain = render("other", { own: true });
    expect(plain.ask()).toBe(null);
    expect(host.querySelector(".warning")).toBe(null);
  });
});

describe("TokenField, across an unrelated save", () => {
  it("keeps a typed-but-unapplied setting", () => {
    // ⛔ The seeding effect assigned on every run, and it runs whenever the
    // document is re-read — which is after every save of any field on any tab.
    // Typing a service name here and then changing a poll interval two tabs
    // away wiped it with nothing on screen to say so.
    const h = render("work", KEYRING);
    h.type("service", "glab:gitlab.example");
    h.reload();
    expect(h.field("service").value).toBe("glab:gitlab.example");
  });

  it("still follows a change another window made to the token itself", () => {
    const h = render("work", KEYRING);
    h.type("service", "half typed");
    h.reload({ keyring: { service: "glab:elsewhere", user: "sean" } });
    expect(h.field("service").value).toBe("glab:elsewhere");
    expect(h.field("user").value).toBe("sean");
  });

  it("follows a change of variant made elsewhere", () => {
    const h = render("work", KEYRING);
    h.reload({ env: "BRIDGEWATCH_TOKEN_GITLAB" });
    expect(
      host.querySelector<HTMLInputElement>('input[type=radio][value="env"]')!.checked,
    ).toBe(true);
    expect(h.field("BRIDGEWATCH_TOKEN").value).toBe("BRIDGEWATCH_TOKEN_GITLAB");
  });
});

describe("TokenField, bridgewatch's own entry", () => {
  /** A promise and the hand that settles it. */
  function pending() {
    let resolve!: () => void;
    const promise = new Promise<void>((r) => (resolve = r));
    return { promise, resolve };
  }

  async function settle() {
    for (let i = 0; i < 4; i++) {
      await new Promise((r) => setTimeout(r, 0));
      flushSync();
    }
  }

  it("sends one credential-store write for a double click, and nothing else until it lands", async () => {
    // Two clicks can both land before the first one's re-render, and a
    // locked keychain then prompts twice. Forget while Save is on its way
    // would race it.
    const save = pending();
    store.setOwnToken.mockImplementation(() => save.promise);
    const h = render("work", { own: true });
    h.type("glpat", "glpat-xxxxxxxxxxxxxxxxxxxx");
    const saveButton = [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Save token")!;
    const forgetButton = [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Forget")!;
    saveButton.click();
    saveButton.click();
    forgetButton.click();
    expect(store.setOwnToken).toHaveBeenCalledTimes(1);
    expect(store.clearOwnToken).not.toHaveBeenCalled();
    save.resolve();
    await settle();
    expect(host.textContent).toContain("Saved to the credential store.");
    // Once it has landed, the next write goes out.
    forgetButton.click();
    await settle();
    expect(store.clearOwnToken).toHaveBeenCalledTimes(1);
    expect(store.clearOwnToken).toHaveBeenCalledWith("work");
    // The shell reloads the poller on Forget, so the account goes quiet.
    expect(host.querySelector(".message")?.textContent).toBe(
      "Removed from the credential store. This account stops polling until a token is set again.",
    );
  });

  it("shows a structured refusal's message, not [object Object]", async () => {
    store.clearOwnToken.mockImplementation(() => Promise.reject({ kind: "store", message: "the keychain is locked" }));
    render("work", { own: true });
    [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Forget")!.click();
    await settle();
    expect(host.querySelector(".message")?.textContent).toBe("the keychain is locked");
  });
});

describe("TokenField, the provider's CLI preset", () => {
  function renderFor(provider: "gitlab" | "github", baseUrl: string, value: unknown = KEYRING) {
    host = document.createElement("div");
    document.body.append(host);
    const edits: Edit[][] = [];
    const props = reactive({
      account: "acct",
      value,
      provider,
      baseUrl,
      onedit: (e: Edit[]) => {
        edits.push(e);
        return Promise.resolve();
      },
    }) as ComponentProps<typeof TokenField>;
    component = mount(TokenField, { target: host, props });
    flushSync();
    return { edits };
  }

  const presetButton = () => host.querySelector<HTMLButtonElement>("button.preset");
  const input = (placeholder: string) => host.querySelector<HTMLInputElement>(`input[placeholder^="${placeholder}"]`)!;
  const choiceLabels = () =>
    [...host.querySelectorAll('[role="radiogroup"] label')].map((l) => l.textContent?.trim());

  it("offers gh's active-account item on a GitHub account, named for the WEB host", () => {
    const { edits } = renderFor("github", "https://api.github.com");
    expect(choiceLabels()[0]).toBe("gh / OS keyring");
    expect(presetButton()?.textContent).toContain("gh:github.com");
    presetButton()!.click();
    flushSync();
    expect(input("service").value).toBe("gh:github.com");
    expect(input("user").value).toBe("");
    // Filling the fields writes nothing; "Use this source" does.
    expect(edits).toEqual([]);
    host.querySelector<HTMLButtonElement>("button.apply")!.click();
    flushSync();
    expect(edits[0]).toContainEqual({ op: "set", path: "accounts.acct.token.keyring.service", value: { string: "gh:github.com" } });
    expect(edits[0]).toContainEqual({ op: "set", path: "accounts.acct.token.keyring.user", value: { string: "" } });
  });

  it("names a GitHub Enterprise Server item after its own host", () => {
    renderFor("github", "https://ghe.acme.com");
    expect(presetButton()?.textContent).toContain("gh:ghe.acme.com");
  });

  it("offers glab's item on a GitLab account, and never gh's", () => {
    renderFor("gitlab", "https://gitlab.com");
    expect(choiceLabels()[0]).toBe("glab / OS keyring");
    expect(presetButton()?.textContent).toContain("glab:gitlab.com:token");
    expect(host.textContent).not.toContain("gh:");
  });

  it("speaks GitHub in the placeholders of a GitHub account", () => {
    renderFor("github", "https://api.github.com", { own: true });
    expect(host.querySelector<HTMLInputElement>('input[type="password"]')?.placeholder).toContain("github_pat_");
  });
});

describe("TokenField, sign in", () => {
  type Fake = { [K in keyof Required<OAuthApi>]: Mock };
  function fake(available: boolean, status: SignInStatus | null = null): Fake {
    return {
      availability: vi.fn(async (_p: string, _b: string, clientId?: string | null) => ({
        available: available || !!clientId,
        builtin: available,
        needs_client_id: false,
        host: "github.com",
        install_url: available ? "https://github.com/apps/bridgewatch-ci/installations/new" : null,
      })),
      start: vi.fn(async () => ({
        id: "f1",
        user_code: "WDJB-MJHT",
        verification_uri: "https://github.com/login/device",
        expires_in: 900,
        host: "github.com",
      })),
      wait: vi.fn(async () => ({ login: "octocat", scopes: [], expires_at: null })),
      cancel: vi.fn(async () => {}),
      openVerification: vi.fn(async () => {}),
      openInstall: vi.fn(async () => {}),
      status: vi.fn(async () => status),
      signOut: vi.fn(async () => {}),
      copy: vi.fn(async () => true),
      onProgress: vi.fn(async () => () => {}),
    };
  }

  async function settle() {
    for (let i = 0; i < 6; i++) {
      await new Promise((r) => setTimeout(r, 0));
      flushSync();
    }
  }

  async function renderWith(oauth: Fake, value: unknown, baseUrl = "") {
    host = document.createElement("div");
    document.body.append(host);
    const edits: Edit[][] = [];
    const props = reactive({
      account: "gh",
      value,
      provider: "github",
      baseUrl,
      oauth,
      onedit: (e: Edit[]) => {
        edits.push(e);
        return Promise.resolve();
      },
    }) as ComponentProps<typeof TokenField>;
    component = mount(TokenField, { target: host, props });
    await settle();
    return edits;
  }

  const labels = () => [...host.querySelectorAll('[role="radiogroup"] label')].map((l) => l.textContent?.trim());
  const pick = async (kind: string) => {
    const radio = host.querySelector<HTMLInputElement>(`input[type=radio][value="${kind}"]`)!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
  };
  const press = async (selector: string) => {
    host.querySelector<HTMLButtonElement>(selector)!.click();
    await settle();
  };

  const SIGNED_IN: SignInStatus = {
    login: "octocat",
    expires_at: 1_800_000_000,
    refresh_expires_at: null,
    scopes: [],
    obtained_at: 1_700_000_000,
    can_refresh: true,
  };

  it("is not offered where it cannot work, so the four sources read as before", async () => {
    const oauth = fake(false);
    await renderWith(oauth, KEYRING);
    expect(labels()).toEqual(["gh / OS keyring", "Environment variable", "Command", "bridgewatch's own entry"]);
    // Asked about the account's real instance: the provider default when the file has no base_url.
    expect(oauth.availability).toHaveBeenLastCalledWith("github", "https://api.github.com", null);
  });

  it("is offered first where it can, and writes oauth = true for the built-in application", async () => {
    const edits = await renderWith(fake(true), { own: true });
    expect(labels()[0]).toBe("Sign in with GitHub");
    await pick("oauth");
    await press("button.apply");
    expect(edits).toEqual([
      [
        { op: "unset", path: "accounts.gh.token.keyring" },
        { op: "unset", path: "accounts.gh.token.env" },
        { op: "unset", path: "accounts.gh.token.command" },
        { op: "unset", path: "accounts.gh.token.own" },
        { op: "unset", path: "accounts.gh.token.oauth" },
        { op: "set", path: "accounts.gh.token.oauth", value: { boolean: true } },
      ],
    ]);
  });

  it("writes a typed client id as oauth = { client_id }", async () => {
    const edits = await renderWith(fake(false), { oauth: { client_id: "Iv23liOLD" } }, "https://ghe.acme.com");
    const input = host.querySelector<HTMLInputElement>('input[aria-label="OAuth client id"]')!;
    expect(input.value).toBe("Iv23liOLD");
    input.value = "Iv23liNEW";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await press("button.apply");
    expect(edits[0].at(-1)).toEqual({
      op: "set",
      path: "accounts.gh.token.oauth.client_id",
      value: { string: "Iv23liNEW" },
    });
  });

  it("signs in with the device flow and then shows who", async () => {
    const oauth = fake(true);
    await renderWith(oauth, { oauth: true });
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent).toContain("Not signed in");
    oauth.status.mockResolvedValue(SIGNED_IN);
    await press('[data-action="oauth-sign-in"]');
    expect(oauth.start).toHaveBeenCalledWith({
      account: "gh",
      provider: "github",
      base_url: "https://api.github.com",
      client_id: null,
    });
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent).toContain("@octocat");
    // Said once, by the status line.
    expect(host.textContent?.match(/Signed in as/g)).toHaveLength(1);
  });

  it("switches an account that used another source to the sign-in once it succeeds", async () => {
    const oauth = fake(true);
    const edits = await renderWith(oauth, { keyring: { service: "gh:github.com", user: "" } });
    await pick("oauth");
    expect(edits).toEqual([]);
    oauth.status.mockResolvedValue(SIGNED_IN);
    await press('[data-action="oauth-sign-in"]');
    expect(edits).toHaveLength(1);
    expect(edits[0]).toContainEqual({ op: "unset", path: "accounts.gh.token.keyring" });
    expect(edits[0]).toContainEqual({ op: "set", path: "accounts.gh.token.oauth", value: { boolean: true } });
    expect(host.textContent).toContain("This account now uses the sign-in.");
    expect(host.textContent?.match(/Signed in as/g)).toHaveLength(1);
  });

  it("does not rewrite the file when the account already signs in", async () => {
    const oauth = fake(true);
    const edits = await renderWith(oauth, { oauth: true });
    oauth.status.mockResolvedValue(SIGNED_IN);
    await press('[data-action="oauth-sign-in"]');
    expect(edits).toEqual([]);
  });

  it("offers no Use this source for the sign-in the file already uses, and does again for a new client id", async () => {
    await renderWith(fake(true), { oauth: true });
    expect(host.querySelector("button.apply")).toBeNull();
    const input = host.querySelector<HTMLInputElement>('input[aria-label="OAuth client id"]')!;
    input.value = "Iv23liMINE";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(host.querySelector("button.apply")).not.toBeNull();
  });

  it("writes a newly typed client id when the sign-in through it succeeds", async () => {
    const oauth = fake(true);
    const edits = await renderWith(oauth, { oauth: true });
    const input = host.querySelector<HTMLInputElement>('input[aria-label="OAuth client id"]')!;
    input.value = "Iv23liMINE";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    oauth.status.mockResolvedValue(SIGNED_IN);
    await press('[data-action="oauth-sign-in"]');
    expect(oauth.start.mock.calls[0][0]).toMatchObject({ client_id: "Iv23liMINE" });
    expect(edits).toHaveLength(1);
    expect(edits[0].at(-1)).toEqual({ op: "set", path: "accounts.gh.token.oauth.client_id", value: { string: "Iv23liMINE" } });
    expect(host.textContent).toContain("This account now uses the sign-in.");
  });

  it("writes the client id the sign-in started with, whatever the box says when it finishes", async () => {
    const oauth = fake(true);
    let approve: (s: unknown) => void = () => {};
    oauth.wait.mockImplementation(() => new Promise((r) => (approve = r)));
    const edits = await renderWith(oauth, { own: true });
    await pick("oauth");
    const input = host.querySelector<HTMLInputElement>('input[aria-label="OAuth client id"]')!;
    input.value = "Iv23liSTART";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await press('[data-action="oauth-sign-in"]');
    expect(oauth.start.mock.calls[0][0]).toMatchObject({ client_id: "Iv23liSTART" });
    // Edited while the code is pending.
    input.value = "Iv23liLATER";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    oauth.status.mockResolvedValue(SIGNED_IN);
    approve({ login: "octocat", scopes: [], expires_at: null });
    await settle();
    expect(edits).toHaveLength(1);
    expect(edits[0].at(-1)).toEqual({ op: "set", path: "accounts.gh.token.oauth.client_id", value: { string: "Iv23liSTART" } });
    expect(JSON.stringify(edits)).not.toContain("Iv23liLATER");
  });

  it("shows the stored sign-in, signs out, and links the app's install page", async () => {
    const oauth = fake(true, SIGNED_IN);
    await renderWith(oauth, { oauth: true });
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent).toContain("@octocat");
    expect(host.querySelector('[data-action="oauth-sign-in"]')?.textContent?.trim()).toBe("Sign in again");
    await press('[data-slot="install-app"] button');
    expect(oauth.openInstall).toHaveBeenCalledWith("github", "https://api.github.com");
    await press("button.sign-out");
    expect(oauth.signOut).toHaveBeenCalledWith("gh");
    expect(host.textContent).toContain("Signed out.");
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent).toContain("Not signed in");
  });

  it("says a sign-in issued by another server is not used, rather than signed in", async () => {
    // ⛔ The poller never sends a sign-in issued by another instance, and this
    // line said "Signed in as @octocat" for it all the same.
    const oauth = fake(true, {
      ...SIGNED_IN,
      mismatch: { kind: "other_instance", issued: "github.com", expected: "ghe.acme.com" },
    });
    await renderWith(oauth, { oauth: true }, "https://ghe.acme.com");
    const line = host.querySelector('[data-slot="oauth-status"]');
    expect(line?.getAttribute("role")).toBe("alert");
    expect(line?.textContent?.trim()).toBe(
      "Signed in for github.com, not ghe.acme.com, so the sign-in is not used. Sign in again.",
    );
    expect(host.textContent).not.toContain("Signed in as");
    expect(host.querySelector('[data-action="oauth-sign-in"]')?.textContent?.trim()).toBe("Sign in again");
    // It can still be removed.
    expect(host.querySelector("button.sign-out")).not.toBeNull();
  });

  it("says a sign-in for another application is not used", async () => {
    const oauth = fake(true, { ...SIGNED_IN, mismatch: { kind: "other_application" } });
    await renderWith(oauth, { oauth: { client_id: "Iv23liMINE" } });
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent?.trim()).toBe(
      "Signed in with another application than this account's client id, so the sign-in is not used. Sign in again.",
    );
  });

  it("shows a sign-in whose mismatch is null as signed in", async () => {
    const oauth = fake(true, { ...SIGNED_IN, mismatch: null });
    await renderWith(oauth, { oauth: true });
    expect(host.querySelector('[data-slot="oauth-status"]')?.textContent).toContain("Signed in as @octocat");
  });

  it("says the sign-in could not be read, rather than that there is none", async () => {
    // ⛔ A locked keychain or a denied prompt made `oauth_status` reject, and
    // the form said "Not signed in." for an account that is: the user signed
    // in again, minting a device authorisation for nothing.
    const oauth = fake(true);
    oauth.status.mockRejectedValue("the keychain is locked");
    await renderWith(oauth, { oauth: true });
    const line = host.querySelector('[data-slot="oauth-status"]')?.textContent ?? "";
    expect(line).toContain("Could not read the sign-in: the keychain is locked");
    expect(line).not.toContain("Not signed in");
  });

  it("signs out once for a double click", async () => {
    let finish!: () => void;
    const oauth = fake(true, SIGNED_IN);
    oauth.signOut.mockImplementation(() => new Promise<void>((r) => (finish = r)));
    await renderWith(oauth, { oauth: true });
    const button = host.querySelector<HTMLButtonElement>("button.sign-out")!;
    button.click();
    button.click();
    expect(oauth.signOut).toHaveBeenCalledTimes(1);
    finish();
    await settle();
    expect(host.textContent).toContain("Signed out.");
  });

  it("switching away from a sign-in removes it from the file", async () => {
    const edits = await renderWith(fake(true), { oauth: true });
    await pick("env");
    host.querySelector<HTMLInputElement>('input[placeholder^="BRIDGEWATCH_TOKEN"]')!.value = "GH";
    host
      .querySelector<HTMLInputElement>('input[placeholder^="BRIDGEWATCH_TOKEN"]')!
      .dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await press("button.apply");
    expect(edits[0]).toContainEqual({ op: "unset", path: "accounts.gh.token.oauth" });
    expect(edits[0]).toContainEqual({ op: "set", path: "accounts.gh.token.env", value: { string: "GH" } });
  });
});

describe("TokenField, the server an own token is stored for", () => {
  /** `own_token_status`, scripted per call. */
  function renderOwn(answers: OwnTokenStatus[], baseUrl = "https://gitlab.example.com") {
    host = document.createElement("div");
    document.body.append(host);
    const ownStatus = vi.fn(async (_account: string) => answers.shift() ?? { kind: "none" as const });
    const props = reactive({
      account: "work",
      value: { own: true },
      onedit: () => Promise.resolve(),
      baseUrl,
      ownStatus,
    }) as ComponentProps<typeof TokenField>;
    component = mount(TokenField, { target: host, props });
    flushSync();
    return { props, ownStatus };
  }
  const settle = async () => {
    for (let i = 0; i < 4; i++) {
      await Promise.resolve();
      flushSync();
    }
  };
  const line = () => host.querySelector('[data-slot="own-status"]');

  it("says which server the stored token is for", async () => {
    const h = renderOwn([{ kind: "bound", origin: "https://gitlab.example.com" }]);
    await settle();
    expect(h.ownStatus).toHaveBeenCalledWith("work");
    expect(line()?.textContent).toBe("Stored for https://gitlab.example.com.");
    expect(line()?.getAttribute("role")).toBeNull();
  });

  it("says 'stored for X; paste one for Y' once the account points elsewhere, and asks again when it moves", async () => {
    const h = renderOwn([
      { kind: "bound", origin: "https://gitlab.example.com" },
      { kind: "mismatch", stored: "https://gitlab.example.com", expected: "https://gitlab.other.example" },
    ]);
    await settle();
    expect(line()?.textContent).toBe("Stored for https://gitlab.example.com.");

    // The file was saved with another base_url: the parent re-renders with it.
    h.props.baseUrl = "https://gitlab.other.example";
    flushSync();
    await settle();
    expect(h.ownStatus).toHaveBeenCalledTimes(2);
    expect(line()?.textContent).toBe(
      "Stored for https://gitlab.example.com; paste one for https://gitlab.other.example. The stored token is not sent there.",
    );
    expect(line()?.getAttribute("role")).toBe("alert");
  });

  it("names a 1.x value and nothing stored, and reads again after a save", async () => {
    const h = renderOwn([{ kind: "legacy" }, { kind: "bound", origin: "https://gitlab.example.com" }]);
    await settle();
    expect(line()?.textContent).toBe(
      "A token saved by bridgewatch 1.x is stored. It is tied to https://gitlab.example.com the first time it is used.",
    );
    const input = host.querySelector<HTMLInputElement>('input[type="password"]')!;
    input.value = "glpat-new";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Save token")!.click();
    await settle();
    expect(store.setOwnToken).toHaveBeenCalledWith("work", "glpat-new");
    expect(h.ownStatus).toHaveBeenCalledTimes(2);
    expect(line()?.textContent).toBe("Stored for https://gitlab.example.com.");
  });

  it("says a failed read is a failed read, not 'nothing stored'", async () => {
    host = document.createElement("div");
    document.body.append(host);
    const props = reactive({
      account: "work",
      value: { own: true },
      onedit: () => Promise.resolve(),
      ownStatus: () => Promise.reject(new Error("the keychain is locked")),
    }) as ComponentProps<typeof TokenField>;
    component = mount(TokenField, { target: host, props });
    flushSync();
    await settle();
    expect(line()?.textContent).toBe("Could not read the stored token: the keychain is locked");
    expect(line()?.getAttribute("role")).toBe("alert");
  });
});

describe("TokenField, the inset panel and one action row (L2)", () => {
  it("puts the chosen method's details in the panel and every button in one row after it", () => {
    const h = render("work", { keyring: { service: "glab:gitlab.com", user: "" } });
    const panel = () => host.querySelector('[data-slot="token-panel"]')!;
    const actions = () => host.querySelector('[data-slot="token-actions"]')!;
    const labels = () => [...actions().querySelectorAll("button")].map((b) => b.textContent?.trim());
    expect(panel().getAttribute("data-kind")).toBe("keyring");
    expect(panel().querySelector('input[aria-label="Keyring service"]')).toBeTruthy();
    expect(panel().querySelector("button")).toBe(null);
    // The panel comes first, then the row, and the row is the only place with buttons.
    expect(panel().compareDocumentPosition(actions()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(labels().at(-1)).toBe("Use this source");
    h.pick("own");
    expect(panel().getAttribute("data-kind")).toBe("own");
    expect(panel().querySelector('input[type="password"]')).toBeTruthy();
    expect(labels()).toEqual(["Save token", "Forget", "Use this source"]);
    h.pick("env");
    expect(labels()).toEqual(["Use this source"]);
  });

  it("shows P2's stored-for line inside the panel", async () => {
    host = document.createElement("div");
    document.body.append(host);
    const props = reactive({
      account: "work",
      value: { own: true },
      baseUrl: "https://gitlab.example.com",
      onedit: () => Promise.resolve(),
      ownStatus: () => Promise.resolve({ kind: "bound" as const, origin: "https://gitlab.example.com" }),
    }) as ComponentProps<typeof TokenField>;
    component = mount(TokenField, { target: host, props });
    flushSync();
    for (let i = 0; i < 6; i++) await Promise.resolve();
    flushSync();
    const line = host.querySelector('[data-slot="own-status"]');
    expect(line).toBeTruthy();
    expect(line!.closest('[data-slot="token-panel"]')).toBeTruthy();
  });
});
