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
