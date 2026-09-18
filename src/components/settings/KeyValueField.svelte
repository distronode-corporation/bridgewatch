<script lang="ts">
  import { untrack } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import type { Edit } from "../../lib/types";
  import type { RegistryEntry } from "../../lib/settings/registry";
  import { tableEdits } from "../../lib/settings/values";
  import { FIELD_LABEL, FIELD_ROW, HINT, INPUT, SELECT } from "./styles";

  interface Props {
    entry: RegistryEntry;
    /** The path of the TABLE, e.g. `watches.0.jobs` — no trailing wildcard. */
    path: string;
    /** Rows in the order they must be written. */
    rows: [string, string][];
    /** The allowed values, when the value side is an enum. */
    values?: string[];
    /** Placeholder for the key column. */
    keyPlaceholder?: string;
    onedit: (edits: Edit[]) => void | Promise<void>;
  }

  let { entry, path, rows, values, keyPlaceholder = "pattern", onedit }: Props = $props();

  let draft = $state<[string, string][]>([]);
  // Re-seed the draft whenever the saved rows change, so a reload from disk or
  // another window's save is reflected here.
  $effect(() => {
    const saved = rows.map((r) => [r[0], r[1]] as [string, string]);
    // ⛔ A row with no key yet exists ONLY here: `commit` skips it on purpose,
    // because a blank key is not a row the file can hold. Re-seeding without it
    // meant that pressing "Add row" and then saving anything else — on another
    // tab, in another watch, or a reload from disk — deleted the row the user
    // was in the middle of typing, with no message.
    //
    // ⚠ `untrack`, or reading `draft` here would make this effect its own
    // dependency and it would run forever.
    const pending = untrack(() => draft).filter(([key]) => key.trim() === "");
    draft = [...saved, ...pending];
  });

  /**
   * Write the table.
   *
   * ⛔ Order is the rule for `[watches.jobs]` — first match wins — and the
   * `Edit` vocabulary can only append a key, so a REORDER still removes the
   * table and writes it back, losing the comments written inside it.
   * `tableEdits` emits that only when it has to: a changed value, an appended
   * row and a removed row each leave every surviving key where it is, so the
   * ordinary edit now keeps the table's header comment.
   */
  async function commit(next: [string, string][]) {
    const edits: Edit[] = tableEdits(
      path,
      rows,
      next.filter(([key]) => key.trim() !== ""),
    );
    // Nothing to write. A save with no edits is not free: it rebuilds the
    // poller and drops every cache.
    if (edits.length === 0) return;
    await onedit(edits);
  }

  /**
   * One cell changed.
   *
   * ⚠ Nothing puts a refused value back by hand, unlike `Field`, and the
   * difference is measured rather than assumed: the effect above assigns a
   * fresh `draft` every time the document is re-read — including when it comes
   * back unchanged because the save was refused — and Svelte re-asserts each
   * control from it. `Field` has no such render: its `value` prop is a
   * primitive that did not change, so there is nothing for Svelte to do.
   */
  function editCell(index: number, column: 0 | 1, value: string) {
    const next = draft.slice();
    const row = next[index];
    if (!row) return;
    next[index] = column === 0 ? [value, row[1]] : [row[0], value];
    draft = next;
    // A row with no key is not in the file at all: it is kept here, and written
    // the moment it has one.
    if (next[index][0].trim() === "") return;
    void commit(next);
  }

  function move(index: number, delta: number) {
    const to = index + delta;
    if (to < 0 || to >= draft.length) return;
    const next = draft.slice();
    const [row] = next.splice(index, 1);
    next.splice(to, 0, row);
    void commit(next);
  }
</script>

<div class={["kv", FIELD_ROW]}>
  <div class={["label", FIELD_LABEL]}>{entry.label}</div>
  <div class="rows flex flex-col gap-1">
    {#each draft as row, index (index)}
      <div class="row flex items-center gap-1">
        <input
          type="text"
          class={INPUT}
          placeholder={keyPlaceholder}
          value={row[0]}
          onchange={(e) => editCell(index, 0, (e.currentTarget as HTMLInputElement).value)}
        />
        {#if values}
          <select
            class={SELECT}
            value={row[1]}
            onchange={(e) => editCell(index, 1, (e.currentTarget as HTMLSelectElement).value)}
          >
            {#each values as v (v)}
              <option value={v}>{v}</option>
            {/each}
          </select>
        {:else}
          <input
            type="text"
            class={INPUT}
            value={row[1]}
            onchange={(e) => editCell(index, 1, (e.currentTarget as HTMLInputElement).value)}
          />
        {/if}
        <Button variant="ghost" size="icon-xs" title="move up" onclick={() => move(index, -1)}>↑</Button>
        <Button variant="ghost" size="icon-xs" title="move down" onclick={() => move(index, 1)}>↓</Button>
        <Button
          variant="ghost"
          size="icon-xs"
          title="remove"
          onclick={() => {
            // A row that was never written has nothing to remove: it goes from
            // the draft directly, without a save.
            if (draft[index]?.[0].trim() === "") {
              draft = draft.filter((_, i) => i !== index);
              return;
            }
            void commit(draft.filter((_, i) => i !== index));
          }}>✕</Button
        >
      </div>
    {/each}
    <Button
      variant="outline"
      size="xs"
      class="add self-start"
      onclick={() => (draft = [...draft, ["", values ? values[0] : ""]])}>Add row</Button
    >
    {#if entry.hint}
      <div class={["hint mt-1", HINT]}>{entry.hint}</div>
    {/if}
  </div>
</div>
