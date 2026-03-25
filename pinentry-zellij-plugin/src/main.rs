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
    moved_to_tab: bool,
    /// Expected inner width after resize (borderless: pane width = inner width).
    /// render() skips until cols matches, preventing a flash at wrong size.
    expected_cols: u16,
    terminal_cols: u16,
    terminal_rows: u16,
}

register_plugin!(PinentryPlugin);

impl ZellijPlugin for PinentryPlugin {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key, EventType::TabUpdate]);
        request_permission(&[
            PermissionType::ReadCliPipes,
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);

        if let Some(cols) = configuration.get("term_cols") {
            self.terminal_cols = cols.parse().unwrap_or(0);
        }
        if let Some(rows) = configuration.get("term_rows") {
            self.terminal_rows = rows.parse().unwrap_or(0);
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if self.request.is_some() && !self.done {
            return false;
        }
        let PipeSource::Cli(ref pipe_id) = pipe_message.source else {
            return false;
        };
        if let Some(payload) = pipe_message.payload
            && let Ok(req) = serde_json::from_str::<PinentryRequest>(&payload)
        {
            self.input.zeroize();
            if let Some(ref mut old_id) = self.pipe_id {
                old_id.zeroize();
            }
            self.pipe_id = Some(pipe_id.clone());
            block_cli_pipe_input(pipe_id);

            let ids = get_plugin_ids();
            let pane_id = PaneId::Plugin(ids.plugin_id);
            rename_plugin_pane(ids.plugin_id, " ");

            // Hide pane before moving/resizing to prevent a flash on
            // the old tab when the instance is reused. Skip on first
            // invocation (pane was just created on the current tab).
            if self.done {
                hide_pane_with_id(pane_id);
            }

            // Fire-and-forget resize + center using host-provided dims.
            if self.terminal_cols > 0 && self.terminal_rows > 0 {
                let (w, h) = ui::dialog_dimensions(&req, self.terminal_cols, self.terminal_rows);
                let x = self.terminal_cols.saturating_sub(w) / 2;
                let y = self.terminal_rows.saturating_sub(h) / 2;
                if let Some(coords) = FloatingPaneCoordinates::new(
                    Some(format!("{x}")),
                    Some(format!("{y}")),
                    Some(format!("{w}")),
                    Some(format!("{h}")),
                    None,
                    Some(true),
                ) {
                    change_floating_panes_coordinates(vec![(pane_id, coords)]);
                }
                // Borderless: pane inner cols = pane width (no frame).
                self.expected_cols = w;
            }

            // Don't focus here — the pane is still on the old tab.
            // update(TabUpdate) will move, show, and focus it.
            self.request = Some(req);
            self.moved_to_tab = false;
            self.done = false;
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
                if self.moved_to_tab {
                    return false;
                }
                if let Some(active) = tab_infos.iter().find(|t| t.active) {
                    let ids = get_plugin_ids();
                    let pane_id = PaneId::Plugin(ids.plugin_id);

                    // Move to active tab, then float + resize + focus.
                    break_panes_to_tab_with_index(&[pane_id], active.position, false);
                    float_multiple_panes(vec![pane_id]);

                    let tc = active.display_area_columns as u16;
                    let tr = active.display_area_rows as u16;
                    let (w, h) = ui::dialog_dimensions(request, tc, tr);
                    let x = tc.saturating_sub(w) / 2;
                    let y = tr.saturating_sub(h) / 2;
                    if let Some(coords) = FloatingPaneCoordinates::new(
                        Some(format!("{x}")),
                        Some(format!("{y}")),
                        Some(format!("{w}")),
                        Some(format!("{h}")),
                        None,
                        Some(true),
                    ) {
                        change_floating_panes_coordinates(vec![(pane_id, coords)]);
                    }
                    self.expected_cols = w;
                    focus_pane_with_id(pane_id, true, false);
                    self.moved_to_tab = true;
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

        // Skip until update(TabUpdate) has moved/resized the pane, and
        // the resize has been applied (cols matches expected width).
        if !self.moved_to_tab {
            return;
        }
        if self.expected_cols > 0 && cols != self.expected_cols as usize {
            return;
        }
        self.expected_cols = 0;

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
        json.zeroize();
        if let Some(ref mut p) = response.passphrase {
            p.zeroize();
        }
        self.input.zeroize();
        self.done = true;
        close_self();
    }
}
