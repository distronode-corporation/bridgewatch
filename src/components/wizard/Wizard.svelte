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
  import { messageOf } from "../../lib/format";
  import { cliName } from "../../lib/providers";
  import type { OAuthApi, OAuthAvailability, SignedIn } from "../../lib/oauth";
  import AccountStep from "./AccountStep.svelte";
  import ProjectStep from "./ProjectStep.svelte";
  import WatchStep from "./WatchStep.svelte";
  import DeployStep from "./DeployStep.svelte";
  import PreferencesStep from "./PreferencesStep.svelte";
  import ReviewStep from "./ReviewStep.svelte";
  import QuickSetup from "./QuickSetup.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    diagnosticsOf,
    issuesOf,
    needsConfirm,
    type Confirmable,
    type CliTokenDetection,
    type Connection,
    type Identity,
    type ExpectSuggestions,
    type MarkerSuggestions,
    type ProjectListing,
    type ProjectSummary,
    type Provider,
    type QuickWatch,
    type RecentListing,
    type StepIssue,
    type WizardAnswers,
    type WizardApi,
    type WizardPreview,
  } from "./api";
  import {
    STEPS,
    answersOf,
    baseUrlOf,
    followOwnToken,
    adoptOwnTokenStatus,
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
    type SignInStart,
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
    /** Sign-in commands. Absent: "Sign in with ..." is not offered. */
    oauth?: OAuthApi;
    /** The clock the one-click list's "active" labels read. */
    now?: () => number;
  }

  let { api, initial = null, onFinish, onSkip, oauth, now = Date.now }: Props = $props();

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
      draft.tokenMode === "oauth" ? [draft.oauthLogin, draft.oauthAccount] : null,
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
    // The sign-in is found by the account name it was stored under.
    if (draft.tokenMode === "oauth") conn.account = (draft.oauthAccount ?? draft.account).trim();
    if (confirm) conn.confirm = confirm;
    return conn;
  }

  // --- step 1: sign in with GitHub / GitLab ---------------------------------
  let availability = $state<OAuthAvailability | null>(null);
  let availabilitySeq = 0;
  /**
   * The instance and the client id ("" = the built-in application) that the
   * sign-in on this page, or the one a re-run found stored, was made with.
   */
  let signedInBase: string | null = untrack(() => (draft.oauthStored ? baseUrlOf(draft) : null));
  let signedInClientId: string | null = untrack(() => (draft.oauthStored ? draft.oauthClientId.trim() : null));

  /**
   * ⛔ A sign-in, fresh or stored, belongs to the instance and the application
   * it was made with. Once the form names another, it is forgotten and Finish
   * asks for a new one: the file would otherwise name an instance or an
   * application its stored tokens never came from, and the first poll would
   * say "sign in again". Returns whether it forgot one.
   *
   * A renamed ACCOUNT is not forgotten: the sign-in stays stored under
   * `draft.oauthAccount`, and Finish moves it to the saved name.
   */
  function forgetSignInIfMoved(): boolean {
    if (signedInBase === null) return false;
    if (signedInBase === baseUrlOf(draft) && signedInClientId === draft.oauthClientId.trim()) return false;
    draft.oauthLogin = null;
    draft.oauthStored = false;
    signedInBase = null;
    signedInClientId = null;
    return true;
  }

  async function refreshAvailability() {
    if (!oauth) return;
    const seq = ++availabilitySeq;
    const provider = draft.provider;
    try {
      const answer = await oauth.availability(provider, baseUrlOf(draft), draft.oauthClientId.trim() || null);
      if (seq !== availabilitySeq) return;
      availability = answer;
      preselect();
    } catch {
      if (seq === availabilitySeq) {
        availability = null;
        preselect();
      }
    }
  }

  // --- step 1: which token source a fresh start preselects -----------------
  /** The user picked a token source themselves: nothing here changes it after that. */
  let modeChosen = false;

  /**
   * Whether the token source is still the wizard's to choose: a fresh start,
   * one of the three it preselects, and nothing the user did since (a pick, a
   * pasted token, a typed client id, a finished sign-in).
   */
  function mayPreselect(): boolean {
    if (!freshStart || modeChosen) return false;
    if (draft.secret || draft.oauthClientId.trim() || draft.oauthLogin) return false;
    return draft.tokenMode === "paste" || draft.tokenMode === "oauth" || draft.tokenMode === "cli";
  }

  /**
   * Preselect the token source from what is known so far: "Sign in" where this
   * host has a built-in application, else the provider CLI's token when
   * detection found one, else a pasted token.
   *
   * ⛔ Run after EITHER answer arrives, availability or CLI detection, and
   * decided from both, never from the one that just landed. Each used to
   * switch the mode on its own when it found the form still on "paste", so a
   * found gh token beat "Sign in" whenever detection answered first, and the
   * same fresh start ended on a different radio from one launch to the next.
   */
  function preselect() {
    if (!mayPreselect()) return;
    if (availability?.builtin) {
      draft.tokenMode = "oauth";
    } else if (cli?.status === "found" && cliBase === baseUrlOf(draft)) {
      draft.cliSource = cli.source;
      draft.tokenMode = "cli";
    } else {
      draft.tokenMode = "paste";
    }
  }

  function modePicked() {
    modeChosen = true;
  }

  function signedIn(signed: SignedIn, started: SignInStart) {
    draft.oauthLogin = signed.login ?? "(unknown)";
    // ⛔ The name it was stored under, which is not always the one the form
    // shows by now (a rename while the code was pending): Finish moves it from
    // here to the saved name.
    draft.oauthAccount = started.account;
    draft.oauthStored = false;
    modeChosen = true;
    signedInBase = started.base;
    signedInClientId = started.clientId;
    delete errors.token;
    // It finished for what it STARTED with; if the form moved on while the
    // code was pending, it does not count.
    if (forgetSignInIfMoved()) {
      errors.token = "The instance or client id changed while you were signing in. Sign in again.";
      return;
    }
    void loadRecent();
  }

  function clientIdChanged() {
    forgetSignInIfMoved();
    void refreshAvailability();
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
  /** The instance `cli` answers for: a token is only ever used on the instance it was found for. */
  let cliBase: string | null = null;
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
    cliBase = base;
    if (result.status === "found") {
      const adopt = draft.tokenMode === "cli" || mayPreselect();
      if (adopt || sameSource(draft.cliSource, detectedSource)) draft.cliSource = result.source;
      detectedSource = result.source;
    } else {
      if (draft.cliSource !== null && sameSource(draft.cliSource, detectedSource)) draft.cliSource = null;
      detectedSource = null;
      // ⛔ Also when an instance change cleared the source before this answer
      // came, and whatever `modeChosen` says: the CLI option is hidden with
      // no source, and a hidden radio must never stay selected.
      if (draft.tokenMode === "cli" && draft.cliSource === null) leaveCli();
    }
    preselect();
  }

  /** The CLI's token is gone: sign in where this instance offers it, else paste one. */
  function leaveCli() {
    draft.tokenMode = oauth && availability?.available ? "oauth" : "paste";
  }

  function instanceChanged() {
    if (!draft.accountEdited) draft.account = suggestAccountName(baseUrlOf(draft), draft.provider);
    // ⛔ The last instance's CLI token answers nothing about this one. Kept until
    // the new detection answered, it was preselected (and sent by Test
    // connection) against the new host whenever availability answered first.
    if (cliBase !== null && cliBase !== baseUrlOf(draft)) {
      if (sameSource(draft.cliSource, detectedSource)) draft.cliSource = null;
      cli = null;
      cliBase = null;
      detectedSource = null;
    }
    // A sign-in, fresh or stored, belongs to the instance it was made on, and
    // so does a stored token.
    forgetSignInIfMoved();
    followOwnToken(draft);
    void detect();
    void refreshAvailability();
  }

  function providerChanged(provider: Provider) {
    if (draft.provider === provider) return;
    const before = draft.tokenMode;
    switchProvider(draft, provider);
    // A pick the switch undid (a CLI token, a sign-in) is no pick any more.
    if (draft.tokenMode !== before) modeChosen = false;
    // The last provider's detection answers nothing about this one.
    cli = null;
    cliBase = null;
    detectedSource = null;
    listing = null;
    listingKey = null;
    suggestions = null;
    suggestionsKey = null;
    delete errors.project;
    signedInBase = null;
    signedInClientId = null;
    availability = null;
    void detect();
    void refreshAvailability();
  }

  /**
   * A re-run on a stored own token asks the shell which server that token is
   * bound to, and keeps it only for that server (`adoptOwnTokenStatus`). Next
   * and Test connection wait for the answer; a failed read leaves the file's
   * word, which the core still checks.
   */
  let ownCheck: Promise<void> | null = null;

  onMount(() => {
    void detect();
    void refreshAvailability();
    const ask = api.ownTokenStatus;
    if (draft.ownStored && ask) {
      const account = draft.account.trim();
      ownCheck = ask(account)
        .then((status) => adoptOwnTokenStatus(draft, status))
        .catch(() => {});
    }
  });

  let identity = $state<Identity | null>(null);
  let testError = $state<string | null>(null);
  let testKey = $state<string | null>(null);
  let testing = $state(false);
  const shownIdentity = $derived(testKey === connKey ? identity : null);
  const shownTestError = $derived(testKey === connKey ? testError : null);

  /**
   * ⛔ What a changed instance drops, applied again before anything is judged.
   * `instanceChanged` runs on the URL field's blur, and Enter in that field
   * submits the form without one: Next then accepted "leave empty to keep the
   * stored token" for a host that token is not bound to. Every path that
   * validates the account step (Next, Enter, Test connection) calls this first.
   */
  async function settleInstance() {
    if (ownCheck) await ownCheck;
    followOwnToken(draft);
    forgetSignInIfMoved();
  }

  async function testConnection() {
    await settleInstance();
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
      void loadRecent();
    } catch (error) {
      identity = null;
      testError = messageOf(error);
      testKey = key;
    } finally {
      testing = false;
    }
  }

  // --- step 1, after a sign-in: one-click setup ------------------------------
  let recent = $state<RecentListing | null>(null);
  /** The connection `recent` was listed for: a list belongs to the account that made it. */
  let recentKey = $state<string | null>(null);
  let recentLoading = $state(false);
  let recentError = $state<string | null>(null);
  let recentNow = $state(0);
  /** Project id → ticked, seeded from the core's `ticked`. */
  let ticks = $state<Record<number, boolean>>({});
  let quickBusy = $state(false);
  let recentSeq = 0;
  /** The connection the last request was for; its loading line and error belong to it alone. */
  let recentAsked = $state<string | null>(null);
  const shownRecent = $derived(recentKey === connKey ? recent : null);
  const shownRecentLoading = $derived(recentLoading && recentAsked === connKey);
  const shownRecentError = $derived(recentAsked === connKey ? recentError : null);

  /**
   * The one-click list, once per connection: a sign-in or a passed Test
   * connection asks for it, and asking again for the same account sends
   * nothing (one list call per account).
   */
  async function loadRecent() {
    if (!api.recentProjects) return;
    const key = connKey;
    if (recentKey === key && recent !== null) return;
    const seq = ++recentSeq;
    recentAsked = key;
    recentLoading = true;
    recentError = null;
    try {
      const result = await live((c) => api.recentProjects!(c));
      if (seq !== recentSeq || result === null) return;
      recent = result;
      recentKey = key;
      recentNow = now();
      ticks = Object.fromEntries(
        result.mode === "projects" ? result.projects.map((p) => [p.id, p.ticked]) : [],
      );
      // It lands under the sign-in or Test connection result, often below the
      // fold of the account step: bring it into view.
      await tick();
      body?.querySelector<HTMLElement>('[data-slot="quick-setup"]')?.scrollIntoView?.({ block: "nearest" });
    } catch (error) {
      if (seq !== recentSeq) return;
      recent = null;
      recentKey = null;
      recentError = messageOf(error);
    } finally {
      if (seq === recentSeq) recentLoading = false;
    }
  }

  /**
   * "Watch N projects": the ticked rows become watches. Each takes its best
   * deploy marker from the deploy step's own ranking (`suggestDeployMarkers`,
   * the first suggestion), or none when nothing ranks or the read fails; then
   * the wizard goes to Preferences, and Finish writes every watch at once.
   */
  async function quickSetup() {
    const listing = shownRecent;
    if (quickBusy || !listing || listing.mode !== "projects") return;
    await settleInstance();
    const stepErrors = validateStep("account", draft);
    errors = stepErrors;
    if (Object.keys(stepErrors).length > 0) return void focusInvalid();
    if (!(await ensureCommandAck())) return;
    const rows = listing.projects.filter((p) => p.default_branch && ticks[p.id]);
    if (rows.length === 0) return;
    quickBusy = true;
    try {
      const watches: QuickWatch[] = [];
      for (const row of rows) {
        const project = projectRefFor(draft.provider, row);
        const ref = row.default_branch as string;
        let markers: string[] = [];
        try {
          const found = await live((c) => api.suggestDeployMarkers(c, project, ref));
          if (found === null) return;
          const best = found.suggestions[0];
          if (best) markers = [best.name];
        } catch {
          // No marker is a real answer; the watch reports "succeeded" until one is added.
        }
        watches.push({ project, path: row.path, ref_name: ref, deploy_markers: markers, listed_by: listing.listed_by });
      }
      draft.quick = watches;
      delete errors.watches;
      await goTo(STEPS.findIndex((s) => s.id === "preferences"));
    } finally {
      quickBusy = false;
    }
  }

  /** The one-click steps skipped: project, watch and deploy. */
  const QUICK_SKIPS = new Set(["project", "watch", "deploy"]);

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

  // --- step 3, GitHub: expect for a commit group ---------------------------
  let expectSuggestions = $state<ExpectSuggestions | null>(null);
  let expectKey: string | null = null;
  let expectAsking: string | null = null;
  let expectLoading = $state(false);
  let expectError = $state<string | null>(null);

  /**
   * The workflow files the newest pushes ran, once per branch and event set.
   * The first answer pre-ticks the ones every push ran, unless something is
   * ticked already (a re-run's own `expect`).
   */
  async function loadExpect() {
    const project = draft.project;
    const suggest = api.suggestExpectedWorkflows;
    if (!project || draft.provider !== "github" || draft.group !== "commit" || !suggest) return;
    const refName = draft.refName.trim();
    const sources = [...draft.sources];
    const key = JSON.stringify([connKey, project.ref, refName, sources]);
    // Answered already, or being asked right now.
    if (key === expectKey || key === expectAsking) return;
    expectAsking = key;
    expectLoading = true;
    expectError = null;
    try {
      const result = await live((c) => suggest(c, project.ref, refName, sources));
      if (result === null) return;
      expectSuggestions = result;
      expectKey = key;
      if (draft.expect.length === 0) {
        draft.expect = result.workflows.filter((w) => w.seen_in === result.commits).map((w) => w.file);
      }
    } catch (error) {
      expectSuggestions = null;
      expectError = messageOf(error);
    } finally {
      expectLoading = false;
      if (expectAsking === key) expectAsking = null;
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
        // ⛔ Only when something WAS pasted. A re-run that kept the stored
        // token (the field left empty) pasted nothing, and said it had.
        return draft.secret.trim()
          ? `The token you pasted goes into bridgewatch's own keychain entry for "${draft.account.trim()}", never into this file.`
          : `bridgewatch keeps the token already in its own keychain entry for "${draft.account.trim()}"; it never goes into this file.`;
      case "cli":
        return `The token stays in ${cliName(draft.provider)}'s keychain entry; this file only says where it is.`;
      case "env":
        return `The token is read from $${draft.envVar.trim()} when bridgewatch starts.`;
      case "command":
        return `The token is read by running: ${formatCommand(parseCommand(draft.commandText))}`;
      case "oauth":
        return `The sign-in is kept in the system keychain for "${draft.account.trim()}" and renews itself; this file only says to sign in.`;
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
    // Only a sign-in made under another name is reported, so every other save
    // carries exactly the options it always did.
    const moved =
      draft.tokenMode === "oauth" && draft.oauthAccount && draft.oauthAccount !== answers.account
        ? { oauthAccount: draft.oauthAccount }
        : {};
    try {
      let result = await api.save(answers, { secret, ...moved });
      if (result.confirm) {
        const command = draft.tokenMode === "command" ? parseCommand(draft.commandText) : null;
        if (!(await askConfirm("Save with these changes?", result.confirm.changes, command, "Save"))) return;
        result = await api.save(answers, { secret, confirm: result.confirm.id, ...moved });
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
      // Next on the account step is the step-by-step path: a one-click
      // choice made earlier is dropped.
      if (id === "account") {
        draft.quick = null;
        await settleInstance();
      }
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
    if (index === 0) return;
    let target = index - 1;
    while (draft.quick && target > 0 && QUICK_SKIPS.has(STEPS[target].id)) target--;
    void goTo(target);
  }

  /** The stepper: a one-click setup stays one only while no skipped step is opened. */
  function select(stepIndex: number) {
    if (draft.quick && QUICK_SKIPS.has(STEPS[stepIndex].id)) draft.quick = null;
    void goTo(stepIndex);
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (isLast) void finish();
    else void next();
  }

  const busy = $derived(advancing || saving || skipping || quickBusy);
</script>

<!-- One column, at most 560px, with the window's own padding (no second p-4).
     Body text is 13/18 and the step title 15/20; the shared primitives are
     13/18 themselves, so every step reads at one size. Hints stay 12px
     (text-xs). -->
<section
  class="mx-auto flex h-full w-full max-w-[560px] flex-col gap-4 text-[13px]/[18px]"
  aria-labelledby="wizard-heading"
  data-slot="wizard"
  data-step={step.id}
>
  <Stepper steps={STEPS} current={index} onSelect={select} label="Setup steps" />

  <form class="flex min-h-0 flex-1 flex-col gap-4" onsubmit={submit} novalidate>
    <h2 id="wizard-heading" class="text-[15px]/[20px] font-semibold outline-none" tabindex="-1" bind:this={heading}>
      {stepTitle(step, draft.provider)}
    </h2>

    <!-- -mx-1 px-1: room for the focus rings, which the scroll box would clip. -->
    <div class="-mx-1 min-h-0 flex-1 overflow-y-auto px-1" bind:this={body}>
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
          {oauth}
          {availability}
          onSignedIn={signedIn}
          onClientIdChange={clientIdChanged}
          onModePick={modePicked}
        />
        <QuickSetup
          provider={draft.provider}
          listing={shownRecent}
          loading={shownRecentLoading}
          loadError={shownRecentError}
          {ticks}
          onToggle={(id, on) => (ticks = { ...ticks, [id]: on })}
          onSetUp={() => void quickSetup()}
          busy={quickBusy}
          error={errors.watches}
          now={recentNow}
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
          appInstall={oauth && draft.provider === "github" && draft.tokenMode === "oauth"
            ? { url: availability?.install_url ?? null, open: () => void oauth.openInstall(draft.provider, baseUrlOf(draft)) }
            : null}
        />
      {:else if step.id === "watch"}
        <WatchStep
          bind:draft
          {errors}
          {expectSuggestions}
          {expectLoading}
          {expectError}
          onexpect={() => void loadExpect()}
        />
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
          quick={!!draft.quick}
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
