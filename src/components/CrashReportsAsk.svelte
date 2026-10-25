<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";

  interface Props {
    /** The answer: `true` is Send, `false` is Don't send. */
    onanswer: (send: boolean) => void;
  }

  let { onanswer }: Props = $props();
</script>

<!--
  Asked once: the shell says `ask` only while `[ui].send_crash_reports` is
  unset in a build that has somewhere to send reports, and either answer sets
  it. "Don't send" comes first and is the filled button, because it is the
  default: nothing is sent until the user picks Send.
-->
<section
  class="ask border-border bg-muted/40 flex flex-col gap-1.5 border-b px-3 py-2 text-xs"
  aria-labelledby="crash-reports-ask-title"
  data-slot="crash-reports-ask"
>
  <h2 id="crash-reports-ask-title" class="text-foreground m-0 text-xs font-semibold">Send crash reports?</h2>
  <p class="text-muted-foreground m-0" data-slot="crash-reports-ask-body">
    If bridgewatch crashes, it can send Distronode a report: the error, where in the code it happened, and the
    app and system versions. Your user name is taken out of file paths. Nothing is sent unless you choose Send,
    and you can change this in Settings.
  </p>
  <div class="flex gap-1.5">
    <Button size="xs" onclick={() => onanswer(false)} data-answer="dont-send">Don't send</Button>
    <Button size="xs" variant="outline" onclick={() => onanswer(true)} data-answer="send">Send</Button>
  </div>
</section>
