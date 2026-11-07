import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { OAuthApi, OAuthAvailability } from "../../lib/oauth";
import type { Identity, WizardApi } from "./api";
import Wizard from "./Wizard.svelte";

/**
 * The wizard's layout (2.0 layout review, change 9): the access methods as
 * cards holding their own controls, a stepper that labels only the current
 * step, one 13px body size and a 560px column. Classes stand in for pixels,
 * since jsdom computes no Tailwind CSS.
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
const BUILT_IN: OAuthAvailability = {
  available: true,
  builtin: true,
  needs_client_id: false,
  host: "github.com",
  install_url: null,
};
const GH_FOUND = { status: "found", source: { keyring: { service: "gh:github.com", user: "" } } } as const;

function fakeOAuth(): { [K in keyof Required<OAuthApi>]: Mock } {
  return {
    availability: vi.fn(async () => BUILT_IN),
    start: vi.fn(async () => ({
      id: "flow-1",
      user_code: "WDJB-MJHT",
      verification_uri: "https://github.com/login/device",
      expires_in: 900,
      host: "github.com",
    })),
    wait: vi.fn(async () => ({ login: "octocat", scopes: [], expires_at: 1_800_000_000 })),
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
    detectCliToken: vi.fn(async () => GH_FOUND),
    ownTokenStatus: vi.fn(async () => ({ kind: "legacy" })),
    testConnection: vi.fn(async () => IDENTITY),
    listProjects: vi.fn(async () => ({ mode: "projects", truncated: false, projects: [] })),
    resolveProject: vi.fn(async () => ({ id: 1, path: "acme/web", project: "acme/web", default_branch: "main", web_url: null })),
    suggestDeployMarkers: vi.fn(async () => ({ pipeline_id: null, pipeline_url: null, suggestions: [], unread_children: [] })),
    previewConfig: vi.fn(async () => ({ toml: "[accounts.github]\n", warnings: [], edited_existing: false })),
    save: vi.fn(async () => ({ ok: true, diagnostics: [], path: "config.toml" })),
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

const q = <T extends Element = HTMLElement>(sel: string) => document.body.querySelector<T>(sel);
const all = <T extends Element = HTMLElement>(sel: string) => [...document.body.querySelectorAll<T>(sel)];
async function click(sel: string) {
  const el = q(sel);
  if (!el) throw new Error(`no element for ${sel}`);
  el.click();
  await settle();
}
const stepId = () => q<HTMLElement>('[data-slot="wizard"]')?.dataset.step;

async function github(oauth?: OAuthApi) {
  component = mount(Wizard, { target: host, props: { api: fakeApi(), oauth } });
  await settle();
  await click('input[name="provider"][value="github"]');
}

/** The card an element sits in, or null. */
const cardOf = (el: Element | null) => el?.closest<HTMLElement>('[data-slot="access-card"]') ?? null;
const radio = (mode: string) => q<HTMLInputElement>(`input[name="token-mode"][value="${mode}"]`);

/** The first element after `from`, in document order, that Tab would reach. */
function nextTabStop(from: Element): Element | null {
  const focusable = all('input, button, select, textarea, a[href], summary, [tabindex]:not([tabindex="-1"])').filter(
    (el) => !(el as HTMLInputElement).disabled,
  );
  return focusable.find((el) => from.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) ?? null;
}

