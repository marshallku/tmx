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

Releases are driven by [`cargo-dist`](https://github.com/axodotdev/cargo-dist). To cut a new release:

```sh
make release VERSION=0.1.3
git push --follow-tags origin master
```

`make release` bumps `Cargo.toml`, syncs `Cargo.lock`, commits, and creates an annotated `vX.Y.Z` tag. Pushing the tag triggers the GitHub Actions `release` workflow, which builds binaries for every target, publishes them to GitHub Releases, generates the `tmx-cli-installer.sh` script, and (via the `publish-crates` job) runs `cargo publish` so `cargo install tmx-cli` and `cargo binstall tmx-cli` pick up the new version.
