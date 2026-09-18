<script lang="ts">
  import { entriesFor, type Tab } from "../../lib/settings/registry";
  import { getAt } from "../../lib/settings/values";
  import type { Edit } from "../../lib/types";
  import Field from "./Field.svelte";
  import KeyValueField from "./KeyValueField.svelte";

  interface Props {
    tab: Tab;
    config: unknown;
    onedit: (edits: Edit[]) => void | Promise<void>;
  }

  let { tab, config, onedit }: Props = $props();

  // Every non-wildcard key on the tab, straight from the registry. Adding a
  // key to the schema and the registry makes it appear here with no component
  // change, which is the point of the registry existing at all.
  const fields = $derived(entriesFor(tab).filter((e) => !e.path.includes("*")));
  const maps = $derived(entriesFor(tab).filter((e) => e.path.endsWith(".*")));

  function rowsFor(path: string): [string, string][] {
    const table = getAt(config, path);
    if (!table || typeof table !== "object") return [];
    return Object.entries(table as Record<string, unknown>).map(([k, v]) => [k, String(v)]);
  }
</script>

{#each fields as entry (entry.path)}
  <Field {entry} path={entry.path} value={getAt(config, entry.path)} {onedit} />
{/each}

{#each maps as entry (entry.path)}
  {@const table = entry.path.slice(0, -2)}
  <KeyValueField
    {entry}
    path={table}
    rows={rowsFor(table)}
    keyPlaceholder="state"
    {onedit}
  />
{/each}
