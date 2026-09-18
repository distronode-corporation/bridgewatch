<script lang="ts">
  import { onMount } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import AccountsTab from "./components/settings/AccountsTab.svelte";
  import Ask from "./components/settings/Ask.svelte";
  import SimpleTab from "./components/settings/SimpleTab.svelte";
  import TextTab from "./components/settings/TextTab.svelte";
  import WatchesTab from "./components/settings/WatchesTab.svelte";
  import type { Tab } from "./lib/settings/registry";
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
  import { isFirstRun } from "./lib/format";
  import type { Edit, Status, Validation } from "./lib/types";

  type TabId = Tab | "text";

  const TABS: { id: TabId; label: string }[] = [
    { id: "accounts", label: "Accounts" },
    { id: "watches", label: "Watches" },
    { id: "icon", label: "Icon" },
    { id: "verdict", label: "Verdict" },
    { id: "ui", label: "UI" },
    { id: "log", label: "Log" },
    { id: "text", label: "Edit as text" },
  ];

  let tab = $state<TabId>("accounts");
  let config = $state<unknown>(null);
  let jobOrder = $state<string[][]>([]);
  let text = $state("");
  let status = $state<Status | null>(null);
  let validation = $state<Validation | null>(null);
  let notice = $state("");

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
   */
  async function load(keepValidation = false) {
    if (!inTauri()) return;
    const payload = await getConfigJson();
    config = payload?.config ?? null;
    jobOrder = payload?.job_order ?? [];
    text = await readConfigText();
    status = await getStatus();
    if (keepValidation) return;
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
    void takeSettingsTab().then((t) => {
      if (t) tab = t as TabId;
    });
    const unlisten = onOpenSettings((t) => (tab = t as TabId));
    // H15: any writer (this window, the tray, $EDITOR, the CLI) moves the
    // file; the shell says so, and every copy this window holds follows. The
    // text tab keeps a DIRTY buffer regardless, and the shell's
    // compare-and-swap refuses a save based on the old text.
    const unlistenChanged = onConfigChanged(() => void load(validation?.ok === true));
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

<div class="settings bg-background text-foreground flex h-screen flex-col px-3.5 pt-2.5 pb-3.5 text-[13px]">
  <nav class="border-border flex items-center gap-1 border-b pb-2">
    <div class="bg-muted flex items-center gap-0.5 rounded-2xl p-0.5" role="tablist">
      {#each TABS as t (t.id)}
        <button
          role="tab"
          aria-selected={tab === t.id}
          class={[
            "tab rounded-xl px-2.5 py-1 text-xs font-medium outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
            tab === t.id
              ? "active bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          ]}
          onclick={() => (tab = t.id)}>{t.label}</button
        >
      {/each}
    </div>
    <span class="flex-1"></span>
    <Button variant="ghost" size="sm" class="wizard" title="Walk through the basics again" onclick={() => void openWizard()}
      >Setup wizard…</Button
    >
    <Button variant="outline" size="sm" onclick={() => void openConfigFile()}>Open file</Button>
    <Button
      variant="outline"
      size="sm"
      onclick={async () => {
        await reloadConfig();
        await load();
        notice = "Reloaded from disk.";
      }}>Reload</Button
    >
  </nav>

  <p class="path text-muted-foreground my-1.5 font-mono text-xs">{status?.configPath ?? ""}</p>

  {#if isFirstRun(status)}
    <div class="first-run bg-tone-blue-bg border-border mb-2 flex items-center gap-2 rounded-xl border px-3 py-2 text-xs">
      <span class="flex-1">
        No configuration yet. The setup wizard writes one; or write it yourself on the "Edit as text" tab, where
        Save creates the file.
      </span>
      <Button size="sm" onclick={() => void openWizard()}>Open the setup wizard</Button>
    </div>
  {/if}

  {#if notice}
    <p class="notice text-muted-foreground mt-0 mb-2 text-xs">{notice}</p>
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

  <div class="pane min-h-0 flex-1 overflow-y-auto pr-1.5">
    {#if tab === "accounts"}
      <AccountsTab {config} onedit={edit} />
    {:else if tab === "watches"}
      <WatchesTab {config} {jobOrder} onedit={edit} onremove={remove} onmove={move} onadd={add} />
    {:else if tab === "text"}
      <TextTab
        {text}
        {validation}
        diskText={readConfigText}
        onvalidate={async (t) => (validation = await validateConfigText(t))}
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
    {:else}
      <SimpleTab {tab} {config} onedit={edit} />
    {/if}
  </div>

  {#if tab !== "text" && validation && !validation.ok}
    <ul class="diagnostics border-border mt-2 mb-0 max-h-30 list-none overflow-y-auto border-t p-0 pt-1.5 text-xs">
      {#each validation.diagnostics as d, index (index)}
        <li class={[d.severity, d.severity === "error" ? "text-tone-red" : "text-tone-amber"]}>
          <span class="where text-muted-foreground mr-1.5 font-mono">{d.path}{d.line ? ` (${d.line}:${d.col})` : ""}</span>
          {d.message}
        </li>
      {/each}
    </ul>
  {/if}
</div>
