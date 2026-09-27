//! The end of a job's log, fetched on demand and cleaned for display; and the
//! request plumbing the job actions (retry, play) share with it.
//!
//! # Fetching
//!
//! Neither provider serves a log where the rest of the API lives:
//!
//! - GitHub's `GET /repos/{o}/{r}/actions/jobs/{id}/logs` answers `302` with a
//!   `Location` on a storage host (a signed blob URL that lives about a
//!   minute). Measured 2026-09-26 on a public run: the blob ignores a suffix
//!   `Range` and answers `200` with the whole log, so the tail is cut here.
//! - GitLab's `GET /projects/{id}/jobs/{id}/trace` streams the log itself
//!   while the job is recent. An archived log on gitlab.com is kept in object
//!   storage behind a CDN, and the web UI's raw log was measured answering a
//!   `302` to it; the API endpoint could not be measured without a token, so
//!   the same one follow is allowed for it. The first request carries
//!   `Range: bytes=-N`, which that CDN was measured honouring (`206`,
//!   `content-range: bytes 94613-95636/95637`).
//!
//! ⛔ **The transport refuses redirects, and that stays true.** A redirect
//! followed by the HTTP client would carry the credential header to whatever
//! host it names. So a log gets exactly ONE follow, made here by hand
//! ([`follow`]): the `Location` must be `https`, and the second request is
//! built with no credential header at all and marked
//! [`HttpRequest::anonymous`], which is what stops
//! [`crate::oauth::OAuthTransport`] from adding the sign-in to it. The signed
//! URL is its own authorisation; the account's token has no business there.
//!
//! ⛔ And the signed URL's query string IS a credential for that blob, for as
//! long as it lives, so it never reaches the request ring or a log line: the
//! second leg is recorded as `host/path` ([`display_path`]).
//!
//! # Cleaning
//!
//! What comes back is a terminal's byte stream, and what is shown is plain
//! text, in a popover (rendered as text, never as markup) or a terminal. See
//! [`clean`] for each step. GitHub's per-line timestamps, and the GitLab
//! runner's (`<time> 01O `), are **trimmed**: the popover is 440 pixels wide
//! and a 28-character timestamp on every line leaves no room for the line.

use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::ClientError;
use super::http::{HttpRequest, HttpResponse, InFlight, RequestLog, RequestRing, Transport};
use crate::config::Provider;

/// How many lines of a log are kept.
pub const LOG_TAIL_LINES: usize = 40;

/// How much of a log is read, at most, from its end.
///
/// 256 KiB holds the last 40 lines of any log a person would read, however
/// long its lines, and bounds what one click can cost: a 50 MB log is read
/// through, but never held.
pub const LOG_TAIL_BYTES: usize = 256 * 1024;

/// The longest line kept, in characters. A minified bundle printed to stdout
/// is one line of megabytes; the popover needs its start, not all of it.
pub const MAX_LINE_CHARS: usize = 500;

/// The end of a job's log, ready to show as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogTail {
    /// At most [`LOG_TAIL_LINES`] lines, oldest first, cleaned by [`clean`].
    pub lines: Vec<String>,
    /// True when the log was longer than what was read: only its last
    /// `max_bytes` came back, so the lines above these were never seen.
    pub truncated: bool,
}

impl LogTail {
    /// Clean a fetched body into a tail. See [`clean`].
    pub fn from_body(body: &str, provider: Provider, truncated: bool, max_lines: usize) -> Self {
        Self {
            lines: clean(body, provider, truncated, max_lines),
            truncated,
        }
    }
}

/// What a job action (retry, play) answered with.
///
/// GitLab answers with the job it created (a retry is a NEW job), so its id and
/// page are here. GitHub answers `201` with no body and re-runs the job under
/// its own id, so both are `None` there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobActionOutcome {
    /// The job the action produced, when the provider said.
    pub job_id: Option<u64>,
    /// Its page, when the provider said.
    pub web_url: Option<String>,
}

impl JobActionOutcome {
    /// Read what GitLab answered a retry or play with, leniently: the action
    /// has already happened, and a body this build cannot read must not turn
    /// a success into an error.
    pub fn from_body(body: &str) -> Self {
        #[derive(Deserialize)]
        struct Created {
            id: Option<u64>,
            web_url: Option<String>,
        }
        serde_json::from_str::<Created>(body)
            .map(|c| Self {
                job_id: c.id,
                web_url: c
                    .web_url
                    .filter(|u| u.starts_with("https://") || u.starts_with("http://")),
            })
            .unwrap_or_default()
    }
}

