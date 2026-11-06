#!/usr/bin/env node
// The npm entry point for the bridgewatch CLI. It picks the native binary the release
// workflow put under vendor/<target>/ for this machine, runs it with this process's
// arguments and terminal, and exits the way the binary did. Plain CommonJS with no
// dependencies, so it runs on every Node the package's `engines` admits.
//
// Signals: a terminal sends Ctrl-C (SIGINT) to the whole foreground process group, so
// the binary already receives it and owns what happens next. This process ignores
// SIGINT so that it outlives the binary and reports its exit. SIGTERM and SIGHUP are
// usually aimed at this process alone (a supervisor, a closed session), so they are
// forwarded. A binary killed by a signal is reported by dying of the same signal.
//
// Required as a module (the tests), it only exports its helpers. The
// BRIDGEWATCH_NPM_TEST_* variables exist for those tests and nothing else.

"use strict";

const childProcess = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const RELEASES_URL = "https://github.com/distronode-corporation/bridgewatch/releases";
const MIN_GLIBC = [2, 35];

// process.platform + "-" + process.arch -> the release target under vendor/.
const TARGETS = {
  "darwin-x64": "universal-apple-darwin",
  "darwin-arm64": "universal-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
};

const FORWARDED_SIGNALS = ["SIGTERM", "SIGHUP"];

// "2.35" -> [2, 35]; anything that is not two dot-separated integers -> null.
function parseGlibcVersion(value) {
  const match = /^(\d+)\.(\d+)/.exec(String(value));
  return match ? [Number(match[1]), Number(match[2])] : null;
}

// Returns an error message, or null when the C library can run the Linux binaries.
// Node reports glibcVersionRuntime only when it is linked against glibc, so its
// absence means musl (Alpine and the like).
function checkLinuxLibc(report) {
  const header = (report && report.header) || {};
  const runtime = header.glibcVersionRuntime;
  if (!runtime) {
    return (
      "bridgewatch: this Linux system does not use glibc (musl, as on Alpine, is not supported).\n" +
      `The Linux builds need glibc ${MIN_GLIBC.join(".")} or newer. Other ways to install: ${RELEASES_URL}`
    );
  }
  const have = parseGlibcVersion(runtime);
  const tooOld =
    !have || have[0] < MIN_GLIBC[0] || (have[0] === MIN_GLIBC[0] && have[1] < MIN_GLIBC[1]);
  if (tooOld) {
    return (
      `bridgewatch: this system has glibc ${runtime}; the Linux builds need ${MIN_GLIBC.join(".")} or newer\n` +
      `(Ubuntu 22.04, Debian 12, Fedora 36 or newer). Other ways to install: ${RELEASES_URL}`
    );
  }
  return null;
}

// { target } for a supported machine, { error } otherwise. `report` is only read on
// Linux, and may be a function so the (slow) report is only built when needed.
function resolveTarget(platform, arch, report) {
  const target = TARGETS[`${platform}-${arch}`];
  if (!target) {
    return {
      error:
        `bridgewatch: ${platform}-${arch} is not supported. The npm package runs on macOS ` +
        `(Apple Silicon and Intel) and on Linux x64 and arm64 with glibc ${MIN_GLIBC.join(".")} or newer.\n` +
        `Other ways to install: ${RELEASES_URL}`,
    };
  }
  if (platform === "linux") {
    const error = checkLinuxLibc(typeof report === "function" ? report() : report);
    if (error) return { error };
  }
  return { target };
}

function binaryPath(vendorDir, target) {
  return path.join(vendorDir, target, "bridgewatch");
}

function missingBinaryMessage(file) {
  return (
    `bridgewatch: the native binary is missing at ${file}.\n` +
    "This npm package is incomplete or was installed for another platform; reinstall it, " +
    `or download bridgewatch from ${RELEASES_URL}`
  );
}

// Node's diagnostic report, built without its network section, as detect-libc does.
// `excludeNetwork` (Node 20.13 and 22 or newer) drops header.networkInterfaces and the
// reverse DNS lookups the report would otherwise make, which only slow the launcher
// down: it reads header.glibcVersionRuntime and nothing else. The setting is global,
// so it is put back as it was, even when getReport throws. On an older Node the
// property is ignored and the report is built as before.
function readReport(reporter) {
  const saved = reporter.excludeNetwork;
  reporter.excludeNetwork = true;
  try {
    return reporter.getReport();
  } finally {
    reporter.excludeNetwork = saved;
  }
}

// The machine as this process sees it, unless a test overrides a part of it.
function environment(env) {
  const glibc = env.BRIDGEWATCH_NPM_TEST_GLIBC;
  return {
    platform: env.BRIDGEWATCH_NPM_TEST_PLATFORM || process.platform,
    arch: env.BRIDGEWATCH_NPM_TEST_ARCH || process.arch,
    // "musl" stands for a report without glibcVersionRuntime.
    report:
      glibc === undefined
        ? () => readReport(process.report)
        : { header: glibc === "musl" ? {} : { glibcVersionRuntime: glibc } },
    vendorDir: env.BRIDGEWATCH_NPM_TEST_VENDOR_DIR || path.join(__dirname, "..", "vendor"),
  };
}

function main() {
  const machine = environment(process.env);
  const resolved = resolveTarget(machine.platform, machine.arch, machine.report);
  if (resolved.error) {
    process.stderr.write(resolved.error + "\n");
    process.exit(1);
  }
  const file = binaryPath(machine.vendorDir, resolved.target);
  if (!fs.existsSync(file)) {
    process.stderr.write(missingBinaryMessage(file) + "\n");
    process.exit(1);
  }

  const child = childProcess.spawn(file, process.argv.slice(2), { stdio: "inherit" });

  const ignoreInterrupt = () => {};
  process.on("SIGINT", ignoreInterrupt);
  const forwarders = FORWARDED_SIGNALS.map((signal) => {
    const forward = () => {
      child.kill(signal);
    };
    process.on(signal, forward);
    return [signal, forward];
  });
  const removeHandlers = () => {
    process.removeListener("SIGINT", ignoreInterrupt);
    for (const [signal, forward] of forwarders) process.removeListener(signal, forward);
  };

  child.on("error", (error) => {
    removeHandlers();
    process.stderr.write(`bridgewatch: could not start ${file}: ${error.message}\n`);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    removeHandlers();
    if (signal) {
      // With no listener left the default action applies: this process dies of the
      // same signal, so a shell or supervisor sees what the binary did. The signal may
      // land on another thread a moment later, so the shell-convention exit code waits
      // briefly; it only applies to a signal whose default is not to terminate.
      process.kill(process.pid, signal);
      setTimeout(() => process.exit(128 + (os.constants.signals[signal] || 0)), 1000);
      return;
    }
    process.exit(code === null ? 1 : code);
  });
}

module.exports = {
  RELEASES_URL,
  TARGETS,
  parseGlibcVersion,
  checkLinuxLibc,
  resolveTarget,
  binaryPath,
  missingBinaryMessage,
  readReport,
  environment,
  main,
};

if (require.main === module) {
  main();
}
