# Tokens

bridgewatch never writes a token to its config file, and the config loader refuses a
literal token string with an error that lists the alternatives. The file records *where*
the token comes from; it is resolved when the poller starts and again on every reload.

```toml
token = { keyring = { service = "glab:gitlab.com:token", user = "" } }  # glab's item
token = { keyring = { service = "gh:github.com", user = "" } }          # gh's active account
token = { own = true }                                                  # bridgewatch's own item
token = { env = "BRIDGEWATCH_TOKEN_GITLAB" }                            # an environment variable
token = { command = ["pass", "gitlab/pat"] }                            # a program's output
token = { oauth = true }                                                # a sign-in, see setup.md
```

| Source | Notes |
| --- | --- |
| `keyring` | Any OS keyring item, addressed by service and user. Use it to reuse glab's or gh's login. |
| `own` | bridgewatch's own keyring item, service `bridgewatch:<account>`, written when you paste a token into Settings or the wizard. It is bound to the server it was saved for (see below). This is the default when an account has no `token` key. |
| `env` | Read from the process environment, so it works when bridgewatch is started from a shell or a service unit that sets the variable. |
| `oauth` | A sign-in instead of a token: `{ oauth = true }` on github.com and gitlab.com, or `{ oauth = { client_id = "..." } }` with an application of your own. The tokens are kept in the OS credential store, see [Sign in with GitHub or GitLab](setup.md#sign-in-with-github-or-gitlab). |
| `command` | Runs the program (no shell) with stdin closed and a 10 s timeout, and uses the first line of its stdout. Writing one from Settings or the wizard needs an explicit confirmation, see below. |

A token source table names exactly one source; a table naming two is refused.

## Token health

bridgewatch tells four things apart, per account: a token that **expires** soon, one that
**has expired**, one that was **revoked**, and one the server does not accept (**invalid**).
Each shows in a strip at the top of the popover for as long as it is true, and is one
notification, never repeated. An expiry warns twice: 72 hours before, and 24 hours before. An
expiry that is first seen inside 24 hours gets only the 24-hour warning.

| Credential | Where the expiry comes from | What it costs |
| --- | --- | --- |
| GitLab personal, project or group access token | `expires_at` from `GET /personal_access_tokens/self` (00:00 UTC on that date) | one request per token each time a poller starts (the app starting or reloading its configuration, or a CLI command such as `check`, `watch` or `wait`), asked again only after a rate limit, a server error or a network failure (DNS, TLS, a timeout or a reset connection), and only on a tick where that account polls; plus one expiry lookup per bound own token on every `bridgewatch auth status` run |
| GitHub personal access token with an expiry | the `github-authentication-token-expiration` response header | nothing: it is on every response |
| A sign-in | the refresh token's expiry, stored with the sign-in; for a sign-in that cannot renew itself, the access token's | nothing |

GitLab's 401 says whether a token was revoked or has expired. GitHub's says only "Bad
credentials", so a GitHub token reads as expired when it is refused past a known expiry,
and as invalid otherwise. `bridgewatch auth status` shows each own token's expiry.

## Tokens stay with their server

A token you paste into Settings or the wizard is stored with the server it was saved for:
the scheme, host and port of the account's `base_url` at that moment
(`https://GitLab.com:443/` and `https://gitlab.com` are the same server). It is sent to
that server and nowhere else, the same rule a sign-in follows.

If the account's `base_url` changes afterwards, by hand, by `bridgewatch init --account`,
or by anything else that edits the file, the stored token is **not sent** to the new
address. `check`, `watch`, `wait`, `log`, `retry` and `play` exit 70 with:

```
the stored token of account "gl" was saved for https://a.example, and the account now
points at https://b.example, so it is not sent. Save a token for https://b.example in
Settings, or put base_url back
```

In the app only that account's watches show the error; the other accounts keep polling.
Settings shows "Stored for https://a.example; paste one for https://b.example" on the
account's token field. `bridgewatch auth status` lists every own-token account with the
server its token is bound to. `config validate` stays offline and reads no credential, so
it cannot see a binding.

A `keyring`, `env` or `command` source is not bound: bridgewatch does not own those
credentials, and moving such an account to another host still asks for confirmation in
the app, because its token would go there.

## Upgrading from 1.x

bridgewatch 1.x stored the bare token. 2.0 binds such a value the first time it uses it,
to the server the account names at that moment: it reads the value, writes it back bound,
and only then sends it. If the write fails, nothing is sent. The app and the CLI both do
this; on macOS, the CLI rewriting an item the app created may ask once for keychain
access. The setup wizard never binds one to the address typed into the form: re-run on a
1.x value the app has not used yet, it asks for the token again.

- **A move before that first use keeps the token with the old server.** Moving the
  account to another host in Settings, in the wizard, with `bridgewatch init --account`,
  or by editing the file while the app runs first binds the 1.x value to the server the
  account is leaving, so the new server never receives it. If the keychain is locked or
  cannot be read, the move is refused with that error. The app cannot see an edit made
  while it is not running as a move: the value is then bound to the server the file names
  when it is first used.

- **After a repoint, save the token again.** Once an entry is bound, changing the
  account's `base_url` stops it being sent (exit 70, above). Paste the token for the new
  server in Settings (**Accounts**, the account's sign-in panel) or in the wizard, and it
  is bound to that server instead. A sign-in is the same: sign in again.
- If you changed an account's `base_url` under 1.x, 1.x had already sent the token to the
  new address, and 2.0 binds it there. Paste the token again if that is not the server it
  belongs to.
- **Downgrading.** bridgewatch 1.3 cannot read an entry 2.0 has written. The released
  1.3.0 CLI refuses it ("cannot go in an HTTP header") and exits non-zero without sending
  anything, whichever server the account points at; CI proves this against the real
  1.3.0 release on the macOS keychain and on the Linux Secret Service. To go back to 1.3,
  paste the token again in its Settings; 2.0 binds that value again on its next use.
- Scripts that read the keychain item directly now get a small document,
  `bw-bound-v1:` followed by a tab and JSON with the token and the server, instead of the
  bare token.

Other changes a 1.x user meets: a new github.com or gitlab.com account signs in by
default (in the wizard, in Settings' **Add account**, and in `init` with no token flag);
a failing pipeline is announced once, more than three notifications at once arrive as
one, and a passing push after a failure says "fixed again"; Settings has five panes
instead of seven tabs. The [CHANGELOG](../CHANGELOG.md) lists them all under Breaking.

## Reusing glab's login

If you have run `glab auth login`, the token is already in your OS keyring under service
`glab:<host>:token` (so `glab:gitlab.com:token` for gitlab.com) with an **empty** user
name. The empty user is required: glab's keyring library writes the item that way and a
lookup with any other user misses. The wizard and `bridgewatch init --glab` write this
source for you. The self-managed `glab:<host>:token` form follows glab's naming but has
not been checked against a live self-managed install. gh's login works the same way, see
[Reusing gh's login](github-actions.md#reusing-ghs-login).

On macOS glab's keyring library (which gh uses too) stores the value wrapped as
`go-keyring-base64:` plus base64 (older versions: `go-keyring-encoded:` plus hex);
bridgewatch unwraps either wrapper, and passes an unwrapped value through unchanged, so the same config works on macOS and Linux.

macOS may ask whether bridgewatch may read an item another program created. **Always
Allow** stops it asking again each time the token is read.

⚠️ `secret-tool lookup service glab:gitlab.com:token username ""` **prints the token**.
If you check the Linux item by hand, pipe it to `wc -c`, never into a log or an issue.

## Command sources need confirmation

A `command` source turns the config file into something that runs a program with your
permissions every time a token is needed. So when Settings or the wizard would write one
(a new one, or a changed command), the app writes nothing and shows the exact command for
you to confirm first. The same applies to a change that would send a credential to a host
it has not been sent to before: a new `base_url` for an account that already has a
token, or a keyring or environment source on a host no account used, and to turning
`actions` on for an account (see [Retry and play](retry-and-play.md)). Edits you make in your own editor are not
subject to this: the check is on the app's write path, and the file is yours.
`config validate` warns about every command source.

## GitLab project access tokens

A GitLab project access token can read only its own project, so it cannot list projects.
The wizard then asks you to type the project id or path instead of offering a list, and a
watch can always name a project by id. To watch several projects, use a personal access
token with `read_api`, or add one account per project.
