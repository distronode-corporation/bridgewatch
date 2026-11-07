<script lang="ts">
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import { untrack, type Snippet } from "svelte";

  import { CARD, SECTION_TITLE } from "./styles";

  interface Props {
    title: string;
    /** Collapsible: a native disclosure, so Enter and Space work and a screen reader says "collapsed". */
    collapsible?: boolean;
    /** Whether a collapsible group starts open. */
    open?: boolean;
    /** A short count shown next to a collapsed title, e.g. "2 set". */
    note?: string;
    /** Rows to put inside the card. None: the children are laid out by the caller. */
    card?: boolean;
    children: Snippet;
  }

  let { title, collapsible = false, open = false, note = "", card = true, children }: Props = $props();

  // ⚠ Seeded once: after that the disclosure is the user's. A re-read of the
  // file after every save must not fold a group back up under the cursor.
  let expanded = $state(untrack(() => open));
</script>

{#if collapsible}
  <details
    class="settings-group group/section"
    data-group={title}
    data-default-open={open ? "true" : "false"}
    bind:open={expanded}
  >
    <summary
      class="text-foreground hover:bg-muted/60 flex cursor-pointer list-none items-center gap-1.5 rounded-md px-1 py-1 outline-none select-none focus-visible:ring-2 focus-visible:ring-ring/50 [&::-webkit-details-marker]:hidden"
    >
      <ChevronRightIcon class="text-muted-foreground size-3.5 shrink-0 transition-transform group-open/section:rotate-90" aria-hidden="true" />
      <span class={[SECTION_TITLE, "group-hover/section:text-foreground"]}>{title}</span>
      {#if note && !expanded}
        <span class="text-muted-foreground text-ui-meta" data-slot="group-note">{note}</span>
      {/if}
    </summary>
    <div class={["mt-1", card && CARD]}>
      {@render children()}
    </div>
  </details>
{:else}
  <section class="settings-group" data-group={title}>
    <h3 class={["mb-2 px-1", SECTION_TITLE]}>{title}</h3>
    <div class={[card && CARD]}>
      {@render children()}
    </div>
  </section>
{/if}
