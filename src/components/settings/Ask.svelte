<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { INPUT } from "./styles";

  /**
   * A question asked IN THE DOCUMENT, because the browser's own dialogs are not
   * there to ask it.
   *
   * ⛔ `window.confirm` and `window.prompt` return immediately with `false` and
   * `null` on macOS: wry's `WKUIDelegate` implements none of the
   * `runJavaScript{Alert,Confirm,TextInput}Panel` methods, and WebKit completes
   * an unimplemented one with the empty answer. So "Remove watch" did nothing,
   * "Add watch" did nothing, and the command-source confirmation silently
   * refused itself — three controls that looked broken rather than cancelled.
   *
   * ⚠ Keyboard handling hangs off `<svelte:window>` rather than the panel
   * element: a `keydown` on a `role="dialog"` div is a non-interactive element
   * handler, which `svelte-check` reports, and the listener is only mounted
   * while the question is on screen anyway.
   */

  interface AskField {
    name: string;
    label: string;
    placeholder?: string;
  }

  interface Props {
    title: string;
    /** The consequence, in the words the user would use. */
    message?: string;
    /** Zero or more answers to collect. Every one of them is required. */
    fields?: AskField[];
    confirmLabel?: string;
    /** Colours the confirm button for something that destroys. */
    danger?: boolean;
    onconfirm: (values: Record<string, string>) => void;
    oncancel: () => void;
  }

  let {
    title,
    message = "",
    fields = [],
    confirmLabel = "Continue",
    danger = false,
    onconfirm,
    oncancel,
  }: Props = $props();

  // Keyed by field name and filled by the bindings. An initializer built from
  // `fields` would read a prop outside a derived, which is the shape Svelte
  // warns about because it silently captures only the first value.
  let values = $state<Record<string, string>>({});
  let first = $state<HTMLInputElement | null>(null);

  // Every field is required: the two questions this asks — a watch id and a
  // project — have no default that would not write a watch that 404s on every
  // tick.
  const complete = $derived(fields.every((f) => (values[f.name] ?? "").trim() !== ""));

  $effect(() => {
    first?.focus();
  });

  function confirm() {
    if (!complete) return;
    const answers: Record<string, string> = {};
    for (const field of fields) answers[field.name] = (values[field.name] ?? "").trim();
    onconfirm(answers);
  }

  function key(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      oncancel();
      return;
    }
    // Enter confirms only from a field: a bare Enter anywhere on the page
    // would confirm a destructive question the user had not read.
    if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
      event.preventDefault();
      confirm();
    }
  }
</script>

<svelte:window onkeydown={key} />

<div
  class="ask bg-card text-card-foreground border-border mb-3 flex flex-col gap-2 rounded-2xl border px-3.5 py-3 shadow-sm"
  role="dialog"
  aria-modal="true"
  aria-label={title}
>
  <p class="title m-0 font-semibold">{title}</p>
  {#if message}
    <p class="message text-muted-foreground m-0 text-xs whitespace-pre-wrap">{message}</p>
  {/if}
  {#each fields as field, index (field.name)}
    <label class="row grid grid-cols-[170px_1fr] items-center gap-x-3">
      <span class="text-muted-foreground text-right">{field.label}</span>
      {#if index === 0}
        <input
          type="text"
          class={INPUT}
          placeholder={field.placeholder ?? ""}
          bind:this={first}
          bind:value={values[field.name]}
        />
      {:else}
        <input type="text" class={INPUT} placeholder={field.placeholder ?? ""} bind:value={values[field.name]} />
      {/if}
    </label>
  {/each}
  <div class="actions mt-1 flex gap-2">
    <Button variant="outline" size="sm" class="cancel" onclick={oncancel}>Cancel</Button>
    <Button
      variant={danger ? "destructive" : "default"}
      size="sm"
      class={["confirm", danger && "danger"]}
      onclick={confirm}
      disabled={!complete}
    >
      {confirmLabel}
    </Button>
  </div>
</div>
