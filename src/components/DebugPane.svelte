<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { TONE_TEXT, countdown, millis, shortPath, statusTone } from "../lib/format";
  import { copyText } from "../lib/ipc";
  import type { Snapshot, Status } from "../lib/types";

  interface Props {
    snapshot: Snapshot;
    status: Status | null;
  }

  let { snapshot, status }: Props = $props();

  let open = $state(false);
  let copied = $state<"" | "ok" | "failed">("");

  async function copyVerdict() {
    const ok = await copyText(JSON.stringify(snapshot, null, 2));
    copied = ok ? "ok" : "failed";
    setTimeout(() => (copied = ""), 2000);
  }
</script>

<details class="debug border-border text-muted-foreground border-t px-3 pt-1.5 pb-2.5 text-[11px]" bind:open>
  <summary class="hover:text-foreground cursor-pointer py-0.5">Debug</summary>

  <div class="meta my-1.5 grid gap-0.5">
    <div>
      <span class="inline-block min-w-16">config</span>
      <span class="text-foreground font-mono break-all">{status?.configPath ?? "—"}</span>
    </div>
    <div>
      <span class="inline-block min-w-16">last poll</span>
      <span class="text-foreground">
        {status?.sinceLastPollSecs === null || status?.sinceLastPollSecs === undefined
          ? "never"
          : `${status.sinceLastPollSecs}s ago`}
      </span>
      <span>{countdown(status?.nextPollSecs)}</span>
    </div>
  </div>

  <Button variant="outline" size="xs" class="copy mb-2" onclick={copyVerdict}>
    {copied === "ok" ? "Copied" : copied === "failed" ? "Copy failed" : "Copy verdict JSON"}
  </Button>

  <!--
    The request ring, newest last, exactly as `RequestRing::entries()` returns
    it. `ratelimit_remaining` is the column that matters when GitLab starts
    saying no, and `retry_after` is the one that explains why the interval just
    got longer.
  -->
  {#if snapshot.request_log.length > 0}
    <table class="requests w-full table-fixed border-collapse font-mono text-[10px]">
      <thead>
        <tr class="border-border border-b text-left">
          <th class="px-1 font-normal">method</th>
          <th class="w-[48%] px-1 font-normal">path</th>
          <th class="px-1 font-normal">status</th>
          <th class="px-1 font-normal">ms</th>
          <th class="px-1 font-normal" title="ratelimit-remaining">rl</th>
          <th class="px-1 font-normal" title="retry-after">retry</th>
        </tr>
      </thead>
      <tbody>
        {#each snapshot.request_log as entry, index (`${entry.at}-${index}`)}
          <tr class="text-foreground [&>td]:truncate [&>td]:px-1">
            <td>{entry.method}</td>
            <td title={entry.path}>{shortPath(entry.path)}</td>
            <td class={TONE_TEXT[statusTone(entry.status)]}>{entry.status ?? entry.error ?? "—"}</td>
            <td>{millis(entry.ms)}</td>
            <td>{entry.ratelimit_remaining ?? ""}</td>
            <td>{entry.retry_after ?? ""}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="mt-1.5 mb-0">No requests recorded. `[log].keep_requests = 0` disables the ring.</p>
  {/if}
</details>
