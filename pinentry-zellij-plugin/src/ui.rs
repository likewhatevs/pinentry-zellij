//! Pinentry dialog UI using ratatui widgets.
//!
//! Renders a dynamically-sized bordered dialog with description text,
//! optional masked passphrase input, error display, and keyboard hints.
//! Sizing follows pinentry-curses conventions (MIN_PIN_LENGTH = 40).

use arrayvec::ArrayVec;
use ratatui_core::backend::Backend;
use ratatui_core::buffer::Buffer;
use ratatui_core::layout::{Alignment, Constraint, Layout, Rect};
use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};
use ratatui_core::widgets::Widget;
use ratatui_widgets::block::{Block, Padding};
use ratatui_widgets::borders::BorderType;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use crate::protocol::{PinentryCmd, PinentryRequest};

/// Minimum PIN entry field width (matches pinentry-curses).
const MIN_PIN_LENGTH: u16 = 40;

/// Frame padding: 2 border chars + 2 Block::padding chars.
const FRAME_PAD: u16 = 4;

/// Compute the ideal zellij pane dimensions (width, height) for a request.
///
/// `term_cols` and `term_rows` are the full terminal dimensions, used to
/// scale the dialog proportionally. The dialog takes ~60% of terminal width
/// (with a content-based minimum) for comfortable reading.
pub fn dialog_dimensions(request: &PinentryRequest, term_cols: u16, term_rows: u16) -> (u16, u16) {
    let area = Rect::new(0, 0, 200, 200);
    let content_w = dialog_width(request, area);
    // Use 60% of terminal width, but at least the content minimum.
    // Pane is borderless so pane size = dialog size (no zellij frame).
    let pane_w = ((term_cols as u32 * 60 / 100) as u16).max(content_w);
    let text_w = pane_w.saturating_sub(FRAME_PAD);
    let pane_h = dialog_height(request, text_w).min(term_rows);
    (pane_w.min(term_cols), pane_h)
}

/// Render the pinentry dialog to a ratatui backend.
pub fn render<B: Backend>(backend: &mut B, request: &PinentryRequest, input: &str) {
    let size = backend.size().expect("get size");
    let area = Rect::new(0, 0, size.width, size.height);
    let mut buf = Buffer::empty(area);

    render_to_buffer(&mut buf, request, input);

    let diff = buf.content.iter().enumerate().map(|(i, cell)| {
        let x = (i % size.width as usize) as u16;
        let y = (i / size.width as usize) as u16;
        (x, y, cell)
    });
    backend.draw(diff).expect("draw");
    backend.flush().expect("flush");
}

/// Compute the dialog width based on content.
fn dialog_width(request: &PinentryRequest, area: Rect) -> u16 {
    let mut w: u16 = 0;

    // PIN field width (GetPin only)
    if request.cmd == PinentryCmd::GetPin {
        let prompt_len = request.prompt.as_deref().unwrap_or("Passphrase:").len() as u16;
        w = prompt_len + 1 + MIN_PIN_LENGTH + FRAME_PAD;
    }

    // Hints line width (styled spans: "Enter Submit  Esc Cancel" etc.)
    let hints_len: u16 = match request.cmd {
        PinentryCmd::GetPin => 24,  // "Enter Submit  Esc Cancel"
        PinentryCmd::Confirm => 20, // "Enter OK  Esc Cancel"
        PinentryCmd::Message => 8,  // "Enter OK"
    };
    w = w.max(hints_len + FRAME_PAD);

    // Title: 2 padding spaces + 2 border chars
    let title_len = request.title.as_deref().unwrap_or("Pinentry").len() as u16 + 4;
    w = w.max(title_len + FRAME_PAD);

    // Clamp to available space
    w.min(area.width)
}

