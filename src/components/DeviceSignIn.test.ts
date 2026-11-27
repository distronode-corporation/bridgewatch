import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import type { OAuthApi, StartedSignIn } from "../lib/oauth";
import DeviceSignIn from "./DeviceSignIn.svelte";

/**
 * The device flow's lifecycle at its edges: a component that goes away, or is
 * clicked twice, while the shell is still answering.
 *
 * ⛔ Each case here left the shell polling the provider's token endpoint until
 * the code expired (about 180 requests over 15 minutes) for a sign-in nobody
 * could finish, and the progress listener subscribed for the life of the window.
 */

const STARTED: StartedSignIn = {
  id: "flow-1",
  user_code: "WDJB-MJHT",
  verification_uri: "https://github.com/login/device",
  expires_in: 900,
  host: "github.com",
};

type Fake = { [K in keyof Required<OAuthApi>]: Mock };

/** A promise and the hand that settles it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function fake(): Fake {
  return {
    availability: vi.fn(),
    start: vi.fn(async () => STARTED),
    wait: vi.fn(() => new Promise(() => {})),
    cancel: vi.fn(async () => {}),
    openVerification: vi.fn(async () => {}),
    openInstall: vi.fn(async () => {}),
    status: vi.fn(async () => null),
    signOut: vi.fn(async () => {}),
    copy: vi.fn(async () => true),
    onProgress: vi.fn(async () => () => {}),
  };
}

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function render(api: Fake) {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(DeviceSignIn, {
    target: host,
    props: {
      api,
      request: () => ({ account: "gh", provider: "github", base_url: "https://api.github.com", client_id: null }),
    },
  });
  flushSync();
  return host.querySelector<HTMLButtonElement>('[data-action="oauth-sign-in"]')!;
}

function destroy() {
  void unmount(component!);
  component = null;
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
  flushSync();
}

describe("DeviceSignIn, going away mid-flow", () => {
  it("cancels a flow whose start answers after the component is gone", async () => {
    // The user switched token source (or provider) while it said "Starting…".
    const api = fake();
    const start = deferred<StartedSignIn>();
    api.start.mockImplementation(() => start.promise);
    render(api).click();
    destroy();
    start.resolve(STARTED);
    await settle();
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
    expect(api.wait).not.toHaveBeenCalled();
    expect(api.onProgress).not.toHaveBeenCalled();
  });

  it("drops a progress listener that subscribes after the component is gone", async () => {
    const api = fake();
    const off = vi.fn();
    const subscribed = deferred<() => void>();
    api.onProgress.mockImplementation(() => subscribed.promise);
    render(api).click();
    await settle();
    destroy();
    subscribed.resolve(off);
    await settle();
    expect(off).toHaveBeenCalledTimes(1);
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
    expect(api.wait).not.toHaveBeenCalled();
  });

  it("unsubscribes and cancels when it goes away while waiting", async () => {
    const api = fake();
    const off = vi.fn();
    api.onProgress.mockImplementation(async () => off);
    render(api).click();
    await settle();
    expect(api.wait).toHaveBeenCalledWith("flow-1");
    destroy();
    expect(off).toHaveBeenCalledTimes(1);
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
  });
});

describe("DeviceSignIn, a double click", () => {
  it("starts one flow", async () => {
    // Both clicks land before the re-render that disables the button.
    const api = fake();
    const start = deferred<StartedSignIn>();
    api.start.mockImplementation(() => start.promise);
    const button = render(api);
    button.click();
    button.click();
    expect(api.start).toHaveBeenCalledTimes(1);
    start.resolve(STARTED);
    await settle();
    expect(api.cancel).not.toHaveBeenCalled();
    expect(host.querySelector('[data-slot="user-code"]')?.textContent).toBe("WDJB-MJHT");
  });
});

describe("DeviceSignIn, the rest of the flow", () => {
  const button = (action: string) => host.querySelector<HTMLButtonElement>(`[data-action="${action}"]`)!;

  it("counts the polls of its own flow, and ignores another flow's and a slow-down", async () => {
    const api = fake();
    let report!: (p: unknown) => void;
    api.onProgress.mockImplementation(async (handler: (p: unknown) => void) => {
      report = handler;
      return () => {};
    });
    render(api).click();
    await settle();
    report({ id: "flow-1", state: "waiting", polls: 1 });
    flushSync();
    expect(host.querySelector('[data-slot="sign-in-waiting"]')?.textContent).toContain("(checked 1 time)");
    report({ id: "flow-1", state: "waiting", polls: 3 });
    report({ id: "another-flow", state: "waiting", polls: 9 });
    report({ id: "flow-1", state: "slowed_down", interval: 10 });
    flushSync();
    expect(host.querySelector('[data-slot="sign-in-waiting"]')?.textContent).toContain("(checked 3 times)");
  });

  it("copies the code and opens the page for the flow on screen, and survives a refused open", async () => {
    const api = fake();
    api.openVerification.mockRejectedValue(new Error("no browser"));
    render(api).click();
    await settle();
    button("copy-code").click();
    await settle();
    expect(api.copy).toHaveBeenCalledWith("WDJB-MJHT");
    expect(button("copy-code").textContent?.trim()).toBe("Copied");
    button("open-verification").click();
    await settle();
    expect(api.openVerification).toHaveBeenCalledWith("flow-1");
  });

  it("drops a flow cancelled while its progress listener was being set up, and one cancelled while waiting", async () => {
    const api = fake();
    const subscribed = deferred<() => void>();
    api.onProgress.mockImplementationOnce(() => subscribed.promise);
    render(api).click();
    await settle();
    button("cancel-sign-in").click();
    subscribed.resolve(() => {});
    await settle();
    expect(api.wait).not.toHaveBeenCalled();
    // A second run, cancelled while the shell waits; its late answer changes nothing.
    const signed = deferred<{ login: string; scopes: string[]; expires_at: null }>();
    api.wait.mockImplementationOnce(() => signed.promise);
    button("oauth-sign-in").click();
    await settle();
    button("cancel-sign-in").click();
    signed.resolve({ login: "octocat", scopes: [], expires_at: null });
    await settle();
    expect(host.querySelector('[data-action="oauth-sign-in"]')?.textContent?.trim()).toBe("Sign in with GitHub");
  });

  it("stops a flow that has not started yet, and lets a refused cancel go", async () => {
    const api = fake();
    const start = deferred<StartedSignIn>();
    api.start.mockImplementation(() => start.promise);
    api.cancel.mockRejectedValue(new Error("already gone"));
    render(api).click();
    await settle();
    // No code yet, so Cancel has nothing to tell the shell; the start that lands later is cancelled.
    expect(host.querySelector('[data-action="cancel-sign-in"]')).toBeNull();
    destroy();
    start.resolve(STARTED);
    await settle();
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
  });

  it("names the provider generically when the flow failed before any host was known", async () => {
    const api = fake();
    api.start.mockRejectedValue({ kind: "denied", message: "access_denied" });
    render(api).click();
    await settle();
    expect(host.textContent).toContain("You cancelled the sign-in on the provider.");
  });

  it("cancels a waiting flow when the window goes away, even if the shell refuses", async () => {
    const api = fake();
    api.cancel.mockRejectedValue(new Error("already gone"));
    render(api).click();
    await settle();
    destroy();
    await settle();
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
  });

  it("tells the shell to stop when Cancel is pressed on a waiting flow, even if it refuses", async () => {
    const api = fake();
    api.cancel.mockRejectedValue(new Error("already gone"));
    render(api).click();
    await settle();
    button("cancel-sign-in").click();
    await settle();
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
  });

  it("reports a finished sign-in, and drops a refusal that arrives after Cancel", async () => {
    const api = fake();
    const onsignedin = vi.fn();
    api.wait.mockImplementationOnce(async () => ({ login: "octocat", scopes: [], expires_at: null }));
    host = document.createElement("div");
    document.body.append(host);
    component = mount(DeviceSignIn, {
      target: host,
      props: {
        api,
        onsignedin,
        request: () => ({ account: "gh", provider: "github", base_url: "https://api.github.com", client_id: null }),
      },
    });
    flushSync();
    button("oauth-sign-in").click();
    await settle();
    expect(onsignedin).toHaveBeenCalledWith({ login: "octocat", scopes: [], expires_at: null });
    let refuse!: (e: unknown) => void;
    api.wait.mockImplementationOnce(() => new Promise((_, reject) => (refuse = reject)));
    button("oauth-sign-in").click();
    await settle();
    button("cancel-sign-in").click();
    refuse({ kind: "expired", message: "expired" });
    await settle();
    expect(host.textContent).not.toContain("The code expired");
  });
});
