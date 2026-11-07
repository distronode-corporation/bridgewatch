//! Token source selection, driven through a fake provider.
//!
//! ⛔ Nothing here reads a real credential. Reading a live keychain item can
//! raise a GUI prompt, and a test suite that waits for somebody to click is a
//! test suite nobody runs.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Mutex;

use bridgewatch_core::config::{Config, TokenSource};
use bridgewatch_core::token::{self, Secret, SystemTokenProvider, TokenError, TokenProvider};

/// `token::resolve` for an account whose only setting is `source`.
fn resolve(
    source: &TokenSource,
    name: &str,
    provider: &dyn TokenProvider,
) -> Result<Secret, TokenError> {
    let account = bridgewatch_core::config::Account {
        token: source.clone(),
        ..Default::default()
    };
    token::resolve(name, &account, provider)
}

#[derive(Default)]
struct Fake {
    keyring: Mutex<BTreeMap<(String, String), String>>,
    env: BTreeMap<String, String>,
    commands: Mutex<BTreeMap<String, Result<String, String>>>,
    calls: Mutex<Vec<String>>,
    /// Every `keyring_set` fails, as a locked or read-only store would.
    refuse_writes: bool,
    /// Every `keyring_get` fails, as a locked keychain does.
    refuse_reads: bool,
}

impl Fake {
    fn with_keyring(self, service: &str, user: &str, secret: &str) -> Self {
        self.keyring
            .lock()
            .unwrap()
            .insert((service.into(), user.into()), secret.into());
        self
    }
    fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }
    fn with_command(self, line: &str, out: Result<&str, &str>) -> Self {
        self.commands
            .lock()
            .unwrap()
            .insert(line.into(), out.map(str::to_string).map_err(str::to_string));
        self
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl TokenProvider for Fake {
    fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("keyring_get {service}|{user}"));
        if self.refuse_reads {
            return Err(TokenError::Store {
                service: service.into(),
                message: "the keychain is locked".into(),
            });
        }
        self.keyring
            .lock()
            .unwrap()
            .get(&(service.to_string(), user.to_string()))
            .cloned()
            .ok_or_else(|| TokenError::NoEntry {
                service: service.into(),
                user: user.into(),
            })
    }
    fn keyring_set(&self, service: &str, user: &str, secret: &str) -> Result<(), TokenError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("keyring_set {service}|{user}"));
        if self.refuse_writes {
            return Err(TokenError::Store {
                service: service.into(),
                message: "the store is read-only".into(),
            });
        }
        self.keyring
            .lock()
            .unwrap()
            .insert((service.into(), user.into()), secret.into());
        Ok(())
    }
    fn keyring_delete(&self, service: &str, user: &str) -> Result<(), TokenError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("keyring_delete {service}|{user}"));
        self.keyring
            .lock()
            .unwrap()
            .remove(&(service.into(), user.into()));
        Ok(())
    }
    fn env(&self, name: &str) -> Option<String> {
        self.calls.lock().unwrap().push(format!("env {name}"));
        self.env.get(name).cloned()
    }
    fn run(&self, argv: &[String]) -> Result<String, TokenError> {
        let line = argv.join(" ");
        self.calls.lock().unwrap().push(format!("run {line}"));
        match self.commands.lock().unwrap().get(&line) {
            Some(Ok(out)) => Ok(out.clone()),
            Some(Err(e)) => Err(TokenError::Command {
                command: line,
                message: e.clone(),
            }),
            None => Err(TokenError::Command {
                command: line,
                message: "no such command".into(),
            }),
        }
    }
}

/// The keyring source, including the empty user that `glab` actually uses.
#[test]
fn a_keyring_source_allows_an_empty_user() {
    let fake = Fake::default().with_keyring("glab:gitlab.com:token", "", "glpat-from-glab");
    let source = TokenSource::Keyring {
        service: "glab:gitlab.com:token".into(),
        user: String::new(),
    };

    let secret = resolve(&source, "gitlab", &fake).expect("resolves");
    assert_eq!(secret.expose(), "glpat-from-glab");
    assert_eq!(fake.calls(), ["keyring_get glab:gitlab.com:token|"]);
}

/// A missing entry names what was looked for, because the usual cause is a
/// service string that does not match what the other tool wrote.
#[test]
fn a_missing_keyring_entry_names_the_address() {
    let fake = Fake::default();
    let source = TokenSource::Keyring {
        service: "glab:gitlab.example:token".into(),
        user: "sean".into(),
    };
    let err = resolve(&source, "gitlab", &fake).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("glab:gitlab.example:token"), "{message}");
    assert!(message.contains("sean"), "{message}");
}

