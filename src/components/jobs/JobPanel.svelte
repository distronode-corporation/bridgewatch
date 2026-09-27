<script lang="ts">
  /**
   * What opens under one job's row: the end of its log, and the question
   * before a write (Retry, Play) with its outcome.
   *
   * ⛔ The log is rendered as TEXT, inside a `<pre>`, and never as markup: it
   * is whatever a CI job printed, which is to say whatever anybody who can push
   * a commit chose to print. The core has already taken out escape sequences
   * and control characters; Svelte's text interpolation does the rest.
   *
   * The log is fetched when this panel opens and not before: it is the most
   * expensive thing a provider serves, and the poller never asks for one.
   */
  import { SHELL_API, WRITES, confirmText, errorText, type JobTools } from "./tools";
  import type { JobAction, JobView, LogTail } from "../../lib/types";

  interface Props {
    job: JobView;
    tools: JobTools;
    /** Show the log section. */
    log: boolean;
    /** The write being asked about, or none. */
    confirm: JobAction | null;
    /** Close the question (Cancel, or after it was answered and read). */
    onCancel: () => void;
    /** Open a URL through the shell's link opener. */
    onOpen: (url: string) => void;
    /** The element id, for the row buttons' `aria-controls`. */
    id: string;
  }

  let { job, tools, log, confirm, onCancel, onOpen, id }: Props = $props();

  const api = $derived(tools.api ?? SHELL_API);

  type Loaded = { state: "loading" } | { state: "error"; message: string } | { state: "ready"; tail: LogTail };
  let loaded = $state<Loaded>({ state: "loading" });

  type Write = { state: "idle" } | { state: "sending" } | { state: "done"; action: JobAction } | { state: "error"; message: string };
  let write = $state<Write>({ state: "idle" });

  // Fetch when the log section opens; a closed section keeps nothing.
  $effect(() => {
    if (!log || !job.web_url) return;
    const url = job.web_url;
    let cancelled = false;
    loaded = { state: "loading" };
    api.logTail(tools.watchId, url).then(
      (tail) => {
        if (!cancelled) loaded = { state: "ready", tail };
      },
      (error) => {
        if (!cancelled) loaded = { state: "error", message: errorText(error) };
      },
    );
    return () => {
      cancelled = true;
    };
  });

  // A new question starts clean.
  $effect(() => {
    if (confirm) write = { state: "idle" };
  });

  function send(action: JobAction) {
    const url = job.web_url;
    if (!url) return;
    const pending = WRITES.run(`${tools.watchId} ${action} ${url}`, () => api.action(tools.watchId, url, action));
    // Already on its way: this click sends nothing.
    if (!pending) return;
    write = { state: "sending" };
    pending.then(
      () => {
        write = { state: "done", action };
      },
      (error) => {
        write = { state: "error", message: errorText(error) };
      },
    );
  }

  function openJob(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (job.web_url) onOpen(job.web_url);
  }
</script>

<div class="flex flex-col gap-1 py-1 pr-1 pl-4 text-xs" data-slot="job-panel" {id}>
  {#if confirm}
    <div class="bg-muted/60 flex flex-col gap-1 rounded-md px-2 py-1.5" data-slot="job-confirm">
      <p class="text-foreground" data-slot="job-confirm-text">{confirmText(confirm, job, tools.context)}</p>
      {#if write.state === "done"}
        <p class="text-tone-green" role="status" data-slot="job-action-done">
          {write.action === "play" ? "Started." : "Retry sent."} The watch is being refreshed.
        </p>
      {:else}
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="bg-primary text-primary-foreground hover:bg-primary/80 focus-visible:ring-ring/50 rounded-md px-2 py-0.5 font-medium outline-none focus-visible:ring-2 disabled:opacity-50"
            data-slot="job-confirm-yes"
            disabled={write.state === "sending"}
            onclick={() => confirm && send(confirm)}>{confirm === "play" ? "Start" : "Retry"}</button
          >
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 rounded-md px-2 py-0.5 outline-none focus-visible:ring-2"
            data-slot="job-confirm-no"
            onclick={onCancel}>Cancel</button
          >
          {#if write.state === "sending"}
            <span class="text-muted-foreground" role="status">Sending…</span>
          {/if}
        </div>
      {/if}
      {#if write.state === "error"}
        <p class="text-tone-red break-words" role="alert" data-slot="job-action-error">{write.message}</p>
      {/if}
    </div>
  {/if}

  {#if log}
    <div class="flex flex-col gap-1" data-slot="job-log">
      {#if loaded.state === "loading"}
        <p class="text-muted-foreground" role="status" data-slot="job-log-loading">Loading the log…</p>
      {:else if loaded.state === "error"}
        <p class="text-tone-red break-words" role="alert" data-slot="job-log-error">
          Could not read the log: {loaded.message}
        </p>
      {:else if loaded.tail.lines.length === 0}
        <p class="text-muted-foreground" data-slot="job-log-empty">The log is empty.</p>
      {:else}
        <!-- Focusable so a keyboard can scroll it (a scrollable region nobody
             can focus is one a keyboard cannot read to the end); labelled so a
             screen reader says what it is before reading forty lines of it. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <pre
          class="bg-muted text-foreground max-h-48 overflow-auto rounded-md px-2 py-1 font-mono text-[11px] leading-snug whitespace-pre"
          tabindex="0"
          aria-label={`Last ${loaded.tail.lines.length} lines of the log of ${job.name}`}
          data-slot="job-log-text">{loaded.tail.lines.join("\n")}</pre>
        {#if loaded.tail.truncated}
          <p class="text-muted-foreground" data-slot="job-log-truncated">Only the end of a long log was read.</p>
        {/if}
      {/if}
      {#if job.web_url}
        <a
          href={job.web_url}
          class="text-muted-foreground focus-visible:ring-ring/50 self-start rounded-sm outline-none hover:underline focus-visible:ring-2"
          data-slot="job-log-open"
          onclick={openJob}>Open the full log</a
        >
      {/if}
    </div>
  {/if}
</div>
