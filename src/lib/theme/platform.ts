/**
 * Platform theming: which palette, font and radius set the page uses.
 *
 * The stylesheet (./app.css) keys everything off two things on <html>:
 *
 * - `data-platform="macos" | "linux" | "other"` — system font and accent on
 *   macOS, Adwaita-like colours, Cantarell and radii on Linux, plain Rhea
 *   otherwise.
 * - the `vibrancy` class — a transparent page so a native macOS window
 *   effect shows through. Ignored by the CSS on any other platform.
 *
 * Light/dark is NOT set here: it follows `prefers-color-scheme`, which both
 * webviews derive from the OS setting.
 *
 * The shell calls `applyPlatformTheme` once at startup with the platform it
 * already knows (`Status.platform`); `detectPlatform` is only the fallback for
 * `vite dev` in a plain browser.
 */

export type Platform = "macos" | "linux" | "other";

/** The class that makes the page transparent for macOS vibrancy. */
export const VIBRANCY_CLASS = "vibrancy";

/**
 * Map whatever the shell reports onto a theme platform.
 *
 * Accepts Rust's `std::env::consts::OS` ("macos", "linux"), Tauri's os plugin
 * names and the common aliases, case-insensitively. Anything else is "other".
 */
export function normalizePlatform(name: string | null | undefined): Platform {
  const value = (name ?? "").trim().toLowerCase();
  if (value === "macos" || value === "darwin" || value === "mac" || value === "osx") return "macos";
  if (value === "linux" || value.endsWith("bsd")) return "linux";
  return "other";
}

/** Best-effort guess from the user agent, for a plain browser during development. */
export function detectPlatform(userAgent: string = globalThis.navigator?.userAgent ?? ""): Platform {
  if (/Mac OS X|Macintosh/i.test(userAgent)) return "macos";
  if (/Linux|X11|BSD/i.test(userAgent) && !/Android/i.test(userAgent)) return "linux";
  return "other";
}

/** Set `html[data-platform]`. Returns the platform actually applied. */
export function setPlatform(
  name: string | null | undefined,
  root: HTMLElement = document.documentElement,
): Platform {
  const platform = normalizePlatform(name);
  root.dataset.platform = platform;
  // A vibrancy class left over from a previous call must not outlive macOS.
  if (platform !== "macos") root.classList.remove(VIBRANCY_CLASS);
  return platform;
}

/**
 * Turn the transparent (vibrancy) background on or off.
 *
 * Only takes effect on macOS; elsewhere the class is removed, because a
 * transparent WebKitGTK window shows the desktop, not a material. Returns
 * whether vibrancy is now on.
 */
export function setVibrancy(enabled: boolean, root: HTMLElement = document.documentElement): boolean {
  const on = enabled && root.dataset.platform === "macos";
  root.classList.toggle(VIBRANCY_CLASS, on);
  return on;
}

export interface PlatformThemeOptions {
  /** `Status.platform` or any name `normalizePlatform` accepts. Omit to detect. */
  platform?: string | null;
  /** macOS popover vibrancy. Default off. */
  vibrancy?: boolean;
  root?: HTMLElement;
}

/** One call for the shell: platform attribute plus vibrancy. */
export function applyPlatformTheme(options: PlatformThemeOptions = {}): {
  platform: Platform;
  vibrancy: boolean;
} {
  const root = options.root ?? document.documentElement;
  const platform = setPlatform(options.platform ?? detectPlatform(), root);
  const vibrancy = setVibrancy(options.vibrancy ?? false, root);
  return { platform, vibrancy };
}
