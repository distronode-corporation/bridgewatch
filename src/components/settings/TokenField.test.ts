import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
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
  it("unsets the other three variants and never the token table itself", () => {
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
