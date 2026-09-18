<script lang="ts">
  import { untrack } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { setOwnToken, clearOwnToken } from "../../lib/ipc";
  import type { Edit } from "../../lib/types";
  import { concretePath, entry as registryEntry } from "../../lib/settings/registry";
  import { CHECK, FIELD_LABEL, FIELD_ROW, HINT, INPUT } from "./styles";

  interface Props {
    account: string;
    /** The `token` value as the core serialised it. */
    value: unknown;
    onedit: (edits: Edit[]) => void | Promise<void>;
  }

  let { account, value, onedit }: Props = $props();

  const meta = registryEntry("accounts.*.token");
  // ⛔ `accounts.${account}.token` was a template string, and an account called
  // `gitlab.com` — which is what the shipped example calls its account — has a
  // dot in it. The core's `split_path` then walked INTO the name and wrote
  // `[accounts.gitlab.com.token]`, a nested table nobody asked for, so the
  // token source of that account could not be changed from this window at all.
  const path = $derived(concretePath("accounts.*.token", account));

  type Kind = "keyring" | "env" | "command" | "own";

  const current = $derived.by<{ kind: Kind; service: string; user: string; env: string; command: string }>(
    () => {
      const v = (value ?? {}) as Record<string, unknown>;
      if ("keyring" in v) {
        const k = v.keyring as { service?: string; user?: string };
        return { kind: "keyring", service: k.service ?? "", user: k.user ?? "", env: "", command: "" };
      }
      if ("env" in v) {
        return { kind: "env", service: "", user: "", env: String(v.env ?? ""), command: "" };
      }
      if ("command" in v) {
        const argv = Array.isArray(v.command) ? (v.command as string[]) : [];
        return { kind: "command", service: "", user: "", env: "", command: argv.join(" ") };
      }
      return { kind: "own", service: "", user: "", env: "", command: "" };
    },
  );

  let kind = $state<Kind>("own");
  let service = $state("");
  let user = $state("");
  let envVar = $state("");
  let command = $state("");
  let pasted = $state("");
  let message = $state("");

  /**
   * Seed the controls from the file, and then LEAVE THEM ALONE.
   *
   * ⛔ This used to assign on every run, and it runs whenever the document is
   * re-read — which is after every save of any other field, on any tab, plus
   * every reload. So typing a service name here and then changing a poll
   * interval two tabs away silently wiped what had been typed, with nothing on
   * screen to say it had happened. Re-seeding only when the FILE's token
   * actually changed keeps another window's edit flowing in while leaving an
   * unapplied one in place.
   *
   * ⚠ `untrack`, or reading `seeded` here would make this effect its own
   * dependency and it would never settle.
   */
  let seeded = $state("");

  $effect(() => {
    const fromFile = JSON.stringify(current);
    if (fromFile === untrack(() => seeded)) return;
    seeded = fromFile;
    kind = current.kind;
    service = current.service;
    user = current.user;
    envVar = current.env;
    command = current.command;
  });

  /** The four variant keys of the `token` enum, of which exactly one may exist. */
  const VARIANTS = ["keyring", "env", "command", "own"] as const;

  /**
   * Write one variant of the token enum.
   *
   * ⛔ The OTHER THREE are unset, not the whole table. The four forms are
   * mutually exclusive variants of one enum, so leaving two keys present makes
   * the file refuse to load with an unhelpful serde message — but removing
   * `token` outright and writing it again moved it to the END of its account
   * table, under `timeout_secs` and the backoff block, every single time
   * anybody touched the source. `toml_edit` appends a key it has never seen;
   * a key it can find keeps its place.
   */
  function write(variant: (typeof VARIANTS)[number], edits: Edit[]) {
    const stale: Edit[] = VARIANTS.filter((v) => v !== variant).map((v) => ({
      op: "unset",
      path: `${path}.${v}`,
    }));
    onedit([...stale, ...edits]);
  }

  function chooseKeyring() {
    write("keyring", [
      { op: "set", path: `${path}.keyring.service`, value: { string: service } },
      { op: "set", path: `${path}.keyring.user`, value: { string: user } },
    ]);
  }

  function chooseEnv() {
    write("env", [{ op: "set", path: `${path}.env`, value: { string: envVar } }]);
  }

  function chooseCommand() {
    // ⛔ No question here. The SHELL asks (Lo13): it answers a write that
    // introduces or changes a `command` source with a confirmation request,
    // worded by Rust and tied to the exact text, and writes nothing until the
    // settings window sends it back. The question this component used to ask
    // was advisory: "Edit as text" and a raw `apply_config_edits` walked past it.
    const argv = command.split(/\s+/).filter((s) => s.length > 0);
    if (argv.length === 0) {
      message = "A command source needs a program to run.";
      return;
    }
    write("command", [
      {
        op: "set",
        path: `${path}.command`,
        value: { array: argv.map((a) => ({ string: a })) },
      },
    ]);
  }

  function chooseOwn() {
    write("own", [{ op: "set", path: `${path}.own`, value: { boolean: true } }]);
  }

  function apply() {
    switch (kind) {
      case "keyring":
        chooseKeyring();
        break;
      case "env":
        chooseEnv();
        break;
      case "command":
        chooseCommand();
        break;
      case "own":
        chooseOwn();
        break;
    }
  }

  async function saveToken() {
    message = "";
    try {
      await setOwnToken(account, pasted);
      // ⛔ Cleared immediately. The token has gone to the OS credential store
      // and must not sit in a DOM node for the life of the window.
      pasted = "";
      message = "Saved to the credential store.";
    } catch (error) {
      message = String(error);
    }
  }

  async function forgetToken() {
    message = "";
    try {
      await clearOwnToken(account);
      message = "Removed from the credential store.";
    } catch (error) {
      message = String(error);
    }
  }
