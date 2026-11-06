"use strict";

// Shared by the tests: a scratch directory of their own (packaging/npm/test/.scratch,
// gitignored, removed afterwards) and a fake native binary to stand in for the CLI.

const fs = require("node:fs");
const path = require("node:path");

const PACKAGE_DIR = path.resolve(__dirname, "..");
const WRAPPER = path.join(PACKAGE_DIR, "bin", "bridgewatch.js");
const SCRATCH_ROOT = path.join(__dirname, ".scratch");

function makeScratch(name) {
  fs.mkdirSync(SCRATCH_ROOT, { recursive: true });
  return fs.mkdtempSync(path.join(SCRATCH_ROOT, `${name}-`));
}

function removeScratch(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  try {
    fs.rmdirSync(SCRATCH_ROOT); // only once the last test file is done with it
  } catch {
    // another test file still has a directory in it
  }
}

// A Node script run directly through its shebang, as the wrapper runs the real binary.
//   exit N          exit with code N
//   argv ...        print the arguments it was given, as JSON
//   self-kill SIG   send SIG to itself (no handler, so it dies of it)
//   wait            print "ready <pid>", then exit 10, 11 or 12 on SIGINT, SIGTERM or
//                   SIGHUP, printing which arrived (98 if nothing arrives in 30 s)
const FAKE_BINARY = `#!${process.execPath}
"use strict";
const [command, ...rest] = process.argv.slice(2);
if (command === "exit") process.exit(Number(rest[0]));
if (command === "argv") {
  process.stdout.write(JSON.stringify(rest) + "\\n");
  process.exit(0);
}
if (command === "self-kill") {
  process.kill(process.pid, rest[0]);
  setTimeout(() => process.exit(97), 30000);
} else if (command === "wait") {
  const codes = { SIGINT: 10, SIGTERM: 11, SIGHUP: 12 };
  for (const signal of Object.keys(codes)) {
    process.on(signal, () => {
      process.stdout.write("got " + signal + "\\n");
      process.exit(codes[signal]);
    });
  }
  // Never outlive a test run that went wrong (a wrapper killed before it could forward).
  setTimeout(() => process.exit(98), 30000);
  process.stdout.write("ready " + process.pid + "\\n");
} else {
  process.stderr.write("fake binary: unknown command " + command + "\\n");
  process.exit(99);
}
`;

function writeFakeBinary(file, mode = 0o755) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, FAKE_BINARY);
  fs.chmodSync(file, mode);
}

module.exports = { PACKAGE_DIR, WRAPPER, makeScratch, removeScratch, writeFakeBinary };
