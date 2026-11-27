# Test coverage

How much of the code the automated tests run, measured with the project's own tooling. CI
holds each part to a floor (see "Tests" in [CONTRIBUTING.md](../CONTRIBUTING.md)); this page
records the last full measurement, with the command and the date, so the numbers can be
checked and repeated.

## Last measurement: 2026-11-22

Measured on `main` at `b8e0fd4` (Rust 1.100.0, cargo-llvm-cov 0.9.2, Node 24, vitest 5.1.0).

### Rust, line coverage per crate

Lines under each crate's `src/`, counted as `scripts/check-coverage.py` counts them.

| Crate | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: |
| bridgewatch-core | 10903 | 9805 | 89.92% | 89% |
| bridgewatch-cli | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | not measured | | | 61% |
| **core and cli together** | **13950** | **12551** | **89.97%** | |

bridgewatch-app (`src-tauri/`) was not measured in this run: it links WebKitGTK, and the
machine had no WebKitGTK development files. Its floor of 61% is the 61.31% CI's `coverage`
job measured when the floor was set (`coverage-floors.toml`). CI's `coverage` job measures it
on every pull request.

Command for the two crates measured here:

```
cargo llvm-cov --locked -p bridgewatch-core -p bridgewatch-cli \
  --json --summary-only --output-path coverage.json
```

The figures above are read from that report's per-crate totals. `scripts/check-coverage.py`
checks every crate in `coverage-floors.toml`, so on this partial report it reports
bridgewatch-app missing and exits 1. It needs a whole-workspace report, which is what CI's
`coverage` job makes, with WebKitGTK installed:

```
cargo llvm-cov --locked --workspace --json --summary-only --output-path coverage.json
python3 scripts/check-coverage.py coverage.json
```

### Frontend, every file under `src/`

Re-measured on 2026-11-26, after the unused vendored ui components were deleted and the
Svelte compiler's template fallback was left out of the report (see "Tests" in
[CONTRIBUTING.md](../CONTRIBUTING.md)).

| Measure | Covered | Total | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: |
| Lines | 3333 | 3468 | 96.10% | 96% |
| Statements | 4967 | 5257 | 94.48% | 94% |
| Branches | 2300 | 2586 | 88.94% | 88% |
| Functions | 1443 | 1526 | 94.56% | 94% |

Command: `npm ci && npx vitest run --coverage` (42 test files, 672 tests).

### Total

Rust (core and cli) and the frontend together: 15884 of 17418 lines, 91.19%. This leaves out
bridgewatch-app, so it is not the whole workspace.

## CI, the whole workspace: 2026-11-27, pull request #136

The `coverage` job on pull request #136 at `90ebebd`
([run 45974911891](https://github.com/distronode-corporation/bridgewatch/actions/runs/45974911891)),
with #137 merged into it, printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 11003 | 10448 | 94.95% | 93% |
| bridgewatch-cli | 4 | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19527** | **16539** | **84.69%** | |

## CI, the whole workspace: 2026-11-27, pull request #132

The `coverage` job on pull request #132 at `64689ca`
([run 45955220679](https://github.com/distronode-corporation/bridgewatch/actions/runs/45955220679))
printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 10874 | 10033 | 92.26% | 92% |
| bridgewatch-cli | 4 | 3047 | 2745 | 90.08% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19398** | **16123** | **83.12%** | |

The core figure rose with that pull request's tests. The cli figure is one line below the run
below with no cli change, so one cli line is reached on some runs and not others.

## CI, the whole workspace: 2026-11-22 (before #132)

CI's `coverage` job measures all three crates, bridgewatch-app included, because its runner
has WebKitGTK. Its run on `main` at `69597c0`
([run 44983580934](https://github.com/distronode-corporation/bridgewatch/actions/runs/44983580934))
printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 10903 | 9811 | 89.98% | 89% |
| bridgewatch-cli | 4 | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19427** | **15902** | **81.85%** | |

With the frontend lines from the measurement above (3333 of 3468), the whole project is 19235
of 22895 lines, 84.01%. The job's log is kept for GitHub's log retention period; this table is
the record that lasts.
