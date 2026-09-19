//! Resolving a token from the source named in the configuration.
//!
//! bridgewatch never stores a token in its config file and never logs one. A
//! token exists in memory as a [`Secret`], whose `Debug` and `Display` both
//! print `<redacted>`, so it cannot reach a log line by accident.
//!
//! # The empty keyring user
//!
//! `glab` stores its token under service `glab:gitlab.com:token` with an
//! **empty** account on the macOS Keychain and an empty `username` attribute on
//! the Linux Secret Service. Reusing that entry is the whole point of the
//! `keyring` source, so an empty `user` has to work.
//!
//! It does not work through the crate. Measured against `keyring` 4.2.0 on
//! macOS, `Entry::new(service, "")` returns `Err(Invalid("user", "cannot be
//! empty"))` **before** it touches the credential store at all, so there is no
//! store-level call to fall back to — the rejection is in the client layer.
//! [`SystemTokenProvider::keyring_get`] therefore shells out:
//!
//! - **macOS**: `security find-generic-password -s <service> -a "" -w`.
//!   Verified to return immediately with exit 44 and no GUI prompt for a
//!   service that does not exist.
//! - **Linux**: `secret-tool lookup service <service> username ""`, when
//!   `secret-tool` is on `PATH`. This mirrors the attributes `glab` writes;
//!   it could not be verified from the macOS box this was written on, so it is
//!   best-effort and falls through to a clear [`TokenError::NoEntry`].
//!
//! Neither fallback runs unless `user` is empty, so the ordinary path is still
//! the crate's.
//!
//! ⛔ **The service string is the only thing that is provider-specific here,
//! and `-a ""` is why an empty user can be taken at its word.** `gh` writes
//! TWO keychain items under `gh:github.com`: one whose account is the login,
//! and one whose account is EMPTY, which is the *active account* slot
//! `gh auth switch` moves. `security find-generic-password` **without `-a`**
//! returns the named item, so `user = ""` against that service used to read the
//! per-user slot rather than the active one, the same token today, and a
//! different one the moment somebody switches accounts, with nothing to say
//! which had been read. `go-keyring` itself passes `-a ""`; so does this now,
//! which makes the empty user mean what the configuration says it means. glab
//! writes only the empty-account item, so its lookup is unaffected.
//!
//! # The go-keyring envelope
//!
//! ⛔ **What `glab` stores is not the token.** `glab` uses
//! [`zalando/go-keyring`](https://github.com/zalando/go-keyring), whose macOS
//! backend base64-encodes **every** password before calling
//! `security add-generic-password`, because a secret with a newline or a
//! non-ASCII byte comes back hex-mangled otherwise. Read v0.2.8's
//! `keyring_darwin.go`: `Set` unconditionally writes
//! `go-keyring-base64:` + `base64::StdEncoding` of the value, and `Get` unwraps
//! that prefix, or the legacy `go-keyring-encoded:` + lowercase hex.
//!
//! So `security find-generic-password -s glab:gitlab.com:token -w` returns a
//! 102-character string beginning `go-keyring-base64:`, and sending it as a
//! `PRIVATE-TOKEN` header is a 401 — a well-formed header carrying the wrong
//! bytes, which is the failure that looks most like a scope problem and is not.
//! [`decode_keyring_envelope`] unwraps it after **every** keyring read, the
//! `keyring`-crate path included: a go-keyring writer may have used a non-empty
//! user, and the envelope is a property of the writer, not of the address.

use crate::config::TokenSource;

