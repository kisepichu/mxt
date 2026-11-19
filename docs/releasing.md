# Releasing

This page is for maintainers: how mxt is versioned, how changes are recorded,
and how a release is cut.

## Versions

mxt follows [Semantic Versioning](https://semver.org/) in its 0.x form. The
version is `version` in `[workspace.package]` of the root `Cargo.toml`, which
every crate and the flake's package share, and a release is the tag
`v<version>`, for example `v0.1.0`.

- A change that breaks users' setups bumps the minor version (0.1.3 to
  0.2.0). That is a change to a name or layout users have written down or
  keep on disk: configuration keys, keymap table names, action names, command
  names, and the layout of the data directory or the session (`session.json`,
  the stores), when an existing configuration stops loading or means
  something else, or existing data has to be moved or set aside.
- Everything else bumps the patch version (0.1.3 to 0.1.4): new features,
  fixes, and changes to defaults or behaviour that leave existing setups
  working.

## The changelog

[CHANGELOG.md](../CHANGELOG.md) follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). A pull request with
a user-visible change adds a line under `## [Unreleased]`, in the section
that fits: `### Added`, `### Changed`, `### Fixed` or `### Removed`. A line
for a change that breaks users' setups starts with `**Breaking:**` and says
what users have to change, for example (a made-up change):

```markdown
### Changed

- **Breaking:** the action `message.yank` is now `message.copy`; rename it
  in your keymap bindings.
```

Changes that users cannot see (refactoring, tests, CI) need no line.

## Cutting a release

1. Choose the version: the next minor version if `Unreleased` has a
   `**Breaking:**` line, otherwise the next patch version.
2. On a branch from `main`, in `CHANGELOG.md`, rename `## [Unreleased]` to
   `## [<version>] - <YYYY-MM-DD>`, put a new empty `## [Unreleased]` above
   it, and update the links at the end of the file:

   ```markdown
   [Unreleased]: https://github.com/kisepichu/mxt/compare/v0.1.1...HEAD
   [0.1.1]: https://github.com/kisepichu/mxt/compare/v0.1.0...v0.1.1
   ```

3. Set `version` in `[workspace.package]` of `Cargo.toml`, and run
   `cargo update --workspace`, which changes only the workspace crates'
   entries in `Cargo.lock` (CI builds with `--locked`).
4. Open a pull request, and merge it into `main` when CI passes.
5. Tag the merge commit on `main` and push the tag:

   ```sh
   git switch main && git pull
   git tag -a v0.1.1 -m "mxt 0.1.1"
   git push origin v0.1.1
   ```

6. The [release workflow](../.github/workflows/release.yml) builds and checks
   the archives and publishes the GitHub release, with the version's
   `CHANGELOG.md` section as the release notes. It fails before building if
   the tag is not `v` followed by the workspace version, or if
   `CHANGELOG.md` has no section for the version.

A tag is never moved or deleted once it is pushed: mise, Nix users who pin
`github:kisepichu/mxt/v0.1.1`, and anyone who recorded a checksum rely on it
naming one commit. If the workflow fails for a reason outside the
repository, such as a runner problem, re-run its failed jobs. A release that
needs a change in the repository, including a broken published one, is fixed
by the next patch release.

## The release workflow

`.github/workflows/release.yml` runs on `v*` tags, and by hand:

```sh
gh workflow run release.yml --ref <branch>
```

A run by hand does everything a tag run does except publishing: it builds and
checks the archives for the branch's workspace version and keeps them, and
the extracted release notes, as workflow artifacts. Use it to test changes
to the workflow.

For each target it builds `mxt-v<version>-<target>.tar.gz`, which holds one
directory with `mxt`, `LICENSE`, `README.md` and `CHANGELOG.md`, and
`mxt-v<version>-<target>.tar.gz.sha256`:

| Target | Runner | Runs on |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-22.04` | glibc 2.35 or newer (Ubuntu 22.04 and later) |
| `aarch64-apple-darwin` | `macos-latest` | macOS 11 or newer on Apple Silicon |

The names are the ones mise's github backend recognizes, so
`mise use -g github:kisepichu/mxt` installs the right archive.

The binaries link SQLite statically. The workflow downloads the official
SQLite amalgamation, checks it against the SHA3-256 in the workflow, builds
a static library with the options of libsqlite3-sys's bundled build, and
points libsqlite3-sys at it. It then checks that the binary has no dynamic
SQLite dependency and contains the source ID of exactly that SQLite, and,
for Linux, that no symbol needs a glibc newer than 2.35. The reason is
RUSTSEC-2026-0459 (see `deny.toml`): the SQLite that libsqlite3-sys 0.38
bundles is affected, and Ubuntu 22.04's system SQLite is older than the fix.

Two values in the workflow's `env` are maintained by hand:

- `RUSTUP_TOOLCHAIN`, the Rust version of the release builds, follows the
  flake's rustc (`nix develop -c rustc --version`). Update it when
  `nix flake update` changes rustc.
- `SQLITE_URL` and `SQLITE_SHA3_256` name the SQLite release. To update
  SQLite, take both from the amalgamation's row on
  <https://sqlite.org/download.html>, and change the version in `deny.toml`'s
  comment.
