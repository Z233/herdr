use super::*;

/// Decode a captured screen once, outside presentation loops, using the normal terminal styles.
pub(crate) fn render_ansi_snapshot(
    text: &str,
    cols: u16,
    rows: u16,
) -> Result<ratatui::buffer::Buffer, crate::ghostty::Error> {
    let mut terminal = crate::ghostty::Terminal::new(cols, rows, 0)?;
    let mut state = crate::ghostty::RenderState::new()?;
    state.update(&terminal)?;
    let initial = state.colors()?;
    terminal.write(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
    state.update(&terminal)?;
    let colors = state.colors()?;
    let host_theme = crate::terminal_theme::TerminalTheme::default();
    let foreground = ghostty_default_fg(colors.foreground, host_theme, Some(initial.foreground));
    let background = ghostty_default_bg(colors.background, host_theme, Some(initial.background));
    let overrides = PaletteOverrides::new(&colors.palette, &terminal.default_palette()?);
    let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, cols, rows));
    let mut row_iterator = crate::ghostty::RowIterator::new()?;
    let mut row_cells = crate::ghostty::RowCells::new()?;
    let mut iterator = state.populate_row_iterator(&mut row_iterator)?;
    let mut grapheme_bytes = Vec::new();
    let mut symbol = String::new();
    let mut y = 0;
    while y < rows && iterator.next() {
        let mut cells = iterator.populate_cells(&mut row_cells)?;
        let mut x = 0;
        while x < cols && cells.next() {
            let basic = cells.basic_data()?;
            let style = ghostty_cell_style(
                &cells,
                &basic,
                foreground,
                background,
                Some(ghostty_color(colors.foreground)),
                Some(ghostty_color(colors.background)),
                overrides.as_ref(),
            );
            let text = ghostty_buffer_symbol_into(
                &cells,
                basic.wide,
                true,
                &mut grapheme_bytes,
                &mut symbol,
            )?;
            buffer[(x, y)].set_symbol(text).set_style(style);
            x += 1;
        }
        y += 1;
    }
    Ok(buffer)
}