/// A token, wrapped so it cannot be printed by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a string as a secret.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the token. The only way to see the value; call it as late as
    /// possible, ideally straight into a header.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// True when the resolved token is empty, which almost always means the
    /// source pointed at nothing rather than that the token is legitimately "".
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Why a token could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// The credential store has no entry at that address.
    #[error("no credential found for service {service:?} user {user:?}")]
    NoEntry {
        /// The service that was looked up.
        service: String,
        /// The user that was looked up.
        user: String,
    },
    /// The credential store itself failed.
    #[error("credential store error for service {service:?}: {message}")]
    Store {
        /// The service that was looked up.
        service: String,
        /// What the platform said.
        message: String,
    },
    /// The named environment variable is unset or empty.
    #[error("environment variable {0} is not set")]
    MissingEnv(String),
    /// The token command could not be run or failed.
    #[error("token command {command:?} failed: {message}")]
    Command {
        /// The command line that was attempted.
        command: String,
        /// What went wrong.
        message: String,
    },
    /// The source resolved, but to nothing usable.
    #[error("token source resolved to an empty string")]
    Empty,
    /// The resolved value cannot be sent as an HTTP header.
    #[error(
        "the resolved token contains {character}, which cannot go in an HTTP header \
         (the value is {length} bytes). A command source is already read one line at a \
         time, so this is the line itself: check the credential store entry, or that the \
         program prints the token FIRST rather than a banner"
    )]
    Malformed {
        /// The first offending character, debug-quoted. Never the token.
        character: String,
        /// How long the resolved value was, which is usually the giveaway.
        length: usize,
    },
    /// The configuration selected no source at all.
    #[error("no token source configured for account {0:?}")]
    NotConfigured(String),
    /// The account signs in, and nothing is stored for it.
    #[error(
        "account {0:?} is not signed in: sign in from Settings, or run \
         bridgewatch auth login --account {0}"
    )]
    NotSignedIn(String),
    /// The credential carries a `go-keyring-*` envelope that would not decode.
    #[error(
        "the credential is wrapped in a {envelope:?} envelope that will not decode: {message}. \
         That prefix is written by zalando/go-keyring (which glab uses); a value carrying it \
         but failing to decode has been truncated or edited"
    )]
    Envelope {
        /// The envelope prefix that was found. Never the payload.
        envelope: String,
        /// Why it would not decode.
        message: String,
    },
}

/// The side effects token resolution needs, behind a trait so the selection
/// logic can be tested without touching a real credential store.
///
/// Reading a live credential can raise a GUI prompt on macOS; the unit tests
/// therefore drive a fake implementation and never the system one.
pub trait TokenProvider: Send + Sync {
    /// Read a credential from the OS store. `user` may be the empty string.
    fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError>;

    /// Write a credential to the OS store.
    fn keyring_set(&self, service: &str, user: &str, secret: &str) -> Result<(), TokenError>;

    /// Delete a credential from the OS store. Deleting an absent entry is Ok.
    fn keyring_delete(&self, service: &str, user: &str) -> Result<(), TokenError>;

    /// Read an environment variable.
    fn env(&self, name: &str) -> Option<String>;

    /// Run a program and return its stdout.
    fn run(&self, argv: &[String]) -> Result<String, TokenError>;
}

/// The real provider: the OS credential store, the process environment, and
/// `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTokenProvider;

impl SystemTokenProvider {
    /// The keyring user bridgewatch uses for its own entries.
    ///
    /// Deliberately not the empty string: an empty account is needed only to
    /// *read* another tool's entry, and using a real one for our own writes
    /// keeps every platform on the well-trodden path.
    pub const OWN_USER: &'static str = "token";

    /// The service name for bridgewatch's own entry for an account.
    pub fn own_service(account: &str) -> String {
        format!("bridgewatch:{account}")
    }
}

impl TokenProvider for SystemTokenProvider {
    fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError> {
        // `keyring` rejects an empty user in its own client layer, so for the
        // entries other tools actually wrote there is nothing to try first.
        let raw = if user.is_empty() {
            empty_user_lookup(service).ok_or_else(|| TokenError::NoEntry {
                service: service.to_string(),
                user: String::new(),
            })?
        } else {
            keyring::Entry::new(service, user)
                .and_then(|e| e.get_password())
                .map_err(|e| classify_keyring_error(e, service, user))?
        };
        // Whoever wrote the credential decides whether it is enveloped, so this
        // runs on both read paths.
        decode_keyring_envelope(&raw)
    }

    fn keyring_set(&self, service: &str, user: &str, secret: &str) -> Result<(), TokenError> {
        if user.is_empty() {
            // bridgewatch only ever reads another tool's empty-account entry; it
            // writes its own with `OWN_USER`, so refusing here keeps the write
            // path on the supported API rather than shelling out to create a
            // credential shape we would then have to own.
            return Err(TokenError::Store {
                service: service.to_string(),
                message: "bridgewatch will not write a credential with an empty user; \
                          use token = { own = true }, which stores it under the user \
                          \"token\""
                    .to_string(),
            });
        }
        keyring::Entry::new(service, user)
            .and_then(|e| e.set_password(secret))
            .map_err(|e| classify_keyring_error(e, service, user))
    }

