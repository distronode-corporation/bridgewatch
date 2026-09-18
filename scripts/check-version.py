#!/usr/bin/env python3
"""Assert that bridgewatch's three version literals agree, and optionally that
they agree with a release tag.

    python3 scripts/check-version.py            # the three agree with each other
    python3 scripts/check-version.py v0.1.2     # ...and with this tag
    python3 scripts/check-version.py 0.1.2      # a bare version works too

The version is written down three times and nothing derives one from another:

    Cargo.toml                 [workspace.package] version   -> both binaries
    package.json               version                       -> the npm package
    src-tauri/tauri.conf.json  version                       -> the BUNDLE NAMES

The third is the one that bites. Tauri names every asset after it, so a `v0.1.1`
tag on a tree whose tauri.conf.json still says `0.1.0` produces a release called
0.1.1 containing four files called `bridgewatch_0.1.0_*`. Nothing downstream
objects: the build succeeds, the upload succeeds, and the mismatch is visible
only to whoever reads the asset list.

Run by the `versions` job in .github/workflows/ci.yml on every push and pull
request (no argument, so it only enforces internal agreement) and by the `guard`
job in release.yml with the tag (so a mistagged tree never reaches a build).
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def cargo_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)["workspace"]["package"]["version"]


def package_json_version() -> str:
    return json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]


def tauri_conf_version() -> str:
    conf = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    # `version` may be omitted in tauri.conf.json, in which case Tauri reads the
    # crate's version. It is present here, and a missing one is a real finding
    # rather than something to paper over, so this raises instead of defaulting.
    return conf["version"]


def main(argv: list[str]) -> int:
    found = {
        "Cargo.toml": cargo_version(),
        "package.json": package_json_version(),
        "src-tauri/tauri.conf.json": tauri_conf_version(),
    }

    width = max(len(name) for name in found)
    for name, value in found.items():
        print(f"  {name:<{width}}  {value}")

    distinct = set(found.values())
    if len(distinct) != 1:
        print(
            f"\nERROR: the three version literals disagree: {sorted(distinct)}",
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
                f"       Bump Cargo.toml, package.json and src-tauri/tauri.conf.json,\n"
                f"       then move the tag.",
                file=sys.stderr,
            )
            return 1
        print(f"\nversion {version} agrees with tag {argv[1]}")
        return 0

    print(f"\nversion {version} is consistent across all three files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
