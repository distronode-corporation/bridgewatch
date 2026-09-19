<script lang="ts">
  /**
   * The optional first-run setup wizard: six steps, back/next with per-step
   * validation, skippable everywhere, and nothing written until Finish.
   *
   * Every live call goes through the injected `api` (see api.ts). A `command`
   * token source is never run on a click alone: the user first confirms the
   * exact argv here, and a `NeedsConfirm` answer from the shell (Lo13) gets
   * its own dialog in the shell's words before the request is repeated.
   */
  import { onMount, tick, untrack } from "svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Stepper } from "$lib/components/ui/stepper/index.js";
  import type { DiagnosticView } from "../../lib/types";
  import AccountStep from "./AccountStep.svelte";
  import ProjectStep from "./ProjectStep.svelte";
  import WatchStep from "./WatchStep.svelte";
  import DeployStep from "./DeployStep.svelte";
  import PreferencesStep from "./PreferencesStep.svelte";
  import ReviewStep from "./ReviewStep.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    diagnosticsOf,
    issuesOf,
    messageOf,
    needsConfirm,
    type Confirmable,
    type CliTokenDetection,
    type Connection,
    type Identity,
    type MarkerSuggestions,
    type ProjectListing,
    type ProjectSummary,
    type Provider,
    type StepIssue,
    type WizardAnswers,
    type WizardApi,
    type WizardPreview,
  } from "./api";
  import {
    STEPS,
    answersOf,
    baseUrlOf,
    cliName,
    draftFromAnswers,
    emptyDraft,
    formatCommand,
    parseCommand,
    projectRefFor,
    stepTitle,
    suggestAccountName,
    suggestWatchId,
    switchProvider,
    tokenSourceOf,
    validateStep,
    type Draft,
    type StepErrors,
  } from "./model";

  interface Props {
    api: WizardApi;
    /** Answers to start from when re-running on an existing config. */
    initial?: Partial<WizardAnswers> | null;
    /** Called after a successful save. */
    onFinish?: (result: { path?: string }) => void;
    /** Called after `api.skip()` resolved: nothing was written. */
    onSkip?: () => void;
  }

  let { api, initial = null, onFinish, onSkip }: Props = $props();

  // The draft is seeded once; later changes to `initial` do not reset a form in progress.
  const seed = (): Draft => (initial ? draftFromAnswers(initial) : emptyDraft());
  let draft = $state<Draft>(seed());
  const freshStart = untrack(() => !initial?.token);

  let index = $state(0);
  let errors = $state<StepErrors>({});
  const step = $derived(STEPS[index]);
  const isLast = $derived(index === STEPS.length - 1);

  // --- connection ----------------------------------------------------------
  const connKey = $derived(
    JSON.stringify([
      draft.provider,
      baseUrlOf(draft),
      tokenSourceOf(draft),
      draft.tokenMode === "paste" ? draft.secret : null,
    ]),
  );

  function connection(confirm?: string): Connection {
    // `provider` only for GitHub: the shell reads its absence as gitlab, so a
    // GitLab connection is exactly the one it always was.
    const conn: Connection =
      draft.provider === "github"
        ? { provider: "github", base_url: baseUrlOf(draft), token: tokenSourceOf(draft) }
        : { base_url: baseUrlOf(draft), token: tokenSourceOf(draft) };
    if (draft.tokenMode === "paste" && draft.secret.trim()) conn.secret = draft.secret.trim();
    if (confirm) conn.confirm = confirm;
    return conn;
  }

  // --- confirmation dialog -------------------------------------------------
  let dialog = $state<{ title: string; lines: string[]; command: string[] | null; label: string } | null>(null);
  let resolveDialog: ((ok: boolean) => void) | null = null;

  function askConfirm(title: string, lines: string[], command: string[] | null, label = "Run it"): Promise<boolean> {
    resolveDialog?.(false);
    dialog = { title, lines, command, label };
    return new Promise((resolve) => (resolveDialog = resolve));
  }

  function closeDialog(ok: boolean) {
    const resolve = resolveDialog;
    resolveDialog = null;
    dialog = null;
    resolve?.(ok);
  }

  /** The command text the user has agreed to run. Editing the command resets it. */
  let commandAck = $state<string | null>(null);

  async function ensureCommandAck(): Promise<boolean> {
    if (draft.tokenMode !== "command" || commandAck === draft.commandText) return true;
    const ok = await askConfirm(
      "Run this command for your token?",
      [
        "bridgewatch will run this program whenever it needs the token, and use what it prints.",
        "It runs directly, without a shell, exactly as listed below.",
      ],
      parseCommand(draft.commandText),
    );
    if (ok) commandAck = draft.commandText;
    return ok;
  }

  /** Run a live request, handling both confirmation layers. `null` means the user declined. */
  async function live<T>(call: (conn: Connection) => Promise<Confirmable<T>>): Promise<T | null> {
    if (!(await ensureCommandAck())) return null;
    let answer = await call(connection());
    if (needsConfirm(answer)) {
      const command = draft.tokenMode === "command" ? parseCommand(draft.commandText) : null;
      if (!(await askConfirm("Allow this?", answer.confirm.changes, command))) return null;
      answer = await call(connection(answer.confirm.id));
      if (needsConfirm(answer)) throw new Error("The request was not allowed to run.");
    }
    return answer;
  }

  // --- step 1: CLI token detection (glab or gh) and test connection -------
  let cli = $state<CliTokenDetection | null>(null);
  let detectedSource: Draft["cliSource"] = null;
  let detectSeq = 0;
  // `draft` is a deep $state proxy, so compare sources by value, never by identity.
  const sameSource = (a: Draft["cliSource"], b: Draft["cliSource"]) => JSON.stringify(a) === JSON.stringify(b);

  async function detect() {
    if (!api.detectCliToken) return;
    const base = baseUrlOf(draft);
    const provider = draft.provider;
    const seq = ++detectSeq;
    let result: CliTokenDetection;
    try {
      result = await api.detectCliToken(base, provider);
    } catch (error) {
      result = { status: "unavailable", reason: messageOf(error) };
    }
    if (seq !== detectSeq) return;
    cli = result;
    if (result.status === "found") {
      const adopt = draft.tokenMode === "cli" || (freshStart && draft.tokenMode === "paste" && !draft.secret);
      if (adopt || sameSource(draft.cliSource, detectedSource)) draft.cliSource = result.source;
      detectedSource = result.source;
      if (adopt) draft.tokenMode = "cli";
    } else if (draft.cliSource !== null && sameSource(draft.cliSource, detectedSource)) {
      draft.cliSource = null;
      detectedSource = null;
      if (draft.tokenMode === "cli") draft.tokenMode = "paste";
    }
  }

  function instanceChanged() {
    if (!draft.accountEdited) draft.account = suggestAccountName(baseUrlOf(draft), draft.provider);
    void detect();
  }

  function providerChanged(provider: Provider) {
    if (draft.provider === provider) return;
    switchProvider(draft, provider);
    // The last provider's detection answers nothing about this one.
    cli = null;
    detectedSource = null;
    listing = null;
    listingKey = null;
    suggestions = null;
    suggestionsKey = null;
    delete errors.project;
    void detect();
  }

  onMount(() => {
    void detect();
  });

  let identity = $state<Identity | null>(null);
  let testError = $state<string | null>(null);
  let testKey = $state<string | null>(null);
  let testing = $state(false);
  const shownIdentity = $derived(testKey === connKey ? identity : null);
  const shownTestError = $derived(testKey === connKey ? testError : null);

  async function testConnection() {
    const stepErrors = validateStep("account", draft);
    errors = stepErrors;
    if (Object.keys(stepErrors).length > 0) return focusInvalid();
    testing = true;
    const key = connKey;
    try {
      const result = await live((c) => api.testConnection(c));
      if (result === null) return;
      identity = result;
      testError = null;
      testKey = key;
    } catch (error) {
      identity = null;
      testError = messageOf(error);
      testKey = key;
    } finally {
      testing = false;
    }
  }

  // --- step 2: project -----------------------------------------------------
  let listing = $state<ProjectListing | null>(null);
  let listingKey: string | null = null;
  let listLoading = $state(false);
  let listError = $state<string | null>(null);
  let resolving = $state(false);

  async function loadListing(search?: string) {
    const key = connKey;
    listLoading = true;
    listError = null;
    try {
      const result = await live((c) => api.listProjects(c, search || undefined));
      if (result === null) return;
      listing = result;
      listingKey = key;
      if (result.mode === "type_id_or_path" && result.suggestion !== null && !draft.projectInput.trim()) {
        draft.projectInput = String(result.suggestion);
      }
    } catch (error) {
      listError = messageOf(error);
    } finally {
      listLoading = false;
    }
  }

  function setProject(project: NonNullable<Draft["project"]>) {
    const changed = draft.project?.ref !== project.ref;
    draft.project = project;
    if (changed && project.default_branch) draft.refName = project.default_branch;
    if (!draft.watchIdEdited) draft.watchId = suggestWatchId(project.path, draft.refName);
    delete errors.project;
  }

  function pick(project: ProjectSummary) {
    draft.projectInput = project.path;
    setProject({
      ref: projectRefFor(draft.provider, project),
      path: project.path,
      default_branch: project.default_branch,
    });
  }

  /** True when the typed text already names the chosen project. */
  const inputMatchesProject = () => {
    const typed = draft.projectInput.trim();
    return !!draft.project && (typed === "" || typed === draft.project.path || typed === String(draft.project.ref));
  };

  async function resolve(): Promise<boolean> {
    const typed = draft.projectInput.trim();
    if (!typed) return false;
    resolving = true;
    try {
      const result = await live((c) => api.resolveProject(c, typed));
      if (result === null) return false;
      setProject({
        ref: result.project ?? projectRefFor(draft.provider, result),
        path: result.path,
        default_branch: result.default_branch,
      });
      return true;
    } catch (error) {
      const issue = issuesOf(error).find((i) => i.step === "project");
      errors = { ...errors, project: issue?.message ?? messageOf(error) };
      return false;
    } finally {
      resolving = false;
    }
  }

  // --- step 4: deploy markers ---------------------------------------------
  let suggestions = $state<MarkerSuggestions | null>(null);
  let suggestionsKey: string | null = null;
  let sugLoading = $state(false);
  let sugError = $state<string | null>(null);

  async function loadSuggestions() {
    const project = draft.project;
    if (!project) return;
    // GitHub only: the workflow picks which run the names come from.
    const workflow = draft.provider === "github" ? draft.workflow.trim() : "";
    const key = JSON.stringify([connKey, project.ref, draft.refName.trim(), workflow]);
    if (key === suggestionsKey) return;
    sugLoading = true;
    sugError = null;
    try {
      const result = await live((c) =>
        workflow
          ? api.suggestDeployMarkers(c, project.ref, draft.refName.trim(), workflow)
          : api.suggestDeployMarkers(c, project.ref, draft.refName.trim()),
      );
      if (result === null) return;
      suggestions = result;
      suggestionsKey = key;
    } catch (error) {
      suggestions = null;
      sugError = messageOf(error);
    } finally {
      sugLoading = false;
    }
  }

  // --- step 6: preview and save -------------------------------------------
  let preview = $state<WizardPreview | null>(null);
  let previewLoading = $state(false);
  let previewError = $state<string | null>(null);
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let saveDiagnostics = $state<DiagnosticView[]>([]);

  async function loadPreview() {
    previewLoading = true;
    previewError = null;
    preview = null;
    try {
      preview = await api.previewConfig(answersOf(draft));
    } catch (error) {
      const issues = issuesOf(error);
      if (issues.length > 0) return void jumpToIssues(issues);
      previewError = messageOf(error);
      saveDiagnostics = diagnosticsOf(error);
    } finally {
      previewLoading = false;
    }
  }

  const tokenNote = $derived.by(() => {
    switch (draft.tokenMode) {
      case "paste":
        return `The token you pasted goes into bridgewatch's own keychain entry for "${draft.account.trim()}", never into this file.`;
      case "cli":
        return `The token stays in ${cliName(draft.provider)}'s keychain entry; this file only says where it is.`;
      case "env":
        return `The token is read from $${draft.envVar.trim()} when bridgewatch starts.`;
      case "command":
        return `The token is read by running: ${formatCommand(parseCommand(draft.commandText))}`;
      default:
        return null;
    }
  });

  async function finish() {
    if (saving) return;
    saveError = null;
    saveDiagnostics = [];
    if (!(await ensureCommandAck())) return;
    saving = true;
    const answers = answersOf(draft);
    const secret = draft.tokenMode === "paste" && draft.secret.trim() ? draft.secret.trim() : undefined;
    try {
      let result = await api.save(answers, { secret });
      if (result.confirm) {
        const command = draft.tokenMode === "command" ? parseCommand(draft.commandText) : null;
        if (!(await askConfirm("Save with these changes?", result.confirm.changes, command, "Save"))) return;
        result = await api.save(answers, { secret, confirm: result.confirm.id });
      }
      if (result.issues && result.issues.length > 0) return void jumpToIssues(result.issues);
      if (result.ok && !result.confirm) {
        onFinish?.({ path: result.path });
        return;
      }
      if (result.conflict) {
        saveError = "config.toml changed on disk while the wizard was open. The preview now shows the new file; check it and Finish again.";
        await loadPreview();
        return;
      }
      saveDiagnostics = result.diagnostics.filter((d) => d.severity === "error");
      if (saveDiagnostics.length === 0) saveError = "The config was not written.";
    } catch (error) {
      const issues = issuesOf(error);
      if (issues.length > 0) return void jumpToIssues(issues);
      saveError = messageOf(error);
      saveDiagnostics = diagnosticsOf(error);
    } finally {
      saving = false;
    }
  }

  // --- skip ----------------------------------------------------------------
  let skipping = $state(false);
  let skipError = $state<string | null>(null);

  async function skip() {
    if (skipping) return;
    skipping = true;
    skipError = null;
    try {
      await api.skip();
      onSkip?.();
    } catch (error) {
      skipError = messageOf(error);
    } finally {
      skipping = false;
    }
  }

  // --- navigation ----------------------------------------------------------
  let heading = $state<HTMLHeadingElement | null>(null);
  let body = $state<HTMLDivElement | null>(null);

  async function focusInvalid() {
    await tick();
    const target = body?.querySelector<HTMLElement>('[aria-invalid="true"]');
    (target ?? heading)?.focus();
  }

  function enter(stepIndex: number) {
    switch (STEPS[stepIndex].id) {
      case "project":
        if (listingKey !== connKey) void loadListing();
        break;
      case "deploy":
        void loadSuggestions();
        break;
      case "review":
        saveError = null;
        saveDiagnostics = [];
        void loadPreview();
        break;
    }
  }

  async function goTo(stepIndex: number, keepErrors: StepErrors = {}) {
    index = stepIndex;
    errors = keepErrors;
    enter(stepIndex);
    await tick();
    heading?.focus();
  }

  async function jumpToIssues(issues: StepIssue[]) {
    const first = issues[0];
    const target = STEPS.findIndex((s) => s.id === first.step);
    const stepErrors: StepErrors = {};
    for (const issue of issues) if (issue.step === first.step && !stepErrors[issue.field]) stepErrors[issue.field] = issue.message;
    if (target !== index) await goTo(Math.max(0, target), stepErrors);
    else errors = stepErrors;
    await focusInvalid();
  }

  let advancing = $state(false);

  async function next() {
    if (advancing) return;
    advancing = true;
    try {
      const id = step.id;
      if (id === "project" && draft.projectInput.trim() && !inputMatchesProject()) {
        if (!(await resolve())) return void focusInvalid();
      }
      const stepErrors = validateStep(id, draft);
      errors = stepErrors;
      if (Object.keys(stepErrors).length > 0) return void focusInvalid();
      if (id === "account" && !(await ensureCommandAck())) return;
      await goTo(index + 1);
    } finally {
      advancing = false;
    }
  }

  function back() {
    if (index > 0) void goTo(index - 1);
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (isLast) void finish();
    else void next();
  }

  const busy = $derived(advancing || saving || skipping);
