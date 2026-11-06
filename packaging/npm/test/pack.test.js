"use strict";

// What `npm pack` ships, in the state the release workflow packs it: a copy of this
// directory with each CLI tarball extracted into vendor/<target>/ (so the binary arrives
// next to the tarball's other files) and LICENSE, NOTICE and
// THIRD-PARTY-LICENSES-cli.txt added at the top. Exactly nine files may come out.
//
// Why package.json's `files` entries start with "/": npm reads them with gitignore
// rules, where a bare name like "LICENSE" matches at any depth, so vendor/<target>/LICENSE
// from a tarball would ship too. Anchored, each entry is one path.

const assert = require("node:assert/strict");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { after, before, test } = require("node:test");

const { PACKAGE_DIR, makeScratch, removeScratch, writeFakeBinary } = require("./helpers.js");

const TARGETS = ["universal-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];

const EXPECTED = [
  "LICENSE",
  "NOTICE",
  "README.md",
  "THIRD-PARTY-LICENSES-cli.txt",
  "bin/bridgewatch.js",
  "package.json",
  "vendor/aarch64-unknown-linux-gnu/bridgewatch",
  "vendor/universal-apple-darwin/bridgewatch",
  "vendor/x86_64-unknown-linux-gnu/bridgewatch",
];

let scratch;

before(() => {
  scratch = makeScratch("pack");
  // Everything in this directory, tests included, so nothing is left out of the copy that
  // could have leaked into the package.
  // (fs.cpSync refuses a copy into its own source, so test/ goes entry by entry,
  // leaving out test/.scratch, which holds this copy.)
  for (const entry of fs.readdirSync(PACKAGE_DIR)) {
    if (entry === "test") continue;
    fs.cpSync(path.join(PACKAGE_DIR, entry), path.join(scratch, entry), { recursive: true });
  }
  for (const entry of fs.readdirSync(__dirname)) {
    if (entry === ".scratch") continue;
    fs.cpSync(path.join(__dirname, entry), path.join(scratch, "test", entry), { recursive: true });
  }
  for (const target of TARGETS) {
    const dir = path.join(scratch, "vendor", target);
    writeFakeBinary(path.join(dir, "bridgewatch"));
    // What else a tarball could carry next to the binary; none of it may ship.
    for (const stray of ["LICENSE", "NOTICE", "THIRD-PARTY-LICENSES-cli.txt", "README.md", "extra.txt"]) {
      fs.writeFileSync(path.join(dir, stray), "stray\n");
    }
  }
  for (const name of ["LICENSE", "NOTICE", "THIRD-PARTY-LICENSES-cli.txt"]) {
    fs.writeFileSync(path.join(scratch, name), `${name}\n`);
  }
  fs.writeFileSync(path.join(scratch, "bin", "extra.js"), "// not shipped\n");
});

after(() => removeScratch(scratch));

test("npm pack ships exactly the nine files, the binaries executable", (t) => {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const result = childProcess.spawnSync(npm, ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: scratch,
    encoding: "utf8",
  });
  if (result.error && result.error.code === "ENOENT") {
    t.skip("npm is not on PATH");
    return;
  }
  assert.equal(result.status, 0, result.stderr);
  const [packed] = JSON.parse(result.stdout);
  const files = packed.files.map((file) => file.path).sort();
  assert.deepEqual(files, [...EXPECTED].sort());
  for (const file of packed.files) {
    if (file.path.startsWith("vendor/")) {
      assert.equal(file.mode & 0o111, 0o111, `${file.path} is not executable in the package`);
    }
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(PACKAGE_DIR, "package.json"), "utf8"));
  assert.equal(packed.name, "bridgewatch");
  assert.equal(packed.version, manifest.version);
});

test("package.json: no scripts, no dependencies, the provenance repository", () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(PACKAGE_DIR, "package.json"), "utf8"));
  for (const field of ["scripts", "dependencies", "optionalDependencies", "peerDependencies"]) {
    assert.equal(manifest[field], undefined, `package.json has ${field}`);
  }
  assert.deepEqual(manifest.bin, { bridgewatch: "bin/bridgewatch.js" });
  assert.deepEqual(manifest.repository, {
    type: "git",
    url: "git+https://github.com/distronode-corporation/bridgewatch.git",
  });
  assert.deepEqual(manifest.os, ["darwin", "linux"]);
  assert.deepEqual(manifest.cpu, ["x64", "arm64"]);
  assert.equal(manifest.license, "Apache-2.0");
});
