import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
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

function render(config: unknown = { accounts: {} }) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const props = reactive({
    config,
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
