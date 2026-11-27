// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { describe, expect, it } from "vitest";

import { createSingleFlight } from "./single-flight";

describe("createSingleFlight", () => {
  it("runs one task per key, and frees the key when it settles", async () => {
    const flight = createSingleFlight();
    let release!: () => void;
    const first = flight.run("retry:1", () => new Promise<void>((r) => (release = r)));
    expect(first).not.toBeNull();
    expect(flight.busy("retry:1")).toBe(true);
    // A second click before the first settles sends nothing; another key is not held up.
    expect(flight.run("retry:1", () => Promise.resolve())).toBeNull();
    await expect(flight.run("play:2", () => Promise.resolve(2))).resolves.toBe(2);
    release();
    await first;
    expect(flight.busy("retry:1")).toBe(false);
  });

  it("frees the key when the task rejects", async () => {
    const flight = createSingleFlight();
    await expect(flight.run("k", () => Promise.reject(new Error("403")))).rejects.toThrow("403");
    expect(flight.busy("k")).toBe(false);
  });

  it("frees the key and rejects when the task throws before returning a promise", async () => {
    const flight = createSingleFlight();
    const thrown = new Error("synchronous");
    const pending = flight.run("k", () => {
      throw thrown;
    });
    expect(flight.busy("k")).toBe(false);
    await expect(pending).rejects.toBe(thrown);
  });
});
