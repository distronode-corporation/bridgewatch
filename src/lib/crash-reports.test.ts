import { describe, expect, it } from "vitest";

import schema from "./config.schema.json" with { type: "json" };
import { CRASH_REPORTS_PATH, crashReportsAnswer, shouldAskAboutCrashReports } from "./crash-reports";
import { entry } from "./settings/registry";
import type { Status } from "./types";

const base: Status = {
  configPath: "/tmp/bridgewatch.toml",
  seeded: false,
  configOk: true,
  diagnostics: [],
  sinceLastPollSecs: null,
  nextPollSecs: null,
  launchAtLogin: false,
  platform: "linux",
  vibrancy: false,
};

describe("the crash-report answer", () => {
  it("is a set of ui.send_crash_reports, never an unset (an unset key is what asks)", () => {
    expect(crashReportsAnswer(true)).toEqual([
      { op: "set", path: "ui.send_crash_reports", value: { boolean: true } },
    ]);
    expect(crashReportsAnswer(false)).toEqual([
      { op: "set", path: "ui.send_crash_reports", value: { boolean: false } },
    ]);
  });

  it("names a key the schema has, unset by default, with a Settings checkbox", () => {
    const ui = (schema as unknown as { $defs: Record<string, { properties: Record<string, { type?: unknown; default?: unknown }> }> })
      .$defs.UiConfig.properties;
    expect(ui.send_crash_reports?.default).toBeNull();
    expect(ui.send_crash_reports?.type).toEqual(["boolean", "null"]);
    const control = entry(CRASH_REPORTS_PATH);
    expect(control.control).toBe("boolean");
    expect(control.label).toBe("Send crash reports");
  });
});

describe("whether to ask", () => {
  it("is yes only for `ask`", () => {
    expect(shouldAskAboutCrashReports({ ...base, crashReports: "ask" })).toBe(true);
    for (const crashReports of ["unavailable", "off", "on", undefined] as const) {
      expect(shouldAskAboutCrashReports({ ...base, crashReports }), String(crashReports)).toBe(false);
    }
    expect(shouldAskAboutCrashReports(null)).toBe(false);
    expect(shouldAskAboutCrashReports(undefined)).toBe(false);
  });
});
