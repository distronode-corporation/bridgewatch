import { describe, expect, it } from "vitest";

import schema from "../config.schema.json" with { type: "json" };
import { REGISTRY, entry, inapplicableNote, providerOf } from "./registry";
import { leafPaths, type JsonSchema } from "./schema";

// ⚠ Through `unknown`: TypeScript infers the imported JSON as its exact literal
// shape, where a variant lacking a key has that key typed `undefined`, and
// `Record<string, JsonSchema>` will not accept that. The double cast is the
// documented escape and costs nothing — the real check is `leafPaths` walking
// the file, not the compiler's opinion of it.
const SCHEMA = schema as unknown as JsonSchema;

/**
 * The parity gate.
 *
 * `config.schema.json` is committed output of `bridgewatch config schema`, and
 * its freshness is held by `src-tauri/tests/schema_parity.rs`, which regenerates
 * it from the same `schemars::schema_for!(Config)` the CLI prints. What THIS
 * test holds is the other half: that the settings pane can edit exactly the
 * keys the file has.
 *
 * Both directions matter, and they fail differently:
 *
 *   * a schema key with no registry entry is a key the settings pane silently
 *     cannot reach, so the only way to change it is the text tab and nobody
 *     would know;
 *   * a registry entry with no schema key is a control writing something the
 *     core will reject, which shows up as a save that fails with a serde
 *     message about an unknown field.
 */
describe("settings registry / config schema parity", () => {
  const schemaPaths = leafPaths(SCHEMA);
  const registryPaths = REGISTRY.map((e) => e.path).sort();

  it("the schema has leaves at all", () => {
    // Guards against a walker change that returns [] and makes both
    // set-difference assertions below pass vacuously.
    expect(schemaPaths.length).toBeGreaterThan(30);
    expect(schemaPaths).toContain("watches.*.deploy_markers");
    expect(schemaPaths).toContain("accounts.*.token");
  });

  it("every schema key has a settings control", () => {
    const missing = schemaPaths.filter((p) => !registryPaths.includes(p));
    expect(missing, `no settings control for: ${missing.join(", ")}`).toEqual([]);
  });

  it("every settings control has a schema key", () => {
    const extra = registryPaths.filter((p) => !schemaPaths.includes(p));
    expect(extra, `registry entries with no schema key: ${extra.join(", ")}`).toEqual([]);
  });

  it("has no duplicate paths", () => {
    expect(new Set(registryPaths).size).toBe(registryPaths.length);
  });

  it("gives every select control its options", () => {
    // Except `watches.*.account`, whose options are the account names in the
    // document rather than a fixed list.
    const bare = REGISTRY.filter(
      (e) => e.control === "select" && !e.options && e.path !== "watches.*.account",
    );
    expect(bare.map((e) => e.path)).toEqual([]);
  });

  it("offers exactly the enum values the schema allows", () => {
    // A select that offers a value the core rejects is a form that writes a
    // file that will not load.
    const defs = SCHEMA.$defs ?? {};
    const check = (path: string, defName: string) => {
      const entry = REGISTRY.find((e) => e.path === path);
      // ⚠ schemars 1.x renders a unit variant as `{"const": "primary"}`, not
      // as an `enum` array. Reading only `enum` finds nothing and the
      // comparison passes against two empty lists, which is a gate that checks
      // nothing — hence the length assertion below.
      const allowed = (defs[defName]?.oneOf ?? [])
        .flatMap((v) => (v.const !== undefined ? [String(v.const)] : ((v.enum ?? []) as string[])))
        .sort();
      expect(allowed.length, `${defName} has no variants in the schema`).toBeGreaterThan(1);
      expect(entry?.options?.slice().sort(), path).toEqual(allowed);
    };
    check("watches.*.role", "Role");
    check("watches.*.sibling_failure", "FailurePolicy");
    check("watches.*.post_deploy_failure", "FailurePolicy");
    check("watches.*.notify.click", "ClickTarget");
    check("accounts.*.header", "AuthHeader");
    check("ui.jobs", "JobsMode");

  });
});

