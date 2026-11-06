# bridgewatch

The `bridgewatch` command-line tool on npm: bridge-aware GitLab CI and GitHub Actions
verdicts, config tooling and fixture recording. This package carries the prebuilt native
CLI from the bridgewatch release and a small Node launcher that runs the right one for
your machine. It has no dependencies and no install scripts.

Run it once without installing:

```
npx bridgewatch --help
```

Or install it globally:

```
npm install -g bridgewatch
bridgewatch --version
```

## Supported platforms

| Platform | Status |
| --- | --- |
| macOS, Apple Silicon and Intel (one universal binary) | Supported. |
| Linux x64 and arm64, glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or newer) | Supported. |
| Linux with musl (Alpine) or an older glibc | Refused with a message. |
| Windows, FreeBSD, 32-bit x86 and anything else | Not supported. npm refuses the install. |

On Windows, FreeBSD, 32-bit x86 and any other platform or CPU outside the package's
`os` and `cpu` lists, npm itself refuses the install (`EBADPLATFORM`). On Linux with musl
or a glibc older than 2.35 the package installs, and the launcher refuses to run: it
says why and exits 1. The desktop app and other downloads are on the
[Releases page](https://github.com/distronode-corporation/bridgewatch/releases).

The launcher passes its arguments and terminal straight to the native binary and exits
with its exit code. SIGINT (Ctrl-C), SIGTERM and SIGHUP sent to the launcher are forwarded
to the binary, so `kill -INT` on the launcher stops `bridgewatch watch` too.

## Documentation

Configuration, commands and exit codes are in the
[bridgewatch README](https://github.com/distronode-corporation/bridgewatch#readme).

Licensed under Apache-2.0. The package includes `LICENSE`, `NOTICE` and the licence texts
of the third-party code built into the CLI (`THIRD-PARTY-LICENSES-cli.txt`).