/// Clean a log body for display. In order:
///
/// 1. a byte-order mark comes off the front, and when the body is a tail
///    (`truncated`) its first line, which starts mid-line, is dropped;
/// 2. per line, a trailing `\r` (CRLF) comes off, then the runner's timestamp
///    prefix: GitHub's `2026-09-26T12:00:00.1234567Z `, and the GitLab
///    runner's `2026-09-26T12:00:00.123456Z 01O ` (whose `+` form continues
///    the previous line, and is joined to it);
/// 3. GitLab's collapsible-section markers
///    (`section_start:<time>:<name>[options]\r` and `section_end:...`);
/// 4. a line overwritten with `\r` (a progress bar) keeps only what followed
///    the last `\r`, which is what a terminal would be showing;
/// 5. every ANSI escape sequence (CSI, OSC and the two-byte forms), then every
///    remaining control character except a tab, and the Unicode bidirectional
///    overrides, which could make a line display as something it does not say;
/// 6. GitHub's `##[group]` prefix (the group's title stays) and its
///    `##[endgroup]` lines;
/// 7. a line longer than [`MAX_LINE_CHARS`] is cut and ends in `…`;
///
/// and finally trailing blank lines go and the last `max_lines` are kept.
pub fn clean(body: &str, provider: Provider, truncated: bool, max_lines: usize) -> Vec<String> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let body = match (truncated, body.find('\n')) {
        (true, Some(first_newline)) => &body[first_newline + 1..],
        _ => body,
    };
    let mut out: Vec<String> = Vec::new();
    for raw in body.split('\n') {
        let raw = raw.trim_end_matches('\r');
        let (raw, continues) = strip_timestamp(raw, provider);
        let marker = SECTION.is_match(raw);
        let line = SECTION.replace_all(raw, "");
        let line = match line.rfind('\r') {
            Some(i) => &line[i + 1..],
            None => &line[..],
        };
        let line = ANSI.replace_all(line, "");
        let mut line: String = line
            .chars()
            .filter(|c| *c == '\t' || !(c.is_control() || is_bidi_control(*c) || *c == '\u{feff}'))
            .collect();
        if provider == Provider::Github {
            if line.starts_with("##[endgroup]") {
                continue;
            }
            if let Some(title) = line.strip_prefix("##[group]") {
                line = title.to_string();
            }
        }
        // Trailing space is kept until the end: a GitLab continuation (`+`)
        // may be joined on after it.
        let line = cap(&line);
        // A line that held nothing but a section marker (every `section_end`)
        // is not a blank line the job printed.
        if marker && line.trim().is_empty() {
            continue;
        }
        match (continues, out.last_mut()) {
            (true, Some(previous)) => {
                let joined = format!("{previous}{line}");
                *previous = cap(&joined);
            }
            _ => out.push(line),
        }
    }
    for line in &mut out {
        line.truncate(line.trim_end().len());
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let start = out.len().saturating_sub(max_lines);
    out.split_off(start)
}

/// GitLab's section markers, with the `\r` that ends each. The name is
/// `[A-Za-z0-9_.-]`, optionally followed by `[collapsed=true]`-style options.
static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"section_(?:start|end):[0-9]+:[A-Za-z0-9_.\-]*(?:\[[^\]\r\n]*\])?\r?")
        .expect("a valid pattern")
});

/// ANSI escapes: CSI (`ESC [ ... final`), OSC (`ESC ] ... BEL` or `ESC ] ...
/// ESC \`), the character-set designations (`ESC ( B`), and any other
/// two-byte `ESC x`.
static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?|\x1b[ -/]+[0-~]|\x1b[@-_]?",
    )
    .expect("a valid pattern")
});

/// GitHub's per-line timestamp.
static GITHUB_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?Z ")
        .expect("a valid pattern")
});

/// The GitLab runner's per-line timestamp, stream and type (`01O`, `00E`),
/// and the `+` that marks a line continuing the previous one.
static GITLAB_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?Z [0-9a-fA-F]{2}[OE](\+)? ?",
    )
    .expect("a valid pattern")
});

/// A line without its runner timestamp, and whether it continues the one
/// before it. Each provider's form is stripped only on that provider's logs:
/// a GitLab job whose own output starts with a timestamp keeps it.
fn strip_timestamp(line: &str, provider: Provider) -> (&str, bool) {
    let pattern = match provider {
        Provider::Github => &*GITHUB_TIME,
        Provider::Gitlab => &*GITLAB_TIME,
    };
    match pattern.captures(line) {
        Some(found) => {
            let end = found.get(0).map(|m| m.end()).unwrap_or(0);
            (&line[end..], found.get(1).is_some())
        }
        None => (line, false),
    }
}

