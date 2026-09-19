<script lang="ts">
  /** Step 1: provider, instance, token source, "Test connection". */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import DeviceSignIn from "../DeviceSignIn.svelte";
  import type { CliTokenDetection, Identity, Provider } from "./api";
  import { baseUrlOf, cliName, type Draft, type StepErrors, type TokenMode } from "./model";
  import { providerName, type OAuthApi, type OAuthAvailability, type SignedIn } from "../../lib/oauth";

  interface Props {
    draft: Draft;
    errors: StepErrors;
    /** What detection found for the provider's CLI (glab or gh). */
    cli: CliTokenDetection | null;
    identity: Identity | null;
    testing: boolean;
    testError: string | null;
    onTest: () => void;
    /** The instance address settled (radio change or URL blur): re-detect the CLI's token. */
    onInstanceChange: () => void;
    /** The provider radio changed. */
    onProviderChange: (provider: Provider) => void;
    /** Sign-in commands; absent (a test, a browser) means no sign-in is offered. */
    oauth?: OAuthApi;
    /** Whether sign-in is offered for this instance and client id. */
    availability?: OAuthAvailability | null;
    /** A sign-in finished. */
    onSignedIn?: (signed: SignedIn) => void;
    /** The typed client id changed. */
    onClientIdChange?: () => void;
  }

  let {
    draft = $bindable(),
    errors,
    cli,
    identity,
    testing,
    testError,
    onTest,
    onInstanceChange,
    onProviderChange,
    oauth,
    availability = null,
    onSignedIn,
    onClientIdChange,
  }: Props = $props();

  /** "Sign in with ..." is offered when a client id exists, built in or typed, or is already chosen. */
  const offerSignIn = $derived(!!oauth && (availability?.available === true || draft.tokenMode === "oauth"));
  const signInName = $derived(`Sign in with ${providerName(draft.provider)}`);
  /** The own-application box starts open where only a typed client id will do, or one is typed. */
  const clientIdOpen = $derived(draft.oauthClientId.trim() !== "" || availability?.needs_client_id === true);

  const github = $derived(draft.provider === "github");
  const cliTool = $derived(cliName(draft.provider));

  /** Offer the keyring option when detection found the CLI's item, or when re-running on a config that already uses one. */
  const cliFound = $derived(cli?.status === "found" || draft.cliSource !== null);

  const PROVIDERS: { value: Provider; label: string }[] = [
    { value: "gitlab", label: "GitLab" },
    { value: "github", label: "GitHub Actions" },
  ];

  const modes = $derived<{ value: TokenMode; radio: string; label: string; hint: string }[]>([
    {
      value: "cli",
      radio: cliTool,
      label: `Use ${cliTool}'s token`,
      hint: github
        ? "Found in the system keychain; bridgewatch reads gh's active account where gh keeps it, so gh auth switch carries over."
        : "Found in the system keychain; bridgewatch reads it where glab keeps it.",
    },
    {
      value: "paste",
      radio: "paste",
      label: "Paste a token",
      hint: "Stored in bridgewatch's own keychain entry, never in config.toml.",
    },
    { value: "env", radio: "env", label: "Environment variable", hint: "Only the variable's NAME goes in the file." },
    {
      value: "command",
      radio: "command",
      label: "Run a command",
      hint: github
        ? "Its output is the token, e.g. gh auth token. You will be asked before it runs."
        : "Its output is the token, e.g. pass gitlab/pat. You will be asked before it runs.",
    },
  ]);

  /** What the token must be allowed to do, said before anyone pastes one. */
  const needs = $derived(
    github
      ? "A classic token needs the repo scope for a private repository (none for a public one); a fine-grained token needs Actions: read and Metadata: read."
      : "The token needs the read_api scope.",
  );

  const kindWord = (identity: Identity) => {
    switch (identity.token.kind) {
      case "personal":
        return "personal token";
      case "project":
        return "project token";
      case "group":
        return "group token";
      case "service_account":
        return "service account";
      default:
        return "token";
    }
  };

  function selectMode(mode: TokenMode) {
    draft.tokenMode = mode;
    if (mode === "cli" && cli?.status === "found") draft.cliSource = cli.source;
  }
</script>

