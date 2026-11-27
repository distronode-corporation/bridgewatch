<!--
SPDX-FileCopyrightText: Distronode Corporation

SPDX-License-Identifier: Apache-2.0
-->

<script lang="ts">
  /**
   * "Sign in with GitHub" / "Sign in with GitLab": the device flow, on screen.
   *
   * One button; then the code, large, with Copy and "Open github.com/login/device";
   * then a waiting line until the user approves in the browser; then who signed
   * in. Every failure is one sentence in plain words. Used by the wizard's
   * account step and by Settings' token field, with the shell's `OAuthApi` or a
   * test's fake.
   */
  import { onDestroy } from "svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { providerName } from "../lib/providers";
  import {
    failureOf,
    failureSentence,
    shortUrl,
    type OAuthApi,
    type SignInFailure,
    type SignInRequest,
    type SignedIn,
    type StartedSignIn,
  } from "../lib/oauth";

  interface Props {
    api: OAuthApi;
    /** What to sign in to. Read when the button is pressed. */
    request: () => SignInRequest;
    /** Called once the sign-in is stored. */
    onsignedin?: (signed: SignedIn) => void;
    /** Called when a sign-in fails or is stopped. */
    onfailed?: (failure: SignInFailure) => void;
    /** The button's label; defaults to "Sign in with GitHub". */
    label?: string;
    disabled?: boolean;
    /** The sign-in button's look: "outline" once an account is signed in, so the next step keeps the one filled button. */
    variant?: "default" | "outline";
  }

  let { api, request, onsignedin, onfailed, label, disabled = false, variant = "default" }: Props = $props();

  type Phase = "idle" | "starting" | "waiting" | "done" | "failed";
  let phase = $state<Phase>("idle");
  let started = $state<StartedSignIn | null>(null);
  let failure = $state<SignInFailure | null>(null);
  let host = $state("");
  let copied = $state(false);
  let polls = $state(0);
  let unsubscribe: (() => void) | null = null;
  /** Bumped by every start, every stop and the destroy, so a late answer to an old one is ignored. */
  let generation = 0;
  let destroyed = false;

  const buttonLabel = $derived(label ?? `Sign in with ${providerName(request().provider)}`);

  /**
   * Start a flow and wait for it.
   *
   * ⛔ A flow is the shell polling the provider until the code is entered or
   * expires (15 minutes). One this component started and will never show (it
   * went away, or another start replaced it, before `start` answered) is
   * cancelled HERE: nothing else knows its id. The synchronous `starting`
   * check stops a double click, whose two clicks both land before the
   * re-render that disables the button, from starting two.
   */
  async function begin() {
    if (phase === "starting") return;
    const mine = ++generation;
    const req = request();
    phase = "starting";
    failure = null;
    copied = false;
    polls = 0;
    try {
      const s = await api.start(req);
      if (mine !== generation) {
        void api.cancel(s.id).catch(() => {});
        return;
      }
      started = s;
      host = s.host;
      phase = "waiting";
      if (api.onProgress && !unsubscribe) {
        const off = await api.onProgress((p) => {
          if (started && p.id === started.id && p.state === "waiting") polls = p.polls;
        });
        // Gone while subscribing: onDestroy had nothing to unsubscribe yet.
        if (destroyed) off();
        else unsubscribe = off;
      }
      if (mine !== generation) return;
      const signed = await api.wait(s.id);
      if (mine !== generation) return;
      phase = "done";
      started = null;
      onsignedin?.(signed);
    } catch (error) {
      if (mine !== generation) return;
      failure = failureOf(error);
      phase = "failed";
      started = null;
      onfailed?.(failure);
    }
  }

  // Cancel is drawn only while a code is waiting, and passes that sign-in.
  async function stop(s: StartedSignIn) {
    generation++;
    started = null;
    phase = "idle";
    await api.cancel(s.id).catch(() => {});
  }

  // The Copy and Open buttons are drawn only while a code is waiting, and pass that sign-in.
  async function copy(s: StartedSignIn) {
    copied = await api.copy(s.user_code);
  }

  async function open(s: StartedSignIn) {
    await api.openVerification(s.id).catch(() => {});
  }

  onDestroy(() => {
    // First, so a `start` still in flight finds itself stale and cancels.
    destroyed = true;
    generation++;
    unsubscribe?.();
    unsubscribe = null;
    // A window closed mid-sign-in stops polling rather than leaving the shell
    // waiting on a code nobody will enter.
    if (started) void api.cancel(started.id).catch(() => {});
  });
</script>

<div class="device-sign-in flex flex-col gap-2" data-slot="device-sign-in">
  {#if phase === "waiting" && started}
    {@const waiting = started}
    <p class="m-0 text-[13px]/[18px]">
      Enter this code at <span class="font-mono">{shortUrl(started.verification_uri)}</span>:
    </p>
    <div class="flex items-center gap-2">
      <output
        class="bg-muted rounded-md px-3 py-1.5 font-mono text-2xl font-semibold tracking-widest select-all"
        data-slot="user-code"
        aria-label="Your sign-in code">{started.user_code}</output
      >
      <Button variant="outline" size="sm" onclick={() => void copy(waiting)} data-action="copy-code">
        {copied ? "Copied" : "Copy"}
      </Button>
    </div>
    <div class="flex flex-wrap items-center gap-2">
      <Button size="sm" onclick={() => void open(waiting)} data-action="open-verification">
        Open {shortUrl(started.verification_uri)}
      </Button>
      <Button variant="ghost" size="sm" onclick={() => void stop(waiting)} data-action="cancel-sign-in">Cancel</Button>
    </div>
    <p class="text-muted-foreground m-0 text-xs" aria-live="polite" data-slot="sign-in-waiting">
      <!-- The count is for the eye: announced, it would repeat the sentence on every poll. -->
      Waiting for you to approve it on {started.host}…{#if polls > 0}<span aria-hidden="true">
          (checked {polls} {polls === 1 ? "time" : "times"})</span
        >{/if}
    </p>
  {:else}
    <div>
      <Button
        size="sm"
        {variant}
        onclick={() => void begin()}
        disabled={disabled || phase === "starting"}
        data-action="oauth-sign-in"
      >
        {phase === "starting" ? "Starting…" : phase === "failed" ? "Start again" : buttonLabel}
      </Button>
    </div>
    {#if phase === "failed" && failure}
      <p class="text-destructive m-0 text-xs" role="alert" data-slot="sign-in-error" data-kind={failure.kind}>
        {failureSentence(failure, host || "the provider")}
      </p>
    {/if}
  {/if}
</div>
