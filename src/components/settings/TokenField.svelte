<script lang="ts">
  import { untrack } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import DeviceSignIn from "../DeviceSignIn.svelte";
  import { setOwnToken, clearOwnToken, inTauri, oauthApi } from "../../lib/ipc";
  import { defaultBaseUrl, providerName, type OAuthApi, type OAuthAvailability, type SignInStatus } from "../../lib/oauth";
  import type { Edit, Provider } from "../../lib/types";
  import { concretePath, entry as registryEntry } from "../../lib/settings/registry";
  import { cliKeyringService } from "../wizard/model";
  import { CHECK, FIELD_LABEL, FIELD_ROW, HINT, INPUT } from "./styles";

  interface Props {
    account: string;
    /** The `token` value as the core serialised it. */
    value: unknown;
    onedit: (edits: Edit[]) => void | Promise<void>;
    /** The account's provider: which CLI's keyring item the preset names. */
    provider?: Provider;
    /** The account's `base_url`, which the preset's service is derived from. */
    baseUrl?: string;
    /** Sign-in commands: the shell's in the app, a fake in a test, none elsewhere. */
    oauth?: OAuthApi;
  }

  let {
    account,
    value,
    onedit,
    provider = "gitlab",
    baseUrl = "",
    oauth = inTauri() ? oauthApi() : undefined,
  }: Props = $props();

  /** The instance the account talks to: its `base_url`, or the provider's default when the file has none. */
  const instance = $derived(baseUrl.trim() || defaultBaseUrl(provider));

  const github = $derived(provider === "github");
  /** glab for a GitLab account, gh for a GitHub one: never offered across. */
  const cli = $derived(github ? "gh" : "glab");
  const preset = $derived(cliKeyringService(provider, baseUrl));
  const kinds = $derived<[Kind, string][]>([
    ["keyring", `${cli} / OS keyring`],
    ["env", "Environment variable"],
    ["command", "Command"],
    ["own", "bridgewatch's own entry"],
  ]);

  /** Fill the keyring pair with the CLI's item. Nothing is written until "Use this source". */
  function usePreset() {
    if (!preset) return;
    service = preset;
    user = "";
  }

  const meta = registryEntry("accounts.*.token");
  // ⛔ `accounts.${account}.token` was a template string, and an account called
  // `gitlab.com` (the obvious name, since an account is usually named after
  // its host) has a dot in it. The core's `split_path` then walked INTO the
  // name and wrote `[accounts.gitlab.com.token]`, a nested table nobody asked
  // for, so the token source of that account could not be changed from this
  // window at all.
  const path = $derived(concretePath("accounts.*.token", account));

  type Kind = "oauth" | "keyring" | "env" | "command" | "own";

  const current = $derived.by<{
    kind: Kind;
    service: string;
    user: string;
    env: string;
    command: string;
    clientId: string;
  }>(
    () => {
      const v = (value ?? {}) as Record<string, unknown>;
      if ("oauth" in v) {
        const o = v.oauth as { client_id?: string } | boolean;
        const clientId = typeof o === "object" && o !== null ? (o.client_id ?? "") : "";
        return { kind: "oauth", service: "", user: "", env: "", command: "", clientId };
      }
      if ("keyring" in v) {
        const k = v.keyring as { service?: string; user?: string };
        return { kind: "keyring", service: k.service ?? "", user: k.user ?? "", env: "", command: "", clientId: "" };
      }
      if ("env" in v) {
        return { kind: "env", service: "", user: "", env: String(v.env ?? ""), command: "", clientId: "" };
      }
      if ("command" in v) {
        const argv = Array.isArray(v.command) ? (v.command as string[]) : [];
        return { kind: "command", service: "", user: "", env: "", command: argv.join(" "), clientId: "" };
      }
      return { kind: "own", service: "", user: "", env: "", command: "", clientId: "" };
    },
  );

  let kind = $state<Kind>("own");
  let service = $state("");
  let user = $state("");
  let envVar = $state("");
  let command = $state("");
  let pasted = $state("");
  let message = $state("");
  let clientId = $state("");
  let availability = $state<OAuthAvailability | null>(null);
  let status = $state<SignInStatus | null>(null);

  /** Ask the shell whether sign-in can work here, whenever the instance or the typed client id changes. */
  let availabilitySeq = 0;
  $effect(() => {
    const api = oauth;
    const seq = ++availabilitySeq;
    const args = [provider, instance, clientId.trim() || null] as const;
    if (!api) return;
    void api
      .availability(...args)
      .then((a) => {
        if (seq === availabilitySeq) availability = a;
      })
      .catch(() => {});
  });

  /**
   * "Sign in" is listed first when it can work (a built-in or typed client id)
   * or when the file already says so; otherwise the four others are all there is,
   * exactly as before.
   */
  const offerSignIn = $derived(
    !!oauth && (current.kind === "oauth" || kind === "oauth" || availability?.available === true),
  );
  const choices = $derived<[Kind, string][]>(
    offerSignIn ? [["oauth", `Sign in with ${providerName(provider)}`], ...kinds] : kinds,
  );

  /** Who this account is signed in as, when it signs in. */
  async function loadStatus() {
    if (!oauth) return;
    try {
      status = await oauth.status(account);
    } catch {
      status = null;
    }
  }
  $effect(() => {
    if (current.kind === "oauth") void untrack(() => loadStatus());
  });

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
    clientId = current.clientId;
  });

  /** The five variant keys of the `token` enum, of which exactly one may exist. */
  const VARIANTS = ["keyring", "env", "command", "own", "oauth"] as const;

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

  function chooseOauth() {
    // `oauth` is written as `true` or as `{ client_id = ".." }`, and the core
    // cannot set a key inside a boolean, so the key is cleared first either way.
    const id = clientId.trim();
    write("oauth", [
      { op: "unset", path: `${path}.oauth` },
      id
        ? { op: "set", path: `${path}.oauth.client_id`, value: { string: id } }
        : { op: "set", path: `${path}.oauth`, value: { boolean: true } },
    ]);
  }

  async function signOut() {
    if (!oauth) return;
    message = "";
    try {
      await oauth.signOut(account);
      status = null;
      message = "Signed out. The sign-in is removed from the credential store.";
    } catch (error) {
      message = String(error);
    }
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
      case "oauth":
        chooseOauth();
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
      {#each choices as [id, label] (id)}
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

    {#if kind === "oauth" && oauth}
      <input
        type="text"
        class={INPUT}
        placeholder={availability?.builtin ? "client id (empty: bridgewatch's own application)" : "client id of your OAuth application"}
        aria-label="OAuth client id"
        spellcheck={false}
        autocomplete="off"
        bind:value={clientId}
      />
      <p class={["hint m-0", HINT]}>
        {#if provider === "github"}
          A GitHub App with "Enable Device Flow" ticked; GitHub Enterprise Server needs one of its own.
        {:else}
          An application that is not confidential, with the read_api scope; a self-managed GitLab needs one of its own.
        {/if}
        The sign-in is kept in the OS credential store and renews itself.
      </p>
      {#if status}
        <p class="signed-in m-0 text-[13px]" data-slot="oauth-status">
          Signed in as <code>@{status.login ?? "unknown"}</code>
        </p>
      {:else if current.kind === "oauth"}
        <p class={["hint m-0", HINT]} data-slot="oauth-status">Not signed in.</p>
      {/if}
      <div class="pair flex flex-wrap items-start gap-1.5">
        <DeviceSignIn
          api={oauth}
          request={() => ({ account, provider, base_url: instance, client_id: clientId.trim() || null })}
          label={status ? "Sign in again" : undefined}
          onsignedin={(signed) => {
            // Signing in stores the tokens; the FILE still names whatever source
            // it named until "Use this source" writes `oauth`.
            message =
              current.kind === "oauth"
                ? `Signed in as @${signed.login ?? "unknown"}.`
                : `Signed in as @${signed.login ?? "unknown"}. Press "Use this source" to switch this account to it.`;
            void loadStatus();
          }}
        />
        {#if status}
          <Button variant="ghost" size="sm" class="sign-out" onclick={() => void signOut()}>Sign out</Button>
        {/if}
      </div>
      {#if provider === "github" && availability?.install_url}
        <p class={["hint m-0", HINT]} data-slot="install-app">
          The app sees only repositories on accounts it is installed on.
          <button type="button" class="underline" onclick={() => void oauth?.openInstall(provider, instance)}
            >Install it on an organisation</button
          >
        </p>
      {/if}
    {:else if kind === "keyring"}
      <div class="pair flex gap-1.5">
        <input type="text" class={INPUT} placeholder="service" aria-label="Keyring service" bind:value={service} />
        <input type="text" class={INPUT} placeholder="user (may be empty)" aria-label="Keyring user" bind:value={user} />
      </div>
      {#if preset}
        <Button variant="outline" size="sm" class="preset self-start" onclick={usePreset}>
          Fill in {cli}'s item ({preset})
        </Button>
      {/if}
      {#if github}
        <p class={["hint m-0", HINT]}>
          <code>gh</code> keeps two items per host; the one with an empty user is its active account,
          which follows <code>gh auth switch</code>. The command <code>gh auth token</code> is the
          portable alternative.
        </p>
      {:else}
        <p class={["hint m-0", HINT]}>
          An empty user is legal and is what <code>glab</code> writes. The core shells out to
          <code>security</code> / <code>secret-tool</code> for that case, because the keyring crate
          refuses it before reaching the store.
        </p>
      {/if}
    {:else if kind === "env"}
      <input
        type="text"
        class={INPUT}
        placeholder={github ? "BRIDGEWATCH_TOKEN_GITHUB" : "BRIDGEWATCH_TOKEN_GITLAB"}
        bind:value={envVar}
      />
    {:else if kind === "command"}
      <input type="text" class={INPUT} placeholder={github ? "gh auth token" : "pass gitlab/pat"} bind:value={command} />
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
        <input
          type="password"
          class={INPUT}
          placeholder={github ? "ghp_… or github_pat_…" : "glpat-…"}
          bind:value={pasted}
          autocomplete="off"
        />
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