describe("the access methods are cards", () => {
  it("puts every method in its own bordered card, one radio each, with only the chosen card checked", async () => {
    await github(fakeOAuth());
    const cards = all('[data-slot="access-card"]');
    expect(cards.map((c) => c.querySelector<HTMLInputElement>('input[name="token-mode"]')?.value)).toEqual([
      "oauth",
      "gh",
      "paste",
      "env",
      "command",
    ]);
    for (const card of cards) {
      expect(card.classList.contains("rounded-lg")).toBe(true);
      expect(card.classList.contains("border")).toBe(true);
      expect(card.classList.contains("p-3")).toBe(true);
      expect(card.querySelectorAll('input[name="token-mode"]')).toHaveLength(1);
    }
    for (const mode of ["oauth", "gh", "paste", "env", "command"]) {
      await click(`input[name="token-mode"][value="${mode}"]`);
      const checked = all('[data-slot="access-card"][data-state="checked"]');
      expect(checked, mode).toHaveLength(1);
      expect(checked[0]).toBe(cardOf(radio(mode)));
    }
  });

  it("keeps the chosen method's controls inside its own card", async () => {
    await github(fakeOAuth());
    expect(cardOf(radio("oauth"))).not.toBeNull();
    expect(cardOf(q('[data-action="oauth-sign-in"]'))).toBe(cardOf(radio("oauth")));
    expect(cardOf(q('[data-slot="oauth-panel"]'))).toBe(cardOf(radio("oauth")));
    const controls: [string, string][] = [
      ["paste", "#wizard-secret"],
      ["env", "#wizard-env"],
      ["command", "#wizard-command"],
    ];
    for (const [mode, sel] of controls) {
      await click(`input[name="token-mode"][value="${mode}"]`);
      expect(cardOf(q(sel)), mode).not.toBeNull();
      expect(cardOf(q(sel)), mode).toBe(cardOf(radio(mode)));
      // The others are not rendered at all, so no card carries a stray field.
      for (const [other, otherSel] of controls) if (other !== mode) expect(q(otherSel), other).toBeNull();
      expect(q('[data-action="oauth-sign-in"]')).toBeNull();
    }
  });

  it("says a missing token inside the chosen card, once", async () => {
    await github(fakeOAuth());
    await click('input[name="token-mode"][value="paste"]');
    await click('[data-action="next"]');
    expect(stepId()).toBe("account");
    expect(all("#wizard-token-msg")).toHaveLength(1);
    expect(cardOf(q("#wizard-token-msg"))).not.toBeNull();
    expect(cardOf(q("#wizard-token-msg"))).toBe(cardOf(radio("paste")));
  });

  it("is one native radio group: Tab goes from the chosen radio into its card's control", async () => {
    await github(fakeOAuth());
    const radios = all<HTMLInputElement>('input[name="token-mode"]');
    expect(radios.every((r) => r.type === "radio")).toBe(true);
    expect(new Set(radios.map((r) => r.closest("fieldset")))).toHaveLength(1);
    expect(radios[0].closest("fieldset")?.querySelector("legend")?.textContent?.trim()).toBe("Access");
    // A visible keyboard focus ring on each radio, and a label that names it.
    for (const r of radios) {
      expect(r.className).toContain("focus-visible:outline-2");
      expect(r.closest("label")?.textContent?.trim()).not.toBe("");
    }
    await click('input[name="token-mode"][value="paste"]');
    expect(nextTabStop(radio("paste")!)?.id).toBe("wizard-secret");
    await click('input[name="token-mode"][value="oauth"]');
    expect(nextTabStop(radio("oauth")!)?.getAttribute("data-action")).toBe("oauth-sign-in");
  });
});

describe("the stepper", () => {
  async function toWatch() {
    await github();
    await click('input[name="token-mode"][value="paste"]');
    const secret = q<HTMLInputElement>("#wizard-secret")!;
    secret.value = "a-token";
    secret.dispatchEvent(new Event("input", { bubbles: true }));
    await click('[data-action="next"]');
    const project = q<HTMLInputElement>("#wizard-project-input")!;
    project.value = "acme/web";
    project.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await click('[data-action="resolve-project"]');
    await click('[data-action="next"]');
    expect(stepId()).toBe("watch");
  }

  it("shows the current step's label only, untruncated, and names every step for assistive tech", async () => {
    await toWatch();
    const items = all('[data-slot="stepper-item"]');
    expect(items).toHaveLength(6);
    // Visible text: everything outside the sr-only boxes (a dot shows its number, a done dot a check).
    const visible = (el: Element) => {
      const copy = el.cloneNode(true) as Element;
      for (const hidden of copy.querySelectorAll(".sr-only")) hidden.remove();
      return copy.textContent?.replace(/\s+/g, "") ?? "";
    };
    expect(items.map(visible)).toEqual(["", "", "3Watch", "4", "5", "6"]);
    expect(q('[data-slot="stepper"] .truncate')).toBeNull();
    expect(q('[aria-current="step"]')?.className).toContain("whitespace-nowrap");
    // The accessible names are all still there.
    expect(all<HTMLButtonElement>('[data-slot="stepper"] button').map((b) => b.getAttribute("aria-label"))).toEqual([
      "Account, completed. Go back to this step",
      "Project, completed. Go back to this step",
    ]);
    // Each upcoming or current step is named "Step n of 6: Label", with the space kept.
    expect(items.slice(2).map((li) => li.querySelector(":scope > span:not([aria-hidden])")?.textContent?.trim())).toEqual([
      "Step 3 of 6: Watch",
      "Step 4 of 6: Deploy",
      "Step 5 of 6: Preferences",
      "Step 6 of 6: Review",
    ]);
  });
});