</script>

<div class={["token", FIELD_ROW]}>
  <div class={["label", FIELD_LABEL]}>{meta.label}</div>
  <div class="body flex flex-col gap-1.5">
    <div class="choices flex flex-wrap gap-x-3 gap-y-1 pt-1" role="radiogroup" aria-label={meta.label}>
      {#each [["keyring", "glab / OS keyring"], ["env", "Environment variable"], ["command", "Command"], ["own", "bridgewatch's own entry"]] as [id, label] (id)}
        <label class="choice flex cursor-pointer items-center gap-1.5 text-[13px]">
          <input
            type="radio"
            class={CHECK}
            name={`token-${account}`}
            value={id}
            checked={kind === id}
            onchange={() => (kind = id as Kind)}
          />
          {label}
        </label>
      {/each}
    </div>

    {#if kind === "keyring"}
      <div class="pair flex gap-1.5">
        <input type="text" class={INPUT} placeholder="service" bind:value={service} />
        <input type="text" class={INPUT} placeholder="user (may be empty)" bind:value={user} />
      </div>
      <p class={["hint m-0", HINT]}>
        An empty user is legal and is what <code>glab</code> writes. The core shells out to
        <code>security</code> / <code>secret-tool</code> for that case, because the keyring crate
        refuses it before reaching the store.
      </p>
    {:else if kind === "env"}
      <input type="text" class={INPUT} placeholder="BRIDGEWATCH_TOKEN_GITLAB" bind:value={envVar} />
    {:else if kind === "command"}
      <input type="text" class={INPUT} placeholder="pass gitlab/pat" bind:value={command} />
      <p class={["hint m-0", HINT]}>
        Runs on every token refresh. A program that prints anything besides the token is refused at
        resolve time, with advice.
      </p>
    {:else}
      <p class={["hint m-0", HINT]}>
        Read from <code>bridgewatch:{account}</code> in the OS credential store. Paste a token below
        to write it; it is never put in the configuration file.
      </p>
      <div class="pair flex gap-1.5">
        <input type="password" class={INPUT} placeholder="glpat-…" bind:value={pasted} autocomplete="off" />
        <Button variant="outline" size="sm" onclick={saveToken} disabled={pasted.trim() === ""}>Save token</Button>
        <Button variant="ghost" size="sm" onclick={forgetToken}>Forget</Button>
      </div>
    {/if}

    <Button variant="secondary" size="sm" class="apply self-start" onclick={apply}>Use this source</Button>
    {#if message}
      <p class={["message m-0", HINT]}>{message}</p>
    {/if}
    {#if current.kind === "command"}
      <!-- ⚠ The confirmation is one moment; this is the standing fact. The
           dialog is also only a GUI courtesy — the core runs whatever the file
           says, however the file came to say it — so the account that does it
           says so on the form for as long as it is true. -->
      <p class="warning text-tone-amber m-0 text-[11px]">
        This account runs <code>{current.command}</code> on every token refresh.
      </p>
    {/if}
    <p class={["hint m-0", HINT]}>{meta.hint}</p>
  </div>
</div>
