# Retry and play (opt in)

bridgewatch reads by default and writes nothing. Set `actions = true` on an account and
the popover offers **retry** on a failed or canceled job and **play** on a GitLab manual
job, for that account's watches only. Each one asks first, naming the job, the project and
the pipeline, and sends one request however many times it is clicked. When the request
goes through, the watch is polled at once. A refusal is shown under the job.

```toml
[accounts.gitlab]
token = { keyring = { service = "glab:gitlab.com:token", user = "" } }
actions = true
```

- **Refused in the core, not just hidden.** With `actions` off (the default) the client
  refuses a retry or play before anything is sent, whatever asks for it: the popover, the
  CLI or a script in the webview. Turning `actions` on from Settings (or **Edit as text**)
  is one of the changes the app asks you to confirm before it writes the file, see
  [Command sources need confirmation](tokens.md#command-sources-need-confirmation).
- **Every write is logged** at `info` in the log file: the account, the project, the job
  id, the action and how it ended. Never the token.
- **GitHub has no manual jobs**, so there is no play there. Retry is GitHub's re-run of
  one job, which runs again under the same id as a new attempt of its run.
- **The CLI:** `bridgewatch retry <job-url>` and `bridgewatch play <job-url>`, see [CLI](cli.md).

What the token needs:

| Token | To read logs | To retry and play |
| --- | --- | --- |
| GitLab personal access token, or glab's login | `read_api` | `api`, and a role that may run pipelines on the project (Developer or above). `read_api` cannot run jobs; `glab auth login` creates a token with `api` unless it was narrowed. |
| GitLab sign-in (`token = { oauth = .. }`) | `read_api` | `api`. The sign-in asks for `api` only on an account with `actions = true`, so after turning it on, sign in again: a sign-in made with `read_api` is refused before a write is sent, with a message that says so. The OAuth application must allow the `api` scope too (on your own application, tick it; the built-in gitlab.com application has to allow it as well). |
| GitHub classic token, or gh's login | `repo` (private repositories) | `repo`, and write access to the repository. gh's default login includes `repo`. |
| GitHub fine-grained token | Actions: read | Actions: read and write. |
| GitHub sign-in (the bridgewatch-ci GitHub App) | Actions: read | The App needs **Actions: read and write** in its own permissions, which its owner sets on GitHub; each installation then has to approve the new permission. A GitHub App's user token asks for no scope, so signing in again changes nothing until then. |

A 403 on a write reads "this token cannot run jobs", with what it needs, and not "check
the token": the token works, it may only read. A 401 is still a bad token, and a GitHub
rate limit is still a rate limit.
