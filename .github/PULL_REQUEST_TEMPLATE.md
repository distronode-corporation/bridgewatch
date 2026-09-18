<!-- Thanks for the contribution. Delete any section that genuinely does not apply. -->

## What changed

<!-- One or two sentences. The diff says what; this says it in words. -->

## Why

<!-- The problem, not the patch. If it fixes an issue, link it (Fixes #123). If you hit
     it in practice rather than reading the code, say what you saw. -->

## How it was tested

<!-- Commands you actually ran, and what they said. "Should work" is not a test.
     This list is exactly what CI runs, in the order it runs it. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo check -p bridgewatch-app --locked` (CI runs it on Ubuntu 22.04, the Linux
      release floor, and on macOS, plus an `x86_64-apple-darwin` cross-check)
- [ ] `npm run typecheck` (tsc + the referenced project) and `npm run check` (svelte-check)
- [ ] `npx vitest run` (including the Settings registry ↔ JSON Schema parity gate)
- [ ] `npm run build` (the Vite production build, which is where Tailwind compiles)
- [ ] Touches `src-tauri/icons/`, and so `python3 src-tauri/icons/generate.py --check` was run
- [ ] Bumps the version, and so `python3 scripts/check-version.py` was run — it is
      written in `Cargo.toml`, `package.json` and `src-tauri/tauri.conf.json`, and
      Tauri names every release asset after the third one

## Verdict changes

<!-- Only if you touched crates/bridgewatch-core/src/verdict/. -->

The verdict engine is the whole product, and every rule in it is a claim about a real
pipeline shape. A change here is much easier to review with a recording than with
prose:

- [ ] A fixture under `crates/bridgewatch-core/tests/fixtures/` covers the new or
      changed case, with its `expected.json`
- [ ] The README's rule table still describes what the code does

## Anything a reviewer should know

<!-- A decision you were unsure about, something you deliberately left out, a follow-up
     you think is needed. Saying "I could not test X" here is useful, not a problem —
     the macOS bundling path, for instance, is only exercised at release time. -->
