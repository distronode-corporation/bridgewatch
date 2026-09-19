import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { OAuthApi, SignInStatus } from "../../lib/oauth";
import type { Edit } from "../../lib/types";
import TokenField from "./TokenField.svelte";

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
    expect(host.textContent).toContain("Signed in as @octocat.");
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
    expect(host.textContent).toContain("Signed in as @octocat. This account now uses the sign-in.");
  });

  it("does not rewrite the file when the account already signs in", async () => {
    const oauth = fake(true);
    const edits = await renderWith(oauth, { oauth: true });
    oauth.status.mockResolvedValue(SIGNED_IN);
    await press('[data-action="oauth-sign-in"]');
    expect(edits).toEqual([]);
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
