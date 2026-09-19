<script lang="ts">
  /** Step 5: notifications, launch at login, live poll speed. */
  import { Label } from "$lib/components/ui/label/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import { liveSpeeds, type Draft, type StepErrors } from "./model";

  interface Props {
    draft: Draft;
    errors: StepErrors;
  }

  let { draft = $bindable(), errors }: Props = $props();

  const speeds = $derived(liveSpeeds(draft.provider));
  // GitHub's budget is 5,000 requests an HOUR per token (shared with gh and
  // anything else using it), so its watches start slower and idle at 120 s.
  const idleHint = $derived(
    draft.provider === "github"
      ? "When nothing is running it checks every two minutes. GitHub allows 5,000 requests an hour per token, shared with gh and anything else using it."
      : "When nothing is running it checks once a minute.",
  );

  const NOTIFY: { key: keyof Draft["notify"]; label: string }[] = [
    { key: "deployed", label: "A deploy finished" },
    { key: "blocking_failure", label: "A pipeline failed in a way that blocks it" },
    { key: "finished", label: "Any watched pipeline finished" },
  ];
</script>

<div class="flex flex-col gap-4">
  <fieldset class="flex flex-col gap-2">
    <legend class="mb-1 text-sm font-medium">Notify me when</legend>
    {#each NOTIFY as item (item.key)}
      <div class="flex items-center gap-2">
        <Switch id={`wizard-notify-${item.key}`} bind:checked={draft.notify[item.key]} />
        <Label for={`wizard-notify-${item.key}`} class="font-normal">{item.label}</Label>
      </div>
    {/each}
  </fieldset>

  <div class="flex items-center gap-2">
    <Switch id="wizard-launch" bind:checked={draft.launchAtLogin} />
    <Label for="wizard-launch" class="font-normal">Start bridgewatch when I log in</Label>
  </div>

  <div class="flex flex-col gap-1">
    <Label for="wizard-live-secs">While a pipeline is running, check</Label>
    <select
      id="wizard-live-secs"
      name="live_secs"
      class="border-input bg-input/30 focus-visible:border-ring focus-visible:ring-ring/30 h-8 w-48 rounded-2xl border px-3 text-sm outline-none focus-visible:ring-3"
      value={String(draft.liveSecs)}
      onchange={(event) => (draft.liveSecs = Number(event.currentTarget.value))}
      aria-invalid={errors.live_secs ? "true" : undefined}
      aria-describedby="wizard-live-msg"
    >
      {#if !speeds.some((s) => s.value === draft.liveSecs)}
        <option value={String(draft.liveSecs)}>Every {draft.liveSecs} s (current)</option>
      {/if}
      {#each speeds as speed (speed.value)}
        <option value={String(speed.value)}>{speed.label}</option>
      {/each}
    </select>
    <FieldMessage
      id="wizard-live-msg"
      message={errors.live_secs}
      hint={idleHint}
    />
  </div>
</div>