    fn keyring_delete(&self, service: &str, user: &str) -> Result<(), TokenError> {
        if user.is_empty() {
            return Ok(());
        }
        match keyring::Entry::new(service, user).and_then(|e| e.delete_credential()) {
            Ok(()) => Ok(()),
            Err(e) => match classify_keyring_error(e, service, user) {
                TokenError::NoEntry { .. } => Ok(()),
                other => Err(other),
            },
        }
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn run(&self, argv: &[String]) -> Result<String, TokenError> {
        run_bounded(argv, COMMAND_TIMEOUT, MAX_STDOUT_BYTES)
    }
}

/// How long a token command may take before it is killed.
///
/// ⛔ There was no limit at all, and the token command runs on the poll task
/// before its `select!`: `pass`, `gpg` or any helper that waits on a pinentry
/// the user never sees parks the whole tray — no icon, no refresh, no reload —
/// for as long as the prompt sits there, which is forever on a headless
/// session. Ten seconds is far longer than a credential lookup and far shorter
/// than a poll interval.
pub const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How much of a token command's stdout is kept.
///
/// The token is on the first line; the rest is somebody's help text. Keeping a
/// bounded prefix means a program that prints megabytes cannot be answered with
/// megabytes of allocation.
pub const MAX_STDOUT_BYTES: usize = 64 * 1024;

/// How much of a failing command's stderr is quoted back in the error.
///
/// ⚠ The error is rendered in the popover and in `bridgewatch check` output.
/// The whole of stderr went in, which for a GPG failure is dozens of lines of
/// key ids and paths in a UI with no scrollback.
const MAX_STDERR_CHARS: usize = 300;

/// Run a token command with explicit bounds, returning its stdout.
///
/// [`SystemTokenProvider::run`] is this with bridgewatch's own limits;
/// the bounds are parameters so the tests can prove the timeout without
/// waiting for it.
///
/// ⚠ `stdin` is `/dev/null` on purpose. A helper that tries to prompt on the
/// terminal it inherited would otherwise sit waiting for a person who is
/// looking at a menu bar, not at a shell; with no stdin it fails fast and says
/// so.
pub fn run_bounded(
    argv: &[String],
    timeout: std::time::Duration,
    max_stdout: usize,
) -> Result<String, TokenError> {
    use std::process::{Command, Stdio};

    let Some((program, args)) = argv.split_first() else {
        return Err(TokenError::Command {
            command: String::new(),
            message: "empty command".into(),
        });
    };
    let failed = |message: String| TokenError::Command {
        command: argv.join(" "),
        message,
    };

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(e.to_string()))?;

