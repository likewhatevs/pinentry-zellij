//! Zellij pinentry plugin entry point.
//!
//! Implements the ZellijPlugin trait for passphrase dialog handling.
//! Built as a wasm32-wasip1 binary that Zellij loads as a floating pane.

use std::collections::BTreeMap;

use zellij_tile::prelude::*;
use zeroize::Zeroize;

use pinentry_zellij_plugin::protocol::{PinResponse, PinStatus, PinentryCmd, PinentryRequest};
use pinentry_zellij_plugin::ui;

/// Maximum passphrase length to prevent unbounded memory growth.
const MAX_INPUT_LEN: usize = 1024;

#[derive(Default)]
struct PinentryPlugin {
    request: Option<PinentryRequest>,
    pipe_id: Option<String>,
    input: String,
    done: bool,
    resized: bool,
    resized_with_term_dims: bool,
    terminal_rows: usize,
    terminal_cols: usize,
}

register_plugin!(PinentryPlugin);

impl ZellijPlugin for PinentryPlugin {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key, EventType::TabUpdate]);
        request_permission(&[
            PermissionType::ReadCliPipes,
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if self.request.is_some() {
            return false;
        }
        // Only accept CLI pipe sources — we need the pipe_id to send
        // the response back. Reject Plugin/Keybind sources.
        let PipeSource::Cli(ref pipe_id) = pipe_message.source else {
            return false;
        };
        if let Some(payload) = pipe_message.payload
            && let Ok(req) = serde_json::from_str::<PinentryRequest>(&payload)
        {
            self.pipe_id = Some(pipe_id.clone());
            block_cli_pipe_input(pipe_id);
            self.request = Some(req);
            self.resized = false;
            self.resized_with_term_dims = false;
            self.done = false;
            // Focus immediately so the plugin pane grabs focus before
            // the first render. Without this, there is a window between
            // pane creation and the first post-pipe render where no
            // focus_pane_with_id call has been made, causing Zellij to
            // leave focus on the first tiled pane.
            let ids = get_plugin_ids();
            focus_pane_with_id(PaneId::Plugin(ids.plugin_id), true);
            return true;
        }
        false
    }

    fn update(&mut self, event: Event) -> bool {
        if self.done {
            return false;
        }
        let Some(ref request) = self.request else {
            return false;
        };

        match event {
            Event::TabUpdate(tab_infos) => {
                for tab in &tab_infos {
                    if tab.active {
                        self.terminal_rows = tab.display_area_rows;
                        self.terminal_cols = tab.display_area_columns;
                        break;
                    }
                }
                false
            }
            Event::Key(key) if key.bare_key == BareKey::Enter && key.key_modifiers.is_empty() => {
                let response = match request.cmd {
                    PinentryCmd::GetPin => PinResponse {
                        status: PinStatus::Ok,
                        passphrase: Some(std::mem::take(&mut self.input)),
                    },
                    PinentryCmd::Confirm | PinentryCmd::Message => PinResponse {
                        status: PinStatus::Ok,
                        passphrase: None,
                    },
                };
                self.send_response(response);
                true
            }
            Event::Key(key) if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() => {
                // Esc always means "cancel/abort" — distinct from "not confirmed"
                // (which would require an explicit NotOk button).
                let response = PinResponse {
                    status: PinStatus::Canceled,
                    passphrase: None,
                };
                self.send_response(response);
                true
            }
            Event::Key(key)
                if key.bare_key == BareKey::Backspace && key.key_modifiers.is_empty() =>
            {
                self.input.pop();
                true
            }
            Event::Key(key) => {
                if let BareKey::Char(c) = key.bare_key
                    && key.key_modifiers.is_empty()
                    && request.cmd == PinentryCmd::GetPin
                    && self.input.len() < MAX_INPUT_LEN
                {
                    self.input.push(c);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let Some(ref request) = self.request else {
            return;
        };

        // Resize and center floating pane on first render, and again if
        // terminal dimensions arrive later via TabUpdate.
        let have_term_dims = self.terminal_cols > 0 && self.terminal_rows > 0;
        if !self.resized || (have_term_dims && !self.resized_with_term_dims) {
            if have_term_dims {
                self.resized_with_term_dims = true;
            }
            self.resized = true;
            let ids = get_plugin_ids();
            let pane_id = PaneId::Plugin(ids.plugin_id);
            rename_plugin_pane(ids.plugin_id, " ");

            // Use terminal dimensions if available, otherwise use a
            // reasonable estimate from pane dimensions.
            let term_cols = if self.terminal_cols > 0 {
                self.terminal_cols
            } else {
                cols
            };
            let term_rows = if self.terminal_rows > 0 {
                self.terminal_rows
            } else {
                rows
            };
            let (w, h) = ui::dialog_dimensions(request, term_cols as u16, term_rows as u16);
            let x = term_cols.saturating_sub(w as usize) / 2;
            let y = term_rows.saturating_sub(h as usize) / 2;
            if let Some(coords) = FloatingPaneCoordinates::new(
                Some(format!("{x}")),
                Some(format!("{y}")),
                Some(format!("{w}")),
                Some(format!("{h}")),
                None,
            ) {
                change_floating_panes_coordinates(vec![(pane_id, coords)]);
            }
            focus_pane_with_id(pane_id, true);
        }

        let mut ansi_backend =
            pinentry_zellij_plugin::backend::AnsiBackend::new(cols as u16, rows as u16);
        pinentry_zellij_plugin::ui::render(&mut ansi_backend, request, &self.input);
        print!("{}", ansi_backend.to_ansi());
    }
}

impl PinentryPlugin {
    fn send_response(&mut self, mut response: PinResponse) {
        let mut json = serde_json::to_string(&response).expect("serialize response");
        if let Some(ref pipe_id) = self.pipe_id {
            cli_pipe_output(pipe_id, &json);
            unblock_cli_pipe_input(pipe_id);
        }
        // Zeroize sensitive data before drop.
        json.zeroize();
        if let Some(ref mut p) = response.passphrase {
            p.zeroize();
        }
        self.input.zeroize();
        self.done = true;
        close_self();
    }
}