/// Compute the dialog height based on content.
fn dialog_height(request: &PinentryRequest, inner_width: u16) -> u16 {
    let mut h: u16 = 2; // top + bottom border

    // Description — estimate wrapped lines using word-boundary simulation.
    // Ratatui wraps at word boundaries, so we walk words and count line breaks.
    let desc_lines = if let Some(desc) = &request.desc {
        let wrap_width = inner_width.saturating_sub(2) as usize;
        if wrap_width > 0 {
            count_wrapped_lines(desc, wrap_width)
        } else {
            1
        }
    } else {
        1 // Min(1) constraint always takes at least 1 row
    };
    h += desc_lines as u16;

    // Error row
    if request.error.as_ref().is_some_and(|e| !e.is_empty()) {
        h += 1;
    }

    if request.cmd == PinentryCmd::GetPin {
        h += 4; // spacer + input + spacer + hints
    } else {
        h += 2; // spacer + hints
    }

    h
}

/// Estimate line count for word-wrapped text. Handles embedded newlines
/// (from Assuan percent-decoded `%0A`) and words wider than the wrap width.
fn count_wrapped_lines(text: &str, width: usize) -> usize {
    if width == 0 || text.is_empty() {
        return 1;
    }
    let mut total = 0;
    for paragraph in text.split('\n') {
        total += count_paragraph_lines(paragraph, width);
    }
    total.max(1)
}

/// Count lines for a single paragraph (no embedded newlines).
fn count_paragraph_lines(text: &str, width: usize) -> usize {
    if text.is_empty() {
        return 1; // empty paragraph = blank line
    }
    let mut lines = 1;
    let mut col = 0;
    for word in text.split_whitespace() {
        let wlen = word.len();
        if col == 0 {
            col = wlen;
        } else if col + 1 + wlen <= width {
            col += 1 + wlen;
        } else {
            lines += 1;
            col = wlen;
        }
        // Word wider than width wraps mid-word across multiple lines
        if col > width {
            lines += (col - 1) / width;
            col %= width;
            if col == 0 {
                col = width;
            }
        }
    }
    lines
}

