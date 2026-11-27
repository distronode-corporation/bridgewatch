// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { describe, expect, it } from "vitest";

import { ownTokenText } from "./own-token";

describe("ownTokenText", () => {
  it("says what each stored state is, and only a mismatch is an alert", () => {
    expect(ownTokenText({ kind: "none" }, "gitlab.com")).toEqual({
      text: "No token is stored for this account yet.",
      alert: false,
    });
    expect(ownTokenText({ kind: "legacy" }, "gitlab.com").text).toContain("tied to gitlab.com");
    expect(ownTokenText({ kind: "bound", origin: "https://gitlab.com" }, "gitlab.com")).toEqual({
      text: "Stored for https://gitlab.com.",
      alert: false,
    });
    expect(ownTokenText({ kind: "needs_approval" }, "gitlab.com").alert).toBe(false);
    expect(
      ownTokenText({ kind: "mismatch", stored: "https://old.example", expected: "https://gitlab.com" }, "gitlab.com"),
    ).toEqual({
      text: "Stored for https://old.example; paste one for https://gitlab.com. The stored token is not sent there.",
      alert: true,
    });
  });

  it("names the account's address in words when the file gives none", () => {
    expect(ownTokenText({ kind: "mismatch", stored: "https://old.example", expected: null }, "gitlab.com").text).toBe(
      "Stored for https://old.example; paste one for this account's address. The stored token is not sent there.",
    );
  });
});
