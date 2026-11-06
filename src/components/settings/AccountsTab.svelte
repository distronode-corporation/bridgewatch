<script lang="ts">
  import { concretePath, entriesFor, providerOf } from "../../lib/settings/registry";
  import { providerSwitch } from "../../lib/settings/provider-switch";
  import { defaultBaseUrl } from "../../lib/providers";
  import { getAt } from "../../lib/settings/values";
  import { inTauri, oauthApi } from "../../lib/ipc";
  import type { OAuthApi } from "../../lib/oauth";
  import type { Edit, Provider } from "../../lib/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import Field from "./Field.svelte";
  import { INPUT, SELECT } from "./styles";
  import TokenField from "./TokenField.svelte";

  interface Props {
    config: unknown;
    onedit: (edits: Edit[]) => void | Promise<void>;
    /** Sign-in commands: the shell's in the app, a fake in a test, none elsewhere. */
    oauth?: OAuthApi;
  }

  let { config, onedit, oauth = inTauri() ? oauthApi() : undefined }: Props = $props();

  const names = $derived(Object.keys((getAt(config, "accounts") ?? {}) as Record<string, unknown>));
  const fields = entriesFor("accounts").filter((e) => e.control !== "token");
  let newName = $state("");
  /** GitLab first and the default, so adding an account works as it always did. */
  let newProvider = $state<Provider>("gitlab");

  const accountAt = (name: string) => getAt(config, concretePath("accounts.*", name));

  /** Per account, the line saying which customised values a provider change kept. */
  let switchNotes = $state<Record<string, string>>({});

  /**
   * Every edit from an account's fields. A PROVIDER change also removes the
   * old provider's defaults for base_url, api_path and header, in the same
   * write, so the new provider's apply (see `providerSwitch`).
   */
  function editAccount(name: string, edits: Edit[]) {
    const providerPath = concretePath("accounts.*.provider", name);
    const change = edits.find((e) => (e.op === "set" || e.op === "unset") && e.path === providerPath);
    if (!change) return onedit(edits);
    const to: Provider =
      change.op === "set" && "string" in change.value && change.value.string === "github" ? "github" : "gitlab";
    const from = providerOf(accountAt(name));
    if (from === to) return onedit(edits);
    const { edits: extra, note } = providerSwitch(name, accountAt(name), from, to);
    const notes = { ...switchNotes };
    delete notes[name];
    if (note !== null) notes[name] = note;
    switchNotes = notes;
    return onedit([...edits, ...extra]);
  }

  /**
   * Whether a new account on `provider`'s hosted instance (where a new account
   * starts) can sign in with nothing to type: this build has a built-in
   * application there. Asked when the account is added, so the answer is the
   * provider actually chosen. No sign-in commands, or no answer: no.
   */
  async function builtinSignIn(provider: Provider): Promise<boolean> {
    if (!oauth) return false;
    try {
      return (await oauth.availability(provider, defaultBaseUrl(provider), null)).builtin;
    } catch {
      return false;
    }
  }

  async function addAccount() {
    const name = newName.trim();
    if (name === "") return;
    const provider = newProvider;
    // ⛔ Cleared before the question, so a second click while it is out finds
    // no name and adds nothing, rather than the same account twice.
    newName = "";
    // Sign-in where it works with nothing to type, as the wizard preselects
    // it, else bridgewatch's own entry. The Token source below changes either.
    // Without sign-in commands nothing is awaited, so the write is as immediate as it always was.
    const signIn = oauth ? await builtinSignIn(provider) : false;
    const token: Edit = signIn
      ? { op: "set", path: concretePath("accounts.*.token.oauth", name), value: { boolean: true } }
      : { op: "set", path: concretePath("accounts.*.token.own", name), value: { boolean: true } };
    if (provider === "github") {
      // ⛔ `provider` and the token, and nothing else. base_url, api_path and
      // header all default per provider, so writing GitLab's values (or even
      // today's GitHub ones) would pin them into a file that should follow the
      // code, and https://gitlab.com with /api/v4 on a github account is a
      // file that loads and then 404s every request.
      await onedit([{ op: "set", path: concretePath("accounts.*.provider", name), value: { string: "github" } }, token]);
      return;
    }
    // Enough keys to make the account valid on its own; everything else has a
    // default the core fills in.
    //
    // ⛔ The name is QUOTED, by the same helper the fields use. An account
    // called `gitlab.com` (naming an account after its host is the obvious
    // thing to do) went into the core as `accounts` → `gitlab` → `com` and
    // wrote a nested table that is not an account at all.
    await onedit([
      {
        op: "set",
        path: concretePath("accounts.*.base_url", name),
        value: { string: defaultBaseUrl("gitlab") },
      },
      token,
    ]);
  }
</script>

{#each names as name (name)}
  <section class="account border-border mb-3.5 border-b pb-2.5">
    <h3 class="mt-0 mb-2.5 text-[13px] font-semibold">{name}</h3>
    {#each fields as entry (entry.path)}
      {@const path = concretePath(entry.path, name)}
      <Field {entry} {path} value={getAt(config, path)} onedit={(edits) => editAccount(name, edits)} />
    {/each}
    {#if switchNotes[name]}
      <p class="text-tone-amber text-xs" data-slot="provider-switch-note">{switchNotes[name]}</p>
    {/if}
    <!-- ⛔ `getAt` splits on unquoted dots exactly as the core does, so an
         account called `gitlab.com` read as `accounts.gitlab.com.token`,
         found nothing, and showed every such account as bridgewatch's own
         entry however its token was really configured. -->
    <TokenField
      account={name}
      value={getAt(config, concretePath("accounts.*.token", name))}
      provider={providerOf(accountAt(name))}
      baseUrl={String(getAt(config, concretePath("accounts.*.base_url", name)) ?? "")}
      {oauth}
      {onedit}
    />
  </section>
{/each}

<div class="add flex max-w-[420px] gap-1.5">
  <input type="text" class={INPUT} placeholder="new account name" aria-label="New account name" bind:value={newName} />
  <select class={[SELECT, "w-32 shrink-0"]} aria-label="New account's provider" bind:value={newProvider}>
    <option value="gitlab">GitLab</option>
    <option value="github">GitHub</option>
  </select>
  <Button variant="outline" size="sm" onclick={() => void addAccount()} disabled={newName.trim() === ""}>Add account</Button>
</div>