    // Both pipes are drained on their own threads. Reading them in sequence
    // deadlocks the moment a program writes more to the pipe we are not reading
    // than its buffer holds, and a program that writes a lot to stderr is
    // exactly the one whose stdout we are trying to bound.
    //
    // ⛔ And the threads are never JOINED. A pipe reaches EOF when every holder
    // of its write end has closed it, not when the child exits: a helper that
    // starts an agent, or any stray `&`, leaves a grandchild holding stdout,
    // and a join waited for that grandchild however long it lived — deadline or
    // no deadline. Each drain fills a shared buffer and reports EOF on a
    // channel; the wait for EOF is bounded by the same deadline as the child.
    let stdout = Drain::start(child.stdout.take(), max_stdout);
    // Four bytes per char is UTF-8's ceiling, so this cannot cut short of the
    // characters the error is allowed to show.
    let stderr = Drain::start(child.stderr.take(), MAX_STDERR_CHARS * 4);

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| failed(e.to_string()))? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!(
                    "timed out after {}s and was killed. A token command must return without \
                     waiting for anybody: a pinentry or a passphrase prompt has nowhere to \
                     appear when bridgewatch runs it",
                    timeout.as_secs_f32().round()
                )));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };

    // Whatever the child wrote before exiting is already in the pipe, so once
    // it has exited a second is ample: waiting out the rest of the deadline
    // would charge every poll ten seconds for a grandchild that is never going
    // to close its copy. The floor covers a child that exited right at the
    // deadline.
    let now = std::time::Instant::now();
    let settle = (now + std::time::Duration::from_secs(1))
        .min(deadline)
        .max(now + std::time::Duration::from_millis(200));
    let (out, out_complete) = stdout.finish(settle);
    let (err, _) = stderr.finish(settle);

    if !status.success() {
        let text = String::from_utf8_lossy(&err);
        let text = text.trim();
        let quoted: String = text.chars().take(MAX_STDERR_CHARS).collect();
        let elided = if quoted.len() < text.len() {
            " […]"
        } else {
            ""
        };
        return Err(failed(format!(
            "exited with {}: {quoted}{elided}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
        )));
    }
    // A pipe still open after the deadline means something the command
    // started is holding it. The first line is the token and is safe to use
    // once its newline has arrived; without one, what was read may be half a
    // token, and sending half a token is a 401 that reads like a scope problem.
    if !out_complete && !out.contains(&b'\n') {
        return Err(failed(
            "exited, but a process it started kept its output open, and no complete line \
             had been printed by then"
                .into(),
        ));
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// One output pipe, read on its own thread into a bounded buffer.
struct Drain {
    kept: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    eof: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    fn start<R: std::io::Read + Send + 'static>(pipe: Option<R>, limit: usize) -> Self {
        let kept = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, eof) = std::sync::mpsc::channel();
        let sink = kept.clone();
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut kept) = sink.lock() {
                        let room = limit.saturating_sub(kept.len());
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    // Keep reading even when full: the child blocks on a full
                    // pipe, and a blocked child never exits.
                }
            }
            let _ = tx.send(());
        });
        Self { kept, eof }
    }

    /// What was read by `until`, and whether the pipe had reached EOF.
    ///
    /// A thread still reading is left to finish on its own; it exits when the
    /// last holder of the pipe does.
    fn finish(self, until: std::time::Instant) -> (Vec<u8>, bool) {
        let wait = until.saturating_duration_since(std::time::Instant::now());
        let complete = self.eof.recv_timeout(wait).is_ok();
        let kept = self.kept.lock().map(|k| k.clone()).unwrap_or_default();
        (kept, complete)
    }
}

/// Map a `keyring` error onto ours, collapsing every "it is not there" spelling
/// the crate has used across versions onto [`TokenError::NoEntry`].
fn classify_keyring_error(err: keyring::Error, service: &str, user: &str) -> TokenError {
    let message = err.to_string();
    let missing = matches!(err, keyring::Error::NoEntry)
        || message.to_ascii_lowercase().contains("no matching entry")
        || message.to_ascii_lowercase().contains("not found");
    if missing {
        TokenError::NoEntry {
            service: service.to_string(),
            user: user.to_string(),
        }
    } else {
        TokenError::Store {
            service: service.to_string(),
            message,
        }
    }
}

/// The prefix `zalando/go-keyring` writes on macOS today.
pub const GO_KEYRING_BASE64_PREFIX: &str = "go-keyring-base64:";

/// The prefix `zalando/go-keyring` wrote before it moved to base64.
pub const GO_KEYRING_HEX_PREFIX: &str = "go-keyring-encoded:";

