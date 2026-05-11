# tmx — Project-aware tmux session manager

Make tmux powerful.

## Install

### Shell installer (macOS, Linux)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/marshallku/tmx/releases/latest/download/tmx-cli-installer.sh | sh
```

Picks the right prebuilt binary for your platform (macOS arm64/x86_64, Linux x86_64/aarch64) and installs into `$CARGO_HOME/bin` (defaults to `~/.cargo/bin`). Make sure that directory is on your `PATH`.

### From crates.io

```sh
cargo install tmx-cli
```

Or, with [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) (downloads the prebuilt binary, no compile):

```sh
cargo binstall tmx-cli
```

> The crate is published as `tmx-cli` because the `tmx` name was already taken on crates.io. The installed binary is still `tmx`.

### From source

```sh
git clone https://github.com/marshallku/tmx.git
cd tmx
make install   # installs to ~/.local/bin
```

## Supported targets

Prebuilt binaries are produced for:

- `aarch64-apple-darwin` (Apple Silicon macOS)
- `x86_64-apple-darwin` (Intel macOS)
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`

## Releasing

Releases are driven by [`cargo-dist`](https://github.com/astral-sh/cargo-dist). To cut a new release:

```sh
# bump version in Cargo.toml, commit, then:
git tag v0.1.0
git push --tags
```

The GitHub Actions `release` workflow builds binaries for every target, publishes them to GitHub Releases, and generates the `tmx-cli-installer.sh` script.

To also publish to crates.io (so `cargo install tmx-cli` and `cargo binstall tmx-cli` work), run once per release after the tag push:

```sh
cargo publish
```
