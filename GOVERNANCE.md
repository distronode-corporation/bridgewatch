# Governance

bridgewatch is owned by Distronode Corporation and published from the `distronode-corporation`
organization on GitHub. This file says who decides what, how a change gets in, how a release
goes out, how a vulnerability report is handled, and what happens if the maintainer is no
longer available. It is the bridgewatch-specific version of the organization's
[GOVERNANCE.md](https://github.com/distronode-corporation/.github/blob/main/GOVERNANCE.md);
where the two differ, this file is the one that applies here.

## How decisions are made

bridgewatch is maintained by a company, not a foundation: there is no elected committee and no
vote. The maintainer decides, on behalf of Distronode Corporation.

A change to the code or the documentation is proposed as a pull request against `main`,
discussed there, and merged only once the code owner approves it. Larger questions (a new
provider, a change to how a verdict is decided, a breaking change to the config file or to
`check --json`, a change of scope) are raised as an issue or a discussion first, and the
decision and its reason are written on that issue, discussion or pull request, so they are
public with the change. A breaking change is also listed in the Breaking section of
[CHANGELOG.md](CHANGELOG.md).

A disagreement is settled in the thread where it comes up, by the evidence: a failing test, a
recorded pipeline, a line of the provider's API documentation. If it is not settled there, the
code owner decides and says why in public. bridgewatch is Apache-2.0, so a disagreement that
cannot be settled never leaves anyone stuck: you are free to fork.

What the repository enforces today:

- **Every path has a code owner.** `.github/CODEOWNERS` assigns every path to the maintainer,
  and names `.github/workflows/` and `src-tauri/capabilities/` again because a change to
  either lands on users rather than on the tree. Every pull request therefore requests the
  maintainer's review.
- **`main` takes changes only through a pull request.** The default branch ruleset requires
  an approving review from the code owner, given after the most recent push (a new push
  dismisses an earlier approval); every review thread resolved; and the required checks
  passing on a branch that is up to date with `main`. The required checks are the three
  `rust` legs, `msrv`, `repo`, `frontend`, `cargo-deny`, `zizmor`, `dependency-review` and
  `review-threads`, and CodeQL must report no error and no security alert of high severity or
  above. The ruleset refuses force pushes and deletion of `main`, keeps its history linear,
  and allows squash or rebase merges only. One team in the organization can bypass it.
- **The author never approves their own change.** Pull requests are often written by the
  organization's own account, which cannot approve them, so the approval always comes from a
  different account.
- **Review threads must be resolved.** The `review-threads` check is red while any thread on a
  pull request is open, whether a person or a review bot started it.
- **Dependabot gets no exception.** Its patch and minor updates are queued to merge
  themselves, but the merge happens only once the same ruleset is satisfied, the code
  owner's approval included; a major update is left for a person to merge (see
  [Installing bridgewatch](docs/install.md#what-guards-the-code-and-the-release-path)).

What makes a pull request acceptable (the style rules, the test and coverage rules, and how to
run what CI runs) is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Roles and responsibilities

| Role | Who | Responsibilities |
| --- | --- | --- |
| Contributor | Anyone who opens an issue, a discussion or a pull request | Follow [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Say why a change is needed, with its tests and docs in the same pull request. |
| Reviewer | Anyone may review a pull request in public; a review counts toward merging only from the code owner | Read the change against CONTRIBUTING.md and, for anything that touches tokens, links, IPC or the verdict script, against [SECURITY-MODEL.md](SECURITY-MODEL.md). Ask for changes in a review thread, so it has to be resolved before the merge. |
| Maintainer and code owner | The account named in `.github/CODEOWNERS`, acting for Distronode Corporation | Approve every change to `main`, triage issues and discussions, keep CI green and dependencies current, keep the docs consistent with the code, and decide on the project's direction ([ROADMAP.md](ROADMAP.md)). |
| Release manager | The maintainer, or any organization owner, for the release in hand | Make the release as described under [How a release is made](#how-a-release-is-made). |
| Security contact | Distronode Corporation, through private vulnerability reporting and opensource@distronode.com | Handle reports as described in [SECURITY.md](SECURITY.md#how-a-report-is-handled). |

One account can hold several roles. CODEOWNERS is the public record of who holds the code
owner role; GitHub ignores an entry there without write access, so whoever is named in it is
also a maintainer. Write access, and with it the maintainer role, is given by Distronode
Corporation to people who have shown over a run of contributions and reviews that they know
the project's rules. If you would like to take on more, say so in a
[discussion](https://github.com/distronode-corporation/bridgewatch/discussions).

## How a release is made

A release is a `vX.Y.Z` tag on `main`, and
[`.github/workflows/release.yml`](.github/workflows/release.yml) builds, signs and publishes
everything from that tag. The release manager's part:

1. A pull request bumps the version in the three places it is written down (`Cargo.toml`,
   `package.json` and `packaging/npm/package.json`; `python3 scripts/check-version.py`
   checks they agree) and fills in the version's section of [CHANGELOG.md](CHANGELOG.md),
   which becomes the GitHub Release text.
2. The checks in [tools/real-device/CHECKS.md](tools/real-device/CHECKS.md) are run on Linux
   and macOS against the commit that will be tagged.
3. Once that pull request is merged, the release manager pushes the tag on the merged commit.
4. The workflow refuses a tag whose commit is not on `main` or whose version does not match
   the three files, builds every asset, signs and notarises the macOS ones, attaches a build
   provenance attestation over all of them, checks that the attestations verify, and only
   then publishes the release, which until then is a draft. The `npm` job then publishes the
   same CLI binaries to npm with provenance. What is signed and how to check it is in
   [Verifying a release](docs/verifying-releases.md).

A ruleset refuses moving or deleting a release tag (repository administrators can bypass
it), and a bad release is never re-tagged: it gets a new patch version. The tags themselves
are annotated but not signed.

## Vulnerability response

Report a vulnerability privately, never in a public issue: through
[GitHub's private vulnerability reporting](https://github.com/distronode-corporation/bridgewatch/security/advisories/new),
or by email to opensource@distronode.com. The steps from report to advisory, the supported
versions, what is in scope and how reporters are credited are in
[SECURITY.md](SECURITY.md#how-a-report-is-handled).

## Continuity

The project does not depend on one person's account. The `distronode-corporation` GitHub
organization has more than one owner: besides the maintainer and the organization's own
account, a second owner account, `distronode-legal`, is held by Distronode Corporation's
legal function. An owner can give write access, change CODEOWNERS and the rulesets, and run
the workflows, so if the maintainer is lost or unavailable, Distronode Corporation can, within
one week, create and close issues, name a new code owner, accept changes and make a release.

Making a release needs no one's personal credential or device:

- The GitHub Release, its assets and the build provenance attestation use the workflow's own
  repository token and its OIDC identity. The attestation is signed through Sigstore with a
  short-lived certificate issued to the workflow, so there is no long-lived signing key for
  anyone to hold.
- The npm package is published through npm trusted publishing: npm exchanges the workflow's
  OIDC token for a one-time publish token, so no npm token is stored.
- The macOS signing identity and the notarisation key belong to Distronode Corporation and are
  stored as secrets of the repository's `release` environment, which only a `v*` tag can
  reach. The workflow reads them when it runs; no branch and no pull request can.

## Changing this document

A change to this file is a pull request, under the same rules as any other change.
