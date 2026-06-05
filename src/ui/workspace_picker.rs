use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Clear, Paragraph},
    Frame,
};

use super::{
    scrollbar::{render_scrollbar, should_show_scrollbar},
    widgets::{panel_contrast_fg, render_panel_shell},
};
use crate::{
    app::state::{AppState, WorkspacePickerPreview, WorkspacePickerRow},
    terminal::TerminalRuntimeRegistry,
};

pub(super) fn render_workspace_picker_overlay(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
) {
    let popup = app.workspace_picker_popup_rect();
    let Some(inner) = render_panel_shell(frame, popup, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };

    let search = app.workspace_picker_search_rect();
    let body = app.workspace_picker_body_rect();
    let divider = app.workspace_picker_divider_rect();
    let preview = app.workspace_picker_preview_rect();
    let footer = app.workspace_picker_footer_rect();

    render_search(app, terminal_runtimes, frame, search);
    render_separator(
        frame,
        Rect::new(inner.x, search.y + 1, inner.width, 1),
        app.palette.surface1,
    );
    render_rows(app, terminal_runtimes, frame, body);
    render_workspace_picker_scrollbar(app, terminal_runtimes, frame, body);
    render_vertical_divider(app, frame, divider);
    render_preview(app, terminal_runtimes, frame, preview);
    render_footer(app, frame, footer);
}

fn render_search(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let p = &app.palette;
    let rows = app.workspace_picker_rows_from(terminal_runtimes);
    let query = app.workspace_picker.query.trim();
    let mut spans = vec![
        Span::styled(" workspaces ", Style::default().fg(p.accent)),
        Span::styled("/ ", Style::default().fg(p.overlay0)),
    ];
    if query.is_empty() {
        spans.push(Span::styled(
            "search workspace names",
            Style::default().fg(p.overlay0),
        ));
    } else {
        spans.push(Span::styled(query.to_string(), Style::default().fg(p.text)));
    }
    spans.push(Span::styled(
        format!(
            "{:>width$} shown",
            rows.len(),
            width = area.width.saturating_sub(24) as usize
        ),
        Style::default().fg(p.overlay0),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_rows(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    body: Rect,
) {
    if body.height == 0 || body.width == 0 {
        return;
    }

    let rows = app.workspace_picker_rows_from(terminal_runtimes);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(" no workspaces").style(Style::default().fg(app.palette.overlay0)),
            body,
        );
        return;
    }

    let start = app.workspace_picker.scroll.min(rows.len());
    let end = rows.len().min(start.saturating_add(body.height as usize));
    for (visible_idx, row) in rows[start..end].iter().enumerate() {
        let idx = start + visible_idx;
        let y = body.y + visible_idx as u16;
        let rect = Rect::new(body.x, y, body.width, 1);
        render_row(app, frame, rect, row, idx == app.workspace_picker.selected);
    }
}

fn render_row(
    app: &AppState,
    frame: &mut Frame,
    rect: Rect,
    row: &WorkspacePickerRow,
    selected: bool,
) {
    let p = &app.palette;
    frame.render_widget(Clear, rect);
    let base_style = if selected {
        Style::default().bg(p.accent).fg(panel_contrast_fg(p))
    } else {
        Style::default().bg(p.panel_bg).fg(p.text)
    };
    let dim_style = if selected {
        base_style
    } else {
        Style::default().fg(p.overlay0).bg(p.panel_bg)
    };
    let text_style = if selected {
        base_style.add_modifier(Modifier::BOLD)
    } else if row.is_current {
        Style::default()
            .fg(p.text)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.subtext0).bg(p.panel_bg)
    };

    let marker = if selected { "→" } else { " " };
    let current = if row.is_current { "◆" } else { " " };
    let fixed = format!(" {marker} {current} ");
    let meta_width = row_meta_width(rect.width);
    let title_budget = rect
        .width
        .saturating_sub(meta_width)
        .saturating_sub(fixed.chars().count() as u16)
        .saturating_sub(1) as usize;
    let title = truncate_text(&row.label, title_budget);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(fixed, dim_style),
            Span::styled(title, text_style),
        ]))
        .style(base_style),
        rect,
    );

    if meta_width > 0 {
        let meta_rect = Rect::new(
            rect.x + rect.width.saturating_sub(meta_width),
            rect.y,
            meta_width,
            1,
        );
        let meta = truncate_text(&row.meta, meta_width.saturating_sub(1) as usize);
        let style = if selected {
            base_style
        } else {
            Style::default().fg(p.overlay0).bg(p.panel_bg)
        };
        frame.render_widget(Paragraph::new(format!(" {meta}")).style(style), meta_rect);
    }
}

