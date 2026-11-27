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

/**
 * Taking focus means giving it back: a question that closes with Cancel
 * focused would otherwise drop the keyboard on the body, at the top of the
 * window, rather than where the user was.
 */
describe("Ask, going away", () => {
  let outside: HTMLElement[] = [];

  afterEach(() => {
    for (const element of outside) element.remove();
    outside = [];
  });

  function button(): HTMLButtonElement {
    const element = document.createElement("button");
    document.body.append(element);
    outside.push(element);
    return element;
  }

  async function close() {
    void unmount(component!);
    component = null;
    // The hand-back waits a microtask, for the question's nodes to go.
    await Promise.resolve();
  }

  it("gives focus back to the control that asked", async () => {
    const opener = button();
    opener.focus();
    render({ message: "The watch web goes.", danger: true });
    expect(document.activeElement).toBe(host.querySelector("button.cancel"));
    await close();
    expect(document.activeElement).toBe(opener);
  });

  it("leaves focus on the body when the control that asked has gone", async () => {
    const opener = button();
    opener.focus();
    render({ danger: true });
    // A confirmed removal takes the watch, and its Remove button, with it.
    opener.remove();
    await close();
    expect(document.activeElement).toBe(document.body);
  });

  it("does not take focus back from where it has gone since", async () => {
    const opener = button();
    const elsewhere = button();
    opener.focus();
    render({ danger: true });
    elsewhere.focus();
    await close();
    expect(document.activeElement).toBe(elsewhere);
  });
});

describe("Ask from the keyboard", () => {
  const fields = [
    { name: "id", label: "Watch id", placeholder: "main-push" },
    { name: "project", label: "Project" },
  ];

  function type(input: HTMLInputElement, text: string) {
    input.value = text;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
  }

  const press = (target: EventTarget, key: string) => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    flushSync();
  };

  it("confirms with Enter from a field once every field is filled, trimmed", () => {
    const onconfirm = vi.fn();
    render({ fields, onconfirm });
    const [id, project] = [...host.querySelectorAll<HTMLInputElement>("input")];
    expect(project.placeholder).toBe("");
    type(id, " web ");
    press(id, "Enter");
    expect(onconfirm).not.toHaveBeenCalled();
    type(project, " acme/web ");
    press(project, "Enter");
    expect(onconfirm).toHaveBeenCalledWith({ id: "web", project: "acme/web" });
  });

  it("ignores Enter outside a field, and any other key", () => {
    const onconfirm = vi.fn();
    const oncancel = vi.fn();
    render({ fields: [fields[0]], onconfirm, oncancel });
    type(host.querySelector("input")!, "web");
    press(host.querySelector("button.cancel")!, "Enter");
    press(host.querySelector("input")!, "a");
    expect(onconfirm).not.toHaveBeenCalled();
    expect(oncancel).not.toHaveBeenCalled();
    press(host.querySelector("input")!, "Escape");
    expect(oncancel).toHaveBeenCalledTimes(1);
  });
});