/// The env source, and its two failure modes.
#[test]
fn an_env_source_reads_the_named_variable() {
    let fake = Fake::default().with_env("BRIDGEWATCH_TOKEN_GITLAB", "  glpat-env  ");
    let source = TokenSource::Env("BRIDGEWATCH_TOKEN_GITLAB".into());

    assert_eq!(
        resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-env",
        "surrounding whitespace is trimmed, because a heredoc adds a newline"
    );

    let missing = TokenSource::Env("NOT_SET".into());
    assert!(matches!(
        resolve(&missing, "gitlab", &fake),
        Err(TokenError::MissingEnv(_))
    ));

    let empty = Fake::default().with_env("EMPTY", "");
    assert!(matches!(
        resolve(&TokenSource::Env("EMPTY".into()), "gitlab", &empty),
        Err(TokenError::MissingEnv(_)),
        // An empty variable is not a token; treating it as one produces a 401
        // that reads like a scope problem.
    ));
}

/// The command source trims stdout and fails loudly on a non-zero exit.
#[test]
fn a_command_source_trims_stdout_and_fails_on_a_bad_exit() {
    let fake = Fake::default()
        .with_command("pass gitlab/pat", Ok("glpat-from-pass\n"))
        .with_command("pass missing", Err("exited with 1: no such entry"));

    let source = TokenSource::Command(vec!["pass".into(), "gitlab/pat".into()]);
    assert_eq!(
        resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-from-pass"
    );

    let bad = TokenSource::Command(vec!["pass".into(), "missing".into()]);
    let err = resolve(&bad, "gitlab", &fake).unwrap_err();
    assert!(err.to_string().contains("no such entry"), "{err}");
}

/// `own = true` addresses bridgewatch's own entry, `bridgewatch:<account>`.
#[test]
fn the_own_source_addresses_bridgewatchs_own_entry() {
    let fake = Fake::default();
    token::set_own_token(
        "gitlab",
        "https://gitlab.com",
        &Secret::new("glpat-own"),
        &fake,
    )
    .expect("stores");

    let secret = resolve(&TokenSource::Own(true), "gitlab", &fake).expect("reads back");
    assert_eq!(secret.expose(), "glpat-own");
    assert_eq!(
        fake.calls(),
        [
            "keyring_set bridgewatch:gitlab|token",
            "keyring_get bridgewatch:gitlab|token"
        ],
        "the service is per account, so two accounts cannot collide"
    );
    assert_eq!(
        SystemTokenProvider::own_service("self-hosted"),
        "bridgewatch:self-hosted"
    );

    token::clear_own_token("gitlab", &fake).expect("clears");
    assert!(resolve(&TokenSource::Own(true), "gitlab", &fake).is_err());
}

/// `own = false` selects nothing, and says so rather than silently sending an
/// empty token.
#[test]
fn own_false_is_not_a_source() {
    let fake = Fake::default();
    assert!(matches!(
        resolve(&TokenSource::Own(false), "gitlab", &fake),
        Err(TokenError::NotConfigured(_))
    ));
}

/// A source that resolves to whitespace is an error, not an empty token.
#[test]
fn a_whitespace_only_token_is_refused() {
    let fake = Fake::default().with_keyring("s", "u", "   \n  ");
    let source = TokenSource::Keyring {
        service: "s".into(),
        user: "u".into(),
    };
    assert!(matches!(
        resolve(&source, "a", &fake),
        Err(TokenError::Empty)
    ));
}

/// Nothing in an error message is the token itself.
#[test]
fn no_error_message_leaks_a_token() {
    let fake = Fake::default()
        .with_keyring("s", "u", "glpat-SECRET")
        .with_env("V", "glpat-SECRET")
        .with_command("prog", Ok("glpat-SECRET"));

    let seen = RefCell::new(Vec::new());
    for source in [
        TokenSource::Keyring {
            service: "s".into(),
            user: "u".into(),
        },
        TokenSource::Env("V".into()),
        TokenSource::Command(vec!["prog".into()]),
    ] {
        let secret = resolve(&source, "a", &fake).unwrap();
        seen.borrow_mut().push(format!("{secret} {secret:?}"));
    }
    for rendered in seen.borrow().iter() {
        assert!(!rendered.contains("glpat-SECRET"), "{rendered}");
    }
}

/// A value that cannot be an HTTP header is refused at resolve time, with a
/// message that names the likely cause.
///
/// ⛔ Without this the failure surfaces as reqwest's `builder error`, on every
/// request, forever.
#[test]
fn a_token_that_cannot_be_a_header_is_refused_with_advice() {
    // A credential store entry with a non-ASCII character in it: nothing can
    // make this a header value, and there is no line to fall back to.
    let fake = Fake::default().with_env("TOK", "glpat-real\u{a0}token");
    let err = resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake)
        .expect_err("must not reach reqwest");
    let message = err.to_string();
    assert!(matches!(err, TokenError::Malformed { .. }), "{err:?}");
    assert!(message.contains("HTTP header"), "{message}");
    assert!(
        !message.contains("glpat-real"),
        "and it does not leak the token: {message}"
    );
}

