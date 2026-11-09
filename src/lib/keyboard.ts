/**
 * Keyboard navigation in the popover.
 *
 * Tab already reaches every link and button, but a popover with a dozen rows
 * and their job tools is a long way to Tab. These keys move between pipeline
 * ROWS, landing on the row's commit link, where Enter opens the pipeline:
 *
 *   ↓ or j      the next row        ↑ or k    the previous row
 *   Home        the first row       End       the last row
 *   r           refresh now         Escape    hide (handled by the caller)
 *
 * The first ↓ or j with nothing in the list focused lands on the first row,
 * and the first ↑ or k on the last.
 */

/** What a key press asks for. */
export type PopoverKeyAction = { kind: "focus"; target: HTMLElement } | { kind: "refresh" } | null;

/** The rows the keys move between, in the order they are drawn. */
export function navigableRows(root: ParentNode): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>("[data-scroll] .row")];
}

/** Where focus goes in a row: its commit link, else the first link or button, else the row. */
export function rowTarget(row: HTMLElement): HTMLElement {
  return row.querySelector<HTMLElement>("a.sha[href]") ?? row.querySelector<HTMLElement>("a[href], button") ?? row;
}

/**
 * The action for a key press, or null to leave it to the browser.
 *
 * ⛔ Never with Ctrl, Alt or Meta held (those are the system's and the
 * webview's), and never while a text field has focus, so typing a `j` or an
 * `r` into one stays typing.
 */
export function popoverKey(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "altKey" | "metaKey">,
  rows: HTMLElement[],
  active: Element | null,
): PopoverKeyAction {
  if (event.ctrlKey || event.altKey || event.metaKey) return null;
  if (active instanceof HTMLElement && (active.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(active.tagName))) {
    return null;
  }
  if (event.key === "r") return { kind: "refresh" };

  const step: Record<string, number> = { ArrowDown: 1, j: 1, ArrowUp: -1, k: -1 };
  if (!(event.key in step) && event.key !== "Home" && event.key !== "End") return null;
  if (rows.length === 0) return null;

  const current = rows.findIndex((row) => active !== null && row.contains(active));
  let next: number;
  if (event.key === "Home") next = 0;
  else if (event.key === "End") next = rows.length - 1;
  else if (current === -1) next = step[event.key] > 0 ? 0 : rows.length - 1;
  else next = Math.min(rows.length - 1, Math.max(0, current + step[event.key]));
  return { kind: "focus", target: rowTarget(rows[next]) };
}
