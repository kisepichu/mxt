# Installing

mxt runs on Linux and macOS. Install a prebuilt binary, the Nix flake, or
build it from source.

## Prebuilt binaries

Each [release](https://github.com/kisepichu/mxt/releases) has archives for
x86_64 Linux with glibc 2.35 or newer (Ubuntu 22.04 and later, for example)
and for Apple Silicon macOS 11 or newer. They link SQLite statically, so they
need no system SQLite. With [mise](https://mise.jdx.dev):

```sh
mise use -g github:kisepichu/mxt
```

Or download `mxt-v<version>-<target>.tar.gz` and its `.sha256` file from the
release, check and unpack the archive, and put `mxt` on your `PATH`:

```sh
sha256sum -c mxt-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256  # macOS: shasum -a 256 -c
tar -xzf mxt-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cp mxt-v0.1.0-x86_64-unknown-linux-gnu/mxt ~/.local/bin/
```

If macOS refuses to run an `mxt` that a browser downloaded, remove the
quarantine attribute with `xattr -d com.apple.quarantine mxt`.

## With Nix

```sh
nix profile install github:kisepichu/mxt
```

The flake builds `packages.default` for `x86_64-linux`, `aarch64-linux` and
`aarch64-darwin`. To install it from a NixOS or home-manager configuration,
add the flake as an input:

```nix
{
  inputs.mxt.url = "github:kisepichu/mxt";
  # ...
}
```

and add the package in a module that gets the flake inputs (for example
through `specialArgs`):

```nix
{ pkgs, inputs, ... }:
{
  # home-manager: home.packages
  environment.systemPackages = [
    inputs.mxt.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

## From source, without Nix

Requirements:

- Rust 1.96 or newer.
- SQLite 3.54.0 or newer, with its development files. mxt links the system
  SQLite instead of bundling one, and older versions have a known
  vulnerability (RUSTSEC-2026-0459). `pkg-config --modversion sqlite3` shows
  the version that the build will find. If pkg-config does not find
  sqlite3, the build still links any `libsqlite3` the linker finds, without
  checking its version.
- `pkg-config`, which the build uses to find SQLite.
- A C compiler: some dependencies build C code.

```sh
cargo install --locked --git https://github.com/kisepichu/mxt mxt
```

`--locked` builds with the dependency versions in the repository's
`Cargo.lock`. If the system SQLite is too old, install a newer one and point
`pkg-config` at it, for example with Homebrew on macOS:

```sh
PKG_CONFIG_PATH="$(brew --prefix sqlite)/lib/pkgconfig" \
  cargo install --locked --git https://github.com/kisepichu/mxt mxt
```
