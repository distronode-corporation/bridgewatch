import { describe, expect, it } from "vitest";

import { navigableRows, popoverKey, rowTarget } from "./keyboard";

/** A list of three rows like the popover draws: each with a commit link and a tool. */
function list(): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = `
    <header><button class="refresh">refresh</button></header>
    <div data-scroll>
      <section class="watch">
        <div class="row" id="r1"><a class="sha" href="https://example.test/1">1111111</a><button>log</button></div>
        <div class="row" id="r2"><a class="sha" href="https://example.test/2">2222222</a><button>log</button></div>
      </section>
      <section class="watch">
        <div class="row" id="r3"><span class="sha">3333333</span><button>copy</button></div>
      </section>
    </div>
    <input id="field" />`;
  return root;
}

const key = (k: string, mods: Partial<Record<"ctrlKey" | "altKey" | "metaKey", boolean>> = {}) => ({
  key: k,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe("popoverKey", () => {
  const root = list();
  const rows = navigableRows(root);
  const sha = (n: number) => root.querySelector<HTMLElement>(`#r${n} .sha, #r${n} a`)!;
  const focusOf = (action: ReturnType<typeof popoverKey>) =>
    action?.kind === "focus" ? action.target : action?.kind ?? null;

  it("finds the rows in drawn order, across watches", () => {
    expect(rows.map((r) => r.id)).toEqual(["r1", "r2", "r3"]);
  });

  it("lands on the first row with ↓ and the last with ↑ when nothing in the list has focus", () => {
    expect(focusOf(popoverKey(key("ArrowDown"), rows, null))).toBe(sha(1));
    expect(focusOf(popoverKey(key("j"), rows, root.querySelector(".refresh")))).toBe(sha(1));
    expect(focusOf(popoverKey(key("ArrowUp"), rows, null))).toBe(root.querySelector("#r3 button"));
    expect(focusOf(popoverKey(key("k"), rows, null))).toBe(root.querySelector("#r3 button"));
  });

  it("moves one row from anywhere inside the focused row, and stops at the ends", () => {
    const inRow1 = root.querySelector("#r1 button");
    expect(focusOf(popoverKey(key("ArrowDown"), rows, inRow1))).toBe(sha(2));
    expect(focusOf(popoverKey(key("k"), rows, sha(2)))).toBe(sha(1));
    expect(focusOf(popoverKey(key("ArrowUp"), rows, sha(1)))).toBe(sha(1));
    expect(focusOf(popoverKey(key("j"), rows, root.querySelector("#r3 button")))).toBe(
      root.querySelector("#r3 button"),
    );
  });

  it("jumps with Home and End", () => {
    expect(focusOf(popoverKey(key("End"), rows, sha(1)))).toBe(root.querySelector("#r3 button"));
    expect(focusOf(popoverKey(key("Home"), rows, sha(2)))).toBe(sha(1));
  });

  it("refreshes on r", () => {
    expect(popoverKey(key("r"), rows, null)).toEqual({ kind: "refresh" });
  });

  it("leaves modified keys, other keys and typing in a field alone", () => {
    expect(popoverKey(key("ArrowDown", { ctrlKey: true }), rows, null)).toBeNull();
    expect(popoverKey(key("r", { metaKey: true }), rows, null)).toBeNull();
    expect(popoverKey(key("j", { altKey: true }), rows, null)).toBeNull();
    expect(popoverKey(key("x"), rows, null)).toBeNull();
    expect(popoverKey(key("Tab"), rows, null)).toBeNull();
    const field = root.querySelector("#field");
    expect(popoverKey(key("j"), rows, field)).toBeNull();
    expect(popoverKey(key("r"), rows, field)).toBeNull();
  });

  it("does nothing with no rows to move between", () => {
    expect(popoverKey(key("ArrowDown"), [], null)).toBeNull();
  });
});

describe("rowTarget", () => {
  it("prefers the commit link, then any link or button, then the row itself", () => {
    const row = document.createElement("div");
    expect(rowTarget(row)).toBe(row);
    row.innerHTML = `<button>log</button>`;
    expect(rowTarget(row)).toBe(row.querySelector("button"));
    row.innerHTML = `<button>log</button><a class="sha" href="https://example.test">abc</a>`;
    expect(rowTarget(row)).toBe(row.querySelector("a.sha"));
  });
});
