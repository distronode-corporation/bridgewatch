/**
 * What the frontend knows per provider, in one place: the defaults the core
 * fills in for an account or a watch that does not say, and the names shown
 * for the provider and its CLI.
 *
 * ⛔ These mirror the core (`Provider::default_*` in `config/schema.rs`, and
 * `GITHUB_LIVE_SECS`/`GITHUB_IDLE_SECS` in `wizard.rs`). GitLab's are also the
 * schema's defaults, which `providers.test.ts` reads back from
 * `config.schema.json`. A provider switch in Settings decides which keys to
 * unset by comparing against this table, so a stale copy here would keep an
 * old default as if somebody had chosen it.
 */

import type { Provider } from "./types";

export interface ProviderDefaults {
  /** `Provider::default_base_url`: the hosted service's API root. */
  base_url: string;
  /** `Provider::default_api_path`. */
  api_path: string;
  /** `Provider::default_header`, as the file spells it. */
  header: string;
  /**
   * The live poll interval a new watch starts at. GitHub's budget is 5,000
   * requests an HOUR per token, about 24 times less than gitlab.com's, so a
   * GitHub watch starts at 30 s rather than 5.
   */
  live_secs: number;
  /** The idle poll interval a new watch starts at. */
  idle_secs: number;
}

export const PROVIDER_DEFAULTS: Record<Provider, ProviderDefaults> = {
  gitlab: {
    base_url: "https://gitlab.com",
    api_path: "/api/v4",
    header: "PRIVATE-TOKEN",
    live_secs: 5,
    idle_secs: 60,
  },
  github: {
    base_url: "https://api.github.com",
    api_path: "",
    header: "Authorization: Bearer",
    live_secs: 30,
    idle_secs: 120,
  },
};

/** The instance root an account talks to when its file does not say. */
export function defaultBaseUrl(provider: Provider): string {
  return PROVIDER_DEFAULTS[provider].base_url;
}

/** "GitHub" or "GitLab", for a button or a sentence. */
export function providerName(provider: Provider): "GitHub" | "GitLab" {
  return provider === "github" ? "GitHub" : "GitLab";
}

/** The provider's CLI, whose keyring item a token source can read: glab or gh. */
export function cliName(provider: Provider): "glab" | "gh" {
  return provider === "github" ? "gh" : "glab";
}
