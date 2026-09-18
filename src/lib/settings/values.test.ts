import { describe, expect, it } from "vitest";

import { getAt, linesToList, splitPath, tableEdits } from "./values";

describe("splitPath and getAt", () => {
  it("treats a quoted segment as one key, dots and all", () => {
    // The core's `split_path` does the same, which is what lets an account
    // called `gitlab.com` be addressed at all.
    expect(splitPath('accounts."gitlab.com".token')).toEqual(["accounts", "gitlab.com", "token"]);
    expect(getAt({ accounts: { "gitlab.com": { token: { own: true } } } }, 'accounts."gitlab.com".token')).toEqual({
      own: true,
    });
  });

  it("walks an array of tables by index", () => {
    expect(getAt({ watches: [{ id: "a" }, { id: "b" }] }, "watches.1.id")).toBe("b");
  });

  it("returns undefined rather than throwing on a path that is not there", () => {
    expect(getAt({}, "watches.0.dive.bridges")).toBe(undefined);
  });
});

describe("linesToList", () => {
  it("drops blank lines and trims what is left", () => {
    expect(linesToList("deploy:origins\n\n  deploy:cdn_seo  \n")).toEqual([
      "deploy:origins",
      "deploy:cdn_seo",
    ]);
  });
});

/**
 * ⛔ Every `set` here writes `path."key"`, quoted with `JSON.stringify`, which
 * is the encoding the core's `split_path` decodes. The two halves are one
 * contract: changing the encoding on this side silently writes a key nobody
 * can read back.
 */
describe("tableEdits", () => {
  const before: [string, string][] = [
    ["verify:*", "warning"],
    ["deploy:origins", "blocking"],
  ];

  it("changes one value with one edit, leaving the table in place", () => {
    // 🔑 The whole point. Removing and rewriting the table loses its header
    // comment and every comment written inside it — two of them in the shipped
    // example, gone the first time anybody touched a dropdown.
    expect(tableEdits("watches.0.jobs", before, [
      ["verify:*", "ignore"],
      ["deploy:origins", "blocking"],
    ])).toEqual([
      { op: "set", path: 'watches.0.jobs."verify:*"', value: { string: "ignore" } },
    ]);
  });

  it("appends a new row without rewriting the ones above it", () => {
    expect(tableEdits("watches.0.jobs", before, [...before, ["kics-iac-sast", "ignore"]])).toEqual([
      { op: "set", path: 'watches.0.jobs."kics-iac-sast"', value: { string: "ignore" } },
    ]);
  });

  it("removes a row by unsetting its key alone", () => {
    expect(tableEdits("watches.0.jobs", before, [before[0]])).toEqual([
      { op: "unset", path: 'watches.0.jobs."deploy:origins"' },
    ]);
  });

  it("writes nothing when nothing changed", () => {
    expect(tableEdits("watches.0.jobs", before, [...before])).toEqual([]);
  });

  it("rewrites the whole table for a reorder, because order IS the rule", () => {
    // `[watches.jobs]` is first-match-wins and `toml_edit` can only append, so
    // there is no way to express "move this key up" as a key-level edit.
    expect(tableEdits("watches.0.jobs", before, [before[1], before[0]])).toEqual([
      { op: "unset", path: "watches.0.jobs" },
      { op: "set", path: 'watches.0.jobs."deploy:origins"', value: { string: "blocking" } },
      { op: "set", path: 'watches.0.jobs."verify:*"', value: { string: "warning" } },
    ]);
  });

  it("rewrites when a new key would have to sit above an existing one", () => {
    const after: [string, string][] = [["new:job", "gate"], ...before];
    expect(tableEdits("watches.0.jobs", before, after)[0]).toEqual({
      op: "unset",
      path: "watches.0.jobs",
    });
  });

  it("rewrites rather than writing one of two rows with the same key", () => {
    // The second would overwrite the first and the file would end up with one
    // row where the form shows two; the rewrite at least makes them agree.
    const after: [string, string][] = [["verify:*", "warning"], ["verify:*", "ignore"]];
    expect(tableEdits("watches.0.jobs", before, after)[0]).toEqual({
      op: "unset",
      path: "watches.0.jobs",
    });
  });

  it("quotes a key that holds a dot", () => {
    expect(tableEdits("icon.states", [], [["deployed.with.failure", "triangle"]])).toEqual([
      { op: "set", path: 'icon.states."deployed.with.failure"', value: { string: "triangle" } },
    ]);
  });
});
