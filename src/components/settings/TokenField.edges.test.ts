// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { OAuthApi, OAuthAvailability, SignInStatus } from "../../lib/oauth";
import type { OwnTokenStatus } from "../../lib/own-token";
import type { Edit } from "../../lib/types";
import TokenField from "./TokenField.svelte";

/**
 * The token source field on the files and answers TokenField.test.ts does not
 * write: token tables with keys missing, answers that arrive out of order or
 * fail, the GitLab wording, and the shell's own commands when none are injected.
 */

const shell = vi.hoisted(() => ({
  answers: new Map<string, unknown>(),
  invoked: [] as string[],
  setOwnToken: vi.fn((_account: string, _token: string) => Promise.resolve()),
  clearOwnToken: vi.fn((_account: string) => Promise.resolve()),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string) => {
    shell.invoked.push(cmd);
    return Promise.resolve(shell.answers.get(cmd) ?? null);
  },
}));

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/ipc")>()),
  setOwnToken: shell.setOwnToken,
  clearOwnToken: shell.clearOwnToken,
}));

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
  shell.answers.clear();
  shell.invoked.length = 0;
  shell.setOwnToken.mockReset();
  shell.clearOwnToken.mockReset();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
  }
}

type Fake = { [K in keyof Required<OAuthApi>]: Mock };

function fakeOAuth(status: SignInStatus | null = null): Fake {
  return {
    availability: vi.fn(async (): Promise<OAuthAvailability> => ({
      available: true,
      builtin: true,
      needs_client_id: false,
      host: "gitlab.com",
      install_url: null,
    })),
    start: vi.fn(),
    wait: vi.fn(),
    cancel: vi.fn(async () => {}),
    openVerification: vi.fn(async () => {}),
    openInstall: vi.fn(async () => {}),
    status: vi.fn(async () => status),
    signOut: vi.fn(async () => {}),
    copy: vi.fn(async () => true),
    onProgress: vi.fn(async () => () => {}),
  };
}

function render(props: Partial<ComponentProps<typeof TokenField>>) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const all = reactive({
    account: "work",
    value: { own: true },
    onedit: (e: Edit[]) => {
      edits.push(e);
      return Promise.resolve();
    },
    ...props,
  }) as ComponentProps<typeof TokenField>;
  component = mount(TokenField, { target: host, props: all });
  flushSync();
  return { props: all, edits };
}