/// The Unicode bidirectional formatting characters: embeddings, overrides,
/// isolates and the two marks.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A line, cut to [`MAX_LINE_CHARS`] characters.
fn cap(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// The one explicit follow a log gets: the request for the `Location` a
/// provider's log endpoint redirected to.
///
/// ⛔ Refused unless the location is an absolute `https` URL with a host and
/// no user information. The request carries NO credential header and is
/// marked [`HttpRequest::anonymous`]; see the module documentation. `range`
/// asks for the tail with `Range: bytes=-N` (GitLab's CDN honours it,
/// GitHub's blob store ignores it), and the transport keeps only the tail
/// either way.
pub fn follow(
    location: Option<&str>,
    tail_bytes: usize,
    range: bool,
    from: &str,
) -> Result<HttpRequest, ClientError> {
    let refuse = |why: &str| ClientError::Unsupported {
        message: format!(
            "{from} redirected the log to {why}; bridgewatch follows a log's redirect only to \
             an https URL"
        ),
    };
    let location = location
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| ClientError::Unsupported {
            message: format!("{from} answered with a redirect that names no location"),
        })?;
    if location
        .chars()
        .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(refuse("a location that is not a URL"));
    }
    let Some((scheme, rest)) = location.split_once("://") else {
        return Err(refuse("a location that is not an absolute URL"));
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(refuse(&format!("a {scheme}:// URL")));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(refuse("a URL with no host, or with a user name in it"));
    }
    let mut headers = Vec::new();
    if range {
        headers.push(("Range".to_string(), format!("bytes=-{tail_bytes}")));
        // A range of a compressed body is a slice of the compressed bytes;
        // asking for the plain one keeps the range meaning what it says.
        headers.push(("Accept-Encoding".to_string(), "identity".to_string()));
    }
    Ok(HttpRequest {
        method: "GET",
        url: location.to_string(),
        path: display_path(location),
        headers,
        body: None,
        anonymous: true,
        tail_bytes: Some(tail_bytes),
    })
}

/// What the ring and the log show for a signed URL: `host/path`, and never
/// the query, which is the signature.
pub fn display_path(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let cut = rest.find(['?', '#']).unwrap_or(rest.len());
    format!("(redirect) {}", &rest[..cut])
}

/// Whether a status is a redirect a log may follow once.
pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Classify the answer of a log's second leg, the anonymous one.
///
/// ⚠️ Not the provider's classifier: a 403 here is a signed URL that expired
/// or was refused by a storage host, which says nothing about the account's
/// token, and must not read as "check the token's scope".
pub fn blob_error(status: u16, path: &str) -> Option<ClientError> {
    match status {
        200..=299 => None,
        300..=399 => Some(ClientError::Redirect {
            status,
            path: path.to_string(),
        }),
        404 => Some(ClientError::NotFound {
            path: path.to_string(),
        }),
        500..=599 => Some(ClientError::Server { status }),
        other => Some(ClientError::Unexpected {
            status: other,
            path: path.to_string(),
        }),
    }
}

/// The `message` of a provider's JSON error body, reduced to one short line
/// of plain text: the one piece of a write's refusal worth showing.
///
/// GitLab sends `{"message": "403 Forbidden - Job is not retryable"}` (and
/// sometimes an object or an array under `message`), GitHub
/// `{"message": "...", "documentation_url": ...}`. Anything else, or nothing,
/// is `None`.
pub fn sanitize_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value.get("message")?;
    let text = match message {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("; "),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k} {s}"),
                serde_json::Value::Array(a) => format!(
                    "{k} {}",
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                _ => k.clone(),
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => return None,
    };
    let clean: String = text
        .chars()
        .map(|c| {
            if c.is_control() || is_bidi_control(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if clean.is_empty() {
        return None;
    }
    let mut short: String = clean.chars().take(200).collect();
    if clean.chars().count() > 200 {
        short.push('…');
    }
    Some(short)
}

/// Send one request the way every request is sent: through the account's
/// in-flight bound, timed from when it got a slot, logged at `debug`, and
/// recorded in the ring. `classify` decides which answers are errors.
///
/// The JSON reads keep their own loops in each client; this is the path for
/// the requests that are not JSON reads (a log's two legs, a job action),
/// which differ in what counts as success and share everything else.
pub async fn send(
    transport: &dyn Transport,
    in_flight: &InFlight,
    ring: &RequestRing,
    request: HttpRequest,
    classify: impl Fn(&HttpResponse) -> Option<ClientError>,
) -> Result<HttpResponse, ClientError> {
    let method = request.method;
    let path = request.path.clone();
    // Before the clock starts: `ms` is the request, not the queue.
    let _slot = in_flight.acquire().await;
    let started = Instant::now();
    let result = transport.execute(request).await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(response) => {
            let error = classify(&response);
            tracing::debug!(
                method,
                path = %path,
                status = response.status,
                bytes = response.body.len(),
                truncated = response.truncated,
                ms,
                "request"
            );
            ring.record(RequestLog {
                method: method.into(),
                path: path.clone(),
                status: Some(response.status),
                ms,
                ratelimit_remaining: response.ratelimit_remaining,
                ratelimit_reset: response.ratelimit_reset,
                retry_after: response.retry_after,
                error: error.as_ref().map(ToString::to_string),
                at: chrono::Utc::now(),
            });
            match error {
                Some(e) => Err(e),
                None => Ok(response),
            }
        }
        Err(e) => {
            tracing::debug!(method, path = %path, error = %e, ms, "request failed");
            ring.record(RequestLog {
                method: method.into(),
                path,
                status: None,
                ms,
                ratelimit_remaining: None,
                ratelimit_reset: None,
                retry_after: e.retry_after(),
                error: Some(e.to_string()),
                at: chrono::Utc::now(),
            });
            Err(e)
        }
    }
}
