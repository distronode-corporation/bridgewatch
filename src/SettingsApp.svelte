<script lang="ts">
  import { onMount, tick } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import AccountsTab from "./components/settings/AccountsTab.svelte";
  import Ask from "./components/settings/Ask.svelte";
  import SimpleTab from "./components/settings/SimpleTab.svelte";
  import { HINT, SECTION_TITLE } from "./components/settings/styles";
  import TextTab from "./components/settings/TextTab.svelte";
  import WatchesTab from "./components/settings/WatchesTab.svelte";
  import { PANE_GROUPS, PANES, paneFor, rememberPane, restorePane, type Pane } from "./lib/settings/panes";
  import {
    applyConfigEdits,
    getConfigJson,
    getStatus,
    moveWatch,
    inTauri,
    onConfigChanged,
    onOpenSettings,
    openConfigFile,
    openWizard,
    readConfigText,
    reloadConfig,
    saveConfigText,
    takeSettingsTab,
    validateConfigText,
  } from "./lib/ipc";
  import { isFirstRun, messageOf } from "./lib/format";
  import type { Edit, Status, Validation } from "./lib/types";

  /** This window's storage, or none where the webview refuses it. */
  function storage(): Storage | null {
    try {
      return window.localStorage;
    } catch {
      return null;
    }
  }

  /** The pane on screen: the last one used, until the shell asks for another. */
  let pane = $state<Pane>(restorePane(storage()));

  /**
   * Show a pane. `remember` is for the user's own choice only: the shell
   * opening the window on "Edit as text" for a broken file is not a pane the
   * user chose, and must not become the one every later window opens on.
   */
  async function show(name: string | null | undefined, remember: boolean) {
    const request = paneFor(name);
    if (!request) return;
    pane = request.pane;
    if (remember) rememberPane(storage(), request.pane);
    if (request.focus === "text") {
      await tick();
      document.getElementById("edit-as-text")?.scrollIntoView?.({ block: "start" });
    }
  }

  /** The sidebar is one vertical tab list: arrows, Home and End move AND show, as a tab list does. */
  function sidebarKey(event: KeyboardEvent, index: number) {
    const last = PANES.length - 1;
    const to =
      event.key === "ArrowDown"
        ? (index + 1) % PANES.length
        : event.key === "ArrowUp"
          ? (index + last) % PANES.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (to === null) return;
    event.preventDefault();
    void show(PANES[to].id, true);
    document.getElementById(`tab-${PANES[to].id}`)?.focus();
  }

  const paneLabel = $derived(PANES.find((p) => p.id === pane)?.label ?? "");

  let config = $state<unknown>(null);
  let jobOrder = $state<string[][]>([]);
  let text = $state("");
  let status = $state<Status | null>(null);
  let validation = $state<Validation | null>(null);
  /**
   * Who `validation` answers: a form edit or move, or the file (on load, or
   * the text editor's own Validate and Save).
   *
   * ⛔ The Advanced pane holds form fields (Log) AND the editor, which lists
   * the file's diagnostics under its 340px box. A refused Log edit put its
   * reason there, out of sight below the editor, while the notice said "see
   * the errors below" (Seer, #65). A FORM answer is listed under the pane on
   * every pane, and the editor lists only the file's.
   */
  let validationFrom = $state<"form" | "file">("file");
  let notice = $state("");
  /** The text this window's last successful text-tab save wrote, if any. */
  let savedText: string | null = null;

  /**
   * Re-read everything the pane renders.
   *
   * ⛔ The diagnostics come with it. A file that does not load opens this window
   * on the text tab with the box full of the broken TOML and NOTHING saying
   * what is wrong with it — the user had to guess that "Validate" was the way
   * to find out, on a tab they did not choose to open. `Status` has carried
   * `configOk` and the diagnostics all along; nothing rendered them.
   *
   * ⚠ `keepValidation` is for the one case where the fresher answer is the one
   * already on screen: a REFUSED save explains why it was refused, and the file
   * on disk is unchanged, so re-seeding from it would replace the reason with
   * the state of a file the user did not just try to write.
   *
   * ⛔ `"if-ours"` is the config-changed case: keep a "valid" only while the
   * file still holds exactly what this window saved. A save announces itself
   * with that event too, so the verdict has to survive it; but a file broken
   * in $EDITOR afterwards announces itself the same way, and kept the stale
   * "valid" with no diagnostics.
   */
  async function load(keepValidation: boolean | "if-ours" = false) {
    if (!inTauri()) return;
    const payload = await getConfigJson();
    config = payload?.config ?? null;
    jobOrder = payload?.job_order ?? [];
    text = await readConfigText();
    status = await getStatus();
    const keep =
      keepValidation === "if-ours" ? validation?.ok === true && text === savedText : keepValidation;
    if (keep) return;
    validationFrom = "file";
    // A first launch has no file YET: that is the wizard's moment, not an
    // error to list in red under every tab.
    validation =
      status && !status.configOk && !isFirstRun(status)
        ? { ok: false, diagnostics: status.diagnostics }
        : null;
  }

  onMount(() => {
    if (!inTauri()) return;
    void load();
    void takeSettingsTab().then((t) => void show(t, false));
    const unlisten = onOpenSettings((t) => void show(t, false));
    // H15: any writer (this window, the tray, $EDITOR, the CLI) moves the
    // file; the shell says so, and every copy this window holds follows. The
    // text tab keeps a DIRTY buffer regardless, and the shell's
    // compare-and-swap refuses a save based on the old text.
    const unlistenChanged = onConfigChanged(() => void load("if-ours"));
    return () => {
      void unlisten.then((f) => f());
      void unlistenChanged.then((f) => f());
    };
  });

  /**
   * The shell's question about a sensitive write, while it is on screen.
   *
   * ⛔ The shell, not this window, decides what needs confirming and words
   * it: a new `command` token source (runs a program) or an account moved to
   * another host (sends its token there). It answers the first request with
   * `confirm` and writes NOTHING; the same request sent back with that id is
   * what writes. Every route to the file goes through here, so "Edit as text"
   * cannot skip the question the token form asks.
   */
  let pendingConfirm = $state<{ changes: string[]; answer: (yes: boolean) => void } | null>(
    null,
  );

  async function confirmed(
    first: Validation,
    retry: (id: string) => Promise<Validation>,
  ): Promise<Validation> {
    let result = first;
    // A second question can only come back if the text changed in between;
    // the loop is bounded so a misbehaving shell cannot trap the window.
    for (let round = 0; round < 3 && result.confirm; round++) {
      const request = result.confirm;
      const yes = await new Promise<boolean>((answer) => {
        pendingConfirm = { changes: request.changes, answer };
      });
      pendingConfirm = null;
      if (!yes) {
        return { ok: false, diagnostics: [], cancelled: true } as Validation & {
          cancelled: boolean;
        };
      }
      result = await retry(request.id);
    }
    return result;
  }

  /**
   * Apply a batch of edits.
   *
   * The form writes EDITS, never a whole document: the core applies them
   * through `toml_edit` and refuses to write at all if the result would not
   * load, so a bad value costs a red message rather than a broken config.
   */
  async function edit(edits: Edit[]): Promise<void> {
    const result = await confirmed(await applyConfigEdits(edits), (id) =>
      applyConfigEdits(edits, id),
    );
    if (isCancelled(result)) {
      notice = "Not saved: the change was not confirmed.";
      return;
    }
    validation = result;
    validationFrom = "form";
    notice = result.ok ? "Saved." : "Not saved — see the errors below.";
    await load(!result.ok);
  }

  /**
   * The question on screen, or none.
   *
   * ⛔ Not `window.confirm`/`window.prompt`. Neither ever appears in this
   * window: wry implements no `WKUIDelegate` panel methods, so on macOS the
   * confirm returned `false` and the prompt returned `null` the instant they
   * were called — "Remove watch" and "Add watch" were dead buttons that looked
   * like they had been cancelled.
   */
  let asking = $state<"remove" | "add" | null>(null);
  let removing = $state("");

  function isCancelled(v: Validation): boolean {
    return (v as Validation & { cancelled?: boolean }).cancelled === true;
  }

  function remove(id: string) {
    removing = id;
    asking = "remove";
  }

  async function move(id: string, delta: number) {
    // H11: the shell renumbers the watches and writes the file; the window then
    // re-reads it, so the order on screen is the file's order, never a guess.
    const result = await moveWatch(id, delta);
    validation = result;
    validationFrom = "form";
    notice = result.ok ? "Moved." : "Not moved — see the errors below.";
    await load(!result.ok);
  }

  function add() {
    const accounts = accountNames();
    if (accounts.length === 0) {
      notice = "Add an account first: a watch has to name one.";
      return;
    }
    asking = "add";
  }

  function accountNames(): string[] {
    return Object.keys(
      ((config as Record<string, unknown> | null)?.accounts ?? {}) as Record<string, unknown>,
    );
  }

  /** Ask the shell to open something; say why when it could not. */
  function openOrSay(what: string, request: () => Promise<void>) {
    request().catch((error) => (notice = `Could not open ${what}: ${messageOf(error)}`));
  }

  async function addWatch(answers: Record<string, string>) {
    asking = null;
    const accounts = accountNames();
    if (accounts.length === 0) return;
    const project = answers.project;
    // `AddWatch` takes a whole `Watch`, so every key without a default has to
    // be here. The rest are left out deliberately: a file full of explicit
    // defaults is harder to read than one that relies on them.
    await edit([
      {
        op: "add_watch",
        watch: {
          id: answers.id,
          account: accounts[0],
          project: /^\d+$/.test(project) ? Number(project) : project,
          ref: "main",
          role: "secondary",
        },
      },
    ]);
  }
