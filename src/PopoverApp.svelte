<script lang="ts">
  import { onMount } from "svelte";
  import Popover from "./components/Popover.svelte";
  import {
    getSnapshot,
    getStatus,
    hidePopover,
    inTauri,
    onSnapshot,
    openExternal,
    openSettings,
    openWizard,
    pipelinesUrl,
    popoverOpened,
    quit,
    refreshNow,
    resizePopover,
  } from "./lib/ipc";
  import type { Snapshot, Status } from "./lib/types";

  // An empty snapshot rather than a null one: the popover renders the same way
  // before the first tick as it does when every watch matched nothing, and a
  // separate loading state would be a second layout to keep correct.
  let snapshot = $state<Snapshot>({
    icon_state: "unknown",
    watches: [],
    errors: [],
    last_poll: new Date().toISOString(),
    request_log: [],
  });
  let status = $state<Status | null>(null);
  let now = $state(Date.now());

  onMount(() => {
    if (!inTauri()) return;

    let stop = false;
    const unlisten = onSnapshot((s) => (snapshot = s));

    void getSnapshot().then((s) => (snapshot = s));
    const refreshStatus = () => void getStatus().then((s) => (status = s));
    refreshStatus();

    /**
     * Whether anybody can see this window.
     *
     * ⛔ The shell hides the popover the moment it loses focus
     * (`WindowEvent::Focused(false)`), so for the overwhelming majority of the
     * time this window exists it is invisible — and the timer below went on
     * calling `get_status` once a second anyway, for the life of the process,
     * to update a clock nobody was reading.
     *
     * ⚠ Tracked as a flag rather than read from `document.hasFocus()` at each
     * tick: the two signals disagree across platforms, and a flag seeded true
     * fails in the harmless direction — a popover that keeps ticking — rather
     * than one that has gone quiet while on screen.
     */
    let visible = document.visibilityState !== "hidden";

    // One clock for every relative age on screen, ticked once a second. Each
    // component reading Date.now() itself would make a frame disagree with
    // itself by up to a second, which shows up as two rows the same age
    // rendering differently.
    const timer = setInterval(() => {
      if (stop || !visible) return;
      now = Date.now();
      refreshStatus();
      // Catches anything the click handler and the snapshot effect miss: a
      // relative age growing a character, a font finishing loading.
      measureAndResize();
    }, 1000);

    /** Everything a tick does, at the moment the window comes back. */
    function wake() {
      visible = true;
      now = Date.now();
      refreshStatus();
      measureAndResize();
    }

    // The window is shown by the Rust side, which also asks for the poll; this
    // covers the case where it is already visible and regains focus.
    const onFocus = () => {
      wake();
      void popoverOpened();
    };
    const onBlur = () => (visible = false);
    const onVisibility = () => {
      if (document.visibilityState === "hidden") visible = false;
      else wake();
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    document.addEventListener("visibilitychange", onVisibility);

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void hidePopover();
    };
    window.addEventListener("keydown", onKey);

    // A resize on the next frame, so a click that opened a disclosure is
    // measured after the DOM has been laid out again rather than before.
    const scheduleResize = () => requestAnimationFrame(measureAndResize);
    document.addEventListener("click", scheduleResize);
    measureAndResize();

    return () => {
      stop = true;
      clearInterval(timer);
      document.removeEventListener("click", scheduleResize);
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("keydown", onKey);
      void unlisten.then((f) => f());
    };
  });

  /**
   * Ask the shell for the window this content needs.
   *
   * ⛔ `document.documentElement.scrollHeight` is the obvious measurement and
   * it is WRONG here: the popover is `height: 100vh`, so that number is always
   * exactly the current window height and the window can never shrink or grow.
   * The natural height is the chrome plus the LIST's scroll height, which is
   * what this reconstructs — and unlike the element's own height it is not
   * clamped by the window it is trying to size.
   */
  let lastHeight = 0;

  function measureAndResize() {
    const root = document.querySelector<HTMLElement>("[data-popover]");
    const list = document.querySelector<HTMLElement>("[data-scroll]");
    if (!root || !list) return;
    const chrome = root.getBoundingClientRect().height - list.clientHeight;
    const natural = Math.ceil(chrome + list.scrollHeight);
    // A pixel of hysteresis: sub-pixel layout would otherwise send a command
    // every animation frame forever.
    if (natural > 0 && Math.abs(natural - lastHeight) > 1) {
      lastHeight = natural;
      void resizePopover(natural);
    }
  }

  // Re-measure whenever the frame changes.
  $effect(() => {
    void snapshot;
    if (inTauri()) requestAnimationFrame(measureAndResize);
  });

  async function openPipelines() {
    const url = await pipelinesUrl();
    if (url) await openExternal(url);
  }
</script>

<Popover
  {snapshot}
  {status}
  {now}
  onrefresh={() => void refreshNow()}
  onsettings={() => void openSettings()}
  onpipelines={() => void openPipelines()}
  onquit={() => void quit()}
  onfix={() => void openSettings("text")}
  onsetup={() => void openWizard()}
/>