</script>

<section class="flex h-full flex-col gap-4 p-4" aria-labelledby="wizard-heading" data-slot="wizard" data-step={step.id}>
  <Stepper steps={STEPS} current={index} onSelect={(i) => void goTo(i)} label="Setup steps" />

  <form class="flex min-h-0 flex-1 flex-col gap-4" onsubmit={submit} novalidate>
    <h2 id="wizard-heading" class="text-base font-semibold outline-none" tabindex="-1" bind:this={heading}>
      {stepTitle(step, draft.provider)}
    </h2>

    <div class="min-h-0 flex-1 overflow-y-auto" bind:this={body}>
      {#if step.id === "account"}
        <AccountStep
          bind:draft
          {errors}
          {cli}
          identity={shownIdentity}
          {testing}
          testError={shownTestError}
          onTest={() => void testConnection()}
          onInstanceChange={instanceChanged}
          onProviderChange={providerChanged}
        />
      {:else if step.id === "project"}
        <ProjectStep
          bind:draft
          {errors}
          {listing}
          loading={listLoading}
          loadError={listError}
          {resolving}
          onPick={pick}
          onResolve={() => void resolve()}
          onSearch={(term) => void loadListing(term)}
        />
      {:else if step.id === "watch"}
        <WatchStep bind:draft {errors} />
      {:else if step.id === "deploy"}
        <DeployStep bind:draft {errors} {suggestions} loading={sugLoading} loadError={sugError} />
      {:else if step.id === "preferences"}
        <PreferencesStep bind:draft {errors} />
      {:else}
        <ReviewStep
          {preview}
          loading={previewLoading}
          error={previewError}
          {saveDiagnostics}
          {saveError}
          {tokenNote}
          primary={draft.primary}
          onprimary={(primary) => {
            draft.primary = primary;
            void loadPreview();
          }}
        />
      {/if}
    </div>

    {#if skipError}
      <p class="text-destructive text-xs" role="alert">{skipError}</p>
    {/if}

    <div class="flex items-center gap-2 border-t pt-3">
      <Button variant="ghost" size="sm" onclick={() => void skip()} disabled={busy} data-action="skip">
        Skip, I'll edit config.toml
      </Button>
      <span class="flex-1"></span>
      <Button variant="outline" size="sm" onclick={back} disabled={index === 0 || busy} data-action="back">Back</Button>
      {#if isLast}
        <Button type="submit" size="sm" disabled={busy || previewLoading || !preview} data-action="finish">
          {saving ? "Saving…" : "Finish"}
        </Button>
      {:else}
        <Button type="submit" size="sm" disabled={busy} data-action="next">Next</Button>
      {/if}
    </div>
  </form>
</section>

<ConfirmDialog
  open={dialog !== null}
  title={dialog?.title ?? ""}
  lines={dialog?.lines ?? []}
  command={dialog?.command ?? null}
  confirmLabel={dialog?.label ?? "Run it"}
  onConfirm={() => closeDialog(true)}
  onCancel={() => closeDialog(false)}
/>
