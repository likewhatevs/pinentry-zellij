//! Pinentry program for Zellij.

mod askpass;
mod assuan;
mod embed;
mod handler;
mod protocol;
mod state;
mod tty;
mod zellij;

use std::io::{self, BufReader};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    // Handle --help/--version before any other initialization.
    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--version" => {
                println!("pinentry-zellij {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }

    // Quiet by default — set RUST_LOG=pinentry_zellij=debug for verbose.
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .init();

    // Install embedded wasm plugin if needed (never abort on failure).
    if let Err(e) = embed::ensure_plugin_installed() {
        tracing::warn!("plugin install: {e}");
    }

    // Askpass mode: argv[1] present → prompt, print passphrase, exit.
    // SSH_ASKPASS and SUDO_ASKPASS invoke the program this way.
    if args.len() > 1 {
        return askpass::run(&args[1]);
    }

    // Assuan mode: gpg-agent talks protocol on stdin/stdout.
    let mut server = assuan::Server::new(
        BufReader::new(io::stdin().lock()),
        io::stdout().lock(),
        handler::dispatch,
    );
    match server.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn help_text() -> String {
    format!(
        "\
pinentry-zellij {}
Pinentry program for Zellij

USAGE:
    pinentry-zellij              Assuan mode (gpg-agent)
    pinentry-zellij <prompt>     Askpass mode (SSH_ASKPASS / SUDO_ASKPASS)

ENVIRONMENT:
    PINENTRY_ZELLIJ_PLUGIN   Override plugin path (skips auto-install)
    RUST_LOG                 Tracing filter (e.g. pinentry_zellij=debug)

Inside Zellij, prompts appear as a floating plugin pane.
Outside Zellij, falls back to rpassword on the TTY.",
        env!("CARGO_PKG_VERSION")
    )
}

fn print_help() {
    println!("{}", help_text());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_contains_expected_content() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("USAGE"));
        assert!(text.contains("Assuan"));
        assert!(text.contains("Askpass"));
        assert!(text.contains("PINENTRY_ZELLIJ_PLUGIN"));
    }
}
