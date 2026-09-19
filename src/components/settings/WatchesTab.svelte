<script lang="ts">
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";

  import { Button } from "$lib/components/ui/button/index.js";
  import { concretePath, entriesFor, providerOf } from "../../lib/settings/registry";
  import { getAt } from "../../lib/settings/values";
  import type { Edit } from "../../lib/types";
  import Field from "./Field.svelte";
  import KeyValueField from "./KeyValueField.svelte";

  interface Props {
    config: unknown;
    /** Per watch, the job-override patterns in FILE order. */
    jobOrder: string[][];
    onedit: (edits: Edit[]) => void | Promise<void>;
    onremove: (id: string) => void;
    onmove: (id: string, delta: number) => void;
    onadd: () => void;
  }

  let { config, jobOrder, onedit, onremove, onmove, onadd }: Props = $props();

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
  <section class="watch border-border mb-2.5 overflow-hidden rounded-xl border">
    <header class="bg-muted/60 flex items-center gap-2 px-2 py-1.5">
      <button
        class="disclosure text-muted-foreground hover:text-foreground rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        aria-expanded={open === id}
        aria-label={open === id ? "collapse" : "expand"}
        onclick={() => (openId = open === id ? null : id)}
      >
        <ChevronRightIcon class={["size-3.5 transition-transform", open === id && "rotate-90"]} aria-hidden="true" />
      </button>
      <strong class="text-[13px]">{id || "(no id)"}</strong>
      <span class="role text-muted-foreground text-[11px]">{String(watch.role ?? "primary")}</span>
      <span class="flex-1"></span>
      <Button variant="ghost" size="icon-xs" title="move up" disabled={index === 0} onclick={() => onmove(id, -1)}
        >↑</Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        title="move down"
        disabled={index === watches.length - 1}
        onclick={() => onmove(id, 1)}>↓</Button
      >
      <Button variant="ghost" size="icon-xs" class="text-tone-red" title="remove this watch" onclick={() => onremove(id)}
        >✕</Button
      >
    </header>

    {#if open === id}
      <div class="body px-2 pt-3 pb-1">
        {#each fields as entry (entry.path)}
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
        <KeyValueField
          entry={jobsEntry}
          path={`watches.${index}.jobs`}
          rows={jobRows(index)}
          values={["gate", "warning", "blocking", "ignore"]}
          keyPlaceholder="job name or re:pattern"
          {onedit}
        />
      </div>
    {/if}
  </section>
{/each}

<Button variant="outline" size="sm" class="add mt-1" onclick={onadd}>Add watch</Button>