fn render_workspace_picker_scrollbar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    body: Rect,
) {
    if body.width <= 1 || body.height == 0 {
        return;
    }
    let rows = app.workspace_picker_rows_from(terminal_runtimes).len();
    let viewport = body.height as usize;
    if rows <= viewport {
        return;
    }
    let metrics = crate::pane::ScrollMetrics {
        viewport_rows: viewport,
        offset_from_bottom: rows
            .saturating_sub(viewport)
            .saturating_sub(app.workspace_picker.scroll),
        max_offset_from_bottom: rows.saturating_sub(viewport),
    };
    if !should_show_scrollbar(metrics) {
        return;
    }
    let track = Rect::new(body.x + body.width - 1, body.y, 1, body.height);
    render_scrollbar(
        frame,
        metrics,
        track,
        app.palette.surface_dim,
        app.palette.overlay0,
        "▕",
    );
}

fn render_vertical_divider(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = Text::from(
        (0..area.height)
            .map(|_| Line::from("│"))
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(app.palette.surface1)),
        area,
    );
}

fn render_preview(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let selected_label = app
        .workspace_picker_rows_from(terminal_runtimes)
        .get(app.workspace_picker.selected)
        .map(|row| row.label.clone())
        .unwrap_or_else(|| "preview".to_string());
    let title = truncate_text(
        &format!(" preview: {selected_label}"),
        area.width.saturating_sub(1) as usize,
    );
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(app.palette.overlay0)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    render_separator(
        frame,
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.min(1),
        ),
        app.palette.surface1,
    );

    let content = Rect::new(
        area.x,
        area.y.saturating_add(2),
        area.width,
        area.height.saturating_sub(2),
    );
    if content.height == 0 {
        return;
    }

    match &app.workspace_picker.preview {
        WorkspacePickerPreview::Content { text, .. } => {
            frame.render_widget(Paragraph::new(ansi_to_text(text)), content);
        }
        WorkspacePickerPreview::Empty { message } => {
            frame.render_widget(
                Paragraph::new(format!(" {message}"))
                    .style(Style::default().fg(app.palette.overlay0)),
                content,
            );
        }
    }
}

fn render_footer(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let p = &app.palette;
    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);
    let line = Line::from(vec![
        Span::styled(" enter", key),
        Span::styled(" switch  ", dim),
        Span::styled("type", key),
        Span::styled(" search  ", dim),
        Span::styled("j/k/↑↓", key),
        Span::styled(" move  ", dim),
        Span::styled("esc", key),
        Span::styled(" close", dim),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_separator(frame: &mut Frame, area: Rect, color: Color) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(color)),
        area,
    );
}

fn row_meta_width(width: u16) -> u16 {
    if width >= 38 {
        13
    } else if width >= 30 {
        9
    } else {
        0
    }
}

fn truncate_text(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let prefix: String = text.chars().take(max_width.saturating_sub(1)).collect();
    format!("{prefix}…")
}

fn ansi_to_text(input: &str) -> Text<'static> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut style = Style::default();
    let mut i = 0usize;

    while i < input.len() {
        let rest = &input[i..];
        if let Some(after_escape) = rest.strip_prefix("\x1b[") {
            flush_span(&mut spans, &mut buf, style);
            if let Some((offset, final_byte)) = csi_final_byte(after_escape) {
                let params = &after_escape[..offset];
                if final_byte == 'm' {
                    apply_sgr(params, &mut style);
                }
                i += 2 + offset + final_byte.len_utf8();
                continue;
            }
        }

        let Some(ch) = rest.chars().next() else {
            break;
        };
        i += ch.len_utf8();
        match ch {
            '\n' => {
                flush_span(&mut spans, &mut buf, style);
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            '\r' => {}
            _ => buf.push(ch),
        }
    }

    flush_span(&mut spans, &mut buf, style);
    lines.push(Line::from(spans));
    Text::from(lines)
}