describe("type and width", () => {
  /**
   * Elements asking for a size the root does not map to 13px: text-sm is
   * mapped (the primitives use it), and so is the input's text-base; any
   * other text-base or larger is an oversized body.
   */
  const oversized = () =>
    all('[data-slot="wizard"] [class]')
      .filter((el) => !el.matches('[data-slot="input"]'))
      .filter((el) => /(^|\s)text-(base|lg|xl)(\s|$)/.test(el.getAttribute("class") ?? ""))
      .map((el) => `${el.tagName.toLowerCase()}.${el.getAttribute("class")}`);
  /** Controls with a fixed width that would clip longer text at the narrowest window. */
  const fixedWidths = () =>
    all('[data-slot="wizard"] input, [data-slot="wizard"] select')
      .filter((el) => /(^|\s)w-\d+(\s|$)/.test(el.getAttribute("class") ?? ""))
      .map((el) => `${el.id}.${el.getAttribute("class")}`);

  it("caps the column at 560px with one padding, and sets 13/18 body and a 15/20 title", async () => {
    await github(fakeOAuth());
    const root = q('[data-slot="wizard"]')!;
    expect(root.className).toContain("max-w-[560px]");
    expect(root.classList.contains("p-4")).toBe(false);
    expect(root.className).toContain("text-[13px]/[18px]");
    // The shared primitives' 14px (text-sm) and the input's 16px both map to 13/18 here.
    expect(root.className).toContain("[&_.text-sm]:text-[13px]/[18px]");
    expect(root.className).toContain("[&_[data-slot=input]]:text-[13px]/[18px]");
    expect(q("#wizard-heading")?.className).toContain("text-[15px]/[20px]");
  });

  it.each(["gitlab", "github"])("asks for no other body size, and fixes no control width, on any %s step", async (provider) => {
    component = mount(Wizard, { target: host, props: { api: fakeApi(), oauth: fakeOAuth() } });
    await settle();
    await click(`input[name="provider"][value="${provider}"]`);
    const seen: string[] = [];
    const sizes: string[] = [];
    const widths: string[] = [];
    const record = () => {
      seen.push(stepId() ?? "?");
      sizes.push(...oversized());
      widths.push(...fixedWidths());
    };
    record();
    await click('input[name="token-mode"][value="paste"]');
    const secret = q<HTMLInputElement>("#wizard-secret")!;
    secret.value = "a-token";
    secret.dispatchEvent(new Event("input", { bubbles: true }));
    await click('[data-action="test-connection"]');
    record();
    await click('[data-action="next"]');
    record();
    const project = q<HTMLInputElement>("#wizard-project-input")!;
    project.value = "acme/web";
    project.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await click('[data-action="resolve-project"]');
    record();
    await click('[data-action="next"]');
    // GitLab's preflight branches: the pattern field appears once it is ticked.
    if (provider === "gitlab") {
      await click("#wizard-preflight");
      const pattern = q<HTMLInputElement>("#wizard-preflight-ref")!;
      pattern.value = "pf/*";
      pattern.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
    }
    record();
    await click('[data-action="next"]');
    record();
    await click('input[name="marker-mode"][value="none"]');
    await click('[data-action="next"]');
    record();
    await click('[data-action="next"]');
    record();
    expect(seen).toEqual(["account", "account", "project", "project", "watch", "deploy", "preferences", "review"]);
    expect(sizes).toEqual([]);
    expect(widths).toEqual([]);
  });
});
