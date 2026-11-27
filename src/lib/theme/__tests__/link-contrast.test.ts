// SPDX-FileCopyrightText: Distronode Corporation
//
// SPDX-License-Identifier: Apache-2.0

import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it } from "vitest";

// The components, not the barrels: plain `tsc` cannot see `<script module>`
// exports, which is why tsconfig.tsc.json leaves the ui index.ts files out.
import Badge from "$lib/components/ui/badge/badge.svelte";
import Button from "$lib/components/ui/button/button.svelte";
// Vite `?raw`: the file as authored, before Tailwind compiles it.
import appCss from "../app.css?raw";

/**
 * The link variants' text colour, resolved from app.css the way the cascade
 * resolves it on <html>, and held to WCAG 1.4.3 (4.5:1) in Linux dark.
 *
 * ⛔ With --primary at Adwaita blue 4 (#1c71d8), so that white button text
 * reaches 4.5:1, link text in that colour is 3.32:1 on the Linux dark window
 * (#222226). No one colour does both there, so Linux dark gives the link
 * variants Adwaita's accent text colour, and every other theme keeps them on
 * --primary exactly as before.
 */

type Platform = "linux" | "macos" | "other";
interface Theme {
  platform: Platform;
  dark: boolean;
}

interface Block {
  /** The at-rule preludes around the block, outermost first, then its selector. */
  context: string[];
  body: string;
}

const css = appCss.replace(/\/\*[\s\S]*?\*\//g, "");

/** Every innermost block, with the preludes it sits in. */
function blocks(source: string, context: string[] = []): Block[] {
  const found: Block[] = [];
  let depth = 0;
  let start = 0;
  let prelude = "";
  let last = 0;
  for (let i = 0; i < source.length; i++) {
    const c = source[i];
    if (c === "{") {
      if (depth === 0) {
        prelude = source.slice(last, i).split(";").pop()!.trim();
        start = i + 1;
      }
      depth++;
    } else if (c === "}") {
      depth--;
      if (depth === 0) {
        const body = source.slice(start, i);
        if (body.includes("{")) found.push(...blocks(body, [...context, prelude]));
        else found.push({ context: [...context, prelude], body });
        last = i + 1;
      }
    }
  }
  return found;
}

function declarations(body: string): [string, string][] {
  return [...body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]);
}

const ALL = blocks(css);

/** Whether a block applies to <html> in this theme, and how specific its selector is. */
function applies(block: Block, theme: Theme): number | null {
  const selector = block.context[block.context.length - 1];
  for (const prelude of block.context.slice(0, -1)) {
    if (prelude === "@media (prefers-color-scheme: dark)") {
      if (!theme.dark) return null;
    } else if (!prelude.startsWith("@supports")) {
      // Width and motion queries, @layer, @theme, @keyframes: not <html>'s palette.
      return null;
    }
  }
  if (selector === ":root") return 1;
  if (selector === `html[data-platform="${theme.platform}"]`) return 2;
  return null;
}

/** The custom properties on <html> in this theme, var() references followed. */
function resolve(name: string, theme: Theme): string {
  const winner = new Map<string, string>();
  const matching = ALL.map((block, order) => ({ block, order, rank: applies(block, theme) }))
    .filter((b) => b.rank !== null)
    .sort((a, b) => a.rank! - b.rank! || a.order - b.order);
  for (const { block } of matching) for (const [k, v] of declarations(block.body)) winner.set(k, v);
  let value = winner.get(name);
  for (let hops = 0; value?.startsWith("var("); hops++) {
    if (hops > 8) throw new Error(`${name}: var() loop`);
    value = winner.get(value.slice(4, -1).trim());
  }
  if (value === undefined) throw new Error(`${name} is not defined for ${JSON.stringify(theme)}`);
  return value;
}

/** The token map: `--color-<name>` (what `text-<name>` paints with) to its custom property. */
const COLOURS = new Map(
  declarations(ALL.find((b) => b.context[b.context.length - 1] === "@theme inline")!.body),
);

/** `text-<name>` to the custom property @theme inline maps `--color-<name>` to. */
function colourOf(textClass: string): string {
  const mapped = COLOURS.get(`--color-${textClass.slice("text-".length)}`);
  expect(mapped, `${textClass} has no --color- mapping`).toMatch(/^var\(--[\w-]+\)$/);
  return mapped!.slice(4, -1);
}

function luminance(hex: string): number {
  const channel = (i: number) => {
    const c = parseInt(hex.slice(1 + 2 * i, 3 + 2 * i), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2);
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** The one text colour class the link variant renders with. */
function linkTextClass(component: typeof Button | typeof Badge, tag: string): string {
  const host = document.createElement("div");
  document.body.append(host);
  const mounted = mount(component, { target: host, props: { variant: "link" } });
  flushSync();
  // Colour classes only: `text-xs` is a size, and has no --color-xs.
  const colours = [...host.querySelector(tag)!.classList].filter(
    (c) => c.startsWith("text-") && COLOURS.has(`--color-${c.slice("text-".length)}`),
  );
  void unmount(mounted);
  host.remove();
  expect(colours).toHaveLength(1);
  return colours[0];
}

const LINUX_DARK: Theme = { platform: "linux", dark: true };
const OTHERS: Theme[] = [
  { platform: "linux", dark: false },
  { platform: "macos", dark: false },
  { platform: "macos", dark: true },
  { platform: "other", dark: false },
  { platform: "other", dark: true },
];

describe.each([
  ["Button", Button, "button"],
  ["Badge", Badge, "span"],
] as const)("the %s link variant", (_name, component, tag) => {
  const property = colourOf(linkTextClass(component, tag));

  it("reaches 4.5:1 on the Linux dark window", () => {
    const background = resolve("--background", LINUX_DARK);
    expect(background).toBe("#222226");
    const text = resolve(property, LINUX_DARK);
    expect(text).toMatch(/^#[0-9a-f]{6}$/i);
    expect(contrast(text, background)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(OTHERS)("is the primary colour, unchanged, in %o", (theme) => {
    expect(resolve(property, theme)).toBe(resolve("--primary", theme));
  });
});

describe("the Linux dark primary", () => {
  it("stays Adwaita blue 4, so white button text keeps its 4.5:1", () => {
    const primary = resolve("--primary", LINUX_DARK);
    expect(primary).toBe("#1c71d8");
    expect(contrast(resolve("--primary-foreground", LINUX_DARK), primary)).toBeGreaterThanOrEqual(4.5);
  });
});
