// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { describe, expect, it } from "vitest";

import { failureOf, failureSentence, mismatchText, shortUrl } from "./oauth";

describe("failureOf", () => {
  it("keeps a SignInFailure's kind and message", () => {
    expect(failureOf({ kind: "denied", message: "access_denied" })).toEqual({ kind: "denied", message: "access_denied" });
  });

  it("reads a message without a string kind as a plain error", () => {
    expect(failureOf({ message: "timed out" })).toEqual({ kind: "error", message: "timed out" });
    expect(failureOf({ kind: 7, message: "timed out" })).toEqual({ kind: "error", message: "timed out" });
  });

  it("takes a bare string rejection as the message, and anything else as a generic one", () => {
    expect(failureOf("network down")).toEqual({ kind: "error", message: "network down" });
    expect(failureOf(null)).toEqual({ kind: "error", message: "The sign-in did not finish." });
    expect(failureOf({ kind: "denied", message: 42 })).toEqual({ kind: "error", message: "The sign-in did not finish." });
  });
});

describe("failureSentence", () => {
  it("says each failure in plain words, naming the host for a refusal", () => {
    expect(failureSentence({ kind: "expired", message: "x" }, "github.com")).toBe(
      "The code expired before it was entered. Start again.",
    );
    expect(failureSentence({ kind: "denied", message: "x" }, "github.com")).toBe("You cancelled the sign-in on github.com.");
    expect(failureSentence({ kind: "cancelled", message: "x" }, "github.com")).toBe("Sign-in stopped.");
    expect(failureSentence({ kind: "no_client_id", message: "No client id is set." }, "github.com")).toBe(
      "No client id is set.",
    );
  });
});

describe("mismatchText and shortUrl", () => {
  it("names both instances for a sign-in made elsewhere", () => {
    expect(mismatchText({ kind: "other_instance", issued: "gitlab.com", expected: "gitlab.example.com" })).toBe(
      "Signed in for gitlab.com, not gitlab.example.com, so the sign-in is not used. Sign in again.",
    );
    expect(mismatchText({ kind: "other_application" })).toContain("another application");
  });

  it("drops the scheme and a trailing slash", () => {
    expect(shortUrl("https://github.com/login/device/")).toBe("github.com/login/device");
    expect(shortUrl("HTTP://gitlab.example.com/oauth/device")).toBe("gitlab.example.com/oauth/device");
  });
});