/// ⛔ M28. A command source reads the FIRST LINE of stdout, which is what the
/// README has always promised and what every password manager emits.
/// `glab config get token` on 1.117.0 prints the token followed by 27 lines of
/// help, and `pass` prints the password followed by its metadata; refusing them
/// — and advising `head -1` — made the documented recipe fail.
#[test]
fn a_command_source_reads_the_first_line_and_ignores_the_rest() {
    let fake = Fake::default()
        .with_command(
            "glab config get token",
            Ok("glpat-real-token\nBy default, the lookup order is: environment variables, then\nmore help\n"),
        )
        .with_command("pass gitlab/pat", Ok("  glpat-padded  \nurl: gitlab.com\n"))
        .with_command("prints nothing", Ok("\n\n"));

    let source = TokenSource::Command(vec![
        "glab".into(),
        "config".into(),
        "get".into(),
        "token".into(),
    ]);
    assert_eq!(
        resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-real-token"
    );

    let pass = TokenSource::Command(vec!["pass".into(), "gitlab/pat".into()]);
    assert_eq!(
        resolve(&pass, "gitlab", &fake).unwrap().expose(),
        "glpat-padded",
        "the line is trimmed, so a padded field is still a token"
    );

    // An empty first line is still nothing, and says so rather than sending "".
    let empty = TokenSource::Command(vec!["prints".into(), "nothing".into()]);
    assert!(matches!(
        resolve(&empty, "gitlab", &fake),
        Err(TokenError::Empty)
    ));
}

/// ⛔ M27. The token command used to have no timeout, no output cap, and put
/// the whole of stderr in an error the popover renders.
///
/// The timeout is the one that mattered: this runs on the poll task before its
/// `select!`, so a `pass`/`gpg` helper waiting on a pinentry nobody can see
/// parked the tray for good.
#[test]
fn a_token_command_is_killed_when_it_waits_forever() {
    let argv = ["sh", "-c", "sleep 30"].map(String::from).to_vec();
    let started = std::time::Instant::now();
    let err = token::run_bounded(&argv, std::time::Duration::from_millis(150), 4096)
        .expect_err("a command that never returns must not park the poller");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "it has to come back promptly, not eventually"
    );
    assert!(err.to_string().contains("timed out"), "{err}");
}

/// ⛔ M27's second half. A command that exits but leaves something it started
/// holding its stdout — a helper that daemonises an agent, or just a stray
/// `&` — used to hang the join on the pipe reader for as long as the
/// grandchild lived, deadline or no deadline. The token it printed before
/// exiting is still the answer, and it has to arrive on time.
#[test]
fn a_command_whose_grandchild_keeps_stdout_open_still_returns_on_time() {
    let argv = ["sh", "-c", "echo glpat-early; sleep 8 &"]
        .map(String::from)
        .to_vec();
    let started = std::time::Instant::now();
    let out = token::run_bounded(&argv, std::time::Duration::from_millis(500), 4096)
        .expect("the command exited 0 with a complete first line");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "it waited on the grandchild: {:?}",
        started.elapsed()
    );
    assert_eq!(out.lines().next(), Some("glpat-early"));
}

/// A program that prints a great deal is answered with a bounded prefix, and
/// still yields its token.
#[test]
fn a_chatty_token_command_is_bounded_but_still_works() {
    let argv = [
        "sh",
        "-c",
        "printf 'glpat-first\n'; head -c 200000 /dev/zero | tr '\\0' 'x'",
    ]
    .map(String::from)
    .to_vec();

    let out = token::run_bounded(&argv, std::time::Duration::from_secs(20), 4096)
        .expect("it exits, so it succeeds");
    assert!(out.len() <= 4096, "stdout is capped: {} bytes", out.len());
    assert_eq!(
        resolve(
            &TokenSource::Command(argv.clone()),
            "gitlab",
            &bridgewatch_core::token::SystemTokenProvider
        )
        .unwrap()
        .expose(),
        "glpat-first",
        "and the first line is still the token"
    );
}

/// A failing command quotes a bounded amount of stderr, not all of it.
#[test]
fn a_failing_token_command_quotes_a_bounded_stderr() {
    let argv = [
        "sh",
        "-c",
        "head -c 5000 /dev/zero | tr '\\0' 'e' >&2; exit 3",
    ]
    .map(String::from)
    .to_vec();
    let err = token::run_bounded(&argv, std::time::Duration::from_secs(20), 4096)
        .expect_err("a non-zero exit is an error");
    let message = err.to_string();
    assert!(message.contains("exited with 3"), "{message}");
    assert!(
        message.len() < 800,
        "the popover has no scrollback: {} chars",
        message.len()
    );
    assert!(message.contains("[…]"), "and it says it was cut: {message}");
}

