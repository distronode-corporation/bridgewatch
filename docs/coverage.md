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

| Measure | Covered | Total | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: |
| Lines | 3438 | 3645 | 94.32% | 94% |
| Statements | 5055 | 5469 | 92.43% | 92% |
| Branches | 2311 | 2749 | 84.06% | 83% |
| Functions | 1485 | 1635 | 90.82% | 90% |

Command: `npm ci && npx vitest run --coverage` (37 test files, 627 tests).

### Total

Rust (core and cli) and the frontend together: 15989 of 17595 lines, 90.87%. This leaves out
bridgewatch-app, so it is not the whole workspace.
