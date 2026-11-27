// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { describe, expect, it } from "vitest";

import { diagnosticsOf, issuesOf, needsConfirm } from "./api";

describe("reading a wizard rejection", () => {
  it("finds no diagnostics on a rejection that is not a WizardFailure", () => {
    expect(diagnosticsOf("network down")).toEqual([]);
    expect(diagnosticsOf(null)).toEqual([]);
    expect(diagnosticsOf({ diagnostics: "not a list" })).toEqual([]);
    expect(issuesOf({ issues: "not a list" })).toEqual([]);
  });

  it("keeps a warning's severity and drops an entry with no message", () => {
    expect(
      diagnosticsOf({
        diagnostics: [
          { severity: "warning", path: "watches[0]", message: "slow" },
          { severity: "error", path: 3, message: "no path" },
          { severity: "error", path: "x" },
          null,
        ],
      }),
    ).toEqual([
      { severity: "warning", path: "watches[0]", message: "slow", line: null, col: null },
      { severity: "error", path: "", message: "no path", line: null, col: null },
    ]);
  });

  it("narrows a confirmation request, and nothing shaped almost like one", () => {
    expect(needsConfirm({ confirm: { id: "q1", command: ["gh"], reason: "x" } } as never)).toBe(true);
    expect(needsConfirm({ confirm: { id: 1 } } as never)).toBe(false);
    expect(needsConfirm({ confirm: null } as never)).toBe(false);
    expect(needsConfirm(null as never)).toBe(false);
  });
});
