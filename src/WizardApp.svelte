<script lang="ts">
  /**
   * The wizard window: the ui-wizard lane's `Wizard`, wired to the shell.
   *
   * The shell shows this window itself on a first launch with no config file,
   * and the tray and Settings re-open it. A re-run pre-fills from the current
   * configuration, and the wizard edits that file in place rather than
   * replacing it.
   *
   * The window is hidden by the SHELL, not from here: `wizard_save` hides it
   * once the file is written, and `wizard_skip` hides it and opens Settings on
   * the text tab. `onfinish` / `onskip` exist for the tests.
   */
  import { onMount } from "svelte";

  import { Wizard } from "./components/wizard";
  import type { WizardAnswers, WizardApi } from "./components/wizard/api";
  import { getConfigJson, inTauri, wizardApi, wizardInitial } from "./lib/ipc";

  interface Props {
    /** Injected by the tests; the shell's commands otherwise. */
    api?: WizardApi;
    onfinish?: () => void;
    onskip?: () => void;
  }

  let { api, onfinish, onskip }: Props = $props();

  // `undefined` until the configuration has been read: the wizard seeds its
  // draft ONCE, so mounting it before the answers arrive would lose them.
  let initial = $state<Partial<WizardAnswers> | null | undefined>(undefined);

  // The account a re-run is editing is the fallback for `Connection.account`,
  // which an `{own: true}` token source needs to find its stored token.
  const shellApi = $derived(api ?? wizardApi(initial?.account));

  onMount(() => {
    if (!inTauri()) {
      initial = null;
      return;
    }
    void getConfigJson()
      .then((payload) => (initial = wizardInitial(payload?.config) ?? null))
      .catch(() => (initial = null));
  });
</script>

<main class="bg-background text-foreground h-screen overflow-y-auto p-4 text-[13px]">
  {#if initial !== undefined}
    <Wizard api={shellApi} {initial} onFinish={() => onfinish?.()} onSkip={() => onskip?.()} />
  {/if}
</main>
