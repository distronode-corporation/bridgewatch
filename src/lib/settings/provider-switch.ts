/**
 * Changing an existing account's provider in Settings.
 *
 * `base_url`, `api_path` and `header` default per provider, so an account that
 * never wrote them follows the provider on its own. One that has them in the
 * file keeps them, and GitLab's values on a github account are an error
 * (`PRIVATE-TOKEN`) or a host that 404s every request (`https://gitlab.com`).
 * So a provider change also removes each of those keys whose value is the OLD
 * provider's default, which is what lets the new provider's default apply, and
 * keeps every value somebody chose, saying so in one line.
 *
 * ⚠️ The settings pane reads the LOADED configuration (defaults filled in), so
 * it cannot tell a written default from an absent key. It does not need to:
 * removing a key that is not in the file changes nothing.
 */

import type { Edit, Provider } from "../types";
import { concretePath } from "./registry";

/** The keys that default per provider, as the core's `Provider::default_*` gives them. */
export const PROVIDER_DEFAULTS: Record<Provider, { base_url: string; api_path: string; header: string }> = {
  gitlab: { base_url: "https://gitlab.com", api_path: "/api/v4", header: "PRIVATE-TOKEN" },
  github: { base_url: "https://api.github.com", api_path: "", header: "Authorization: Bearer" },
};

const KEYS = ["base_url", "api_path", "header"] as const;

const NAMES: Record<Provider, string> = { gitlab: "GitLab", github: "GitHub" };

/** What else a provider change writes, and the note for any value kept. */
export interface ProviderSwitch {
  /** Unsets for the keys that held the old provider's default. */
  edits: Edit[];
  /** One line naming the customised values that were kept, or null. */
  note: string | null;
}

export function providerSwitch(name: string, account: unknown, from: Provider, to: Provider): ProviderSwitch {
  if (from === to) return { edits: [], note: null };
  const values = (account ?? {}) as Record<string, unknown>;
  const edits: Edit[] = [];
  const kept: string[] = [];
  for (const key of KEYS) {
    const value = values[key];
    if (value === undefined || value === null || value === PROVIDER_DEFAULTS[from][key]) {
      edits.push({ op: "unset", path: concretePath(`accounts.*.${key}`, name) });
    } else {
      kept.push(`${key} = ${JSON.stringify(value)}`);
    }
  }
  const note =
    kept.length === 0
      ? null
      : `Kept ${kept.join(", ")} (not ${NAMES[from]}'s default). Check ${kept.length === 1 ? "it suits" : "they suit"} ${NAMES[to]}.`;
  return { edits, note };
}
