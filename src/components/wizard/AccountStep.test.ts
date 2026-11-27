// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { Identity } from "./api";
import AccountStep from "./AccountStep.svelte";
import { emptyDraft, type Draft } from "./model";

/** The account step on its own: the token kinds it names, the instance radios, and the fields' error marks. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(draft: Draft, props: Partial<ComponentProps<typeof AccountStep>> = {}) {
  const onInstanceChange = vi.fn();
  const all = reactive({
    draft,
    errors: {},
    cli: null,
    identity: null,
    testing: false,
    testError: null,
    onTest: vi.fn(),
    onInstanceChange,
    onProviderChange: vi.fn(),
    ...props,
  }) as ComponentProps<typeof AccountStep>;
  host = document.createElement("div");
  document.body.append(host);
  component = mount(AccountStep, { target: host, props: all });
  flushSync();
  return { props: all, onInstanceChange };
}

const identity = (kind: Identity["token"]): Identity => ({
  username: "ci-bot",
  name: null,
  bot: true,
  token: kind,
  scopes: ["read_api"],
  expires_at: null,
  warnings: [],
});

const radio = (name: string, value: string) => host!.querySelector<HTMLInputElement>(`input[type=radio][name="${name}"][value="${value}"]`)!;
const pick = (input: HTMLInputElement) => {
  input.checked = true;
  input.dispatchEvent(new Event("change", { bubbles: true }));
  flushSync();
};

describe("AccountStep", () => {
  it("names a group token and a service account in words", () => {
    render(emptyDraft(), { identity: identity({ kind: "group", group_id: 7 }) });
    expect(host!.querySelector('[data-slot="identity"]')?.textContent).toContain("group token");
    void unmount(component!);
    host!.remove();
    render(emptyDraft(), { identity: identity({ kind: "service_account" }) });
    expect(host!.querySelector('[data-slot="identity"]')?.textContent).toContain("service account");
  });

  it("re-detects on the hosted instance at once, and on self-managed only once it has an address", () => {
    const draft = emptyDraft();
    draft.instance = "self-managed";
    const { props, onInstanceChange } = render(draft);
    pick(radio("instance", "hosted"));
    expect(props.draft.instance).toBe("hosted");
    expect(onInstanceChange).toHaveBeenCalledTimes(1);
    pick(radio("instance", "self-managed"));
    expect(onInstanceChange).toHaveBeenCalledTimes(1);
    props.draft.selfManagedUrl = "https://gitlab.example.com";
    pick(radio("instance", "hosted"));
    pick(radio("instance", "self-managed"));
    expect(onInstanceChange).toHaveBeenCalledTimes(3);
  });

  it("marks the token and account fields invalid when the step has errors for them", () => {
    const draft = emptyDraft();
    draft.tokenMode = "paste";
    render(draft, { errors: { token: "Paste a token.", account: "The account needs a name." } });
    expect(host!.querySelector("#wizard-secret")?.getAttribute("aria-invalid")).toBe("true");
    expect(host!.querySelector("#wizard-account")?.getAttribute("aria-invalid")).toBe("true");
    void unmount(component!);
    host!.remove();
    const env = emptyDraft();
    env.tokenMode = "env";
    render(env, { errors: { token: "Name the variable." } });
    expect(host!.querySelector("#wizard-env")?.getAttribute("aria-invalid")).toBe("true");
    void unmount(component!);
    host!.remove();
    const command = emptyDraft();
    command.tokenMode = "command";
    render(command, { errors: { token: "Name a program." } });
    expect(host!.querySelector("#wizard-command")?.getAttribute("aria-invalid")).toBe("true");
  });

  it("shows only the message under a detected CLI token that has an error", () => {
    const draft = emptyDraft();
    draft.tokenMode = "cli";
    render(draft, {
      cli: { status: "found", source: { keyring: { service: "glab:gitlab.com", user: "" } } },
      errors: { token: "glab's token was refused." },
    });
    const controls = host!.querySelector('[data-slot="access-controls"]')!;
    expect(controls.querySelector("input")).toBeNull();
    expect(controls.textContent).toContain("glab's token was refused.");
  });
});
