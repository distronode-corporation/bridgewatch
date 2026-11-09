# Setup and sign-in

## First run: the setup wizard

When there is no config file at the default path, bridgewatch opens a setup wizard. It is
optional: **Skip, I'll edit config.toml** writes nothing and opens Settings at **Edit as
text** in the **Advanced** pane, where you can write the file yourself (start from
[`examples/distronode.toml`](../examples/distronode.toml) for GitLab or
[`examples/github.toml`](../examples/github.toml) for GitHub). Saving from that tab creates
it.

The wizard's steps:

1. **Account.** GitLab (the default) or GitHub Actions; gitlab.com or a self-managed
   URL, or github.com or a GitHub Enterprise URL; and a token source. On gitlab.com and
   github.com that starts on **Sign in with GitLab** (or GitHub): approve bridgewatch in
   your browser with a short code, and the sign-in is kept in the system keychain and
   renews itself. The others are below it: the provider CLI's keyring item (glab's or
   gh's, offered when one exists for that host, and chosen first where sign-in needs an
   application of your own), a pasted token (stored in bridgewatch's own keyring entry),
   an environment variable, or a command.
   **Test connection** names the user the token authenticates as. On GitLab it warns
   when the token is a project access token or lacks the `read_api` scope; on GitHub it
   lists a classic token's scopes and warns when `repo` is missing, see
   [Tokens for GitHub](github-actions.md#tokens-for-github).
   **One-click setup:** once the sign-in or Test connection succeeds, the step lists the
   account's ten most recently active projects (or repositories), newest first, with the
   top three ticked. Untick any, then **Watch N projects**: each gets the best deploy
   marker from the step 4 ranking, steps 2 to 4 are skipped, and Finish writes all of
   them at once (the first is primary unless the file has one; the rest are secondary).
   A project the file already watches on that branch is not added again. **Next** sets
   up one project step by step instead.
2. **Project.** Pick from the projects (or repositories) the token can list, or type an
   id, a `group/project` path or a project URL on GitLab, or `owner/repo` or a
   repository URL on GitHub. A GitLab project access token cannot list projects, so it
   always gets the typing form.
3. **What to watch.** The default branch and push pipelines are pre-filled. On GitHub
   the events are workflow events, and an optional workflow file narrows the watch to
   one workflow; or **One row per push** groups every workflow of a push into one row
   (`group = "commit"`) and offers the workflows every push must start (`expect`), see
   [the wizard on GitHub](github-actions.md#quick-start-the-wizard). Optionally add a secondary watch for scheduled pipelines on the same
   branch, and on GitLab one for a preflight ref glob such as `pf/*`.
4. **Deploy detection.** Suggested marker jobs, ranked from the latest pipeline's (or
   workflow run's) job names and stages, the parent's and its child pipelines' down to
   three levels (at most 12 children, nearest first; a grandchild's job is labelled with
   the path to it), or none. A job
   whose name or stage leads with a checking word (`verify`, `test`, `lint`, `check`,
   `validate`, `smoke`) is never offered, whatever else its name says.
5. **Notifications and polling.** Which events notify, launch at login, and the live
   poll interval (5 s for GitLab, 30 s for GitHub).
6. **Review config.toml.** The exact file it will write, then **Finish**.

The wizard writes through the same path as Settings. Run on an existing file, it edits
that file rather than replacing it, and it refuses to write text that does not load or
that contains something shaped like a GitLab token (or, for a GitHub account, a GitHub
token). When the file already has a primary watch, the new watch is added as
`role = "secondary"`, so it does not take over the tray icon; tick **Make this watch
primary too** on the review step to keep it primary. Re-open the wizard any time from the
tray menu (**Setup wizard…**) or from Settings.

`bridgewatch init` is the same logic from the command line, see [CLI](cli.md).

A `--config` or `BRIDGEWATCH_CONFIG` path that does not exist is treated as a typo, not
as a first run: bridgewatch reports it and creates nothing.

## Sign in with GitHub or GitLab

Instead of a token, an account can sign in. bridgewatch uses the OAuth device flow: it
shows a short code, you enter it on the provider's own page in your browser, and
bridgewatch waits until the provider says yes. There is no client secret and no local
redirect listener.

- **In the app.** On github.com and gitlab.com, the wizard's **Account** step and an
  account's token field in Settings offer **Sign in with GitHub** or **Sign in with
  GitLab**. The code is shown large, with **Copy** and a button that opens the
  provider's page. In Settings a signed-in account also has **Sign in again** and **Sign
  out**. On GitHub the app sees only repositories on accounts it is installed on, and
  Settings links to **Install it on an organisation**.
- **In the CLI.** The account's token source is `token = { oauth = true }`. Then:

  ```
  bridgewatch auth login --account gh     # prints the page to open and the code to enter
  bridgewatch auth status                 # who each account is signed in as, until when, which scopes
  bridgewatch auth logout --account gh    # removes the sign-in from the credential store
  ```

  The npm package runs the same commands: `npx bridgewatch auth login --account gh`.
  `auth status` never prints a token. It lists own-token accounts too, with the server
  each token is bound to and when it expires (see
  [Tokens stay with their server](tokens.md#tokens-stay-with-their-server)), and
  `--account <name>` limits it to one account. It exits 70 unless every account it
  lists is usable: one that is not signed in, whose stored sign-in was issued by another
  server or for another application than the account now names (such a sign-in is never
  used), that has no own token stored, or whose own token was saved for another server.
- **Which hosts.** The built-in applications cover github.com (the GitHub App
  [bridgewatch-ci](https://github.com/apps/bridgewatch-ci)) and gitlab.com only. GitHub
  Enterprise Server and self-managed GitLab need an OAuth application registered on that
  server, named by its client id: `token = { oauth = { client_id = "..." } }`. A client
  id is not a secret.
- **Where the tokens are kept.** In the OS credential store (the macOS keychain, or the
  Secret Service on Linux), under service `bridgewatch:oauth:<account>` and user
  `oauth`. The config file holds only `token = { oauth = true }`. Signing out deletes
  that item but does not revoke the grant at the provider; revoke it there if you want
  the tokens dead rather than forgotten.

Scopes and permissions, refresh, and what is never logged are in
[SECURITY.md](../SECURITY.md#sign-in-tokens-oauth). For retry and play, see the table in
[Retry and play](retry-and-play.md).
