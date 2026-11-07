/**
 * bridgewatch's own token entry, as the shell describes it (`own_token_status`).
 *
 * ⛔ Nothing here decides anything: whether a stored token is sent is the
 * core's rule (`token::resolve`), and the shell reads the account's
 * `base_url` from the file on disk. This only turns its answer into words.
 */

/** `token::OwnTokenStatus`. Never carries the token. */
export type OwnTokenStatus =
  | { kind: "none" }
  | { kind: "legacy" }
  | { kind: "bound"; origin: string }
  | { kind: "mismatch"; stored: string; expected: string | null };

/** What the token field says about the stored entry, and whether it is a problem. */
export function ownTokenText(status: OwnTokenStatus, instance: string): { text: string; alert: boolean } {
  switch (status.kind) {
    case "none":
      return { text: "No token is stored for this account yet.", alert: false };
    case "legacy":
      return {
        text: `A token saved by bridgewatch 1.x is stored. It is tied to ${instance} the first time it is used.`,
        alert: false,
      };
    case "bound":
      return { text: `Stored for ${status.origin}.`, alert: false };
    case "mismatch": {
      const expected = status.expected ?? "this account's address";
      return {
        text: `Stored for ${status.stored}; paste one for ${expected}. The stored token is not sent there.`,
        alert: true,
      };
    }
  }
}