fn render_to_buffer(buf: &mut Buffer, request: &PinentryRequest, input: &str) {
    // Fill the entire pane — the pane is already sized by dialog_dimensions.
    let dialog = buf.area;

    let title = request.title.as_deref().unwrap_or("Pinentry");

    let title_line = Line::from_iter([
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title_line)
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::new(1, 1, 0, 0));

    let inner = block.inner(dialog);
    block.render(dialog, buf);

    let has_error = request.error.as_ref().is_some_and(|e| !e.is_empty());

    // Max 6 constraints: desc + error + spacer + input + spacer + hints
    let chunks = {
        let mut c = ArrayVec::<Constraint, 6>::new();
        c.push(Constraint::Min(1)); // description
        if has_error {
            c.push(Constraint::Length(1)); // error (red)
        }
        if request.cmd == PinentryCmd::GetPin {
            c.push(Constraint::Length(1)); // spacer
            c.push(Constraint::Length(1)); // input
            c.push(Constraint::Length(1)); // spacer
        } else {
            c.push(Constraint::Length(1)); // spacer
        }
        c.push(Constraint::Length(1)); // hints
        Layout::vertical(c.as_slice()).split(inner)
    };

    // Walk through chunks by index to avoid hardcoded offsets.
    let mut row = 0;

    // Description — slightly dimmed for visual hierarchy
    if let Some(desc) = &request.desc {
        Paragraph::new(desc.as_str())
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center)
            .render(chunks[row], buf);
    }
    row += 1; // past description

    // Error — always red, in its own row
    if has_error {
        if let Some(error) = &request.error {
            Paragraph::new(error.as_str())
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center)
                .render(chunks[row], buf);
        }
        row += 1;
    }

    row += 1; // spacer

    if request.cmd == PinentryCmd::GetPin {
        // Masked input with underscores for empty positions (like pinentry-curses)
        let prompt = request.prompt.as_deref().unwrap_or("Passphrase:");
        let field_width = chunks[row].width.saturating_sub(prompt.len() as u16 + 2) as usize;
        let filled = input.len().min(field_width);
        let remaining = field_width.saturating_sub(filled);

        let mut spans = vec![
            Span::styled(
                prompt,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "*".repeat(filled),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        spans.push(Span::styled("▎", Style::default().fg(Color::White)));
        if remaining > 0 {
            spans.push(Span::styled(
                "_".repeat(remaining),
                Style::default().fg(Color::DarkGray),
            ));
        }
        let input_line = Line::from_iter(spans);
        Paragraph::new(input_line).render(chunks[row], buf);
        row += 1; // past input
        row += 1; // spacer

        let hints = Line::from_iter([
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" Submit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" Cancel", Style::default().fg(Color::DarkGray)),
        ]);
        Paragraph::new(hints)
            .alignment(Alignment::Center)
            .render(chunks[row], buf);
    } else {
        let hints = match request.cmd {
            PinentryCmd::Confirm => Line::from_iter([
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::styled(" OK  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::Cyan)),
                Span::styled(" Cancel", Style::default().fg(Color::DarkGray)),
            ]),
            PinentryCmd::Message => Line::from_iter([
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::styled(" OK", Style::default().fg(Color::DarkGray)),
            ]),
            PinentryCmd::GetPin => Line::default(),
        };
        Paragraph::new(hints)
            .alignment(Alignment::Center)
            .render(chunks[row], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PinentryCmd, PinentryRequest};

    fn make_request(cmd: PinentryCmd) -> PinentryRequest {
        PinentryRequest {
            cmd,
            title: Some("Test".into()),
            desc: Some("Enter your passphrase for key ABC".into()),
            prompt: Some("PIN:".into()),
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        }
    }

    fn render_to_string(request: &PinentryRequest, input: &str) -> String {
        let area = Rect::new(0, 0, 70, 20);
        let mut buf = Buffer::empty(area);
        render_to_buffer(&mut buf, request, input);
        buffer_to_string(&buf)
    }

    #[test]
    fn render_getpin_contains_prompt() {
        let request = make_request(PinentryCmd::GetPin);
        let content = render_to_string(&request, "abc");
        assert!(content.contains("PIN:"));
        assert!(content.contains("***")); // 3 chars masked
    }

    #[test]
    fn render_getpin_contains_desc() {
        let request = make_request(PinentryCmd::GetPin);
        let content = render_to_string(&request, "");
        assert!(content.contains("passphrase"));
    }

    #[test]
    fn render_confirm_contains_hints() {
        let request = make_request(PinentryCmd::Confirm);
        let content = render_to_string(&request, "");
        assert!(content.contains("Enter"));
        assert!(content.contains("OK"));
        assert!(content.contains("Cancel"));
    }

    #[test]
    fn render_message_no_cancel_hint() {
        let request = make_request(PinentryCmd::Message);
        let content = render_to_string(&request, "");
        assert!(content.contains("Enter"));
        assert!(content.contains("OK"));
        assert!(!content.contains("Cancel"));
    }

    #[test]
    fn render_with_error() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.error = Some("Bad passphrase".into());
        let content = render_to_string(&request, "");
        assert!(content.contains("Bad passphrase"));
    }

    #[test]
    fn render_uses_title() {
        let request = make_request(PinentryCmd::GetPin);
        let content = render_to_string(&request, "");
        assert!(content.contains("Test"));
    }

    #[test]
    fn render_default_title() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.title = None;
        let content = render_to_string(&request, "");
        assert!(content.contains("Pinentry"));
    }

    #[test]
    fn render_via_backend() {
        let request = make_request(PinentryCmd::GetPin);
        let mut backend = crate::backend::AnsiBackend::new(70, 20);
        render(&mut backend, &request, "test");
        let ansi = backend.to_ansi();
        assert!(!ansi.is_empty());
        assert!(ansi.contains("\x1b[H"));
    }

    #[test]
    fn render_error_only_no_desc() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.desc = None;
        request.error = Some("Invalid passphrase".into());
        let content = render_to_string(&request, "");
        assert!(content.contains("Invalid passphrase"));
    }

    #[test]
    fn render_no_desc_no_error() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.desc = None;
        request.error = None;
        let content = render_to_string(&request, "x");
        assert!(content.contains("PIN:"));
        assert!(content.contains("*"));
    }

    #[test]
    fn render_default_prompt() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.prompt = None;
        let content = render_to_string(&request, "ab");
        assert!(content.contains("Passphrase:"));
        assert!(content.contains("**"));
    }

    #[test]
    fn render_desc_with_error() {
        let mut request = make_request(PinentryCmd::GetPin);
        request.error = Some("Try again".into());
        let content = render_to_string(&request, "");
        assert!(content.contains("passphrase"));
        assert!(content.contains("Try again"));
    }

    #[test]
    fn dialog_width_respects_min_pin_length() {
        let request = make_request(PinentryCmd::GetPin);
        let area = Rect::new(0, 0, 120, 40);
        let w = dialog_width(&request, area);
        let prompt_len = "PIN:".len() as u16;
        assert!(w >= prompt_len + 1 + MIN_PIN_LENGTH + FRAME_PAD);
    }

    #[test]
    fn dialog_width_clamped_to_area() {
        let request = make_request(PinentryCmd::GetPin);
        let area = Rect::new(0, 0, 30, 10);
        let w = dialog_width(&request, area);
        assert!(w <= 30);
    }

    #[test]
    fn dialog_height_includes_error_row() {
        let mut request = make_request(PinentryCmd::GetPin);
        let h_no_err = dialog_height(&request, 50);
        request.error = Some("Bad passphrase".into());
        let h_with_err = dialog_height(&request, 50);
        assert_eq!(h_with_err, h_no_err + 1);
    }

    #[test]
    fn passphrase_is_left_aligned() {
        let request = make_request(PinentryCmd::GetPin);
        let area = Rect::new(0, 0, 70, 20);
        let mut buf = Buffer::empty(area);
        render_to_buffer(&mut buf, &request, "test");
        let content = buffer_to_string(&buf);
        // PIN: should appear near the left edge of the dialog, not centered
        // Find the line with PIN: and check it's not centered
        assert!(content.contains("PIN: ****"));
    }

    #[test]
    fn count_wrapped_lines_with_newlines() {
        assert_eq!(count_wrapped_lines("line1\nline2\nline3", 40), 3);
        assert_eq!(count_wrapped_lines("line1\n\nline3", 40), 3);
    }

    #[test]
    fn count_wrapped_lines_long_word() {
        let long = "A".repeat(80);
        // 80-char word in 40-char width = 2 lines
        assert_eq!(count_wrapped_lines(&long, 40), 2);
    }

    #[test]
    fn count_wrapped_lines_empty() {
        assert_eq!(count_wrapped_lines("", 40), 1);
    }

    #[test]
    fn dialog_width_confirm_narrower_than_getpin() {
        let getpin = make_request(PinentryCmd::GetPin);
        let confirm = make_request(PinentryCmd::Confirm);
        let area = Rect::new(0, 0, 120, 40);
        assert!(dialog_width(&confirm, area) < dialog_width(&getpin, area));
    }

    #[test]
    fn render_cursor_at_input_position() {
        let request = make_request(PinentryCmd::GetPin);
        let content = render_to_string(&request, "ab");
        // Cursor appears right after masked input
        assert!(content.contains("**▎"));
    }

    #[test]
    fn render_cursor_at_start_when_empty() {
        let request = make_request(PinentryCmd::GetPin);
        let content = render_to_string(&request, "");
        // Cursor at start of input field (after "PIN: ")
        assert!(content.contains("PIN: ▎"));
    }

    #[test]
    fn render_confirm_no_cursor() {
        let request = make_request(PinentryCmd::Confirm);
        let content = render_to_string(&request, "");
        assert!(!content.contains("▎"));
    }

    #[test]
    fn dialog_dimensions_borderless() {
        let request = make_request(PinentryCmd::GetPin);
        let (w, h) = dialog_dimensions(&request, 200, 50);
        // 60% of 200 = 120, content min ~49, so width = 120
        assert_eq!(w, 120);
        // Height: 2 border + 1 desc + 4 (spacer+input+spacer+hints) = 7
        assert_eq!(h, 7);
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let mut s = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                s.push_str(buf[(x, y)].symbol());
            }
        }
        s
    }
}
