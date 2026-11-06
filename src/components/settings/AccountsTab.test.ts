import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { OAuthApi, OAuthAvailability } from "../../lib/oauth";
import type { Edit } from "../../lib/types";
import AccountsTab from "./AccountsTab.svelte";

/**
 * Adding an account. The edits are the contract: what a new account is born
 * with in config.toml.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function render(config: unknown = { accounts: {} }, oauth?: OAuthApi) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const props = reactive({
    config,
    oauth,
    onedit: (e: Edit[]) => {
      edits.push(e);
      return Promise.resolve();
    },
  }) as ComponentProps<typeof AccountsTab>;
  component = mount(AccountsTab, { target: host, props });
  flushSync();
  return {
    edits,
    add: (name: string, provider?: "gitlab" | "github") => {
      const input = host.querySelector<HTMLInputElement>('.add input[type="text"]')!;
      input.value = name;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      if (provider) {
        const select = host.querySelector<HTMLSelectElement>(".add select")!;
        select.value = provider;
        select.dispatchEvent(new Event("change", { bubbles: true }));
      }
      flushSync();
      [...host.querySelectorAll<HTMLButtonElement>(".add button")].find((b) => /Add account/.test(b.textContent ?? ""))!.click();
      flushSync();
    },
  };
}

describe("AccountsTab, adding an account", () => {
  it("offers GitLab first and preselected, and writes exactly what it always did", () => {
    const h = render();
    const select = host.querySelector<HTMLSelectElement>(".add select")!;
    expect([...select.options].map((o) => o.value)).toEqual(["gitlab", "github"]);
    expect(select.value).toBe("gitlab");
    h.add("work");
    expect(h.edits).toEqual([
      [
        { op: "set", path: "accounts.work.base_url", value: { string: "https://gitlab.com" } },
        { op: "set", path: "accounts.work.token.own", value: { boolean: true } },
      ],
    ]);
  });

  it("gives a GitHub account its provider and no GitLab base_url, api_path or header", () => {
    const h = render();
    h.add("github.com", "github");
    expect(h.edits).toEqual([
      [
        { op: "set", path: 'accounts."github.com".provider', value: { string: "github" } },
        { op: "set", path: 'accounts."github.com".token.own', value: { boolean: true } },
      ],
    ]);
    const written = JSON.stringify(h.edits);
    for (const key of ["base_url", "api_path", "header"]) expect(written).not.toContain(key);
  });

  it("hands each account's provider to its token field, so gh is offered only on GitHub", () => {
    render({
      accounts: {
        lab: { provider: "gitlab", base_url: "https://gitlab.com", token: { own: true } },
        hub: { provider: "github", base_url: "https://api.github.com", token: { own: true } },
      },
    });
    const labels = [...host.querySelectorAll("section.account")].map(
      (s) => s.querySelector('[role="radiogroup"] label')?.textContent?.trim(),
    );
    expect(labels).toEqual(["glab / OS keyring", "gh / OS keyring"]);
  });
});

describe("AccountsTab, changing an existing account's provider", () => {
  function switchTo(section: Element, provider: "gitlab" | "github") {
    const select = [...section.querySelectorAll<HTMLSelectElement>("select")].find((s) =>
      [...s.options].some((o) => o.value === "github"),
    )!;
    select.value = provider;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
  }

  it("gitlab to github removes GitLab's defaults in the same write", () => {
    const h = render({
      accounts: {
        work: { provider: "gitlab", base_url: "https://gitlab.com", api_path: "/api/v4", header: "PRIVATE-TOKEN", token: { own: true } },
      },
    });
    switchTo(host.querySelector("section.account")!, "github");
    expect(h.edits).toEqual([
      [
        { op: "set", path: "accounts.work.provider", value: { string: "github" } },
        { op: "unset", path: "accounts.work.base_url" },
        { op: "unset", path: "accounts.work.api_path" },
        { op: "unset", path: "accounts.work.header" },
      ],
    ]);
    expect(host.querySelector('[data-slot="provider-switch-note"]')).toBeNull();
  });

  it("github to gitlab keeps an Enterprise host and API path, and says so in one line", () => {
    const h = render({
      accounts: {
        ghe: { provider: "github", base_url: "https://ghe.acme.com", api_path: "/api/v3", header: "Authorization: Bearer", token: { own: true } },
      },
    });
    switchTo(host.querySelector("section.account")!, "gitlab");
    expect(h.edits).toHaveLength(1);
    expect(h.edits[0][0]).toEqual({ op: "set", path: "accounts.ghe.provider", value: { string: "gitlab" } });
    expect(h.edits[0].slice(1)).toEqual([{ op: "unset", path: "accounts.ghe.header" }]);
    expect(host.querySelector('[data-slot="provider-switch-note"]')?.textContent).toBe(
      'Kept base_url = "https://ghe.acme.com", api_path = "/api/v3" (not GitHub\'s default). Check they suit GitLab.',
    );
  });

  it("an edit to any other field is passed through untouched", () => {
    const h = render({
      accounts: { work: { provider: "gitlab", base_url: "https://gitlab.com", api_path: "/api/v4", header: "PRIVATE-TOKEN", token: { own: true } } },
    });
    const header = [...host.querySelectorAll<HTMLSelectElement>("section.account select")].find((s) =>
      [...s.options].some((o) => o.value === "PRIVATE-TOKEN"),
    )!;
    header.value = "Authorization: Bearer";
    header.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    expect(h.edits).toEqual([[{ op: "set", path: "accounts.work.header", value: { string: "Authorization: Bearer" } }]]);
  });
});

describe("AccountsTab, adding an account where sign-in is built in", () => {
  const availability = (builtin: boolean): OAuthAvailability => ({
    available: builtin,
    builtin,
    needs_client_id: false,
    host: "github.com",
    install_url: null,
  });

  /** A sign-in API whose availability says built-in for the providers listed. */
  function fakeOAuth(builtin: string[]): OAuthApi {
    return {
      availability: vi.fn(async (provider: string) => availability(builtin.includes(provider))),
      start: vi.fn(),
      wait: vi.fn(),
      cancel: vi.fn(async () => {}),
      openVerification: vi.fn(async () => {}),
      openInstall: vi.fn(async () => {}),
      status: vi.fn(async () => null),
      signOut: vi.fn(async () => {}),
      copy: vi.fn(async () => true),
      onProgress: vi.fn(async () => () => {}),
    } as unknown as OAuthApi;
  }

  const settle = async () => {
    for (let i = 0; i < 4; i++) {
      await new Promise((r) => setTimeout(r, 0));
      flushSync();
    }
  };

  it("signs a new gitlab.com or github.com account in, the wizard's default too", async () => {
    const oauth = fakeOAuth(["gitlab", "github"]);
    const h = render({ accounts: {} }, oauth);
    await settle();
    h.add("work");
    await settle();
    h.add("hub", "github");
    await settle();
    h.add("hub2");
    await settle();
    expect(h.edits[0]).toEqual([
      { op: "set", path: "accounts.work.base_url", value: { string: "https://gitlab.com" } },
      { op: "set", path: "accounts.work.token.oauth", value: { boolean: true } },
    ]);
    expect(h.edits[1]).toEqual([
      { op: "set", path: "accounts.hub.provider", value: { string: "github" } },
      { op: "set", path: "accounts.hub.token.oauth", value: { boolean: true } },
    ]);
    // Still github, still answered: the same default.
    expect(h.edits[2].at(-1)).toEqual({ op: "set", path: "accounts.hub2.token.oauth", value: { boolean: true } });
    expect(oauth.availability).toHaveBeenCalledWith("github", "https://api.github.com", null);
  });

  it("keeps bridgewatch's own entry where this build has no built-in sign-in", async () => {
    const h = render({ accounts: {} }, fakeOAuth(["gitlab"]));
    await settle();
    h.add("hub", "github");
    await settle();
    expect(h.edits[0].at(-1)).toEqual({ op: "set", path: "accounts.hub.token.own", value: { boolean: true } });
  });

  it("asks for the provider chosen at the moment of adding, and never adds one name twice", async () => {
    const oauth = fakeOAuth(["github"]);
    const h = render({ accounts: {} }, oauth);
    h.add("hub", "github");
    // A second click while the question is out finds the name already cleared.
    [...host.querySelectorAll<HTMLButtonElement>(".add button")].find((b) => /Add account/.test(b.textContent ?? ""))!.click();
    await settle();
    expect(h.edits).toHaveLength(1);
    expect(h.edits[0].at(-1)).toEqual({ op: "set", path: "accounts.hub.token.oauth", value: { boolean: true } });
  });

  it("falls back to bridgewatch's own entry when availability cannot be read", async () => {
    const oauth = fakeOAuth([]);
    vi.mocked(oauth.availability).mockRejectedValue(new Error("no shell"));
    const h = render({ accounts: {} }, oauth);
    h.add("work");
    await settle();
    expect(h.edits[0].at(-1)).toEqual({ op: "set", path: "accounts.work.token.own", value: { boolean: true } });
  });
});