</script>

<!-- A 168px sidebar of five panes, the pane at most 600px wide, and a 32px
     footer bar with the file. The title and anything the window is asking
     stay put above the scrolling pane, so a question asked from far down a
     long pane is never off screen. -->
<div class="settings bg-background text-foreground text-ui-body flex h-screen">
  <nav class="sidebar border-border bg-muted/30 flex w-[168px] shrink-0 flex-col gap-2 border-r p-2" aria-label="Settings">
    <div class="flex flex-col gap-0.5" role="tablist" aria-orientation="vertical" aria-label="Settings panes">
      {#each PANES as p, index (p.id)}
        <button
          role="tab"
          id={`tab-${p.id}`}
          aria-controls={`pane-${p.id}`}
          aria-selected={pane === p.id}
          tabindex={pane === p.id ? 0 : -1}
          class={[
            "tab text-ui-body rounded-lg px-2.5 py-1.5 text-left font-medium whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
            pane === p.id
              ? "active bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:bg-muted hover:text-foreground",
          ]}
          onclick={() => void show(p.id, true)}
          onkeydown={(event) => sidebarKey(event, index)}>{p.label}</button
        >
      {/each}
    </div>
    <span class="flex-1"></span>
    <Button
      variant="ghost"
      size="sm"
      class="wizard justify-start"
      title="Walk through the basics again"
      onclick={() => openOrSay("the setup wizard", openWizard)}>Setup wizard…</Button
    >
  </nav>

  <div class="main flex min-w-0 flex-1 flex-col">
    <div class="flex max-w-[600px] flex-col gap-2 px-6 pt-5">
      <header class="flex min-h-7 items-center gap-3">
        <h2 class="text-ui-title m-0 font-semibold">{paneLabel}</h2>
        <span class="flex-1"></span>
        {#if pane === "watches"}
          <Button variant="outline" size="sm" class="add" onclick={add}>Add watch</Button>
        {/if}
      </header>

      {#if isFirstRun(status)}
        <div class="first-run bg-tone-blue-bg border-border flex flex-wrap items-center gap-2 rounded-xl border px-3 py-2 text-ui-meta">
          <span class="min-w-0 flex-1 basis-56">
            No configuration yet. The setup wizard writes one; or write it yourself under Advanced, in "Edit as
            text", where Save creates the file.
          </span>
          <Button size="sm" onclick={() => openOrSay("the setup wizard", openWizard)}>Open the setup wizard</Button>
        </div>
      {/if}

      {#if notice}
        <p class="notice text-muted-foreground text-ui-meta m-0">{notice}</p>
      {/if}

      {#if pendingConfirm}
        <Ask
          title="Confirm this change"
          message={pendingConfirm.changes.join("\n\n")}
          confirmLabel="Write it"
          danger
          onconfirm={() => pendingConfirm?.answer(true)}
          oncancel={() => pendingConfirm?.answer(false)}
        />
      {:else if asking === "remove"}
        <Ask
          title={`Remove the watch "${removing}"?`}
          message="The rest of the file is left exactly as it is."
          confirmLabel="Remove"
          danger
          onconfirm={() => {
            asking = null;
            void edit([{ op: "remove_watch", id: removing }]);
          }}
          oncancel={() => (asking = null)}
        />
      {:else if asking === "add"}
        <!-- ⛔ The project is asked for rather than defaulted. `Watch` has no
             default for it, and seeding a placeholder would write a watch that
             looks configured and 404s on every tick. -->
        <Ask
          title="New watch"
          message={`It will use the account "${accountNames()[0] ?? ""}", watch "main", and start as a secondary.`}
          fields={[
            { name: "id", label: "Watch id", placeholder: "main-push" },
            { name: "project", label: "Project", placeholder: "numeric id, or group/path" },
          ]}
          confirmLabel="Add watch"
          onconfirm={addWatch}
          oncancel={() => (asking = null)}
        />
      {/if}
    </div>

    <div
      class="pane min-h-0 flex-1 overflow-y-auto"
      role="tabpanel"
      id={`pane-${pane}`}
      aria-labelledby={`tab-${pane}`}
    >
      <div class="flex max-w-[600px] flex-col gap-6 px-6 pt-3 pb-6">
        {#if pane === "accounts"}
          <div><AccountsTab {config} onedit={edit} /></div>
        {:else if pane === "watches"}
          <div><WatchesTab {config} {jobOrder} onedit={edit} onremove={remove} onmove={move} /></div>
        {:else if pane === "general" || pane === "verdict"}
          {#each PANE_GROUPS[pane] as group (group.tab)}
            <SimpleTab tab={group.tab} title={group.title} {config} onedit={edit} />
          {/each}
        {:else}
          <SimpleTab tab="log" title="Log" {config} onedit={edit} />
          <section id="edit-as-text" class="settings-group flex flex-col gap-2" aria-labelledby="edit-as-text-title">
            <h3 id="edit-as-text-title" class={["px-1", SECTION_TITLE]}>Edit as text</h3>
            <p class={["m-0 px-1", HINT]}>
              The whole file. Save checks it first and writes nothing that would not load; comments and order are kept.
            </p>
            <TextTab
              {text}
              validation={validationFrom === "file" ? validation : null}
              diskText={readConfigText}
              onvalidate={async (t) => {
                validation = await validateConfigText(t);
                validationFrom = "file";
              }}
              onsave={async (t, base) => {
                const result = await confirmed(await saveConfigText(t, base), (id) =>
                  saveConfigText(t, base, id),
                );
                if (isCancelled(result)) {
                  notice = "Not saved: the change was not confirmed.";
                  return result;
                }
                if (result.conflict) {
                  notice = "Not saved: the file changed on disk since this tab read it.";
                  return result;
                }
                validation = result;
                validationFrom = "file";
                if (result.ok) savedText = t;
                notice = result.ok ? "Saved." : "Not saved — the file was left alone.";
                // ⚠ Kept either way, unlike a form save: this tab RENDERS the
                // validation, so re-seeding it from the file would replace both
                // answers the user just asked for — the reason a save was refused,
                // and the "valid" on one that landed.
                await load(true);
                return result;
              }}
              onrevert={() => void load()}
            />
          </section>
        {/if}
      </div>
    </div>

    <!-- The file's diagnostics on Advanced are the editor's to list, under its box. -->
    {#if (pane !== "advanced" || validationFrom === "form") && validation && !validation.ok}
      <ul class="diagnostics border-border m-0 max-h-30 list-none overflow-y-auto border-t px-6 py-1.5 text-ui-meta">
        {#each validation.diagnostics as d, index (index)}
          <li class={[d.severity, d.severity === "error" ? "text-tone-red" : "text-tone-amber"]}>
            <span class="where text-muted-foreground mr-1.5 font-mono">{d.path}{d.line ? ` (${d.line}:${d.col})` : ""}</span>
            {d.message}
          </li>
        {/each}
      </ul>
    {/if}

    <footer class="border-border flex h-8 shrink-0 items-center gap-1 border-t px-3">
      <span class="path text-muted-foreground text-ui-meta min-w-0 flex-1 truncate font-mono" title={status?.configPath ?? ""}
        >{status?.configPath ?? ""}</span
      >
      <Button variant="ghost" size="xs" onclick={() => openOrSay("the file", openConfigFile)}>Open file</Button>
      <Button
        variant="ghost"
        size="xs"
        onclick={async () => {
          await reloadConfig();
          await load();
          notice = "Reloaded from disk.";
        }}>Reload</Button
      >
    </footer>
  </div>
</div>
