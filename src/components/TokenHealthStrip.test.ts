import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import fixture from "../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { Snapshot, TokenHealth } from "../lib/types";
import Popover from "./Popover.svelte";
import TokenHealthStrip from "./TokenHealthStrip.svelte";

/**
 * Token health in the popover (F3): the core lists only the accounts with
 * something to say, and the strip shows each one's sentence for as long as it
 * is true. Expiring is a warning; expired, revoked and invalid are alerts.
 */

const base = fixture as unknown as Snapshot;

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function health(state: TokenHealth["state"], extra: Partial<TokenHealth> = {}): TokenHealth {
  return {
    account: "lab",
    state,
    of: "token",
    expires_at: 1_800_000_000,
    warn_hours: state === "expiring" ? 24 : null,
    message: `Account "lab": ${state}.`,
    ...extra,
  };
}

function strip(tokens: TokenHealth[] | undefined, onsettings?: () => void) {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(TokenHealthStrip, { target: host, props: { tokens, onsettings } });
  flushSync();
  return host.querySelector<HTMLElement>("[data-slot=token-health]");
}

describe("TokenHealthStrip", () => {
  it("is absent when every token is healthy", () => {
    expect(strip(undefined)).toBeNull();
    expect(strip([])).toBeNull();
  });

  it("warns (a status, amber) while a token is only expiring", () => {
    const el = strip([health("expiring")])!;
    expect(el.getAttribute("role")).toBe("status");
    expect(el.className).toContain("text-tone-amber");
    const line = el.querySelector("[data-line]")!;
    expect(line.textContent?.trim()).toBe('Account "lab": expiring.');
    expect(line.getAttribute("data-warn")).toBe("24");
  });

  it.each(["expired", "revoked", "invalid"] as const)("alerts (red) when a token is %s", (state) => {
    const el = strip([health("expiring", { account: "a" }), health(state, { account: "b" })])!;
    expect(el.getAttribute("role")).toBe("alert");
    expect(el.className).toContain("text-tone-red");
    expect([...el.querySelectorAll("[data-line]")].map((l) => l.getAttribute("data-state"))).toEqual([
      "expiring",
      state,
    ]);
  });

  it("opens Settings from its button", () => {
    const onsettings = vi.fn();
    strip([health("revoked")], onsettings)!.querySelector<HTMLButtonElement>("[data-action=token-settings]")!.click();
    expect(onsettings).toHaveBeenCalledOnce();
  });
});

describe("Popover, token health", () => {
  it("draws the strip from the snapshot's tokens", () => {
    host = document.createElement("div");
    document.body.append(host);
    component = mount(Popover, {
      target: host,
      props: { snapshot: { ...base, tokens: [health("expired")] }, now: Date.parse("2026-09-17T15:00:00Z") },
    });
    flushSync();
    expect(host.querySelector("[data-slot=token-health] [data-line]")?.textContent?.trim()).toBe(
      'Account "lab": expired.',
    );
  });

  it("draws no strip for a snapshot without tokens", () => {
    host = document.createElement("div");
    document.body.append(host);
    component = mount(Popover, { target: host, props: { snapshot: base, now: Date.parse("2026-09-17T15:00:00Z") } });
    flushSync();
    expect(host.querySelector("[data-slot=token-health]")).toBeNull();
  });
});
