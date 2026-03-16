# pinentry-zellij

Pinentry for [Zellij](https://zellij.dev). Floating dialog inside Zellij, TTY fallback outside. Works as `SSH_ASKPASS` and `SUDO_ASKPASS` too.

## How it works

Single binary with an embedded WASM plugin. The binary speaks Assuan (for gpg-agent) or askpass (for SSH/sudo). Inside Zellij, passphrase prompts open a floating plugin pane. Outside Zellij, falls back to rpassword on the TTY.

The plugin is auto-installed to `~/.config/zellij/plugins/` on first use.

## Prerequisites

- [Zellij](https://zellij.dev) (TTY fallback works without it)
- Rust nightly with `wasm32-wasip1` target
- Optional: [binaryen](https://github.com/WebAssembly/binaryen) for `wasm-opt`

## Install

```sh
cargo build --release -p pinentry-zellij
cp target/release/pinentry-zellij ~/.local/bin/
```

## Setup

### GPG

```sh
# ~/.gnupg/gpg-agent.conf
pinentry-program ~/.local/bin/pinentry-zellij
```

```sh
gpgconf --kill gpg-agent
```

### SSH / sudo

```sh
export SSH_ASKPASS=~/.local/bin/pinentry-zellij
export SSH_ASKPASS_REQUIRE=prefer
export SUDO_ASKPASS=~/.local/bin/pinentry-zellij
```

### Permissions

First run in a new Zellij session may prompt for plugin permissions. Grant once — cached automatically after that.

## Environment variables

| Variable | Description |
|----------|-------------|
| `PINENTRY_ZELLIJ_PLUGIN` | Override plugin path (skips auto-install) |
| `RUST_LOG` | Tracing (`RUST_LOG=pinentry_zellij=debug`) |

## Development

```sh
cargo test -p pinentry-zellij-protocol
cargo test -p pinentry-zellij
cargo test -p pinentry-zellij-plugin --lib --target x86_64-unknown-linux-gnu
cargo clippy -p pinentry-zellij --tests -- -W clippy::all
```

## License

GPL-2.0