describe("the job-list keys (ui.jobs, watches.*.show.jobs)", () => {
  const schemaPaths = leafPaths(SCHEMA);

  it("are in the schema and in the registry", () => {
    // Named, so a walker or registry change that drops them fails here with
    // the key spelled out rather than as one line in a set difference.
    for (const path of ["ui.jobs", "watches.*.show.jobs"]) {
      expect(schemaPaths, `schema lacks ${path}`).toContain(path);
      expect(REGISTRY.map((e) => e.path), `registry lacks ${path}`).toContain(path);
    }
  });

  it("offer the schema's modes, the per-watch one plus inherit", () => {
    const modes = (SCHEMA.$defs?.JobsMode?.oneOf ?? [])
      .map((v) => String(v.const ?? ""))
      .filter((v) => v !== "")
      .sort();
    expect(modes).toEqual(["all", "failures"]);
    const global = REGISTRY.find((e) => e.path === "ui.jobs")!;
    const perWatch = REGISTRY.find((e) => e.path === "watches.*.show.jobs")!;
    expect(global.tab).toBe("ui");
    expect(perWatch.tab).toBe("watches");
    expect(global.options?.slice().sort()).toEqual(modes);
    // "" is (inherit): the Field removes the key and `ui.jobs` decides.
    expect(perWatch.options?.[0]).toBe("");
    expect(perWatch.options?.slice(1).sort()).toEqual(modes);
  });
});

describe("leafPaths", () => {
  const paths = leafPaths(SCHEMA);

  it("stops at a union rather than inventing its variants' keys", () => {
    // `accounts.*.token` is one of four table shapes; descending would emit
    // `accounts.*.token.keyring.service`, which is a real key only in one of
    // them, and a control bound to it would be wrong three times in four.
    expect(paths).toContain("accounts.*.token");
    expect(paths.filter((p) => p.startsWith("accounts.*.token."))).toEqual([]);
    // `project` is a number OR a `group/path`.
    expect(paths).toContain("watches.*.project");
  });

  it("collapses a dictionary table to one wildcard leaf", () => {
    // The keys of `[watches.jobs]` are the user's own patterns, and the table
    // is edited as ordered rows because first match wins.
    expect(paths).toContain("watches.*.jobs.*");
    expect(paths).toContain("icon.states.*");
  });

  it("turns an array of tables into a wildcard and keeps walking", () => {
    expect(paths).toContain("watches.*.poll.live_secs");
    // ...while an array of scalars is one leaf.
    expect(paths).toContain("watches.*.sources");
    expect(paths.filter((p) => p.startsWith("watches.*.sources."))).toEqual([]);
  });
});

describe("provider applicability", () => {
  it("is data on the entry, and every provider-specific key says why in one line", () => {
    const marked = REGISTRY.filter((e) => e.provider);
    expect(marked.map((e) => e.path).sort()).toEqual([
      "watches.*.dive.bridges",
      "watches.*.dive.depth",
      "watches.*.dive.exclude",
      "watches.*.workflow",
    ]);
    for (const e of marked) expect(e.providerNote, e.path).toMatch(/^Ignored on a (GitLab|GitHub) account: /);
  });

  it("marks a key inert only for the OTHER provider, and never when the provider is unknown", () => {
    const workflow = entry("watches.*.workflow");
    expect(inapplicableNote(workflow, "gitlab")).toMatch(/no workflows/);
    expect(inapplicableNote(workflow, "github")).toBeNull();
    expect(inapplicableNote(workflow, null)).toBeNull();
    const depth = entry("watches.*.dive.depth");
    expect(inapplicableNote(depth, "github")).toMatch(/no nested runs/);
    expect(inapplicableNote(depth, "gitlab")).toBeNull();
    // A key both providers use is never dimmed.
    expect(inapplicableNote(entry("watches.*.ref"), "github")).toBeNull();
    expect(inapplicableNote(entry("watches.*.ref"), "gitlab")).toBeNull();
  });

  it("reads an account's provider as the core serialises it, absent being gitlab", () => {
    expect(providerOf({ provider: "github" })).toBe("github");
    expect(providerOf({ provider: "gitlab" })).toBe("gitlab");
    expect(providerOf({})).toBe("gitlab");
    expect(providerOf(undefined)).toBe("gitlab");
  });
});