<div class="flex flex-col gap-4">
  <fieldset class="flex flex-col gap-2">
    <legend class="mb-1 text-sm font-medium">CI provider</legend>
    {#each PROVIDERS as provider (provider.value)}
      <label class="flex items-center gap-2 text-sm">
        <input
          type="radio"
          name="provider"
          value={provider.value}
          class="accent-primary"
          checked={draft.provider === provider.value}
          onchange={() => onProviderChange(provider.value)}
        />
        {provider.label}
      </label>
    {/each}
  </fieldset>

  <fieldset class="flex flex-col gap-2">
    <legend class="mb-1 text-sm font-medium">{github ? "GitHub host" : "GitLab instance"}</legend>
    <label class="flex items-center gap-2 text-sm">
      <input
        type="radio"
        name="instance"
        value="hosted"
        class="accent-primary"
        checked={draft.instance === "hosted"}
        onchange={() => {
          draft.instance = "hosted";
          onInstanceChange();
        }}
      />
      {github ? "github.com" : "gitlab.com"}
    </label>
    <label class="flex items-center gap-2 text-sm">
      <input
        type="radio"
        name="instance"
        value="self-managed"
        class="accent-primary"
        checked={draft.instance === "self-managed"}
        onchange={() => {
          draft.instance = "self-managed";
          if (draft.selfManagedUrl.trim()) onInstanceChange();
        }}
      />
      {github ? "GitHub Enterprise" : "Self-managed"}
    </label>
    {#if draft.instance === "self-managed"}
      <div class="flex flex-col gap-1 pl-6">
        <Label for="wizard-base-url" class="sr-only">Instance URL</Label>
        <Input
          id="wizard-base-url"
          name="base_url"
          type="url"
          placeholder={github ? "https://github.example.com" : "https://gitlab.example.com"}
          bind:value={draft.selfManagedUrl}
          onblur={onInstanceChange}
          aria-invalid={errors.base_url ? "true" : undefined}
          aria-describedby="wizard-base-url-msg"
        />
        <FieldMessage
          id="wizard-base-url-msg"
          message={errors.base_url}
          hint={github
            ? "Enterprise Server: the address you browse to (its API is /api/v3 there). GHE.com: the api. address."
            : undefined}
        />
      </div>
    {/if}
  </fieldset>

  <fieldset class="flex flex-col gap-2" aria-describedby="wizard-token-needs wizard-token-msg">
    <legend class="mb-1 text-sm font-medium">Token</legend>
    <p id="wizard-token-needs" class="text-muted-foreground text-xs" data-slot="token-needs">{needs}</p>
    {#if offerSignIn}
      <!-- First, and the recommended way: nothing to create, copy or paste
           but a short code, and the tokens renew themselves. -->
      <label class="flex items-start gap-2 text-sm">
        <input
          type="radio"
          name="token-mode"
          value="oauth"
          class="accent-primary mt-0.5"
          checked={draft.tokenMode === "oauth"}
          onchange={() => selectMode("oauth")}
        />
        <span class="flex flex-col">
          <span class="flex items-center gap-2">{signInName} <Badge variant="secondary">Recommended</Badge></span>
          <span class="text-muted-foreground text-xs"
            >Approve bridgewatch in your browser with a short code. The sign-in is kept in the system keychain and
            renews itself.</span
          >
        </span>
      </label>
    {/if}
    {#each modes as mode (mode.value)}
      {#if mode.value !== "cli" || cliFound}
        <label class="flex items-start gap-2 text-sm">
          <input
            type="radio"
            name="token-mode"
            value={mode.radio}
            class="accent-primary mt-0.5"
            checked={draft.tokenMode === mode.value}
            onchange={() => selectMode(mode.value)}
          />
          <span class="flex flex-col">
            <span>{mode.label}</span>
            <span class="text-muted-foreground text-xs">{mode.hint}</span>
          </span>
        </label>
      {/if}
    {/each}

    <div class="pl-6">
      {#if draft.tokenMode === "oauth" && oauth}
        {#if draft.oauthLogin}
          <p class="m-0 text-sm" data-slot="oauth-signed-in">
            Signed in as <span class="font-mono">@{draft.oauthLogin}</span>.
          </p>
        {:else if draft.oauthStored}
          <p class="text-muted-foreground m-0 text-xs" data-slot="oauth-stored">
            This account is already signed in. Sign in again only to replace it.
          </p>
        {/if}
        <DeviceSignIn
          api={oauth}
          request={() => ({
            account: (draft.oauthAccount ?? draft.account).trim() || draft.account.trim(),
            provider: draft.provider,
            base_url: baseUrlOf(draft),
            client_id: draft.oauthClientId.trim() || null,
          })}
          label={draft.oauthLogin || draft.oauthStored ? "Sign in again" : signInName}
          disabled={!draft.account.trim()}
          onsignedin={(signed) => onSignedIn?.(signed)}
        />
      {:else if draft.tokenMode === "paste"}
        <Label for="wizard-secret" class="sr-only">Access token</Label>
        <Input
          id="wizard-secret"
          name="secret"
          type="password"
          autocomplete="off"
          spellcheck={false}
          placeholder={draft.ownStored ? "Leave empty to keep the stored token" : github ? "ghp_… or github_pat_…" : "glpat-…"}
          bind:value={draft.secret}
          aria-invalid={errors.token ? "true" : undefined}
          aria-describedby="wizard-token-msg"
        />
      {:else if draft.tokenMode === "env"}
        <Label for="wizard-env" class="sr-only">Environment variable name</Label>
        <Input
          id="wizard-env"
          name="env"
          placeholder={github ? "GITHUB_TOKEN" : "GITLAB_TOKEN"}
          spellcheck={false}
          class="font-mono"
          bind:value={draft.envVar}
          aria-invalid={errors.token ? "true" : undefined}
          aria-describedby="wizard-token-msg"
        />
      {:else if draft.tokenMode === "command"}
        <Label for="wizard-command" class="sr-only">Command that prints the token</Label>
        <Input
          id="wizard-command"
          name="command"
          placeholder={github ? "gh auth token" : "pass gitlab/pat"}
          spellcheck={false}
          class="font-mono"
          bind:value={draft.commandText}
          aria-invalid={errors.token ? "true" : undefined}
          aria-describedby="wizard-token-msg"
        />
      {/if}
      <FieldMessage id="wizard-token-msg" message={errors.token} />
    </div>
    {#if oauth}
      <details class="text-xs" open={clientIdOpen} data-slot="oauth-own-app">
        <summary class="text-muted-foreground cursor-pointer">Sign in with an OAuth application of your own</summary>
        <div class="flex flex-col gap-1 pt-1.5 pl-4">
          <Label for="wizard-oauth-client-id" class="text-xs">Client id</Label>
          <Input
            id="wizard-oauth-client-id"
            name="oauth_client_id"
            class="w-72 font-mono"
            spellcheck={false}
            autocomplete="off"
            placeholder={availability?.builtin ? "empty: bridgewatch's own" : "client id"}
            bind:value={draft.oauthClientId}
            oninput={() => onClientIdChange?.()}
          />
          <span class="text-muted-foreground" data-slot="oauth-client-id-hint">
            {#if github}
              A GitHub App on {availability?.host ?? "your server"} with "Enable Device Flow" ticked. A client id is not a
              secret; there is no client secret.
            {:else}
              An application on {availability?.host ?? "your instance"} that is not confidential, with the read_api scope.
              Needs GitLab 17.9 or later.
            {/if}
          </span>
        </div>
      </details>
    {/if}
  </fieldset>

  <div class="flex flex-col gap-1">
    <Label for="wizard-account">Account name</Label>
    <Input
      id="wizard-account"
      name="account"
      class="w-48 font-mono"
      bind:value={draft.account}
      oninput={() => (draft.accountEdited = true)}
      aria-invalid={errors.account ? "true" : undefined}
      aria-describedby="wizard-account-msg"
    />
    <FieldMessage
      id="wizard-account-msg"
      message={errors.account}
      hint="The [accounts.…] key in config.toml. Any short name."
    />
  </div>

  <div class="flex flex-col gap-2">
    <div>
      <Button variant="outline" size="sm" onclick={onTest} disabled={testing} data-action="test-connection">
        {testing ? "Testing…" : "Test connection"}
      </Button>
    </div>
    {#if testError}
      <Alert.Root variant="destructive" data-slot="test-error">
        <Alert.Title>Could not connect</Alert.Title>
        <Alert.Description>{testError}</Alert.Description>
      </Alert.Root>
    {:else if identity}
      <Alert.Root data-slot="identity">
        <Alert.Title class="flex items-center gap-2">
          Connected as <span class="font-mono">@{identity.username}</span>
          {#if identity.name}<span class="text-muted-foreground font-normal">({identity.name})</span>{/if}
          <Badge variant="secondary">{kindWord(identity)}</Badge>
        </Alert.Title>
        <Alert.Description>
          {#if identity.scopes.length > 0}
            <span class="block">Scopes: {identity.scopes.join(", ")}</span>
          {:else if github && identity.token.kind === "unknown"}
            <span class="block" data-slot="scopes-unknown"
              >Scopes: not reported. GitHub lists them only for a classic token, so a fine-grained token cannot be
              checked here; it needs Actions: read and Metadata: read.</span
            >
          {/if}
          {#if identity.expires_at}
            <span class="block">Expires {identity.expires_at}</span>
          {/if}
          {#if identity.token.kind === "project"}
            <span class="block">A project token cannot list projects; you will type the project on the next step.</span>
          {/if}
          {#each identity.warnings as warning (warning)}
            <span class="text-tone-amber block">{warning}</span>
          {/each}
        </Alert.Description>
      </Alert.Root>
    {/if}
  </div>
</div>
