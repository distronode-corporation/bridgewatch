Not a recording. Hand-written in the shape GitHub documents for
`GET /repos/{owner}/{repo}/actions/runs`, `.../runs/{id}/jobs` and
`GET /repos/{owner}/{repo}/check-runs/{id}/annotations`, about the made-up
`acme-corp/monorepo`, for the same reason as every body in `tests/github.rs`:
a pasted GitHub response carries commit messages and the people who wrote
them. The `gh-` prefix keeps the GitLab allow-list (`kind_for_file`) off them;
the email sweep in `tests/verdict.rs` still reads them.
