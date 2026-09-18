<script lang="ts">
  import { cn } from "$lib/components/ui/utils.js";
  import { openExternal } from "../lib/ipc";

  interface Props {
    href: string | null | undefined;
    title?: string;
    class?: string;
    children?: import("svelte").Snippet;
  }

  let { href, title, class: klass = "", children }: Props = $props();

  // ⛔ `href` is set so the link has a real target for middle-click, hover
  // preview and a11y, but the default navigation is always cancelled: a GitLab
  // page loaded into a 440px undecorated popover is a trap with no way out.
  function open(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    void openExternal(href);
  }
</script>

{#if href}
  <a
    {href}
    {title}
    class={cn("cursor-pointer rounded-sm underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring/50", klass)}
    onclick={open}>{@render children?.()}</a>
{:else}
  <span {title} class={klass}>{@render children?.()}</span>
{/if}