fn flush_span(spans: &mut Vec<Span<'static>>, buf: &mut String, style: Style) {
    if buf.is_empty() {
        return;
    }
    spans.push(Span::styled(std::mem::take(buf), style));
}

fn csi_final_byte(input: &str) -> Option<(usize, char)> {
    for (idx, ch) in input.char_indices() {
        if ('@'..='~').contains(&ch) {
            return Some((idx, ch));
        }
    }
    None
}

fn apply_sgr(params: &str, style: &mut Style) {
    let params = if params.is_empty() { "0" } else { params };
    let values = params
        .split(';')
        .map(|part| part.parse::<u16>().ok())
        .collect::<Vec<_>>();
    let mut i = 0usize;
    while i < values.len() {
        let value = values[i].unwrap_or(0);
        match value {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            7 => *style = style.add_modifier(Modifier::REVERSED),
            9 => *style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            27 => *style = style.remove_modifier(Modifier::REVERSED),
            29 => *style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => *style = style.fg(basic_ansi_color(value - 30, false)),
            39 => *style = style.fg(Color::Reset),
            40..=47 => *style = style.bg(basic_ansi_color(value - 40, false)),
            49 => *style = style.bg(Color::Reset),
            90..=97 => *style = style.fg(basic_ansi_color(value - 90, true)),
            100..=107 => *style = style.bg(basic_ansi_color(value - 100, true)),
            38 | 48 => {
                if let Some((color, consumed)) = extended_ansi_color(&values[i + 1..]) {
                    if value == 38 {
                        *style = style.fg(color);
                    } else {
                        *style = style.bg(color);
                    }
                    i += consumed;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn extended_ansi_color(values: &[Option<u16>]) -> Option<(Color, usize)> {
    match values.first().copied().flatten()? {
        2 => {
            let r = values.get(1).copied().flatten()? as u8;
            let g = values.get(2).copied().flatten()? as u8;
            let b = values.get(3).copied().flatten()? as u8;
            Some((Color::Rgb(r, g, b), 4))
        }
        5 => {
            let idx = values.get(1).copied().flatten()? as u8;
            Some((indexed_ansi_color(idx), 2))
        }
        _ => None,
    }
}

fn basic_ansi_color(idx: u16, bright: bool) -> Color {
    match (idx, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        (7, true) => Color::White,
        _ => Color::Reset,
    }
}

fn indexed_ansi_color(idx: u8) -> Color {
    if idx < 16 {
        return match idx {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            _ => Color::White,
        };
    }

    if idx <= 231 {
        let idx = idx - 16;
        let r = (idx / 36) % 6;
        let g = (idx / 6) % 6;
        let b = idx % 6;
        return Color::Rgb(
            color_cube_value(r),
            color_cube_value(g),
            color_cube_value(b),
        );
    }

    let level = 8 + (idx - 232).saturating_mul(10);
    Color::Rgb(level, level, level)
}

fn color_cube_value(value: u8) -> u8 {
    if value == 0 {
        0
    } else {
        55 + value.saturating_mul(40)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_to_text_preserves_sgr_styles() {
        let text = ansi_to_text("plain \x1b[31;1mred\x1b[0m\n\x1b[38;2;1;2;3mtrue");

        assert_eq!(text.lines.len(), 2);
        assert_eq!(text.lines[0].spans[0].content.as_ref(), "plain ");
        assert_eq!(text.lines[0].spans[1].content.as_ref(), "red");
        assert_eq!(text.lines[0].spans[1].style.fg, Some(Color::Red));
        assert!(text.lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(text.lines[1].spans[0].style.fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn ansi_to_text_ignores_non_sgr_csi() {
        let text = ansi_to_text("a\x1b[2Kb");

        assert_eq!(text.lines[0].spans[0].content.as_ref(), "a");
        assert_eq!(text.lines[0].spans[1].content.as_ref(), "b");
    }
}
