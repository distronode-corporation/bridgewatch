import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import Ask from "./Ask.svelte";

/**
 * The in-document question. It has to take focus when it appears: a keyboard
 * user who pressed "Remove watch" is otherwise left behind the question, with
 * Escape the only key that reaches it.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function render(props: Record<string, unknown>) {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(Ask, {
    target: host,
    props: { title: "Remove watch", onconfirm: vi.fn(), oncancel: vi.fn(), ...props },
  });
  flushSync();
}

describe("Ask", () => {
  it("focuses Cancel when there is no field to type in", () => {
    render({ message: "The watch web goes.", danger: true });
    expect(document.activeElement).toBe(host.querySelector("button.cancel"));
  });

  it("focuses the first field when there is one", () => {
    render({ fields: [{ name: "id", label: "Watch id" }] });
    expect(document.activeElement).toBe(host.querySelector("input"));
  });

  it("is described by its message, and is not marked modal", () => {
    render({ message: "The watch web goes." });
    const dialog = host.querySelector('[role="dialog"]') as HTMLElement;
    expect(dialog.hasAttribute("aria-modal")).toBe(false);
    const id = dialog.getAttribute("aria-describedby");
    expect(id).toBeTruthy();
    expect(document.getElementById(id as string)?.textContent).toBe("The watch web goes.");
  });

  it("has no description when there is no message", () => {
    render({});
    expect(host.querySelector('[role="dialog"]')?.hasAttribute("aria-describedby")).toBe(false);
  });
});
