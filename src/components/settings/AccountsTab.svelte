<script lang="ts">
  import { concretePath, entriesFor } from "../../lib/settings/registry";
  import { getAt } from "../../lib/settings/values";
  import type { Edit } from "../../lib/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import Field from "./Field.svelte";
  import { INPUT } from "./styles";
  import TokenField from "./TokenField.svelte";

  interface Props {
    config: unknown;
    /** True when this run created the file; shows the first-run banner. */
    seeded: boolean;
    configPath: string;
    onedit: (edits: Edit[]) => void | Promise<void>;
  }

  let { config, seeded, configPath, onedit }: Props = $props();

  const names = $derived(Object.keys((getAt(config, "accounts") ?? {}) as Record<string, unknown>));
  const fields = entriesFor("accounts").filter((e) => e.control !== "token");
  let newName = $state("");

  function addAccount() {
    const name = newName.trim();
    if (name === "") return;
    // Enough keys to make the account valid on its own; everything else has a
    // default the core fills in.
    //
    // ⛔ The name is QUOTED, by the same helper the fields use. An account
    // called `gitlab.com` — which is what the shipped example calls its one
    // account — went into the core as `accounts` → `gitlab` → `com` and wrote
    // a nested table that is not an account at all.
    onedit([
      {
        op: "set",
        path: concretePath("accounts.*.base_url", name),
        value: { string: "https://gitlab.com" },
      },
      {
        op: "set",
        path: concretePath("accounts.*.token.own", name),
        value: { boolean: true },
      },
    ]);
    newName = "";
  }
</script>

{#if seeded}
  <div class="banner bg-tone-blue-bg border-border mb-3.5 rounded-xl border px-3 py-2 text-xs">
    <strong>First run.</strong> bridgewatch wrote the shipped example to
    <code class="font-mono">{configPath}</code>. Its default token source is
    <code class="font-mono">glab</code>'s keyring entry, so if you already use <code class="font-mono">glab</code> it is
    already watching; otherwise pick a token source below.
  </div>
{/if}

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
      {onedit}
    />
  </section>
{/each}

<div class="add flex max-w-[420px] gap-1.5">
  <input type="text" class={INPUT} placeholder="new account name" bind:value={newName} />
  <Button variant="outline" size="sm" onclick={addAccount} disabled={newName.trim() === ""}>Add account</Button>
</div>
