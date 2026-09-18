/**
 * Tailwind class strings for the Settings window's NATIVE controls.
 *
 * The form keeps real `<input>`, `<select>` and `<textarea>` elements rather
 * than the ui kit's bits-ui Select/Checkbox: every control commits on `change`
 * and is re-asserted from the file after the core answers (see Field.svelte),
 * which is exactly the contract a native element has and a headless widget
 * does not. These strings give them Rhea's look (the kit's own `Input` uses
 * the same tokens), so they sit next to ui `Button`s without a seam.
 *
 * ⚠ Whole literal strings, so Tailwind's scanner sees every class.
 */

const CONTROL =
  "bg-input/50 border-transparent focus-visible:border-ring focus-visible:ring-ring/30 rounded-xl border text-[13px] text-foreground outline-none transition-[color,box-shadow] focus-visible:ring-3 placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50";

export const INPUT = `${CONTROL} h-7 w-full min-w-0 px-2.5`;

export const SELECT = `${CONTROL} h-7 w-full min-w-0 px-2`;

export const TEXTAREA = `${CONTROL} w-full min-w-0 resize-y px-2.5 py-1.5 font-mono text-xs leading-relaxed`;

export const CHECK = "accent-primary size-4 cursor-pointer";

/** A labelled row: label column on the left, control on the right. */
export const FIELD_ROW = "grid grid-cols-[170px_1fr] items-start gap-x-3 gap-y-1 mb-2.5";

export const FIELD_LABEL = "text-muted-foreground pt-1 text-right text-[13px]";

export const HINT = "text-muted-foreground text-[11px]";
