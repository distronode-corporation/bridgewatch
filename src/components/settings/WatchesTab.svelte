<script lang="ts">
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";

  import { Button } from "$lib/components/ui/button/index.js";
  import { concretePath, entriesFor, providerOf } from "../../lib/settings/registry";
  import { WATCH_GROUPS, watchGroup, type WatchGroup } from "../../lib/settings/panes";
  import { getAt } from "../../lib/settings/values";
  import type { Edit } from "../../lib/types";
  import Field from "./Field.svelte";
  import KeyValueField from "./KeyValueField.svelte";
  import Section from "./Section.svelte";

  interface Props {
    config: unknown;
    /** Per watch, the job-override patterns in FILE order. */
    jobOrder: string[][];
    onedit: (edits: Edit[]) => void | Promise<void>;
    onremove: (id: string) => void;
    onmove: (id: string, delta: number) => void;
  }

  let { config, jobOrder, onedit, onremove, onmove }: Props = $props();

  const watches = $derived((getAt(config, "watches") ?? []) as Record<string, unknown>[]);
  const accounts = $derived(
    Object.keys((getAt(config, "accounts") ?? {}) as Record<string, unknown>),
  );

  /** A watch's provider is its account's; an unknown account has none, so nothing is dimmed. */
  function providerFor(watch: Record<string, unknown>) {
    const all = (getAt(config, "accounts") ?? {}) as Record<string, unknown>;
    const name = typeof watch.account === "string" ? watch.account : "";
    return Object.hasOwn(all, name) ? providerOf(all[name]) : null;
  }

  const fields = entriesFor("watches").filter((e) => !e.path.endsWith("jobs.*"));
  const jobsEntry = entriesFor("watches").find((e) => e.path.endsWith("jobs.*"))!;

  /**
   * The watch's keys in groups named after their config tables (Source, Rows,
   * Polling, Child pipelines, Deploy, Jobs, Notifications): 24 flat fields read
   * as one undifferentiated form. A key no group claims would vanish, so it
   * goes to Source; `panes.test.ts` fails on it before it ever gets here.
   */
  const grouped = WATCH_GROUPS.map((group) => ({
    group,
    entries: fields.filter((e) => (watchGroup(e.path) ?? "Source") === group),
  }));

  /** How many of a group's keys this watch's table sets, for the collapsed title. */
  function setCount(group: WatchGroup, index: number): string {
    let n = grouped
      .find((g) => g.group === group)!
      .entries.filter((e) => getAt(config, concretePath(e.path, index)) !== undefined).length;
    if (group === "Jobs") n += jobRows(index).length;
    return n === 0 ? "" : `${n} set`;
  }

  /**
   * The open panel, named by WATCH rather than by position.
   *
   * ⛔ This was an index. "Move up" and "Remove" both renumber the list, so
   * the panel stayed at position 2 and the watch it had been showing moved
   * out from under it: after one click on ↑ the open form was a DIFFERENT
   * watch's, with the same fields, and the next thing typed went into it.
   */
  // ⚠ Three states, not two: `undefined` is "nobody has clicked yet", which
  // opens the first watch, and `null` is "closed", which must not equal the id
  // of a watch that has none yet.
  let openId = $state<string | null | undefined>(undefined);

  const open = $derived(openId === undefined ? String(watches[0]?.id ?? "") : openId);

  /**
   * The job overrides in FILE order.
   *
   * ⛔ Not `Object.entries(watch.jobs)`. `[watches.jobs]` is first-match-wins
   * and the JSON the shell hands over has alphabetised the keys, so reading
   * them from the object would show — and then WRITE BACK — the wrong
   * precedence. `jobOrder` is the file's order, recovered by the core.
   */
  function jobRows(index: number): [string, string][] {
    const jobs = (watches[index]?.jobs ?? {}) as Record<string, string>;
    const order = jobOrder[index] ?? Object.keys(jobs);
    return order.filter((k) => k in jobs).map((k) => [k, jobs[k]]);
  }
</script>

{#each watches as watch, index (String(watch.id ?? index))}
  {@const id = String(watch.id ?? "")}
  <section class="watch mb-4" aria-label={`Watch ${id || "(no id)"}`}>
    <header class="bg-muted/60 flex items-center gap-2 rounded-xl px-2 py-1.5">
      <button
        class="disclosure text-muted-foreground hover:text-foreground rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        aria-expanded={open === id}
        aria-label={open === id ? "collapse" : "expand"}
        onclick={() => (openId = open === id ? null : id)}
      >
        <ChevronRightIcon class={["size-3.5 transition-transform", open === id && "rotate-90"]} aria-hidden="true" />
      </button>
      <strong class="text-ui-body min-w-0 truncate font-semibold">{id || "(no id)"}</strong>
      <span class="role text-muted-foreground text-ui-badge">{String(watch.role ?? "primary")}</span>
      <span class="flex-1"></span>
      <Button variant="ghost" size="icon-xs" title="move up" aria-label="move up" disabled={index === 0} onclick={() => onmove(id, -1)}
        >↑</Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        title="move down"
        aria-label="move down"
        disabled={index === watches.length - 1}
        onclick={() => onmove(id, 1)}>↓</Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-tone-red"
        title="remove this watch"
        aria-label="remove this watch"
        onclick={() => onremove(id)}>✕</Button
      >
    </header>

    {#if open === id}
      <!-- Source is open; every other group starts folded, with how many of
           its keys the file sets beside the title. -->
      <div class="body mt-3 flex flex-col gap-3">
        {#each grouped as { group, entries } (group)}
          <Section title={group} collapsible open={group === "Source"} note={setCount(group, index)}>
            {#each entries as entry (entry.path)}
              {@const path = concretePath(entry.path, index)}
              <Field
                {entry}
                {path}
                value={getAt(config, path)}
                options={entry.path === "watches.*.account" ? accounts : undefined}
                provider={providerFor(watch)}
                {onedit}
              />
            {/each}
            {#if group === "Jobs"}
              <KeyValueField
                entry={jobsEntry}
                path={`watches.${index}.jobs`}
                rows={jobRows(index)}
                values={["gate", "warning", "blocking", "ignore"]}
                keyPlaceholder="job name or re:pattern"
                {onedit}
              />
            {/if}
          </Section>
        {/each}
      </div>
    {/if}
  </section>
{/each}

{#if watches.length === 0}
  <p class="empty text-muted-foreground text-ui-meta m-0 px-1">No watches yet. Add one, or run the setup wizard.</p>
{/if}
