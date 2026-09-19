<script lang="ts">
  import type { Edit, Provider } from "../../lib/types";
  import { inapplicableNote, type RegistryEntry } from "../../lib/settings/registry";
  import { linesToList, set, stringList, unset } from "../../lib/settings/values";
  import { CHECK, FIELD_LABEL, FIELD_ROW, HINT, INPUT, SELECT, TEXTAREA } from "./styles";

  interface Props {
    entry: RegistryEntry;
    /** The concrete path, wildcards already filled in. */
    path: string;
    value: unknown;
    /** `select` entries whose options come from the document, e.g. accounts. */
    options?: string[];
    /** Resolves once the core has answered AND the document has been re-read. */
    onedit: (edits: Edit[]) => void | Promise<void>;
    /** The provider of the account this key belongs to, when there is one. */
    provider?: Provider | null;
  }

  let { entry, path, value, options, onedit, provider = null }: Props = $props();

  /**
   * Set when the key means nothing for this account's provider. The field is
   * dimmed and says why, but stays enabled: it is still in the file, and a
   * value carried over from another account has to be visible to be cleared.
   */
  const inapplicable = $derived(inapplicableNote(entry, provider));

  const choices = $derived(options ?? entry.options ?? []);

  type Control = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;

  function control(event: Event): Control {
    return event.currentTarget as Control;
  }

  /**
   * Put the control back in step with the FILE once the edit has settled.
   *
   * ⛔ A refused edit leaves `value` exactly as it was, so there is nothing for
   * Svelte to re-render and the control goes on showing a value the file does
   * not contain — which the next save of any other field then reads back as
   * though it had been accepted. Re-asserting the element after the round trip
   * is a no-op when the edit landed (the prop is already the new value by then)
   * and is the only thing that corrects it when it did not.
   *
   * ⚠ The element is captured by the CALLER, synchronously: `event.currentTarget`
   * is null once dispatch is over, which is before this ever resumes.
   */
  async function settle(element: Control, edits: Edit[]) {
    await onedit(edits);
    reset(element);
  }

  function reset(element: Control) {
    if (element instanceof HTMLInputElement && element.type === "checkbox") {
      element.checked = value === true;
      return;
    }
    element.value = entry.control === "list" ? asLines : asText;
  }

  function commitText(event: Event) {
    const element = control(event);
    const v = element.value;
    if (v !== "") {
      void settle(element, [set(path, v)]);
      return;
    }
    // An empty optional key is REMOVED rather than written as "". `only_when`
    // and `verdict.script` are Option<String> in the core, and `""` is a
    // different thing from absent: an empty `only_when` would match no status
    // at all and silently stop every dive.
    //
    // ⛔ Unless the registry says otherwise. For a key whose DEFAULT is not
    // absence — `dive.bridges` defaults to `"*"` — removing it reinstates the
    // default, so clearing a field hinted `"" is none` dived into every bridge.
    void settle(element, [entry.emptyMeans === "empty" ? set(path, "") : unset(path)]);
  }

  function commitNumber(event: Event) {
    const element = control(event);
    const raw = element.value;
    const n = Number(raw);
    if (raw === "" || !Number.isFinite(n)) {
      // Nothing is written, so the file has not changed: show what it says
      // rather than leaving a number the core has never seen.
      reset(element);
      return;
    }
    void settle(element, [set(path, Math.trunc(n))]);
  }

  function commitBoolean(event: Event) {
    const element = control(event) as HTMLInputElement;
    void settle(element, [set(path, element.checked)]);
  }

  function commitSelect(event: Event) {
    const element = control(event) as HTMLSelectElement;
    // An empty option is "inherit" (a per-watch override): remove the key.
    void settle(element, [element.value === "" ? unset(path) : set(path, element.value)]);
  }

  function commitList(event: Event) {
    const element = control(event);
    void settle(element, [{ op: "set", path, value: stringList(linesToList(element.value)) }]);
  }

  function commitProject(event: Event) {
    const element = control(event);
    const raw = element.value.trim();
    if (raw === "") {
      // A watch with no project 404s on every tick; the core has no default to
      // fall back to, so the old value stands.
      reset(element);
      return;
    }
    // A numeric id is the cheapest and most stable form, so it is preferred
    // whenever the field is all digits; anything else is a group/path.
    void settle(element, [set(path, /^\d+$/.test(raw) ? Number(raw) : raw)]);
  }

  const asText = $derived(value === null || value === undefined ? "" : String(value));
  const asLines = $derived(Array.isArray(value) ? value.join("\n") : "");
</script>

<label class={["field", FIELD_ROW, inapplicable && "opacity-60"]} data-inapplicable={inapplicable ? "true" : undefined}>
  <span class={["label", FIELD_LABEL]}>{entry.label}</span>

  {#if entry.control === "boolean"}
    <input type="checkbox" class={["mt-1.5 justify-self-start", CHECK]} checked={value === true} onchange={commitBoolean} />
  {:else if entry.control === "number"}
    <input type="number" class={INPUT} value={asText} onchange={commitNumber} />
  {:else if entry.control === "select"}
    <select class={SELECT} value={asText} onchange={commitSelect}>
      {#each choices as choice (choice)}
        <option value={choice}>{choice === "" ? "(inherit)" : choice}</option>
      {/each}
    </select>
  {:else if entry.control === "list"}
    <textarea class={TEXTAREA} rows="3" value={asLines} onchange={commitList}></textarea>
  {:else if entry.control === "textarea"}
    <textarea class={TEXTAREA} rows="8" value={asText} onchange={commitText}></textarea>
  {:else if entry.control === "project"}
    <input type="text" class={INPUT} value={asText} onchange={commitProject} />
  {:else}
    <input type="text" class={INPUT} value={asText} onchange={commitText} />
  {/if}

  {#if inapplicable}
    <span class={["provider-note col-start-2", HINT]} data-slot="provider-note">{inapplicable}</span>
  {/if}
  {#if entry.hint}
    <span class={["hint col-start-2", HINT]}>{entry.hint}</span>
  {/if}
</label>
