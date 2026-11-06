"use strict";

// bin/bridgewatch.js: the platform mapping and refusals as pure functions, then the
// wrapper run for real against a fake binary for argv, exit codes and signals.

const assert = require("node:assert/strict");
const childProcess = require("node:child_process");
const path = require("node:path");
const { after, before, describe, test } = require("node:test");

const wrapper = require("../bin/bridgewatch.js");
const { WRAPPER, makeScratch, removeScratch, writeFakeBinary } = require("./helpers.js");

const GLIBC_OK = { header: { glibcVersionRuntime: "2.39" } };

describe("resolveTarget", () => {
  const cases = [
    ["darwin", "x64", "universal-apple-darwin"],
    ["darwin", "arm64", "universal-apple-darwin"],
    ["linux", "x64", "x86_64-unknown-linux-gnu"],
    ["linux", "arm64", "aarch64-unknown-linux-gnu"],
  ];
  for (const [platform, arch, target] of cases) {
    test(`${platform}-${arch} -> ${target}`, () => {
      assert.deepEqual(wrapper.resolveTarget(platform, arch, GLIBC_OK), { target });
    });
  }

  test("macOS never builds a process report", () => {
    const report = () => assert.fail("the report is read on Linux only");
    assert.deepEqual(wrapper.resolveTarget("darwin", "arm64", report), {
      target: "universal-apple-darwin",
    });
  });

  test("a report given as a function is called on Linux", () => {
    assert.deepEqual(wrapper.resolveTarget("linux", "x64", () => GLIBC_OK), {
      target: "x86_64-unknown-linux-gnu",
    });
  });

  for (const [platform, arch] of [
    ["win32", "x64"],
    ["linux", "ia32"],
    ["darwin", "ppc64"],
    ["freebsd", "x64"],
  ]) {
    test(`${platform}-${arch} is refused`, () => {
      const { error, target } = wrapper.resolveTarget(platform, arch, GLIBC_OK);
      assert.equal(target, undefined);
      assert.match(error, new RegExp(`${platform}-${arch} is not supported`));
      assert.ok(error.includes(wrapper.RELEASES_URL));
    });
  }

  test("musl is refused", () => {
    const { error } = wrapper.resolveTarget("linux", "x64", { header: {} });
    assert.match(error, /does not use glibc \(musl/);
    assert.ok(error.includes(wrapper.RELEASES_URL));
  });

  test("a missing report header counts as musl", () => {
    assert.match(wrapper.resolveTarget("linux", "arm64", {}).error, /musl/);
    assert.match(wrapper.resolveTarget("linux", "arm64", null).error, /musl/);
  });

  for (const old of ["2.34", "2.31", "2.17", "1.99", "garbage"]) {
    test(`glibc ${old} is refused`, () => {
      const { error } = wrapper.resolveTarget("linux", "x64", {
        header: { glibcVersionRuntime: old },
      });
      assert.match(error, new RegExp(`glibc ${old}; the Linux builds need 2\\.35 or newer`));
      assert.ok(error.includes(wrapper.RELEASES_URL));
    });
  }

  for (const fine of ["2.35", "2.36", "2.39", "3.0"]) {
    test(`glibc ${fine} is accepted`, () => {
      const result = wrapper.resolveTarget("linux", "arm64", {
        header: { glibcVersionRuntime: fine },
      });
      assert.deepEqual(result, { target: "aarch64-unknown-linux-gnu" });
    });
  }
});

describe("helpers", () => {
  test("parseGlibcVersion", () => {
    assert.deepEqual(wrapper.parseGlibcVersion("2.35"), [2, 35]);
    assert.deepEqual(wrapper.parseGlibcVersion("2.39.9000"), [2, 39]);
    assert.equal(wrapper.parseGlibcVersion("two"), null);
  });

  test("binaryPath", () => {
    assert.equal(
      wrapper.binaryPath(path.join("v"), "x86_64-unknown-linux-gnu"),
      path.join("v", "x86_64-unknown-linux-gnu", "bridgewatch"),
    );
  });

  test("environment defaults to this process and the package's vendor dir", () => {
    const machine = wrapper.environment({});
    assert.equal(machine.platform, process.platform);
    assert.equal(machine.arch, process.arch);
    assert.equal(typeof machine.report, "function");
    assert.equal(machine.vendorDir, path.join(path.dirname(WRAPPER), "..", "vendor"));
  });

  test("readReport builds the report with excludeNetwork set, then restores it", () => {
    const reporter = {
      excludeNetwork: false,
      getReport() {
        assert.equal(this.excludeNetwork, true);
        return { header: { glibcVersionRuntime: "2.39" } };
      },
    };
    assert.deepEqual(wrapper.readReport(reporter), { header: { glibcVersionRuntime: "2.39" } });
    assert.equal(reporter.excludeNetwork, false);
  });

  test("readReport restores excludeNetwork when getReport throws", () => {
    const reporter = {
      excludeNetwork: "as it was",
      getReport() {
        throw new Error("no report");
      },
    };
    assert.throws(() => wrapper.readReport(reporter), /no report/);
    assert.equal(reporter.excludeNetwork, "as it was");
  });

  // `excludeNetwork` exists from Node 20.13 and 22. On an older Node (the package
  // admits 18) the setting is ignored and the report keeps its network section,
  // which costs time and nothing else.
  const excludesNetwork = typeof process.report.excludeNetwork === "boolean";

  test("readReport on this Node: the report it needs, the setting put back", () => {
    const before = process.report.excludeNetwork;
    const report = wrapper.readReport(process.report);
    assert.equal(typeof report.header, "object");
    if (process.platform === "linux") assert.equal(typeof report.header.glibcVersionRuntime, "string");
    if (excludesNetwork) assert.equal(report.header.networkInterfaces, undefined);
    assert.equal(process.report.excludeNetwork, before);
  });

  test("environment's default report goes through readReport", () => {
    const report = wrapper.environment({}).report();
    assert.equal(typeof report.header, "object");
    if (excludesNetwork) assert.equal(report.header.networkInterfaces, undefined);
  });

  test("environment takes the test overrides", () => {
    const machine = wrapper.environment({
      BRIDGEWATCH_NPM_TEST_PLATFORM: "linux",
      BRIDGEWATCH_NPM_TEST_ARCH: "arm64",
      BRIDGEWATCH_NPM_TEST_GLIBC: "2.31",
      BRIDGEWATCH_NPM_TEST_VENDOR_DIR: "/v",
    });
    assert.deepEqual(machine, {
      platform: "linux",
      arch: "arm64",
      report: { header: { glibcVersionRuntime: "2.31" } },
      vendorDir: "/v",
    });
    assert.deepEqual(wrapper.environment({ BRIDGEWATCH_NPM_TEST_GLIBC: "musl" }).report, {
      header: {},
    });
  });
});

describe("the wrapper as a process", () => {
  let scratch;
  let vendorDir;

  before(() => {
    scratch = makeScratch("wrapper");
    vendorDir = path.join(scratch, "vendor");
    writeFakeBinary(path.join(vendorDir, "x86_64-unknown-linux-gnu", "bridgewatch"));
    writeFakeBinary(path.join(vendorDir, "universal-apple-darwin", "bridgewatch"), 0o644);
  });

  after(() => removeScratch(scratch));

  // Runs the wrapper as a fake glibc 2.39 Linux x64 unless `overrides` says otherwise.
  // Resolves { code, signal, stdout, stderr } once it has exited; `onLine` sees each
  // stdout line as it arrives, with the child process to signal.
  function run(args, { overrides = {}, onLine } = {}) {
    const env = {
      ...process.env,
      BRIDGEWATCH_NPM_TEST_PLATFORM: "linux",
      BRIDGEWATCH_NPM_TEST_ARCH: "x64",
      BRIDGEWATCH_NPM_TEST_GLIBC: "2.39",
      BRIDGEWATCH_NPM_TEST_VENDOR_DIR: vendorDir,
      ...overrides,
    };
    return new Promise((resolve, reject) => {
      const proc = childProcess.spawn(process.execPath, [WRAPPER, ...args], {
        env,
        stdio: ["ignore", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      let pending = "";
      proc.stdout.on("data", (chunk) => {
        stdout += chunk;
        pending += chunk;
        let newline;
        while ((newline = pending.indexOf("\n")) !== -1) {
          const line = pending.slice(0, newline);
          pending = pending.slice(newline + 1);
          if (onLine) onLine(line, proc);
        }
      });
      proc.stderr.on("data", (chunk) => {
        stderr += chunk;
      });
      const timer = setTimeout(() => {
        proc.kill("SIGKILL");
        reject(new Error(`the wrapper did not exit in time; stdout ${stdout} stderr ${stderr}`));
      }, 20000);
      proc.on("error", reject);
      proc.on("close", (code, signal) => {
        clearTimeout(timer);
        resolve({ code, signal, stdout, stderr });
      });
    });
  }

  test("argv is passed through untouched", async () => {
    const args = ["--flag", "two words", "", "--", "ünïcode", "$HOME"];
    const result = await run(["argv", ...args]);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), args);
  });

  for (const code of [0, 3, 42]) {
    test(`exit code ${code} is passed through`, async () => {
      const result = await run(["exit", String(code)]);
      assert.equal(result.signal, null);
      assert.equal(result.code, code);
    });
  }

  test("the binary dying of SIGTERM kills the wrapper with SIGTERM", async () => {
    const result = await run(["self-kill", "SIGTERM"]);
    assert.equal(result.code, null);
    assert.equal(result.signal, "SIGTERM");
  });

  test("the binary dying of SIGINT kills the wrapper with SIGINT (its forwarding handler is gone)", async () => {
    const result = await run(["self-kill", "SIGINT"]);
    assert.equal(result.code, null);
    assert.equal(result.signal, "SIGINT");
  });

  test("SIGINT to the wrapper and the binary at once (a terminal's Ctrl-C) exits once", async () => {
    const result = await run(["wait"], {
      onLine(line, proc) {
        if (line.startsWith("ready ")) {
          process.kill(Number(line.slice("ready ".length)), "SIGINT");
          proc.kill("SIGINT");
        }
      },
    });
    // Twice is fine (both can land before the fake binary's exit runs); the binary's
    // own answer is the exit code, once.
    assert.match(result.stdout, /^ready \d+\n(got SIGINT\n)+$/);
    assert.equal(result.signal, null);
    assert.equal(result.code, 10);
  });

  for (const [signal, code] of [
    ["SIGINT", 10],
    ["SIGTERM", 11],
    ["SIGHUP", 12],
  ]) {
    test(`${signal} to the wrapper is forwarded to the binary`, async () => {
      const result = await run(["wait"], {
        onLine(line, proc) {
          if (line.startsWith("ready ")) proc.kill(signal);
        },
      });
      assert.match(result.stdout, new RegExp(`^ready \\d+\\ngot ${signal}\\n$`));
      assert.equal(result.signal, null);
      assert.equal(result.code, code);
    });
  }

  test("a missing binary is reported clearly", async () => {
    const result = await run(["exit", "0"], { overrides: { BRIDGEWATCH_NPM_TEST_ARCH: "arm64" } });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /the native binary is missing at .*aarch64-unknown-linux-gnu/);
    assert.ok(result.stderr.includes(wrapper.RELEASES_URL));
  });

  test("a binary that cannot be started is reported clearly", async () => {
    const result = await run(["exit", "0"], {
      overrides: { BRIDGEWATCH_NPM_TEST_PLATFORM: "darwin", BRIDGEWATCH_NPM_TEST_ARCH: "arm64" },
    });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /could not start .*universal-apple-darwin.*EACCES/);
  });

  test("musl is refused before anything runs", async () => {
    const result = await run(["exit", "0"], { overrides: { BRIDGEWATCH_NPM_TEST_GLIBC: "musl" } });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /musl/);
  });

  test("an old glibc is refused before anything runs", async () => {
    const result = await run(["exit", "0"], { overrides: { BRIDGEWATCH_NPM_TEST_GLIBC: "2.31" } });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /glibc 2\.31; the Linux builds need 2\.35 or newer/);
  });

  test("an unsupported platform is refused before anything runs", async () => {
    const result = await run(["exit", "0"], { overrides: { BRIDGEWATCH_NPM_TEST_PLATFORM: "win32" } });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /win32-x64 is not supported/);
  });
});
