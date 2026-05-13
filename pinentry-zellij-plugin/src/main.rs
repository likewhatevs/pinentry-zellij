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
    /// Set once a TabUpdate has sized + focused the pane using full-tab
    /// display_area dims. Cleared on each new pipe() so the next TabUpdate
    /// re-runs setup. The pane stays hidden until this is true, so the
    /// user only ever sees the dialog at its final size.
    sized_for_tab: bool,
    /// Expected inner width after resize (borderless: pane width = inner width).
    /// render() skips until cols matches, preventing a flash at wrong size.
    expected_cols: u16,
    /// Last seen full-tab display area dims from a TabUpdate. On reuse,
    /// pipe() pre-applies coords from these cached dims while the pane
    /// is still hidden, so the subsequent unhide shows the pane at the
    /// right size in one step (rather than rendering at stale coords
    /// first, then resizing once new coords land).
    last_tab_dims: Option<(u16, u16)>,
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
        // Suppress the pane on creation and remove its border up front,
        // so even if zellij renders the pane before pipe()/TabUpdate set
        // final coords, nothing is visible (and there's no outline flash
        // if it briefly is).
        let ids = get_plugin_ids();
        let pane_id = PaneId::Plugin(ids.plugin_id);
        set_pane_borderless(pane_id, true);
        hide_pane_with_id(pane_id);
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

            // Always hide and move the plugin pane to the user's currently
            // focused tab. zellij does not guarantee the plugin lands on
            // the user's tab when `zellij pipe --plugin` is invoked, and on
            // subsequent calls the pane is on whatever tab it was last
            // shown. Querying synchronously avoids the prior TabUpdate-wait
            // dance that could hang or land on a stale snapshot.
            hide_pane_with_id(pane_id);
            if let Ok((tab_index, _)) = get_focused_pane_info() {
                break_panes_to_tab_with_index(&[pane_id], tab_index, false);
            }
            float_multiple_panes(vec![pane_id]);

            // If a previous TabUpdate gave us full-tab display_area dims,
            // pre-apply the final coords now while the pane is still
            // hidden — so when TabUpdate later focuses it, zellij doesn't
            // briefly render the pane at a stale size before applying
            // the new coords.
            if let Some((tc, tr)) = self.last_tab_dims {
                let (w, h) = ui::dialog_dimensions(&req, tc, tr);
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
            }

            self.request = Some(req);
            self.sized_for_tab = false;
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
                let Some(active) = tab_infos.iter().find(|t| t.active) else {
                    return false;
                };
                let tc = active.display_area_columns as u16;
                let tr = active.display_area_rows as u16;
                self.last_tab_dims = Some((tc, tr));

                // First TabUpdate after a new pipe message: size + center +
                // focus using full-tab display_area dims. Skipped on later
                // TabUpdates so the user isn't disturbed mid-interaction.
                if self.sized_for_tab {
                    return false;
                }
                let ids = get_plugin_ids();
                let pane_id = PaneId::Plugin(ids.plugin_id);
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
                self.sized_for_tab = true;
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

        // Skip the first render after resize until cols matches what we
        // asked for, preventing a flash at the wrong size.
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
        // Hide rather than close, so the plugin instance survives across
        // pinentry calls. The next pipe message reuses this pane — only
        // the very first pinentry invocation in a zellij session pays the
        // cost of zellij creating a new plugin pane.
        let ids = get_plugin_ids();
        hide_pane_with_id(PaneId::Plugin(ids.plugin_id));
    }
}