/// Unwrap a `go-keyring-*` envelope, if there is one.
///
/// A value with neither prefix is returned trimmed and unchanged, so this is
/// safe to run over every credential whatever wrote it. A value that *has* a
/// prefix and will not decode is an error naming the envelope: passing the raw
/// bytes through would send a 102-character string as a `PRIVATE-TOKEN` header
/// and present as a 401, which reads like a scope problem rather than an
/// encoding one.
///
/// Whitespace is trimmed before the prefix is looked for, matching go-keyring's
/// own `strings.TrimSpace`, because `security -w` appends a newline.
///
/// ```
/// use bridgewatch_core::token::decode_keyring_envelope;
///
/// // What glab actually stores: base64 of "glpat-example".
/// assert_eq!(
///     decode_keyring_envelope("go-keyring-base64:Z2xwYXQtZXhhbXBsZQ==\n").unwrap(),
///     "glpat-example"
/// );
/// // Anything else is passed through, trimmed.
/// assert_eq!(decode_keyring_envelope("  glpat-example \n").unwrap(), "glpat-example");
/// ```
pub fn decode_keyring_envelope(raw: &str) -> Result<String, TokenError> {
    use base64::Engine as _;

    let trimmed = raw.trim();

    let (envelope, bytes) = if let Some(payload) = trimmed.strip_prefix(GO_KEYRING_BASE64_PREFIX) {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|e| TokenError::Envelope {
                envelope: GO_KEYRING_BASE64_PREFIX.to_string(),
                message: e.to_string(),
            })?;
        (GO_KEYRING_BASE64_PREFIX, decoded)
    } else if let Some(payload) = trimmed.strip_prefix(GO_KEYRING_HEX_PREFIX) {
        let decoded = hex::decode(payload).map_err(|e| TokenError::Envelope {
            envelope: GO_KEYRING_HEX_PREFIX.to_string(),
            message: e.to_string(),
        })?;
        (GO_KEYRING_HEX_PREFIX, decoded)
    } else {
        return Ok(trimmed.to_string());
    };

    // A token is text. Non-UTF-8 here means the payload is not what it claims,
    // and silently lossy-converting would produce a header that is well-formed
    // and wrong.
    let decoded = String::from_utf8(bytes).map_err(|e| TokenError::Envelope {
        envelope: envelope.to_string(),
        message: format!("decoded bytes are not valid UTF-8: {e}"),
    })?;
    Ok(decoded.trim().to_string())
}

/// Read a credential stored with an empty account, which the `keyring` crate
/// will not address. Returns `None` when there is nothing there.
fn empty_user_lookup(service: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    let command = ("security", security_lookup_args(service));
    #[cfg(target_os = "linux")]
    let command = ("secret-tool", secret_tool_lookup_args(service));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let command: (&str, Vec<&str>) = {
        let _ = service;
        return None;
    };

    let out = std::process::Command::new(command.0)
        .args(&command.1)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    (!secret.is_empty()).then_some(secret)
}

/// `security`'s arguments for an empty-account lookup of `service`.
///
/// ⛔ `-a ""` is the whole point and it is not decoration: see the module
/// documentation. Without it the lookup returns whichever item macOS finds
/// first for the service, which for `gh` is the per-user one and not the active
/// account it was asked for. Built on every platform so the argv is tested
/// where the tests run and not only on macOS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn security_lookup_args(service: &str) -> Vec<&str> {
    vec!["find-generic-password", "-s", service, "-a", "", "-w"]
}

/// `secret-tool`'s arguments for an empty-username lookup of `service`.
///
/// ⚠ Lo11. `secret-tool` parses its arguments with GOption, which reads any
/// token beginning with `-` as an option wherever it appears — including an
/// attribute VALUE. A service named `-weird` would be rejected as an unknown
/// option rather than looked up. `--` ends option parsing, and it is added only
/// for the services that need it: the ordinary call stays byte for byte what it
/// was, because the behaviour of `--` is reasoned from GLib's contract rather
/// than measured on a Secret Service. Built on every platform, so the argv is
/// tested where the tests run and not only on Linux.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn secret_tool_lookup_args(service: &str) -> Vec<&str> {
    if service.starts_with('-') {
        vec!["lookup", "--", "service", service, "username", ""]
    } else {
        vec!["lookup", "service", service, "username", ""]
    }
}

