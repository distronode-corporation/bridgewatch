#!/usr/bin/env python3
"""Assert that bridgewatch's two version literals agree, and optionally that
they agree with a release tag.

    python3 scripts/check-version.py            # the two agree with each other
    python3 scripts/check-version.py v0.1.2     # ...and with this tag
    python3 scripts/check-version.py 0.1.2      # a bare version works too

The version is written down twice and neither is derived from the other:

    Cargo.toml    [workspace.package] version   -> both binaries AND the BUNDLE NAMES
    package.json  version                       -> the npm package

src-tauri/tauri.conf.json deliberately has NO `version`, so Tauri falls back to
the crate's, which src-tauri/Cargo.toml inherits from the workspace. Confirmed in
the source of the versions this repository pins, not assumed from the schema's
wording: tauri-cli 2.11.5 (`RustAppSettings::new`) resolves the workspace version
and hands it to tauri-bundler 2.9.4, whose `version_string()` names the .dmg,
.deb, .rpm and .AppImage and fills CFBundleShortVersionString; tauri-codegen 2.6.3
falls back to `CARGO_PKG_VERSION` for the runtime's package info; and
tauri-action v1.0.0 (`getCargoManifest`) follows `version.workspace = true` to the
same value. So this script also refuses a `version` reappearing in either Tauri
config: one there would silently take over the bundle names again.

Bundle names are the reason any of this matters: a `v0.1.1` tag on a tree that
still says `0.1.0` produces a release called 0.1.1 containing files called
`bridgewatch_0.1.0_*`. Nothing downstream objects: the build succeeds, the
upload succeeds, and the mismatch is visible only to whoever reads the asset list.

Run by the `repo` job in .github/workflows/ci.yml on every push and pull request
(no argument, so it only enforces internal agreement) and by the `guard` job in
release.yml with the tag (so a mistagged tree never reaches a build).
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The Tauri configs a build reads: the base file, and the overlay release.yml
# passes with `--config`, which would override the base if it set a version.
TAURI_CONFIGS = ("src-tauri/tauri.conf.json", "src-tauri/tauri.licenses.conf.json")


def cargo_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)["workspace"]["package"]["version"]


def package_json_version() -> str:
    return json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]


def tauri_configs_with_a_version() -> list[str]:
    return [
        name
        for name in TAURI_CONFIGS
        if "version" in json.loads((ROOT / name).read_text(encoding="utf-8"))
    ]


def main(argv: list[str]) -> int:
    found = {
        "Cargo.toml": cargo_version(),
        "package.json": package_json_version(),
    }

    width = max(len(name) for name in found)
    for name, value in found.items():
        print(f"  {name:<{width}}  {value}")

    stray = tauri_configs_with_a_version()
    if stray:
        print(
            f"\nERROR: {', '.join(stray)} sets `version`. Remove it: Tauri reads the\n"
            f"       version from Cargo.toml, and a literal here overrides it in the\n"
            f"       bundle names without anything comparing the two.",
            file=sys.stderr,
        )
        return 1

    distinct = set(found.values())
    if len(distinct) != 1:
        print(
            f"\nERROR: the two version literals disagree: {sorted(distinct)}",
            file=sys.stderr,
        )
        return 1

    version = distinct.pop()

    if len(argv) > 1:
        # Accept `v0.1.2` and `0.1.2`. A tag with any other shape is a mistake
        # worth stopping on rather than normalising away: release.yml only fires
        # on `v*.*.*`, so anything else here means the caller is not the tag.
        expected = argv[1]
        expected = expected[1:] if expected.startswith("v") else expected
        if expected != version:
            print(
                f"\nERROR: the tree says {version} but the tag says {argv[1]}.\n"
                f"       Bump Cargo.toml and package.json, then move the tag.",
                file=sys.stderr,
            )
            return 1
        print(f"\nversion {version} agrees with tag {argv[1]}")
        return 0

    print(f"\nversion {version} is consistent across both files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