// ---------------------------------------------------------------------------
// The go-keyring envelope
//
// ⛔ glab does not store the token. `zalando/go-keyring`'s macOS backend
// base64-encodes every password before `security add-generic-password`, so what
// comes back is `go-keyring-base64:` + base64 of the real value. Sending that as
// a PRIVATE-TOKEN header is a 401 that reads exactly like a scope problem.
// ---------------------------------------------------------------------------

use bridgewatch_core::token::{
    GO_KEYRING_BASE64_PREFIX, GO_KEYRING_HEX_PREFIX, decode_keyring_envelope,
};

/// The current envelope: base64, `StdEncoding`, as `Set` writes unconditionally.
#[test]
fn the_base64_envelope_is_unwrapped() {
    // base64("glpat-abcdefghijklmnop") — the shape glab stores.
    let stored = "go-keyring-base64:Z2xwYXQtYWJjZGVmZ2hpamtsbW5vcA==";
    assert_eq!(
        decode_keyring_envelope(stored).unwrap(),
        "glpat-abcdefghijklmnop"
    );

    // `security -w` appends a newline, and go-keyring TrimSpaces before looking
    // for the prefix, so leading and trailing whitespace must not defeat it.
    assert_eq!(
        decode_keyring_envelope(&format!("  {stored}  \n")).unwrap(),
        "glpat-abcdefghijklmnop"
    );

    // Padding is required by StdEncoding; a value needing two pad characters
    // round-trips too.
    assert_eq!(
        decode_keyring_envelope("go-keyring-base64:Z2xwYXQtYQ==").unwrap(),
        "glpat-a"
    );
}

/// The legacy envelope: lowercase hex, which `Get` still accepts.
#[test]
fn the_legacy_hex_envelope_is_unwrapped() {
    // hex("glpat-xyz")
    assert_eq!(
        decode_keyring_envelope("go-keyring-encoded:676c7061742d78797a").unwrap(),
        "glpat-xyz"
    );
    assert_eq!(
        decode_keyring_envelope("go-keyring-encoded:676C7061742D78797A").unwrap(),
        "glpat-xyz",
        "Go's hex.DecodeString accepts either case, so this must too"
    );
}

/// A value with neither prefix passes through, trimmed. This is what makes the
/// unwrap safe to run over every credential whatever wrote it.
#[test]
fn a_plain_credential_passes_through_unchanged() {
    assert_eq!(
        decode_keyring_envelope("glpat-plainvalue").unwrap(),
        "glpat-plainvalue"
    );
    assert_eq!(
        decode_keyring_envelope("  glpat-plainvalue \n").unwrap(),
        "glpat-plainvalue"
    );
    assert_eq!(
        decode_keyring_envelope("go-keyring-something-else:xyz").unwrap(),
        "go-keyring-something-else:xyz",
        "only the two prefixes go-keyring actually writes are envelopes"
    );
    // Idempotent, which is what lets `resolve` unwrap again after the provider.
    let once = decode_keyring_envelope("go-keyring-base64:Z2xwYXQtYQ==").unwrap();
    assert_eq!(decode_keyring_envelope(&once).unwrap(), once);
}

/// A prefix that will not decode is an error naming the envelope, never a
/// silent pass-through: passing the raw bytes on produces a 401.
#[test]
fn a_malformed_envelope_is_an_error_naming_it() {
    for (stored, prefix) in [
        (
            "go-keyring-base64:!!!not-base64!!!",
            GO_KEYRING_BASE64_PREFIX,
        ),
        ("go-keyring-base64:Z2xwYXQ", GO_KEYRING_BASE64_PREFIX), // truncated, bad padding
        ("go-keyring-encoded:zzzz", GO_KEYRING_HEX_PREFIX),      // not hex digits
        ("go-keyring-encoded:676c706", GO_KEYRING_HEX_PREFIX),   // odd length
    ] {
        let err = decode_keyring_envelope(stored).expect_err("{stored} must not pass through");
        let TokenError::Envelope { envelope, .. } = &err else {
            panic!("{stored}: expected Envelope, got {err:?}");
        };
        assert_eq!(envelope, prefix);
        assert!(
            err.to_string().contains("go-keyring"),
            "the message points at the cause: {err}"
        );
    }
}

/// Decoded bytes must be text. Lossy conversion would yield a header that is
/// well-formed and wrong.
#[test]
fn a_payload_that_is_not_utf8_is_an_error() {
    // base64 of 0xFF 0xFE, which is not valid UTF-8.
    let err = decode_keyring_envelope("go-keyring-base64://4=").expect_err("not UTF-8");
    assert!(err.to_string().contains("UTF-8"), "{err}");

    let err = decode_keyring_envelope("go-keyring-encoded:fffe").expect_err("not UTF-8");
    assert!(err.to_string().contains("UTF-8"), "{err}");
}

