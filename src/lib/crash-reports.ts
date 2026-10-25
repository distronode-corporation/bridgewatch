/**
 * The crash-report question's one write.
 *
 * The answer is a config key like any other, so it goes through the same
 * `apply_config_edits` as the Settings checkbox, and the shell hands it to its
 * reporter on the reload that follows (`commands::reload_from_disk`).
 */

import type { Edit, Status } from "./types";

/** The key the popover's question and the Settings checkbox both write. */
export const CRASH_REPORTS_PATH = "ui.send_crash_reports";

/** The edit that records an answer. Always a `set`, never an `unset`: an unset key is what makes the app ask. */
export function crashReportsAnswer(send: boolean): Edit[] {
  return [{ op: "set", path: CRASH_REPORTS_PATH, value: { boolean: send } }];
}

/** Whether the popover should put the question: only when the shell says `ask`. */
export function shouldAskAboutCrashReports(status: Status | null | undefined): boolean {
  return status?.crashReports === "ask";
}