const root = () => host!;
const checked = () => root().querySelector<HTMLInputElement>("input[type=radio]:checked")?.value;
const input = (label: string) => root().querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
const pick = (kind: string) => {
  const radio = root().querySelector<HTMLInputElement>(`input[type=radio][value="${kind}"]`)!;
  radio.checked = true;
  radio.dispatchEvent(new Event("change", { bubbles: true }));
  flushSync();
};
const type = (label: string, text: string) => {
  input(label).value = text;
  input(label).dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const apply = () => {
  root().querySelector<HTMLButtonElement>("button.apply")!.click();
  flushSync();
};
const button = (name: string) => [...root().querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === name)!;

describe("TokenField on token tables with keys missing", () => {
  it("reads no token table as bridgewatch's own entry", () => {
    render({ value: null, watched: false });
    expect(checked()).toBe("own");
  });

  it("reads a keyring source with no service or user as an empty pair", () => {
    render({ value: { keyring: {} } });
    expect(checked()).toBe("keyring");
    expect([input("Keyring service").value, input("Keyring user").value]).toEqual(["", ""]);
  });

  it("reads an env source with no variable, and a command that is not a list, as empty", () => {
    render({ value: { env: null } });
    expect(input("Environment variable").value).toBe("");
    void unmount(component!);
    host!.remove();
    render({ value: { command: "pass gitlab/pat" } });
    expect(input("Command").value).toBe("");
  });

  it("reads a sign-in table with no client id as the built-in application", async () => {
    render({ value: { oauth: {} }, oauth: fakeOAuth() });
    await settle();
    expect(checked()).toBe("oauth");
    expect(input("OAuth client id").value).toBe("");
  });
});

describe("TokenField writes", () => {
  it("switches an env source to bridgewatch's own entry", () => {
    const h = render({ value: { env: "BRIDGEWATCH_TOKEN_GITLAB" } });
    pick("own");
    apply();
    expect(h.edits).toContainEqual(expect.arrayContaining([{ op: "set", path: "accounts.work.token.own", value: { boolean: true } }]));
  });

  it("writes a keyring user typed beside the service", () => {
    const h = render({ value: { own: true }, watched: false });
    pick("keyring");
    type("Keyring service", "glab:gitlab.example.com");
    type("Keyring user", "ci-bot");
    apply();
    expect(JSON.stringify(h.edits)).toContain('"ci-bot"');
  });

  it("fills the keyring pair from the CLI's item", () => {
    render({ value: { keyring: { service: "x", user: "y" } }, baseUrl: "https://gitlab.com" });
    button("Fill in glab's item (glab:gitlab.com:token)").click();
    flushSync();
    expect([input("Keyring service").value, input("Keyring user").value]).toEqual(["glab:gitlab.com:token", ""]);
  });

  it("says why saving the token failed", async () => {
    shell.setOwnToken.mockRejectedValueOnce(new Error("the credential store is locked"));
    render({ value: { own: true }, watched: false });
    type("Token to save", "glpat-test");
    button("Save token").click();
    await settle();
    expect(shell.setOwnToken).toHaveBeenCalledWith("work", "glpat-test");
    expect(root().textContent).toContain("the credential store is locked");
  });

  it("says why Forget failed", async () => {
    shell.clearOwnToken.mockRejectedValueOnce(new Error("the credential store is locked"));
    render({ value: { own: true }, watched: false });
    button("Forget").click();
    await settle();
    expect(root().textContent).toContain("the credential store is locked");
  });
});

describe("TokenField's own entry, read out of order", () => {
  it("shows only the answer for the address the account has now", async () => {
    const answers: { resolve: (s: OwnTokenStatus) => void; reject: (e: unknown) => void }[] = [];
    const ownStatus = vi.fn(
      () => new Promise<OwnTokenStatus>((resolve, reject) => answers.push({ resolve, reject })),
    );
    const h = render({ value: { own: true }, baseUrl: "https://one.example", ownStatus });
    h.props.baseUrl = "https://two.example";
    flushSync();
    h.props.baseUrl = "https://three.example";
    flushSync();
    expect(ownStatus).toHaveBeenCalledTimes(3);
    answers[2].resolve({ kind: "bound", origin: "https://three.example" });
    // The two earlier reads finish late, one each way: neither replaces the answer above.
    answers[0].resolve({ kind: "bound", origin: "https://one.example" });
    answers[1].reject(new Error("late"));
    await settle();
    expect(root().querySelector('[data-slot="own-status"]')?.textContent).toBe("Stored for https://three.example.");
  });
});

describe("TokenField sign-in edges", () => {
  it("offers no sign-in when the availability question fails", async () => {
    const oauth = fakeOAuth();
    oauth.availability.mockRejectedValue(new Error("offline"));
    render({ oauth });
    await settle();
    expect(root().querySelector('input[type=radio][value="oauth"]')).toBeNull();
  });

  it("reads no sign-in status without sign-in commands, and offers no sign-in choice", async () => {
    render({ value: { oauth: true } });
    await settle();
    expect(checked()).toBeUndefined();
  });

  it("explains a GitLab application in GitLab's terms", async () => {
    render({ value: { oauth: true }, oauth: fakeOAuth() });
    await settle();
    expect(root().textContent).toContain("with the read_api scope");
  });

  it("says when the stored sign-in could not be read, rather than that there is none", async () => {
    const oauth = fakeOAuth();
    oauth.status.mockRejectedValue(new Error("keychain locked"));
    render({ value: { oauth: true }, oauth });
    await settle();
    expect(root().querySelector('[data-slot="oauth-status"]')?.textContent).toBe("Could not read the sign-in: keychain locked");
  });

  it("does not read the sign-in of an account no watch polls", async () => {
    const oauth = fakeOAuth();
    render({ value: { oauth: true }, oauth, watched: false });
    await settle();
    expect(oauth.status).not.toHaveBeenCalled();
    expect(root().querySelector('[data-slot="oauth-status"]')?.textContent).toBe(
      "No watch polls this account, so its sign-in is not read here.",
    );
  });

  const SIGNED_IN: SignInStatus = {
    login: "ci-bot",
    expires_at: null,
    refresh_expires_at: null,
    scopes: [],
    obtained_at: 1_700_000_000,
    can_refresh: true,
  };

  it("says why Sign out failed", async () => {
    const oauth = fakeOAuth(SIGNED_IN);
    oauth.signOut.mockRejectedValue(new Error("the credential store is locked"));
    render({ value: { oauth: true }, oauth });
    await settle();
    root().querySelector<HTMLButtonElement>("button.sign-out")!.click();
    await settle();
    expect(root().textContent).toContain("the credential store is locked");
  });

  it("takes Sign out away once the sign-in commands are gone", async () => {
    const oauth = fakeOAuth(SIGNED_IN);
    const h = render({ value: { oauth: true }, oauth });
    await settle();
    expect(root().querySelector("button.sign-out")).not.toBeNull();
    h.props.oauth = undefined;
    flushSync();
    expect(root().querySelector("button.sign-out")).toBeNull();
  });
});

describe("TokenField for a GitHub command source", () => {
  it("suggests gh's own command", () => {
    render({ value: { command: ["gh", "auth", "token"] }, provider: "github" });
    expect(input("Command").placeholder).toBe("gh auth token");
  });
});

describe("TokenField inside the Tauri webview, with nothing injected", () => {
  it("asks the shell's own commands for the sign-in and the stored token", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    shell.answers.set("oauth_availability", { available: false, builtin: false, needs_client_id: true, host: "gitlab.com", install_url: null });
    shell.answers.set("own_token_status", { kind: "none" });
    render({ value: { own: true } });
    await settle();
    expect(shell.invoked).toEqual(expect.arrayContaining(["oauth_availability", "own_token_status"]));
    expect(root().querySelector('[data-slot="own-status"]')?.textContent).toBe("No token is stored for this account yet.");
  });
});