/// The envelope is unwrapped by `resolve` too, so a `command` source pointing
/// straight at `security` — the obvious way to reuse glab's token without the
/// keyring source — works rather than 401ing.
#[test]
fn resolve_unwraps_an_envelope_from_any_source() {
    let fake = Fake::default()
        .with_command(
            "security find-generic-password -s glab:gitlab.com:token -w",
            Ok("go-keyring-base64:Z2xwYXQtYWJjZGVmZ2hpamtsbW5vcA==\n"),
        )
        .with_env("TOK", "go-keyring-base64:Z2xwYXQtYWJjZGVmZ2hpamtsbW5vcA==");

    let source = TokenSource::Command(vec![
        "security".into(),
        "find-generic-password".into(),
        "-s".into(),
        "glab:gitlab.com:token".into(),
        "-w".into(),
    ]);
    assert_eq!(
        resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-abcdefghijklmnop",
        "and NOT the 102-character enveloped string, which is a 401"
    );

    assert_eq!(
        resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake)
            .unwrap()
            .expose(),
        "glpat-abcdefghijklmnop"
    );
}

/// A malformed envelope reaching `resolve` fails there, and does not leak the
/// payload into the message.
#[test]
fn resolve_refuses_a_malformed_envelope() {
    let fake = Fake::default().with_env("TOK", "go-keyring-base64:!!!nope!!!");
    let err = resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake).unwrap_err();
    assert!(matches!(err, TokenError::Envelope { .. }), "{err:?}");
    assert!(
        !err.to_string().contains("nope"),
        "no payload in the message: {err}"
    );
}

// --- an own token stays with its server --------------------------------------

/// An account named `gl` on `base_url`, reading its own entry.
fn own_on(base_url: &str) -> bridgewatch_core::config::Account {
    bridgewatch_core::config::Account {
        base_url: base_url.into(),
        token: TokenSource::Own(true),
        ..Default::default()
    }
}

/// What the store holds under `bridgewatch:gl`.
fn stored(fake: &Fake) -> Option<String> {
    fake.keyring
        .lock()
        .unwrap()
        .get(&("bridgewatch:gl".to_string(), "token".to_string()))
        .cloned()
}

