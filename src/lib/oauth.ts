/**
 * "Sign in with GitHub" / "Sign in with GitLab": the wire shapes of the
 * shell's `oauth_*` commands (src-tauri/src/oauth.rs) and the interface the
 * wizard and Settings are handed, so the tests can hand them a fake.
 *
 * ⛔ Nothing here ever holds a token. The webview sees the user code (which
 * it must show), the verification page, and afterwards who signed in and
 * until when.
 */

import type { Provider } from "./types";

/** `oauth::Availability`: whether to offer "Sign in" for an instance. */
export interface OAuthAvailability {
  /** A client id is typed, or this build has one for the host. */
  available: boolean;
  /** This build has an application for the host. */
  builtin: boolean;
  /** GitHub Enterprise Server or self-managed GitLab: only a typed client id will do. */
  needs_client_id: boolean;
  /** `github.com`, for the button. */
  host: string;
  /** Where to install the GitHub App, when this build knows. */
  install_url: string | null;
}

/** What to sign in to, and under which account name to keep it. */
export interface SignInRequest {
  account: string;
  provider: Provider;
  base_url: string;
  /** Absent or null: the host's built-in application. */
  client_id?: string | null;
}

/** `StartedSignIn`: what to show while waiting. */
export interface StartedSignIn {
  id: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  host: string;
}

/** `SignedIn`: who, and until when. */
export interface SignedIn {
  login: string | null;
  scopes: string[];
  /** Unix seconds. */
  expires_at: number | null;
}

/** `SignInStatus`: a stored sign-in, described. */
export interface SignInStatus {
  login: string | null;
  expires_at: number | null;
  refresh_expires_at: number | null;
  scopes: string[];
  obtained_at: number;
  can_refresh: boolean;
}

/** `SignInFailure`: what every rejected sign-in call rejects with. */
export interface SignInFailure {
  kind: "expired" | "denied" | "cancelled" | "no_client_id" | "sign_in_again" | "error";
  message: string;
}

/** A progress event from the shell while it waits. */
export type SignInProgress =
  | { id: string; state: "waiting"; polls: number }
  | { id: string; state: "slowed_down"; interval: number };

/** Everything the sign-in UI needs from the shell. */
export interface OAuthApi {
  availability(provider: Provider, baseUrl: string, clientId?: string | null): Promise<OAuthAvailability>;
  start(request: SignInRequest): Promise<StartedSignIn>;
  /** Resolves when the user approves; rejects with a `SignInFailure`. */
  wait(id: string): Promise<SignedIn>;
  cancel(id: string): Promise<void>;
  /** Open the verification page (the shell trusts its host for that one open). */
  openVerification(id: string): Promise<void>;
  /** Open the GitHub App's install page. */
  openInstall(provider: Provider, baseUrl: string): Promise<void>;
  status(account: string): Promise<SignInStatus | null>;
  signOut(account: string): Promise<void>;
  copy(text: string): Promise<boolean>;
  /** Optional: progress while waiting. Returns an unsubscribe. */
  onProgress?(handler: (progress: SignInProgress) => void): Promise<() => void>;
}

/** The instance root an account talks to when its file does not say. */
export function defaultBaseUrl(provider: Provider): string {
  return provider === "github" ? "https://api.github.com" : "https://gitlab.com";
}

/** "GitHub" or "GitLab", for a button. */
export function providerName(provider: Provider): string {
  return provider === "github" ? "GitHub" : "GitLab";
}

/** A rejection's kind, when it is a `SignInFailure`. */
export function failureOf(error: unknown): SignInFailure {
  if (typeof error === "object" && error !== null && typeof (error as { message?: unknown }).message === "string") {
    const kind = (error as { kind?: unknown }).kind;
    return {
      kind: (typeof kind === "string" ? kind : "error") as SignInFailure["kind"],
      message: (error as { message: string }).message,
    };
  }
  return { kind: "error", message: typeof error === "string" ? error : "The sign-in did not finish." };
}

/** The sentence for a failure, in plain words. */
export function failureSentence(failure: SignInFailure, host: string): string {
  switch (failure.kind) {
    case "expired":
      return "The code expired before it was entered. Start again.";
    case "denied":
      return `You cancelled the sign-in on ${host}.`;
    case "cancelled":
      return "Sign-in stopped.";
    default:
      return failure.message;
  }
}

/** A verification URL without its scheme, for a button: `github.com/login/device`. */
export function shortUrl(url: string): string {
  return url.replace(/^https?:\/\//i, "").replace(/\/$/, "");
}
