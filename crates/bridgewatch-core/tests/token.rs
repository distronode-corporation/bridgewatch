//! Token source selection, driven through a fake provider.
//!
//! ⛔ Nothing here reads a real credential. Reading a live keychain item can
//! raise a GUI prompt, and a test suite that waits for somebody to click is a
//! test suite nobody runs.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Mutex;

use bridgewatch_core::config::TokenSource;
use bridgewatch_core::token::{self, Secret, SystemTokenProvider, TokenError, TokenProvider};

#[derive(Default)]
struct Fake {
    keyring: Mutex<BTreeMap<(String, String), String>>,
    env: BTreeMap<String, String>,
    commands: Mutex<BTreeMap<String, Result<String, String>>>,
    calls: Mutex<Vec<String>>,
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

    let secret = token::resolve(&source, "gitlab", &fake).expect("resolves");
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
    let err = token::resolve(&source, "gitlab", &fake).unwrap_err();
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
        token::resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-env",
        "surrounding whitespace is trimmed, because a heredoc adds a newline"
    );

    let missing = TokenSource::Env("NOT_SET".into());
    assert!(matches!(
        token::resolve(&missing, "gitlab", &fake),
        Err(TokenError::MissingEnv(_))
    ));

    let empty = Fake::default().with_env("EMPTY", "");
    assert!(matches!(
        token::resolve(&TokenSource::Env("EMPTY".into()), "gitlab", &empty),
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
        token::resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-from-pass"
    );

    let bad = TokenSource::Command(vec!["pass".into(), "missing".into()]);
    let err = token::resolve(&bad, "gitlab", &fake).unwrap_err();
    assert!(err.to_string().contains("no such entry"), "{err}");
}

/// `own = true` addresses bridgewatch's own entry, `bridgewatch:<account>`.
#[test]
fn the_own_source_addresses_bridgewatchs_own_entry() {
    let fake = Fake::default();
    token::set_own_token("gitlab", &Secret::new("glpat-own"), &fake).expect("stores");

    let secret = token::resolve(&TokenSource::Own(true), "gitlab", &fake).expect("reads back");
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
    assert!(token::resolve(&TokenSource::Own(true), "gitlab", &fake).is_err());
}

/// `own = false` selects nothing, and says so rather than silently sending an
/// empty token.
#[test]
fn own_false_is_not_a_source() {
    let fake = Fake::default();
    assert!(matches!(
        token::resolve(&TokenSource::Own(false), "gitlab", &fake),
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
        token::resolve(&source, "a", &fake),
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
        let secret = token::resolve(&source, "a", &fake).unwrap();
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
    let err = token::resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake)
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
        token::resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-real-token"
    );

    let pass = TokenSource::Command(vec!["pass".into(), "gitlab/pat".into()]);
    assert_eq!(
        token::resolve(&pass, "gitlab", &fake).unwrap().expose(),
        "glpat-padded",
        "the line is trimmed, so a padded field is still a token"
    );

    // An empty first line is still nothing, and says so rather than sending "".
    let empty = TokenSource::Command(vec!["prints".into(), "nothing".into()]);
    assert!(matches!(
        token::resolve(&empty, "gitlab", &fake),
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
        token::resolve(
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
        token::resolve(&source, "gitlab", &fake).unwrap().expose(),
        "glpat-abcdefghijklmnop",
        "and NOT the 102-character enveloped string, which is a 401"
    );

    assert_eq!(
        token::resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake)
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
    let err = token::resolve(&TokenSource::Env("TOK".into()), "gitlab", &fake).unwrap_err();
    assert!(matches!(err, TokenError::Envelope { .. }), "{err:?}");
    assert!(
        !err.to_string().contains("nope"),
        "no payload in the message: {err}"
    );
}