/// A saved token is stored with the origin it was saved for, and read back
/// for that origin.
#[test]
fn an_own_token_is_stored_bound_to_its_server_and_read_back_there() {
    let fake = Fake::default();
    token::set_own_token(
        "gl",
        "https://gitlab.example.com",
        &Secret::new("glpat-A"),
        &fake,
    )
    .expect("stores");
    let raw = stored(&fake).expect("written");
    assert!(raw.starts_with(token::BOUND_V1), "{raw:?}");
    assert!(
        raw.ends_with(r#"{"version":1,"token":"glpat-A","origin":"https://gitlab.example.com"}"#),
        "{raw:?}"
    );

    let secret = token::resolve("gl", &own_on("https://gitlab.example.com"), &fake).unwrap();
    assert_eq!(secret.expose(), "glpat-A");
    assert_eq!(
        token::own_token_status("gl", &own_on("https://gitlab.example.com"), &fake).unwrap(),
        token::OwnTokenStatus::Bound {
            origin: "https://gitlab.example.com".into()
        }
    );
}

/// ⛔ The bug: a token saved for A, and the account's `base_url` since moved
/// to B. It is refused before any request: the client cannot even be built,
/// and the transport has seen nothing.
#[test]
fn an_own_token_saved_for_another_server_is_refused_and_nothing_is_sent() {
    let fake = std::sync::Arc::new(Fake::default());
    token::set_own_token(
        "gl",
        "https://gitlab.com",
        &Secret::new("glpat-A"),
        fake.as_ref(),
    )
    .unwrap();
    let moved = own_on("https://gitlab.example.com");

    let err = token::resolve("gl", &moved, fake.as_ref()).unwrap_err();
    assert!(matches!(err, TokenError::BoundElsewhere { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        "the stored token of account \"gl\" was saved for https://gitlab.com, and the account \
         now points at https://gitlab.example.com, so it is not sent. Save a token for \
         https://gitlab.example.com in Settings, or put base_url back"
    );
    assert!(!err.to_string().contains("glpat-A"));

    let transport = std::sync::Arc::new(bridgewatch_core::client::ScriptTransport::new());
    let built = bridgewatch_core::poll::client_for_account_on(
        "gl",
        &moved,
        None,
        fake.clone(),
        transport.clone(),
        bridgewatch_core::client::RequestRing::new(4),
    );
    assert!(
        matches!(
            built,
            Err(bridgewatch_core::poll::PollerError::Token(
                _,
                TokenError::BoundElsewhere { .. }
            ))
        ),
        "the client is not built"
    );
    assert!(transport.requests().is_empty(), "nothing was sent");
    assert_eq!(
        token::own_token_status("gl", &moved, fake.as_ref()).unwrap(),
        token::OwnTokenStatus::Mismatch {
            stored: "https://gitlab.com".into(),
            expected: Some("https://gitlab.example.com".into()),
        }
    );
    // And the entry is left exactly as it was: a refusal rewrites nothing.
    assert!(
        !fake
            .calls()
            .iter()
            .skip(1)
            .any(|c| c.starts_with("keyring_set"))
    );
}

/// A 1.x value (bare) is bound to the account's origin at that moment, in
/// the order read, write, then send; from then on it is a bound entry like
/// any other, and a later move is refused.
#[test]
fn a_1x_value_is_bound_on_first_use_then_used() {
    let fake = Fake::default().with_keyring("bridgewatch:gl", "token", "glpat-legacy\n");
    let account = own_on("https://gitlab.example.com");
    assert_eq!(
        token::own_token_status("gl", &account, &fake).unwrap(),
        token::OwnTokenStatus::Legacy
    );
    assert_eq!(
        fake.calls(),
        ["keyring_get bridgewatch:gl|token"],
        "a status binds nothing"
    );

    let secret = token::resolve("gl", &account, &fake).expect("binds and resolves");
    assert_eq!(secret.expose(), "glpat-legacy");
    assert_eq!(
        fake.calls()[1..],
        [
            "keyring_get bridgewatch:gl|token",
            "keyring_set bridgewatch:gl|token"
        ],
        "read, then the bound form written, before the token is handed out"
    );
    let raw = stored(&fake).unwrap();
    assert!(
        raw.starts_with(token::BOUND_V1)
            && raw.contains(r#""origin":"https://gitlab.example.com""#),
        "{raw:?}"
    );

    // Bound now: read again without another write.
    assert_eq!(
        token::resolve("gl", &account, &fake).unwrap().expose(),
        "glpat-legacy"
    );
    assert_eq!(fake.calls().len(), 4, "{:?}", fake.calls());

    // And a move after the bind is refused like any other.
    let err = token::resolve("gl", &own_on("https://gitlab.com"), &fake).unwrap_err();
    assert!(matches!(err, TokenError::BoundElsewhere { .. }), "{err:?}");
}

/// ⛔ A 1.x value whose bind cannot be written is not used: nothing is sent.
#[test]
fn a_1x_value_whose_bind_fails_is_not_sent() {
    let fake = std::sync::Arc::new(Fake {
        refuse_writes: true,
        ..Fake::default().with_keyring("bridgewatch:gl", "token", "glpat-legacy")
    });
    let account = own_on("https://gitlab.example.com");
    let err = token::resolve("gl", &account, fake.as_ref()).unwrap_err();
    assert!(matches!(err, TokenError::Store { .. }), "{err:?}");
    assert!(!err.to_string().contains("glpat-legacy"), "{err}");

    let transport = std::sync::Arc::new(bridgewatch_core::client::ScriptTransport::new());
    let built = bridgewatch_core::poll::client_for_account_on(
        "gl",
        &account,
        None,
        fake.clone(),
        transport.clone(),
        bridgewatch_core::client::RequestRing::new(4),
    );
    assert!(built.is_err(), "no client for an unbound value");
    assert!(transport.requests().is_empty(), "nothing was sent");
    assert_eq!(
        stored(&fake).as_deref(),
        Some("glpat-legacy"),
        "and the entry is untouched"
    );
}

/// A 1.x value on an account whose base_url has no origin has nothing to be
/// bound to, and is not sent.
#[test]
fn a_1x_value_on_a_base_url_with_no_origin_is_not_bound_or_sent() {
    let fake = Fake::default().with_keyring("bridgewatch:gl", "token", "glpat-legacy");
    let err = token::resolve("gl", &own_on("gitlab.example.com"), &fake).unwrap_err();
    assert!(matches!(err, TokenError::Store { .. }), "{err:?}");
    assert!(
        !fake.calls().iter().any(|c| c.starts_with("keyring_set")),
        "{:?}",
        fake.calls()
    );
    // Nor is a token SAVED for such an address.
    assert!(token::set_own_token("gl", "ftp://x", &Secret::new("glpat-B"), &fake).is_err());
    assert_eq!(stored(&fake).as_deref(), Some("glpat-legacy"));
}

/// ⛔ A bound entry whose stored origin is not an origin matches nothing, and
/// is never echoed: an edit could have put a credential in it.
#[test]
fn a_bound_entry_with_no_usable_origin_matches_nothing() {
    let doc = format!(
        "{}{}",
        token::BOUND_V1,
        r#"{"version":1,"token":"glpat-A","origin":"https://me:hunter2@gitlab.com"}"#
    );
    let fake = Fake::default().with_keyring("bridgewatch:gl", "token", &doc);
    let err = token::resolve("gl", &own_on("https://gitlab.com"), &fake).unwrap_err();
    assert!(matches!(err, TokenError::BoundElsewhere { .. }), "{err:?}");
    assert!(!err.to_string().contains("hunter2"), "{err}");

    // Nor does an account whose own base_url has none.
    let fake = Fake::default();
    token::set_own_token("gl", "https://gitlab.com", &Secret::new("glpat-A"), &fake).unwrap();
    let err = token::resolve("gl", &own_on("not a url"), &fake).unwrap_err();
    assert!(
        matches!(err, TokenError::BoundElsewhere { expected: None, .. }),
        "{err:?}"
    );
}

/// The origin is compared as the sign-in compares it: case, the default port
/// and a trailing slash or path make no difference; scheme and port do.
#[test]
fn an_own_tokens_origin_is_normalised_like_a_sign_ins() {
    let fake = Fake::default();
    token::set_own_token(
        "gl",
        "https://GitLab.com:443/",
        &Secret::new("glpat-A"),
        &fake,
    )
    .unwrap();
    assert!(
        stored(&fake)
            .unwrap()
            .contains(r#""origin":"https://gitlab.com""#)
    );
    for same in [
        "https://gitlab.com",
        "https://gitlab.com/",
        "HTTPS://gitlab.COM:443",
    ] {
        assert_eq!(
            token::resolve("gl", &own_on(same), &fake).unwrap().expose(),
            "glpat-A",
            "{same}"
        );
    }
    for other in ["http://gitlab.com", "https://gitlab.com:8443"] {
        assert!(
            matches!(
                token::resolve("gl", &own_on(other), &fake),
                Err(TokenError::BoundElsewhere { .. })
            ),
            "{other}"
        );
    }
}

/// ⛔ What 1.3 does with a 2.0 entry. The encoding carries a control
/// character (the tab after the marker) that `decode_keyring_envelope` keeps,
/// so 1.3's own path (decode, trim, refuse a control character) fails it
/// locally as Malformed. Proven here on the same code: a source that hands the
/// raw entry over, as 1.3's own arm did, refuses it before any request.
#[test]
fn a_bound_entry_read_as_a_bare_token_is_refused_as_malformed() {
    let encoded = token::encode_bound(&Secret::new("glpat-A"), "https://gitlab.com").unwrap();
    assert!(encoded.contains('\t'), "{encoded:?}");
    assert_eq!(
        token::decode_keyring_envelope(&encoded).unwrap(),
        encoded,
        "the envelope decoder leaves it whole, tab and all"
    );
    let fake = Fake::default().with_env("TOK", &encoded);
    let err = resolve(&TokenSource::Env("TOK".into()), "gl", &fake).unwrap_err();
    assert!(
        matches!(&err, TokenError::Malformed { character, .. } if character == "'\\t'"),
        "{err:?}"
    );
}

/// A value that claims to be bound but is not a readable v1 document is an
/// error, never a 1.x value to bind: it is neither sent nor rewritten.
#[test]
fn an_unreadable_or_newer_bound_entry_is_refused_not_rebound() {
    for raw in [
        "bw-bound-v2:\t{\"version\":2}".to_string(),
        format!("{}not json", token::BOUND_V1),
        format!(
            "{}{}",
            token::BOUND_V1,
            r#"{"version":7,"token":"t","origin":"https://gitlab.com"}"#
        ),
    ] {
        let fake = Fake::default().with_keyring("bridgewatch:gl", "token", &raw);
        let err = token::resolve("gl", &own_on("https://gitlab.com"), &fake).unwrap_err();
        assert!(matches!(err, TokenError::Store { .. }), "{raw:?}: {err:?}");
        assert!(err.to_string().contains("not readable"), "{err}");
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("keyring_set")),
            "{raw:?}"
        );
        assert!(token::own_token_status("gl", &own_on("https://gitlab.com"), &fake).is_err());
    }
}

/// Nothing stored is `None` to a status, and `NoEntry` to a resolve.
#[test]
fn an_account_with_no_own_token_says_none() {
    let fake = Fake::default();
    assert_eq!(
        token::own_token_status("gl", &own_on("https://gitlab.com"), &fake).unwrap(),
        token::OwnTokenStatus::None
    );
    assert!(matches!(
        token::resolve("gl", &own_on("https://gitlab.com"), &fake),
        Err(TokenError::NoEntry { .. })
    ));
}

/// Two configurations, before and after an edit that may move `gl`.
fn configs(before: &str, after: &str) -> (Config, Config) {
    let parse = |raw: &str| {
        bridgewatch_core::config::parse_str(raw, std::path::Path::new("x.toml"))
            .expect("fixture parses")
            .config
    };
    (parse(before), parse(after))
}

const ON_OLD: &str =
    "[accounts.gl]\nbase_url = \"https://gitlab.example.com\"\ntoken = { own = true }\n";
const ON_NEW: &str = "[accounts.gl]\nbase_url = \"https://evil.example\"\ntoken = { own = true }\n";

/// Build the moved account's client, as the poller does after the edit, and
/// make one request with it if it is built: what reaches the new host.
async fn poll_after_move(fake: std::sync::Arc<Fake>) -> (bool, Vec<String>) {
    let (_, after) = configs(ON_OLD, ON_NEW);
    let transport = std::sync::Arc::new(bridgewatch_core::client::ScriptTransport::new());
    let built = bridgewatch_core::poll::client_for_account_on(
        "gl",
        &after.accounts["gl"],
        None,
        fake,
        transport.clone(),
        bridgewatch_core::client::RequestRing::new(4),
    );
    let refused = matches!(
        &built,
        Err(bridgewatch_core::poll::PollerError::Token(
            _,
            TokenError::BoundElsewhere { .. }
        ))
    );
    if let Ok(client) = built {
        let _ = client.current_user().await;
    }
    let urls = transport.requests().into_iter().map(|r| r.url).collect();
    (refused, urls)
}

/// ⛔ The Seer finding on the 2.0 release: a 1.x value is bound on first use
/// to whatever `base_url` says THEN, so a host edit made before that first use
/// carried it to the new host. Before the edit lands, it is now bound to the
/// host it is leaving, and the poll after the edit sends nothing.
#[tokio::test]
async fn a_host_edit_binds_a_1x_value_to_the_old_host_and_the_poll_sends_nothing() {
    let fake = std::sync::Arc::new(Fake::default().with_keyring(
        "bridgewatch:gl",
        "token",
        "glpat-legacy",
    ));
    let (before, after) = configs(ON_OLD, ON_NEW);
    let bound = token::bind_before_moves(&before, &after, fake.as_ref()).expect("binds");

    let (refused, urls) = poll_after_move(fake.clone()).await;
    assert!(urls.is_empty(), "a request reached the new host: {urls:?}");
    assert!(refused, "the moved account is refused as bound elsewhere");
    assert_eq!(bound, ["gl"]);
    let raw = stored(&fake).unwrap();
    assert!(
        raw.starts_with(token::BOUND_V1)
            && raw.contains(r#""origin":"https://gitlab.example.com""#),
        "the entry is bound to the OLD host"
    );

    // Bound now, so a second pass (the reload after the write) rewrites nothing.
    let writes = |f: &Fake| {
        f.calls()
            .iter()
            .filter(|c| c.starts_with("keyring_set"))
            .count()
    };
    let before_writes = writes(&fake);
    assert!(
        token::bind_before_moves(&before, &after, fake.as_ref())
            .unwrap()
            .is_empty()
    );
    assert_eq!(writes(&fake), before_writes);
}

/// ⛔ A locked keychain (a read or a write that fails) leaves the value
/// unbound, so the move is refused with the store's error, and the entry is
/// left as it was. The caller writes nothing; nothing is sent.
#[test]
fn a_host_edit_whose_bind_fails_is_refused_and_binds_nothing() {
    for (refuse_reads, refuse_writes) in [(true, false), (false, true)] {
        let fake = Fake {
            refuse_reads,
            refuse_writes,
            ..Fake::default().with_keyring("bridgewatch:gl", "token", "glpat-legacy")
        };
        let (before, after) = configs(ON_OLD, ON_NEW);
        let refused = token::bind_before_moves(&before, &after, &fake).unwrap_err();
        assert_eq!(refused.name, "gl");
        assert_eq!(refused.from, "https://gitlab.example.com");
        assert!(
            matches!(refused.error, TokenError::Store { .. }),
            "{refused:?}"
        );
        let text = refused.to_string();
        assert!(text.contains("was not moved"), "{text}");
        assert!(!text.contains("glpat-legacy"), "{text}");
        assert_eq!(stored(&fake).as_deref(), Some("glpat-legacy"));
    }
}

/// Only a real move of an account that read its own entry reads the store:
/// a respelling, an ordinary edit, a new account, or an account that used
/// another source on the old host costs no credential read at all.
#[test]
fn only_a_move_of_an_own_account_reads_the_store() {
    let fake = Fake::default().with_keyring("bridgewatch:gl", "token", "glpat-legacy");
    for (before, after) in [
        (
            ON_OLD,
            "[accounts.gl]\nbase_url = \"https://GitLab.Example.com:443/\"\ntoken = { own = true }\n",
        ),
        (ON_OLD, ON_OLD),
        (
            "[accounts.other]\nbase_url = \"https://gitlab.example.com\"\ntoken = { own = true }\n",
            ON_NEW,
        ),
        (
            "[accounts.gl]\nbase_url = \"https://gitlab.example.com\"\ntoken = { env = \"T\" }\n",
            ON_NEW,
        ),
    ] {
        let (before, after) = configs(before, after);
        assert!(
            token::bind_before_moves(&before, &after, &fake)
                .unwrap()
                .is_empty()
        );
    }
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());

    // And a move with nothing to bind (no entry, or one already bound) is not
    // an error.
    let (before, after) = configs(ON_OLD, ON_NEW);
    assert!(
        token::bind_before_moves(&before, &after, &Fake::default())
            .unwrap()
            .is_empty()
    );
}
