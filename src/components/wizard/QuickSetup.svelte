<script lang="ts">
  /**
   * Step 1, once a sign-in or Test connection succeeded: the one-click list.
   * The account's most recently active projects, the top three ticked; one
   * button adds every ticked one as a watch, in one write at Finish.
   *
   * Nothing here makes a request or decides a rank: the core ranks and ticks
   * (`rank_recent`), and the wizard does the rest when the button is pressed.
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import type { Provider, RecentListing } from "./api";
  import { activeLabel, watchable } from "./model";

  interface Props {
    provider: Provider;
    listing: RecentListing | null;
    loading: boolean;
    loadError: string | null;
    /** Project id → ticked. */
    ticks: Record<number, boolean>;
    onToggle: (id: number, on: boolean) => void;
    /** "Watch N projects". */
    onSetUp: () => void;
    /** The button's work is under way (deploy detection for each ticked row). */
    busy: boolean;
    /** The core's word about a one-click answer it refused (`watches`). */
    error?: string | null;
    /** The clock the "active" labels are measured against. */
    now: number;
  }

  let { provider, listing, loading, loadError, ticks, onToggle, onSetUp, busy, error = null, now }: Props = $props();

  const github = $derived(provider === "github");
  const noun = $derived(github ? "repositories" : "projects");
  const rows = $derived(listing?.mode === "projects" ? listing.projects : []);
  const count = $derived(rows.filter((r) => watchable(r) && ticks[r.id]).length);
  const one = $derived(github ? "repository" : "project");
</script>

{#if loading || loadError || listing}
  <section class="flex flex-col gap-2 border-t pt-4" aria-labelledby="wizard-quick-heading" data-slot="quick-setup">
    <h3 id="wizard-quick-heading" class="text-[13px]/[18px] font-semibold">Watch your recent {noun}</h3>
    {#if loading}
      <p class="text-muted-foreground m-0" aria-live="polite">Loading your recent {noun}…</p>
    {:else if loadError}
      <p class="text-muted-foreground m-0" data-slot="quick-error">
        Could not list your recent {noun}: {loadError} Choose Next to pick one instead.
      </p>
    {:else if listing?.mode === "unlisted"}
      <p class="text-muted-foreground m-0" data-slot="quick-unlisted">{listing.reason}</p>
    {:else if rows.length === 0}
      <p class="text-muted-foreground m-0" data-slot="quick-empty">
        This account has no {noun} yet. Choose Next to type one instead.
      </p>
    {:else}
      <p class="text-muted-foreground m-0 text-xs">
        The {rows.length} most recently active, newest first. The top three are ticked; untick any you do not want, or choose
        Next to set up one {one} in detail.
      </p>
      <fieldset class="flex flex-col gap-0.5 rounded-lg border p-1" aria-describedby="wizard-quick-msg">
        <legend class="sr-only">Recent {noun}</legend>
        {#each rows as row (row.id)}
          {@const label = activeLabel(row, now)}
          <label
            class="hover:bg-muted flex min-w-0 items-center gap-2 rounded-md px-2 py-1"
            data-slot="quick-row"
            data-id={row.id}
          >
            <input
              type="checkbox"
              class="accent-primary"
              checked={watchable(row) && !!ticks[row.id]}
              disabled={!watchable(row) || busy}
              onchange={(event) => onToggle(row.id, (event.currentTarget as HTMLInputElement).checked)}
            />
            <span class="min-w-0 truncate font-mono">{row.path}</span>
            <span class="text-muted-foreground ml-auto shrink-0 text-xs" data-slot="quick-meta">
              {#if !watchable(row)}empty{:else}{row.default_branch}{/if}{#if label}<span data-slot="quick-active"
                  >{" · "}{label}</span
                >{/if}
            </span>
          </label>
        {/each}
      </fieldset>
      <FieldMessage id="wizard-quick-msg" message={error} />
      <div>
        <Button size="sm" onclick={onSetUp} disabled={busy || count === 0} data-action="quick-setup">
          {busy ? "Looking for deploy jobs…" : `Watch ${count} ${count === 1 ? one : noun}`}
        </Button>
      </div>
    {/if}
  </section>
{/if}
