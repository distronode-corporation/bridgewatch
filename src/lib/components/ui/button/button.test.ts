import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it } from "vitest";

// The component, not the barrel: plain `tsc` cannot see `<script module>`
// exports, which is why tsconfig.tsc.json leaves the ui index.ts files out.
import Button from "./button.svelte";

/** Mount a button of one size and read the classes it rendered. */
function classesOf(size: "sm" | "xs"): string[] {
  const host = document.createElement("div");
  document.body.append(host);
  const component = mount(Button, { target: host, props: { size } });
  flushSync();
  const classes = [...host.querySelector("button")!.classList];
  void unmount(component);
  host.remove();
  return classes;
}

describe("button sizes on the type scale", () => {
  it("renders `sm` at 13px, the body size, not the base 14px", () => {
    // Settings' actions at 14px looked louder than its 12px tabs.
    const classes = classesOf("sm");
    expect(classes).toContain("text-[13px]");
    expect(classes).not.toContain("text-sm");
  });

  it("leaves `xs` at 12px", () => {
    const classes = classesOf("xs");
    expect(classes).toContain("text-xs");
    expect(classes).not.toContain("text-sm");
  });
});
