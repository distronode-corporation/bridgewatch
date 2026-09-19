import { describe, expect, it } from "vitest";

import schema from "../config.schema.json";
import { PROVIDER_DEFAULTS, providerSwitch } from "./provider-switch";

describe("providerSwitch", () => {
  it("gitlab to github removes GitLab's defaults so GitHub's apply", () => {
    const account = { provider: "gitlab", base_url: "https://gitlab.com", api_path: "/api/v4", header: "PRIVATE-TOKEN" };
    expect(providerSwitch("work", account, "gitlab", "github")).toEqual({
      edits: [
        { op: "unset", path: "accounts.work.base_url" },
        { op: "unset", path: "accounts.work.api_path" },
        { op: "unset", path: "accounts.work.header" },
      ],
      note: null,
    });
  });

  it("github to gitlab removes GitHub's defaults, the empty api_path included", () => {
    const account = { provider: "github", base_url: "https://api.github.com", api_path: "", header: "Authorization: Bearer" };
    expect(providerSwitch("hub", account, "github", "gitlab")).toEqual({
      edits: [
        { op: "unset", path: "accounts.hub.base_url" },
        { op: "unset", path: "accounts.hub.api_path" },
        { op: "unset", path: "accounts.hub.header" },
      ],
      note: null,
    });
  });

  it("keeps a self-managed GitLab host and says so, while the default header still goes", () => {
    const account = { base_url: "https://gitlab.example.com", api_path: "/api/v4", header: "PRIVATE-TOKEN" };
    const result = providerSwitch("gitlab.example.com", account, "gitlab", "github");
    expect(result.edits).toEqual([
      { op: "unset", path: 'accounts."gitlab.example.com".api_path' },
      { op: "unset", path: 'accounts."gitlab.example.com".header' },
    ]);
    expect(result.note).toBe(
      'Kept base_url = "https://gitlab.example.com" (not GitLab\'s default). Check it suits GitHub.',
    );
  });

  it("keeps an Enterprise Server host and its API path going to gitlab, in one line", () => {
    const account = { base_url: "https://ghe.acme.com", api_path: "/api/v3", header: "Authorization: Bearer" };
    const result = providerSwitch("ghe", account, "github", "gitlab");
    expect(result.edits).toEqual([{ op: "unset", path: "accounts.ghe.header" }]);
    expect(result.note).toBe(
      'Kept base_url = "https://ghe.acme.com", api_path = "/api/v3" (not GitHub\'s default). Check they suit GitLab.',
    );
    expect(result.note).not.toContain("\n");
  });

  it("a GitLab account that chose Authorization: Bearer keeps it", () => {
    const account = { base_url: "https://gitlab.com", api_path: "/api/v4", header: "Authorization: Bearer" };
    const result = providerSwitch("work", account, "gitlab", "github");
    expect(result.edits.map((e) => ("path" in e ? e.path : ""))).toEqual([
      "accounts.work.base_url",
      "accounts.work.api_path",
    ]);
    expect(result.note).toContain('header = "Authorization: Bearer"');
  });

  it("writes nothing when the provider does not change", () => {
    expect(providerSwitch("work", { base_url: "https://gitlab.com" }, "gitlab", "gitlab")).toEqual({
      edits: [],
      note: null,
    });
  });

  it("GitLab's defaults are the schema's, so the table cannot drift from the core", () => {
    const account = (schema as { $defs: { Account: { properties: Record<string, { default?: unknown }> } } }).$defs
      .Account.properties;
    expect(PROVIDER_DEFAULTS.gitlab).toEqual({
      base_url: account.base_url.default,
      api_path: account.api_path.default,
      header: account.header.default,
    });
  });
});
