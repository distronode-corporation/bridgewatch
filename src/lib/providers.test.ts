import { describe, expect, it } from "vitest";

import schema from "./config.schema.json";
import { PROVIDER_DEFAULTS, cliName, defaultBaseUrl, providerName } from "./providers";

type Properties = Record<string, { default?: unknown }>;
const defs = (
  schema as unknown as {
    $defs: {
      Account: { properties: Properties };
      PollConfig: { properties: Properties };
      AuthHeader: { oneOf: { const: string }[] };
    };
  }
).$defs;

describe("PROVIDER_DEFAULTS", () => {
  it("GitLab's are the schema's, so the table cannot drift from the core", () => {
    // The schema's defaults are GitLab's (an account without `provider` is a
    // gitlab one). GitHub's have no schema default to read; the core's
    // `Provider::default_*` and `GITHUB_*_SECS` are their source.
    const account = defs.Account.properties;
    const poll = defs.PollConfig.properties;
    expect(PROVIDER_DEFAULTS.gitlab).toEqual({
      base_url: account.base_url.default,
      api_path: account.api_path.default,
      header: account.header.default,
      live_secs: poll.live_secs.default,
      idle_secs: poll.idle_secs.default,
    });
  });

  it("GitHub's header is one the schema allows", () => {
    expect(defs.AuthHeader.oneOf.map((h) => h.const)).toContain(PROVIDER_DEFAULTS.github.header);
  });

  it("the helpers read the same table", () => {
    expect(defaultBaseUrl("gitlab")).toBe("https://gitlab.com");
    expect(defaultBaseUrl("github")).toBe("https://api.github.com");
    expect([providerName("gitlab"), providerName("github")]).toEqual(["GitLab", "GitHub"]);
    expect([cliName("gitlab"), cliName("github")]).toEqual(["glab", "gh"]);
  });
});
