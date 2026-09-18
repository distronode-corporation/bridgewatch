import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  applyPlatformTheme,
  detectPlatform,
  normalizePlatform,
  setPlatform,
  setVibrancy,
  VIBRANCY_CLASS,
} from "../index";
// Vite `?raw`: the file as authored, before Tailwind compiles it.
import appCss from "../app.css?raw";

/**
 * The theme setter drives everything in app.css through two things on <html>:
 * `data-platform` and the vibrancy class. These tests use a detached element
 * as the root where they can, and the real documentElement once, because that
 * default is what the shell relies on.
 */

let root: HTMLElement;

beforeEach(() => {
  root = document.createElement("html");
});

afterEach(() => {
  delete document.documentElement.dataset.platform;
  document.documentElement.classList.remove(VIBRANCY_CLASS);
});

describe("normalizePlatform", () => {
  it.each([
    ["macos", "macos"],
    ["Darwin", "macos"],
    ["  MAC ", "macos"],
    ["linux", "linux"],
    ["freebsd", "linux"],
    ["windows", "other"],
    ["", "other"],
    [null, "other"],
    [undefined, "other"],
  ] as const)("%j -> %s", (input, expected) => {
    expect(normalizePlatform(input)).toBe(expected);
  });
});

describe("detectPlatform", () => {
  it("reads the user agent of both webviews", () => {
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe("macos");
    expect(detectPlatform("Mozilla/5.0 (X11; Ubuntu; Linux x86_64) AppleWebKit/605.1.15")).toBe("linux");
    expect(detectPlatform("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36")).toBe("other");
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("other");
  });
});

describe("setPlatform", () => {
  it("sets html[data-platform] and returns what it applied", () => {
    expect(setPlatform("darwin", root)).toBe("macos");
    expect(root.getAttribute("data-platform")).toBe("macos");
    expect(setPlatform("linux", root)).toBe("linux");
    expect(root.getAttribute("data-platform")).toBe("linux");
    expect(setPlatform("haiku", root)).toBe("other");
    expect(root.getAttribute("data-platform")).toBe("other");
  });

  it("defaults to the document element", () => {
    setPlatform("linux");
    expect(document.documentElement.getAttribute("data-platform")).toBe("linux");
  });

  it("drops a leftover vibrancy class when leaving macOS", () => {
    setPlatform("macos", root);
    setVibrancy(true, root);
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(true);
    setPlatform("linux", root);
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(false);
  });
});

describe("setVibrancy", () => {
  it("is the plain `vibrancy` class the stylesheet keys on", () => {
    expect(VIBRANCY_CLASS).toBe("vibrancy");
  });

  it("only turns on under macOS", () => {
    setPlatform("linux", root);
    expect(setVibrancy(true, root)).toBe(false);
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(false);

    setPlatform("macos", root);
    expect(setVibrancy(true, root)).toBe(true);
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(true);

    expect(setVibrancy(false, root)).toBe(false);
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(false);
  });
});

describe("applyPlatformTheme", () => {
  it("sets platform and vibrancy in one call, vibrancy off by default", () => {
    expect(applyPlatformTheme({ platform: "macos", root })).toEqual({ platform: "macos", vibrancy: false });
    expect(applyPlatformTheme({ platform: "macos", vibrancy: true, root })).toEqual({
      platform: "macos",
      vibrancy: true,
    });
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(true);
    expect(applyPlatformTheme({ platform: "linux", vibrancy: true, root })).toEqual({
      platform: "linux",
      vibrancy: false,
    });
    expect(root.dataset.platform).toBe("linux");
    expect(root.classList.contains(VIBRANCY_CLASS)).toBe(false);
  });
});

/**
 * The stylesheet side of the same contract, read as text: every selector the
 * setter feeds must exist, and our own authored CSS must stay inside what
 * WebKitGTK 2.36 (Ubuntu 22.04) renders. Comments are stripped first, because
 * the compatibility notes at the bottom of app.css name the very features
 * they avoid.
 */
describe("app.css", () => {
  const css = appCss.replace(/\/\*[\s\S]*?\*\//g, "");

  it("keys on every value the setter writes", () => {
    expect(css).toContain('html[data-platform="macos"]');
    expect(css).toContain('html[data-platform="linux"]');
    expect(css).toContain(`html.${VIBRANCY_CLASS}[data-platform="macos"]`);
    expect(css).toMatch(/@media \(prefers-color-scheme: dark\)/);
    expect(css).toMatch(/Cantarell/);
    expect(css).toMatch(/-apple-system-control-accent/);
  });

  it.each([
    ["oklch()", /oklch\(/],
    ["oklab()", /oklab\(/],
    ["color-mix()", /color-mix\(/],
    [":has()", /:has\(/],
    ["@property", /@property/],
    ["@container", /@container/],
    ["dynamic viewport units", /\d(dvh|svh|lvh)\b/],
    ["relative colour syntax", /\bfrom\s+var\(/],
  ])("does not author %s", (_name, pattern) => {
    expect(css).not.toMatch(pattern);
  });
});
