/**
 * Tailwind class strings for the Settings window's NATIVE controls and rows.
 *
 * The form keeps real `<input>`, `<select>` and `<textarea>` elements rather
 * than the ui kit's bits-ui Select/Checkbox: every control commits on `change`
 * and is re-asserted from the file after the core answers (see Field.svelte),
 * which is exactly the contract a native element has and a headless widget
 * does not. These strings give them Rhea's look (the kit's own `Input` uses
 * the same tokens), so they sit next to ui `Button`s without a seam.
 *
 * Sizes and gaps follow the app's tokens (`app.css`): 13/18 body, 12/16 hints,
 * 12px between fields, 16px card padding, 24px between sections.
 *
 * ⚠ Whole literal strings, so Tailwind's scanner sees every class.
 */

const CONTROL =
  "bg-input/50 border-transparent focus-visible:border-ring focus-visible:ring-ring/30 rounded-xl border text-ui-body text-foreground outline-none transition-[color,box-shadow] focus-visible:ring-3 placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50";

export const INPUT = `${CONTROL} h-7 w-full min-w-0 px-2.5`;

export const SELECT = `${CONTROL} h-7 w-full min-w-0 px-2`;

export const TEXTAREA = `${CONTROL} w-full min-w-0 resize-y px-2.5 py-1.5 font-mono text-ui-meta`;

export const CHECK = "accent-primary size-4 cursor-pointer";

/**
 * Control widths BY TYPE. A number is a few digits, a select its longest
 * option; only free text, lists and scripts get the row's whole width, on a
 * line of their own under the label.
 */
export const NUMBER_INPUT = `${CONTROL} h-7 w-24 min-w-0 shrink-0 px-2.5`;
export const SHORT_SELECT = `${CONTROL} h-7 w-48 min-w-0 max-w-full shrink-0 px-2`;
/** The new-account form's provider: two short words. */
export const PROVIDER_SELECT = `${CONTROL} h-7 w-32 min-w-0 shrink-0 px-2`;
/** A long control: on its own line under the label. */
export const WIDTH_FULL = "basis-full";

/** A group of rows: a card, one hairline between rows. */
export const CARD = "bg-card border-border divide-border overflow-hidden rounded-xl border divide-y";

/**
 * One row of a card: the label and its hint on the left, a short control on
 * the right, and a long one wrapped below at the full width.
 */
export const ROW = "flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3";

/** The row's text column. It grows; below about 14rem the control wraps under it. */
export const ROW_TEXT = "flex min-w-0 flex-1 basis-56 flex-col gap-1";

export const FIELD_LABEL = "text-ui-body text-foreground";

/** About 60 characters a line: longer lines are hard to read back. */
export const HINT = "text-muted-foreground text-ui-meta max-w-[60ch]";

/**
 * A group's title above its card: 12px, muted. Quieter than the thing it
 * groups (an account's name is 13px semibold), so the levels read apart.
 */
export const SECTION_TITLE = "text-ui-meta text-muted-foreground m-0 font-medium";