/// Resolve a token for one account.
///
/// `account` names the `[accounts.*]` entry and is used only by
/// [`TokenSource::Own`], which addresses `bridgewatch:<account>`.
pub fn resolve(
    source: &TokenSource,
    account: &str,
    provider: &dyn TokenProvider,
) -> Result<Secret, TokenError> {
    let raw = match source {
        TokenSource::Keyring { service, user } => provider.keyring_get(service, user)?,
        TokenSource::Env(name) => provider
            .env(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| TokenError::MissingEnv(name.clone()))?,
        // ⛔ M28. The first line, as the README has always promised and as every
        // password manager emits: `pass` prints the password then the metadata,
        // and `glab config get token` on 1.117.0 follows the value with 27
        // lines of help. Refusing multi-line output instead — and telling the
        // user to pipe through `head -1` — made the documented recipe fail.
        TokenSource::Command(argv) => first_line(&provider.run(argv)?),
        TokenSource::Own(true) => provider.keyring_get(
            &SystemTokenProvider::own_service(account),
            SystemTokenProvider::OWN_USER,
        )?,
        TokenSource::Own(false) => return Err(TokenError::NotConfigured(account.to_string())),
        // The access token as it was last stored, possibly expired. What
        // POLLS goes through `oauth::OAuthTransport`, which refreshes it; this
        // path serves a caller that wants one value now and can say "sign in
        // again" when it is refused.
        TokenSource::Oauth(_) => match crate::oauth::store::load(account, provider) {
            Ok(Some(set)) => set.access_token.expose().to_string(),
            Ok(None) => return Err(TokenError::NotSignedIn(account.to_string())),
            Err(e) => {
                return Err(TokenError::Store {
                    service: crate::oauth::store::service(account),
                    message: e.to_string(),
                });
            }
        },
    };
    // Also unwrap here, not only in the provider. The envelope is a property of
    // whatever WROTE the credential, and the obvious way to reuse glab's token
    // without the keyring source is
    // `token = { command = ["security", "find-generic-password", "-s", "...", "-w"] }`,
    // which returns the enveloped bytes verbatim. Decoding is idempotent: a
    // value with no prefix is returned trimmed and unchanged.
    let raw = decode_keyring_envelope(&raw)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TokenError::Empty);
    }
    // ⛔ A token that cannot be an HTTP header value must be refused here, not
    // at send time. reqwest reports it as `builder error`, with the real cause
    // two levels down the source chain, and it does so on every request
    // forever. The usual cause is a `command` source whose program prints more
    // than the token: `glab config get token` on 1.117.0, for one, follows the
    // value with 27 lines of help.
    if let Some(bad) = trimmed.chars().find(|c| c.is_control() || !c.is_ascii()) {
        return Err(TokenError::Malformed {
            character: format!("{bad:?}"),
            length: trimmed.len(),
        });
    }
    Ok(Secret::new(trimmed))
}

/// The first line of a command's output, trimmed.
///
/// Everything after the first newline is dropped rather than refused: a token
/// is one line, and a program that prints more is printing help, a warning on
/// stdout, or the metadata `pass` keeps under the password.
fn first_line(raw: &str) -> String {
    raw.lines().next().unwrap_or("").trim().to_string()
}

/// Store a token in bridgewatch's own credential entry for an account, which is
/// what `token = { own = true }` reads back.
pub fn set_own_token(
    account: &str,
    token: &Secret,
    provider: &dyn TokenProvider,
) -> Result<(), TokenError> {
    provider.keyring_set(
        &SystemTokenProvider::own_service(account),
        SystemTokenProvider::OWN_USER,
        token.expose(),
    )
}

/// Remove bridgewatch's own credential entry for an account.
pub fn clear_own_token(account: &str, provider: &dyn TokenProvider) -> Result<(), TokenError> {
    provider.keyring_delete(
        &SystemTokenProvider::own_service(account),
        SystemTokenProvider::OWN_USER,
    )
}

#[cfg(test)]
mod tests {
    use super::{secret_tool_lookup_args, security_lookup_args};

    /// ⛔ The empty ACCOUNT is asked for explicitly. `gh` keeps two items under
    /// one service, the login's, and an empty-account copy that is the active
    /// account, and a lookup with no `-a` returns the named one, so an empty
    /// `user` silently read the wrong slot and would have diverged from it the
    /// first time somebody ran `gh auth switch`.
    #[test]
    fn a_keychain_lookup_asks_for_the_empty_account_by_name() {
        assert_eq!(
            security_lookup_args("gh:github.com"),
            [
                "find-generic-password",
                "-s",
                "gh:github.com",
                "-a",
                "",
                "-w"
            ]
        );
    }

    /// Lo11: a service that looks like an option is fenced off with `--`, and
    /// an ordinary one is left exactly as it always was.
    #[test]
    fn a_dash_led_service_is_not_read_as_an_option() {
        assert_eq!(
            secret_tool_lookup_args("-weird"),
            ["lookup", "--", "service", "-weird", "username", ""]
        );
        assert_eq!(
            secret_tool_lookup_args("glab:gitlab.com:token"),
            ["lookup", "service", "glab:gitlab.com:token", "username", ""]
        );
    }
}
