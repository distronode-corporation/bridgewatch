/**
 * Every window loads the same document; the window's LABEL decides which one
 * it is.
 *
 * One HTML entry rather than two keeps the vite build to a single bundle, which
 * is also what makes the popover instant: the settings window's code is already
 * parsed by the time it is shown.
 */

import { mount } from "svelte";

import "./lib/theme/app.css";
import { getStatus, inTauri, readThemeCss } from "./lib/ipc";
import { applyPlatformTheme, detectPlatform } from "./lib/theme";
import PopoverApp from "./PopoverApp.svelte";
import SettingsApp from "./SettingsApp.svelte";
import WizardApp from "./WizardApp.svelte";

/**
 * The current window's label, without importing the Tauri window API when
 * there is no Tauri.
 *
 * `getCurrentWindow()` reads `__TAURI_INTERNALS__.metadata` synchronously, so
 * reading it directly avoids an import that throws in a plain browser during
 * `vite dev`.
 */
function windowLabel(): string {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } };
    }
  ).__TAURI_INTERNALS__;
  return internals?.metadata?.currentWindow?.label ?? "popover";
}

/**
 * Append `[ui].theme_css` as a user stylesheet.
 *
 * Last in the cascade, so it only has to restate the custom properties it wants
 * to change. The Rust side hands over the TEXT, not a path: a `<link>` to a
 * file would need Tauri's `asset:` protocol enabled and scoped, which is a
 * filesystem grant to the webview for the sake of one optional file.
 */
async function applyThemeCss(): Promise<void> {
  if (!inTauri()) return;
  try {
    const css = await readThemeCss();
    if (!css) return;
    const style = document.createElement("style");
    style.dataset.source = "ui.theme_css";
    style.textContent = css;
    document.head.append(style);
  } catch (error) {
    console.error("could not apply [ui].theme_css", error);
  }
}

const target = document.getElementById("app");
if (!target) throw new Error("#app is missing from index.html");

const label = windowLabel();
document.documentElement.dataset.window = label;
// Synchronously, before the first paint: the platform's font and palette.
// The user agent is right on both shipped desktops; the shell's
// `Status.platform` confirms it a moment later.
applyPlatformTheme({ platform: detectPlatform() });
// The popover lets the native material through only once the shell confirms
// it is there (macOS, `windows::apply_vibrancy`): a transparent page with
// nothing behind it shows the desktop.
if (inTauri()) {
  void getStatus()
    .then((s) =>
      applyPlatformTheme({ platform: s.platform, vibrancy: label === "popover" && s.vibrancy }),
    )
    .catch(() => {});
}
// Three windows, one document: "popover", "settings" and "wizard" (the last is
// opened by the shell on a first launch with no config file, and from Settings).
const App = label === "settings" ? SettingsApp : label === "wizard" ? WizardApp : PopoverApp;
mount(App, { target });

void applyThemeCss();
