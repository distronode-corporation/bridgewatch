<script lang="ts">
  import { concretePath, entriesFor, providerOf } from "../../lib/settings/registry";
  import { getAt } from "../../lib/settings/values";
  import type { Edit, Provider } from "../../lib/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import Field from "./Field.svelte";
  import { INPUT, SELECT } from "./styles";
  import TokenField from "./TokenField.svelte";

  interface Props {
    config: unknown;
    onedit: (edits: Edit[]) => void | Promise<void>;
  }

  let { config, onedit }: Props = $props();

  const names = $derived(Object.keys((getAt(config, "accounts") ?? {}) as Record<string, unknown>));
  const fields = entriesFor("accounts").filter((e) => e.control !== "token");
  let newName = $state("");
  /** GitLab first and the default, so adding an account works as it always did. */
  let newProvider = $state<Provider>("gitlab");

  const accountAt = (name: string) => getAt(config, concretePath("accounts.*", name));

  function addAccount() {
    const name = newName.trim();
    if (name === "") return;
    const token: Edit = {
      op: "set",
      path: concretePath("accounts.*.token.own", name),
      value: { boolean: true },
    };
    if (newProvider === "github") {
      // ⛔ `provider` and the token, and nothing else. base_url, api_path and
      // header all default per provider, so writing GitLab's values (or even
      // today's GitHub ones) would pin them into a file that should follow the
      // code, and https://gitlab.com with /api/v4 on a github account is a
      // file that loads and then 404s every request.
      onedit([{ op: "set", path: concretePath("accounts.*.provider", name), value: { string: "github" } }, token]);
      newName = "";
      return;
    }
    // Enough keys to make the account valid on its own; everything else has a
    // default the core fills in.
    //
    // ⛔ The name is QUOTED, by the same helper the fields use. An account
    // called `gitlab.com` (naming an account after its host is the obvious
    // thing to do) went into the core as `accounts` → `gitlab` → `com` and
    // wrote a nested table that is not an account at all.
    onedit([
      {
        op: "set",
        path: concretePath("accounts.*.base_url", name),
        value: { string: "https://gitlab.com" },
      },
      token,
    ]);
    newName = "";
  }
</script>

{#each names as name (name)}
  <section class="account border-border mb-3.5 border-b pb-2.5">
    <h3 class="mt-0 mb-2.5 text-[13px] font-semibold">{name}</h3>
    {#each fields as entry (entry.path)}
      {@const path = concretePath(entry.path, name)}
      <Field {entry} {path} value={getAt(config, path)} {onedit} />
    {/each}
    <!-- ⛔ `getAt` splits on unquoted dots exactly as the core does, so an
         account called `gitlab.com` read as `accounts.gitlab.com.token`,
         found nothing, and showed every such account as bridgewatch's own
         entry however its token was really configured. -->
    <TokenField
      account={name}
      value={getAt(config, concretePath("accounts.*.token", name))}
      provider={providerOf(accountAt(name))}
      baseUrl={String(getAt(config, concretePath("accounts.*.base_url", name)) ?? "")}
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
  <Button variant="outline" size="sm" onclick={addAccount} disabled={newName.trim() === ""}>Add account</Button>
</div>
