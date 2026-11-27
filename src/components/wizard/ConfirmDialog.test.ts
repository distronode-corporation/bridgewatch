// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import ConfirmDialog from "./ConfirmDialog.svelte";

let component: Record<string, unknown> | null = null;
let host: HTMLDivElement | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(overrides: Record<string, unknown> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  const props = reactive({ open: true, title: "Run this command?", lines: ["It prints a token."], onConfirm, onCancel, ...overrides });
  host = document.createElement("div");
  document.body.append(host);
  component = mount(ConfirmDialog, { target: host, props });
  flushSync();
  return { props, onConfirm, onCancel };
}

const dialog = () => document.querySelector<HTMLElement>('[data-slot="confirm-dialog"]');

describe("ConfirmDialog", () => {
  it("cancels when Escape closes it", () => {
    const { onCancel, onConfirm } = render();
    dialog()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("asks for nothing when its parent opens or closes it", () => {
    const { props, onCancel } = render({ open: false });
    props.open = true;
    flushSync();
    expect(dialog()).not.toBeNull();
    props.open = false;
    flushSync();
    expect(onCancel).not.toHaveBeenCalled();
  });
});
