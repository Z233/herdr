use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::{
    scrollbar::{render_scrollbar, should_show_scrollbar},
    status::state_dot,
    widgets::{panel_contrast_fg, render_panel_shell},
};
use crate::{
    app::{
        actions::{tab_aggregate_state, workspace_activity_summary},
        state::{AppState, Mode},
    },
    config::key_event_matches_combo,
    input::TerminalKey,
    layout::PaneId,
    terminal::TerminalRuntimeRegistry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspacePickerTarget {
    Workspace { ws_idx: usize },
    Tab { ws_idx: usize, tab_idx: usize },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum WorkspacePickerMode {
    #[default]
    Search,
    QuickSwitch,
    QuickSwitchSearch,
}

impl WorkspacePickerMode {
    pub(crate) fn search_visible(self) -> bool {
        matches!(self, Self::Search | Self::QuickSwitchSearch)
    }

    pub(crate) fn is_quick_switch(self) -> bool {
        matches!(self, Self::QuickSwitch | Self::QuickSwitchSearch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePickerRow {
    pub target: WorkspacePickerTarget,
    pub ws_idx: usize,
    pub depth: u8,
    pub label: String,
    pub meta: String,
    pub is_current: bool,
    pub expanded: bool,
    pub is_tab: bool,
    pub state: crate::detect::AgentState,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspacePickerPreview {
    Empty { message: String },
    Content { pane_id: PaneId, text: String },
}

impl Default for WorkspacePickerPreview {
    fn default() -> Self {
        Self::Empty {
            message: "select a workspace".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspacePickerState {
    pub mode: WorkspacePickerMode,
    pub query: String,
    pub selected: usize,
    pub scroll: usize,
    pub preview: WorkspacePickerPreview,
    pub preview_ws_idx: Option<usize>,
    pub expanded_workspaces: std::collections::HashSet<String>,
}

// ---------------------------------------------------------------------------
// Workspace picker operations
// ---------------------------------------------------------------------------

impl AppState {
    #[cfg(test)]
    pub(crate) fn open_workspace_picker(&mut self) {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.open_workspace_picker_from(&terminal_runtimes);
    }

    pub(crate) fn open_workspace_picker_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_picker.mode = WorkspacePickerMode::Search;
        self.workspace_picker.query.clear();
        self.workspace_picker.scroll = 0;
        self.workspace_picker.expanded_workspaces.clear();
        self.mode = Mode::WorkspacePicker;

        let target = self.active.unwrap_or(self.selected);
        self.workspace_picker.selected = self
            .workspace_picker_rows_from(terminal_runtimes)
            .iter()
            .position(|row| row.ws_idx == target)
            .unwrap_or(0);
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    pub(crate) fn open_quick_switch_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_picker.mode = WorkspacePickerMode::QuickSwitch;
        self.workspace_picker.query.clear();
        self.workspace_picker.scroll = 0;
        self.workspace_picker.expanded_workspaces.clear();
        self.mode = Mode::WorkspacePicker;

        let rows = self.workspace_picker_rows_from(terminal_runtimes);
        self.workspace_picker.selected = self
            .active
            .and_then(|active| {
                rows.iter()
                    .position(|row| row.ws_idx != active && !row.is_tab)
            })
            .unwrap_or(0);
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    #[cfg(test)]
    pub(crate) fn workspace_picker_rows(&self) -> Vec<WorkspacePickerRow> {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.workspace_picker_rows_from(&terminal_runtimes)
    }

    pub(crate) fn workspace_picker_rows_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Vec<WorkspacePickerRow> {
        let query = self.workspace_picker.query.trim();
        let workspace_indices = if self.workspace_picker.mode.is_quick_switch() {
            self.workspace_mru_indices()
        } else {
            (0..self.workspaces.len()).collect()
        };
        let search_visible = self.workspace_picker.mode.search_visible();
        let include_tabs = self.workspace_picker.mode == WorkspacePickerMode::QuickSwitch;
        let mut rows = Vec::new();

        for (order, ws_idx) in workspace_indices.into_iter().enumerate() {
            let Some(ws) = self.workspaces.get(ws_idx) else {
                continue;
            };
            let label = {
                let raw = ws.display_name_from(&self.terminals, terminal_runtimes);
                if crate::ui::sidebar::is_grouped_child_worktree(self, ws_idx) {
                    crate::ui::sidebar::grouped_child_display_label(
                        &raw,
                        ws.branch().as_deref(),
                        ws.custom_name.is_some(),
                    )
                } else {
                    raw
                }
            };
            let rank = if search_visible && !query.is_empty() {
                match workspace_picker_match_rank(query, &label) {
                    Some(rank) => rank,
                    None => continue,
                }
            } else {
                (0, order)
            };

            let expanded =
                include_tabs && self.workspace_picker.expanded_workspaces.contains(&ws.id);
            rows.push((
                self.workspace_picker_workspace_row(ws_idx, label, expanded),
                rank,
                order,
            ));
            if expanded {
                rows.extend(
                    self.workspace_picker_tab_rows(ws_idx)
                        .into_iter()
                        .map(|row| (row, rank, order)),
                );
            }
        }

        if search_visible && !query.is_empty() {
            rows.sort_by_key(|(_, rank, order)| (*rank, *order));
        }

        rows.into_iter().map(|(row, _, _)| row).collect()
    }

    fn workspace_picker_workspace_row(
        &self,
        ws_idx: usize,
        label: String,
        expanded: bool,
    ) -> WorkspacePickerRow {
        let ws = &self.workspaces[ws_idx];
        let pane_count = ws.tabs.iter().map(|tab| tab.panes.len()).sum::<usize>();
        let mut meta = if pane_count == 1 {
            "1 pane".to_string()
        } else {
            format!("{pane_count} panes")
        };
        let activity = workspace_activity_summary(ws, &self.terminals);
        if !activity.is_empty() {
            meta.push_str(" · ");
            meta.push_str(&activity);
        }
        let (state, seen) = ws.aggregate_state(&self.terminals);

        WorkspacePickerRow {
            target: WorkspacePickerTarget::Workspace { ws_idx },
            ws_idx,
            depth: 0,
            label,
            meta,
            is_current: self.active == Some(ws_idx),
            expanded,
            is_tab: false,
            state,
            seen,
        }
    }

    fn workspace_picker_tab_rows(&self, ws_idx: usize) -> Vec<WorkspacePickerRow> {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return Vec::new();
        };
        ws.tabs
            .iter()
            .enumerate()
            .map(|(tab_idx, tab)| {
                let pane_count = tab.panes.len();
                let (state, seen) = tab_aggregate_state(tab, &self.terminals);
                WorkspacePickerRow {
                    target: WorkspacePickerTarget::Tab { ws_idx, tab_idx },
                    ws_idx,
                    depth: 1,
                    label: tab.display_name(),
                    meta: if pane_count == 1 {
                        "1 pane".to_string()
                    } else {
                        format!("{pane_count} panes")
                    },
                    is_current: self.active == Some(ws_idx) && ws.active_tab_index() == tab_idx,
                    expanded: false,
                    is_tab: true,
                    state,
                    seen,
                }
            })
            .collect()
    }

    pub(crate) fn workspace_picker_max_scroll_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        viewport: usize,
    ) -> usize {
        if viewport == 0 {
            return 0;
        }
        self.workspace_picker_rows_from(terminal_runtimes)
            .len()
            .saturating_sub(viewport)
    }

    pub(crate) fn ensure_workspace_picker_selection_visible_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let viewport = self.workspace_picker_body_rect().height as usize;
        if viewport == 0 {
            self.workspace_picker.scroll = 0;
            return;
        }
        let max_scroll = self.workspace_picker_max_scroll_from(terminal_runtimes, viewport);
        if self.workspace_picker.selected < self.workspace_picker.scroll {
            self.workspace_picker.scroll = self.workspace_picker.selected;
        } else if self.workspace_picker.selected >= self.workspace_picker.scroll + viewport {
            self.workspace_picker.scroll = self
                .workspace_picker
                .selected
                .saturating_add(1)
                .saturating_sub(viewport);
        }
        self.workspace_picker.scroll = self.workspace_picker.scroll.min(max_scroll);
    }

    pub(crate) fn clamp_workspace_picker_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let count = self.workspace_picker_rows_from(terminal_runtimes).len();
        self.workspace_picker.selected =
            self.workspace_picker.selected.min(count.saturating_sub(1));
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    pub(crate) fn move_workspace_picker_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        delta: isize,
    ) {
        let count = self.workspace_picker_rows_from(terminal_runtimes).len();
        if count == 0 {
            self.workspace_picker.selected = 0;
            self.workspace_picker.scroll = 0;
            self.refresh_workspace_picker_preview_from(terminal_runtimes);
            return;
        }

        let current = self.workspace_picker.selected.min(count - 1) as isize;
        self.workspace_picker.selected = (current + delta).clamp(0, count as isize - 1) as usize;
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    pub(crate) fn cycle_quick_switch_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        delta: isize,
    ) {
        let rows = self.workspace_picker_rows_from(terminal_runtimes);
        let workspace_positions = rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| (!row.is_tab).then_some(idx))
            .collect::<Vec<_>>();
        if workspace_positions.is_empty() {
            self.workspace_picker.selected = 0;
            self.workspace_picker.scroll = 0;
            self.refresh_workspace_picker_preview_from(terminal_runtimes);
            return;
        }

        let selected_ws_idx = rows
            .get(self.workspace_picker.selected)
            .map(|row| row.ws_idx)
            .unwrap_or_else(|| rows[workspace_positions[0]].ws_idx);
        let current_pos = workspace_positions
            .iter()
            .position(|idx| rows[*idx].ws_idx == selected_ws_idx)
            .unwrap_or(0);
        let next_pos =
            (current_pos as isize + delta).rem_euclid(workspace_positions.len() as isize) as usize;
        self.workspace_picker.selected = workspace_positions[next_pos];
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    pub(crate) fn expand_selected_workspace_picker_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let Some(ws_idx) = self.selected_workspace_picker_ws_idx_from(terminal_runtimes) else {
            return;
        };
        let Some(workspace_id) = self.workspaces.get(ws_idx).map(|ws| ws.id.clone()) else {
            return;
        };
        self.workspace_picker
            .expanded_workspaces
            .insert(workspace_id);
        self.clamp_workspace_picker_selection_from(terminal_runtimes);
    }

    pub(crate) fn collapse_selected_workspace_picker_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let rows = self.workspace_picker_rows_from(terminal_runtimes);
        let Some(row) = rows.get(self.workspace_picker.selected) else {
            return;
        };
        let ws_idx = row.ws_idx;
        let Some(workspace_id) = self.workspaces.get(ws_idx).map(|ws| ws.id.clone()) else {
            return;
        };
        self.workspace_picker
            .expanded_workspaces
            .remove(&workspace_id);
        self.workspace_picker.selected = self
            .workspace_picker_rows_from(terminal_runtimes)
            .iter()
            .position(|row| row.ws_idx == ws_idx && !row.is_tab)
            .unwrap_or(0);
        self.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_picker_preview_from(terminal_runtimes);
    }

    pub(crate) fn enter_quick_switch_search_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_picker.mode = WorkspacePickerMode::QuickSwitchSearch;
        self.workspace_picker.query.clear();
        self.workspace_picker.expanded_workspaces.clear();
        self.clamp_workspace_picker_selection_from(terminal_runtimes);
    }

    pub(crate) fn leave_quick_switch_search_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_picker.mode = WorkspacePickerMode::QuickSwitch;
        self.workspace_picker.query.clear();
        self.clamp_workspace_picker_selection_from(terminal_runtimes);
    }

    pub(crate) fn accept_workspace_picker_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        let Some(target) = self
            .workspace_picker_rows_from(terminal_runtimes)
            .get(self.workspace_picker.selected)
            .map(|row| row.target.clone())
        else {
            return false;
        };
        self.focus_workspace_picker_target(target)
    }

    fn focus_workspace_picker_target(&mut self, target: WorkspacePickerTarget) -> bool {
        match target {
            WorkspacePickerTarget::Workspace { ws_idx } => {
                if ws_idx >= self.workspaces.len() {
                    return false;
                }
                self.switch_workspace(ws_idx);
                self.mode = Mode::Terminal;
                true
            }
            WorkspacePickerTarget::Tab { ws_idx, tab_idx } => {
                if self.switch_workspace_tab(ws_idx, tab_idx) {
                    self.mode = Mode::Terminal;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn refresh_workspace_picker_preview_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let Some(ws_idx) = self.selected_workspace_picker_ws_idx_from(terminal_runtimes) else {
            self.workspace_picker.preview_ws_idx = None;
            self.workspace_picker.preview = WorkspacePickerPreview::Empty {
                message: if self.workspaces.is_empty() {
                    "no workspaces".to_string()
                } else {
                    "no matching workspaces".to_string()
                },
            };
            return;
        };

        self.workspace_picker.preview_ws_idx = Some(ws_idx);
        let target = self
            .workspace_picker_rows_from(terminal_runtimes)
            .get(self.workspace_picker.selected)
            .map(|row| row.target.clone())
            .unwrap_or(WorkspacePickerTarget::Workspace { ws_idx });
        self.workspace_picker.preview =
            self.workspace_picker_preview_for_target_from(terminal_runtimes, target);
    }

    fn selected_workspace_picker_ws_idx_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Option<usize> {
        self.workspace_picker_rows_from(terminal_runtimes)
            .get(self.workspace_picker.selected)
            .map(|row| row.ws_idx)
    }

    fn workspace_picker_preview_for_target_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        target: WorkspacePickerTarget,
    ) -> WorkspacePickerPreview {
        let (ws_idx, pane_id) = match target {
            WorkspacePickerTarget::Workspace { ws_idx } => {
                let Some(ws) = self.workspaces.get(ws_idx) else {
                    return WorkspacePickerPreview::Empty {
                        message: "workspace unavailable".to_string(),
                    };
                };
                let Some(pane_id) = ws.focused_pane_id() else {
                    return WorkspacePickerPreview::Empty {
                        message: "no pane content".to_string(),
                    };
                };
                (ws_idx, pane_id)
            }
            WorkspacePickerTarget::Tab { ws_idx, tab_idx } => {
                let Some(tab) = self
                    .workspaces
                    .get(ws_idx)
                    .and_then(|ws| ws.tabs.get(tab_idx))
                else {
                    return WorkspacePickerPreview::Empty {
                        message: "tab unavailable".to_string(),
                    };
                };
                (ws_idx, tab.layout.focused())
            }
        };
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return WorkspacePickerPreview::Empty {
                message: "workspace unavailable".to_string(),
            };
        };
        if ws.find_tab_index_for_pane(pane_id).is_none() {
            return WorkspacePickerPreview::Empty {
                message: "no pane content".to_string(),
            };
        }

        // Mirrors pane.read with ReadSource::Visible and ReadFormat::Ansi.
        let Some(runtime) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
        else {
            return WorkspacePickerPreview::Empty {
                message: "no pane content".to_string(),
            };
        };
        let text = runtime.visible_ansi();
        if text.trim().is_empty() {
            return WorkspacePickerPreview::Empty {
                message: "no pane content".to_string(),
            };
        }

        WorkspacePickerPreview::Content { pane_id, text }
    }

    fn workspace_mru_indices(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for workspace_id in &self.workspace_mru {
            if let Some(idx) = self
                .workspaces
                .iter()
                .position(|workspace| &workspace.id == workspace_id)
            {
                if !order.contains(&idx) {
                    order.push(idx);
                }
            }
        }
        if let Some(active) = self.active {
            if let Some(pos) = order.iter().position(|idx| *idx == active) {
                let active = order.remove(pos);
                order.insert(0, active);
            } else if active < self.workspaces.len() {
                order.insert(0, active);
            }
        }
        for idx in 0..self.workspaces.len() {
            if !order.contains(&idx) {
                order.push(idx);
            }
        }
        order
    }
}

fn workspace_picker_match_rank(query: &str, text: &str) -> Option<(u8, usize)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some((0, 0));
    }
    let haystack = text.to_lowercase();
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return Some((0, 0));
    }

    if terms.iter().all(|needle| {
        haystack
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word.starts_with(needle))
    }) {
        return Some((0, workspace_picker_match_position_sum(&terms, &haystack)));
    }

    if terms.iter().all(|needle| haystack.contains(needle)) {
        return Some((1, workspace_picker_match_position_sum(&terms, &haystack)));
    }

    if terms
        .iter()
        .all(|needle| needle.chars().count() > 1 && workspace_picker_fuzzy_match(needle, &haystack))
    {
        return Some((2, usize::MAX / 2));
    }

    None
}

fn workspace_picker_match_position_sum(terms: &[&str], haystack: &str) -> usize {
    terms
        .iter()
        .filter_map(|needle| haystack.find(needle))
        .sum()
}

fn workspace_picker_fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_ch| chars.any(|text_ch| text_ch == needle_ch))
}

pub(crate) fn handle_workspace_picker_key(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    if state.workspace_picker.mode.is_quick_switch()
        && state.workspace_picker.mode != WorkspacePickerMode::QuickSwitchSearch
    {
        handle_quick_switch_workspace_picker_key(state, terminal_runtimes, key);
        return;
    }

    match key.code {
        KeyCode::Esc if state.workspace_picker.mode == WorkspacePickerMode::QuickSwitchSearch => {
            state.leave_quick_switch_search_from(terminal_runtimes);
        }
        KeyCode::Esc => close_workspace_picker(state),
        KeyCode::Enter => {
            state.accept_workspace_picker_selection_from(terminal_runtimes);
        }
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            close_workspace_picker(state)
        }
        KeyCode::Char('q')
            if key.modifiers.is_empty() && state.workspace_picker.query.is_empty() =>
        {
            close_workspace_picker(state);
        }
        KeyCode::Backspace => {
            state.workspace_picker.query.pop();
            state.clamp_workspace_picker_selection_from(terminal_runtimes);
        }
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
            state.workspace_picker.query.clear();
            state.clamp_workspace_picker_selection_from(terminal_runtimes);
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            state.move_workspace_picker_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            state.move_workspace_picker_selection_from(terminal_runtimes, -1);
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
            state.move_workspace_picker_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
            state.move_workspace_picker_selection_from(terminal_runtimes, -1);
        }
        KeyCode::PageDown => {
            state.move_workspace_picker_selection_from(
                terminal_runtimes,
                state.workspace_picker_body_rect().height.max(1) as isize,
            );
        }
        KeyCode::PageUp => {
            state.move_workspace_picker_selection_from(
                terminal_runtimes,
                -(state.workspace_picker_body_rect().height.max(1) as isize),
            );
        }
        KeyCode::Home => {
            state.workspace_picker.selected = 0;
            state.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_picker_preview_from(terminal_runtimes);
        }
        KeyCode::End => {
            state.workspace_picker.selected = state
                .workspace_picker_rows_from(terminal_runtimes)
                .len()
                .saturating_sub(1);
            state.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_picker_preview_from(terminal_runtimes);
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            state.workspace_picker.query.push(c);
            state.clamp_workspace_picker_selection_from(terminal_runtimes);
        }
        _ => {}
    }
}

fn handle_quick_switch_workspace_picker_key(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    let forward_cycle = state.keybinds.quick_switch_forward_combo();
    let backward_cycle = state.keybinds.quick_switch_backward_combo();

    match key.code {
        KeyCode::Esc => close_workspace_picker(state),
        KeyCode::Enter => {
            state.accept_workspace_picker_selection_from(terminal_runtimes);
        }
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            close_workspace_picker(state)
        }
        _ if forward_cycle.is_some_and(|combo| key_event_matches_combo(&key, combo)) => {
            state.cycle_quick_switch_workspace_from(terminal_runtimes, 1);
        }
        _ if backward_cycle.is_some_and(|combo| key_event_matches_combo(&key, combo)) => {
            state.cycle_quick_switch_workspace_from(terminal_runtimes, -1);
        }
        KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
            if quick_switch_command_modifiers(state, key.modifiers) =>
        {
            state.cycle_quick_switch_workspace_from(terminal_runtimes, -1);
        }
        KeyCode::Char('s') if quick_switch_command_modifiers(state, key.modifiers) => {
            state.enter_quick_switch_search_from(terminal_runtimes);
        }
        KeyCode::Char('l') if quick_switch_command_modifiers(state, key.modifiers) => {
            state.expand_selected_workspace_picker_workspace_from(terminal_runtimes);
        }
        KeyCode::Char('h') if quick_switch_command_modifiers(state, key.modifiers) => {
            state.collapse_selected_workspace_picker_workspace_from(terminal_runtimes);
        }
        KeyCode::Down | KeyCode::Char('j')
            if quick_switch_command_modifiers(state, key.modifiers) =>
        {
            state.move_workspace_picker_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Up | KeyCode::Char('k')
            if quick_switch_command_modifiers(state, key.modifiers) =>
        {
            state.move_workspace_picker_selection_from(terminal_runtimes, -1);
        }
        KeyCode::Home => {
            state.workspace_picker.selected = 0;
            state.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_picker_preview_from(terminal_runtimes);
        }
        KeyCode::End => {
            state.workspace_picker.selected = state
                .workspace_picker_rows_from(terminal_runtimes)
                .len()
                .saturating_sub(1);
            state.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_picker_preview_from(terminal_runtimes);
        }
        _ => {}
    }
}

fn quick_switch_command_modifiers(state: &AppState, modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
        || state
            .keybinds
            .quick_switch_command_modifiers()
            .is_some_and(|quick_switch_modifiers| modifiers.contains(quick_switch_modifiers))
}

pub(crate) fn handle_quick_switch_key_release(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    key: TerminalKey,
) -> bool {
    if state.mode != Mode::WorkspacePicker
        || state.workspace_picker.mode != WorkspacePickerMode::QuickSwitch
    {
        return false;
    }

    if state
        .keybinds
        .quick_switch_workspace
        .bindings
        .iter()
        .all(|b| b.trigger.is_direct() && b.trigger.combo().1.is_empty())
    {
        tracing::trace!("quick_switch_workspace has no modifiers; release-accept unavailable");
        return false;
    }

    if !quick_switch_modifier_release_matches(&state.keybinds.quick_switch_workspace, key) {
        return false;
    }

    state.accept_workspace_picker_selection_from(terminal_runtimes)
}

fn quick_switch_modifier_release_matches(
    bindings: &crate::config::ActionKeybinds,
    key: TerminalKey,
) -> bool {
    let Some(modifier) = released_modifier(key.code) else {
        return false;
    };

    bindings
        .bindings
        .iter()
        .any(|binding| binding.trigger.is_direct() && binding.trigger.combo().1.contains(modifier))
}

fn released_modifier(code: KeyCode) -> Option<KeyModifiers> {
    match code {
        KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift) => {
            Some(KeyModifiers::SHIFT)
        }
        KeyCode::Modifier(ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl) => {
            Some(KeyModifiers::CONTROL)
        }
        KeyCode::Modifier(ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt) => {
            Some(KeyModifiers::ALT)
        }
        KeyCode::Modifier(ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper) => {
            Some(KeyModifiers::SUPER)
        }
        KeyCode::Modifier(ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper) => {
            Some(KeyModifiers::HYPER)
        }
        KeyCode::Modifier(ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta) => {
            Some(KeyModifiers::META)
        }
        _ => None,
    }
}

pub(crate) fn handle_workspace_picker_mouse(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    mouse: MouseEvent,
) {
    match mouse.kind {
        MouseEventKind::Moved => {
            if let Some(idx) =
                state.workspace_picker_row_index_at_from(terminal_runtimes, mouse.column, mouse.row)
            {
                if state.workspace_picker.selected != idx {
                    state.workspace_picker.selected = idx;
                    state.ensure_workspace_picker_selection_visible_from(terminal_runtimes);
                    state.refresh_workspace_picker_preview_from(terminal_runtimes);
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(idx) =
                state.workspace_picker_row_index_at_from(terminal_runtimes, mouse.column, mouse.row)
            {
                state.workspace_picker.selected = idx;
                state.accept_workspace_picker_selection_from(terminal_runtimes);
            } else if !state.workspace_picker_popup_contains(mouse.column, mouse.row) {
                close_workspace_picker(state);
            }
        }
        MouseEventKind::ScrollUp => {
            state.workspace_picker.scroll = state.workspace_picker.scroll.saturating_sub(3);
            state.workspace_picker.selected = state.workspace_picker.scroll;
            state.clamp_workspace_picker_selection_from(terminal_runtimes);
        }
        MouseEventKind::ScrollDown => {
            let viewport = state.workspace_picker_body_rect().height as usize;
            let max = state.workspace_picker_max_scroll_from(terminal_runtimes, viewport);
            state.workspace_picker.scroll =
                state.workspace_picker.scroll.saturating_add(3).min(max);
            state.workspace_picker.selected = state.workspace_picker.scroll;
            state.clamp_workspace_picker_selection_from(terminal_runtimes);
        }
        _ => {}
    }
}

fn close_workspace_picker(state: &mut AppState) {
    if state.active.is_some() {
        state.mode = Mode::Terminal;
    } else {
        state.mode = Mode::Navigate;
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn workspace_picker_list_width(width: u16) -> u16 {
    if width < 48 {
        return width;
    }
    (width / 3).clamp(24, 42).min(width.saturating_sub(1))
}

impl AppState {
    pub(crate) fn workspace_picker_popup_rect(&self) -> Rect {
        let area = self.view.sidebar_rect.union(self.view.terminal_area);
        let margin_x = (area.width / 12).max(2);
        let margin_y = (area.height / 9).max(1);
        let width = area.width.saturating_sub(margin_x.saturating_mul(2));
        let height = area.height.saturating_sub(margin_y.saturating_mul(2));
        Rect::new(
            area.x + margin_x,
            area.y + margin_y,
            width.max(4),
            height.max(4),
        )
    }

    pub(crate) fn workspace_picker_inner_rect(&self) -> Rect {
        Block::default()
            .borders(Borders::ALL)
            .inner(self.workspace_picker_popup_rect())
    }

    pub(crate) fn workspace_picker_search_rect(&self) -> Rect {
        if !self.workspace_picker.mode.search_visible() {
            return Rect::default();
        }
        let inner = self.workspace_picker_inner_rect();
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1))
    }

    pub(crate) fn workspace_picker_content_rect(&self) -> Rect {
        let inner = self.workspace_picker_inner_rect();
        if self.workspace_picker.mode.search_visible() {
            if inner.height <= 3 {
                return Rect::default();
            }
            return Rect::new(
                inner.x,
                inner.y + 2,
                inner.width,
                inner.height.saturating_sub(3),
            );
        }
        if inner.height <= 1 {
            return Rect::default();
        }
        Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        )
    }

    pub(crate) fn workspace_picker_body_rect(&self) -> Rect {
        let content = self.workspace_picker_content_rect();
        let list_width = workspace_picker_list_width(content.width);
        Rect::new(content.x, content.y, list_width, content.height)
    }

    pub(crate) fn workspace_picker_divider_rect(&self) -> Rect {
        let content = self.workspace_picker_content_rect();
        let list_width = workspace_picker_list_width(content.width);
        if content.width <= list_width || content.height == 0 {
            return Rect::default();
        }
        Rect::new(content.x + list_width, content.y, 1, content.height)
    }

    pub(crate) fn workspace_picker_preview_rect(&self) -> Rect {
        let content = self.workspace_picker_content_rect();
        let list_width = workspace_picker_list_width(content.width);
        let x = content.x.saturating_add(list_width).saturating_add(1);
        let width = content.width.saturating_sub(list_width).saturating_sub(1);
        Rect::new(x, content.y, width, content.height)
    }

    pub(crate) fn workspace_picker_footer_rect(&self) -> Rect {
        let inner = self.workspace_picker_inner_rect();
        Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            inner.height.min(1),
        )
    }

    pub(crate) fn workspace_picker_popup_contains(&self, col: u16, row: u16) -> bool {
        rect_contains(self.workspace_picker_popup_rect(), col, row)
    }

    pub(crate) fn workspace_picker_row_index_at_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        col: u16,
        row: u16,
    ) -> Option<usize> {
        let body = self.workspace_picker_body_rect();
        if !rect_contains(body, col, row) {
            return None;
        }
        let idx = self
            .workspace_picker
            .scroll
            .saturating_add(row.saturating_sub(body.y) as usize);
        (idx < self.workspace_picker_rows_from(terminal_runtimes).len()).then_some(idx)
    }
}

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

    if app.workspace_picker.mode.search_visible() {
        render_search(app, terminal_runtimes, frame, search);
        render_separator(
            frame,
            Rect::new(inner.x, search.y + 1, inner.width, 1),
            app.palette.surface1,
        );
    }
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
    let title = if app.workspace_picker.mode.is_quick_switch() {
        " quick switch "
    } else {
        " workspaces "
    };
    let mut spans = vec![Span::styled(title, Style::default().fg(p.accent))];
    spans.push(Span::styled("/ ", Style::default().fg(p.overlay0)));
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
        let message = if app.workspaces.is_empty() {
            " no workspaces"
        } else {
            " no matching workspaces"
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(app.palette.overlay0)),
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

    let (dot, dot_style) = state_dot(row.state, row.seen, p);

    let mut spans: Vec<Span> = Vec::new();
    if app.workspace_picker.mode.is_quick_switch() {
        let caret = if row.is_tab {
            " "
        } else if row.expanded {
            "▾"
        } else {
            "▸"
        };
        let indent = "  ".repeat(row.depth as usize);
        spans.push(Span::styled(format!(" {indent}{caret} "), dim_style));
    } else {
        spans.push(Span::styled(" ", dim_style));
    }
    spans.push(Span::styled(dot, dot_style));
    spans.push(Span::styled(" ", dim_style));

    let meta_width = row_meta_width(rect.width);
    let fixed_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
    let title_budget = rect
        .width
        .saturating_sub(meta_width)
        .saturating_sub(fixed_width)
        .saturating_sub(1) as usize;
    let title = truncate_text(&row.label, title_budget);
    spans.push(Span::styled(title, text_style));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), rect);

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
    let line = if app.workspace_picker.mode.is_quick_switch() {
        let esc_label = if app.workspace_picker.mode == WorkspacePickerMode::QuickSwitchSearch {
            " back"
        } else {
            " close"
        };
        Line::from(vec![
            Span::styled(" enter", key),
            Span::styled(" switch  ", dim),
            Span::styled("tab", key),
            Span::styled(" cycle  ", dim),
            Span::styled("l/h", key),
            Span::styled(" expand  ", dim),
            Span::styled("s", key),
            Span::styled(" search  ", dim),
            Span::styled("esc", key),
            Span::styled(esc_label, dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" enter", key),
            Span::styled(" switch  ", dim),
            Span::styled("type", key),
            Span::styled(" search  ", dim),
            Span::styled("j/k/↑↓", key),
            Span::styled(" move  ", dim),
            Span::styled("esc", key),
            Span::styled(" close", dim),
        ])
    };
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
    use crate::workspace::Workspace;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, ModifierKeyCode};
    use ratatui::{backend::TestBackend, Terminal};

    fn app_with_workspaces(names: &[&str]) -> AppState {
        let mut state = AppState::test_new();
        state.toast_config.delay_seconds = 0;
        for name in names {
            let ws = Workspace::test_new(name);
            state.workspaces.push(ws);
        }
        state.ensure_test_terminals();
        if !state.workspaces.is_empty() {
            state.active = Some(0);
            state.mode = Mode::Terminal;
        }
        state
    }

    fn state_with_workspaces(names: &[&str]) -> AppState {
        let mut state = AppState::test_new();
        state.workspaces = names.iter().map(|name| Workspace::test_new(name)).collect();
        if !state.workspaces.is_empty() {
            state.active = Some(0);
            state.selected = 0;
            state.mode = Mode::Navigate;
        }
        state
    }

    fn mark_linked_worktree(state: &mut AppState, ws_idx: usize) {
        state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: format!("/repo/worktree-{ws_idx}").into(),
            is_linked_worktree: true,
        });
    }

    fn mark_parent_worktree(state: &mut AppState, ws_idx: usize) {
        state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr".into(),
            is_linked_worktree: false,
        });
    }

    fn config_with_quick_switch(
        quick_switch_workspace: &str,
        quick_switch_workspace_backward: Option<&str>,
    ) -> crate::config::Config {
        let backward = quick_switch_workspace_backward
            .map(|binding| format!("quick_switch_workspace_backward = {binding:?}\n"))
            .unwrap_or_default();
        toml::from_str(&format!(
            "[keys]\nquick_switch_workspace = {quick_switch_workspace:?}\n{backward}"
        ))
        .expect("quick switch config should parse")
    }

    fn state_with_quick_switch_binding(
        quick_switch_workspace: &str,
        quick_switch_workspace_backward: Option<&str>,
    ) -> (
        AppState,
        crate::terminal::TerminalRuntimeRegistry,
        KeyModifiers,
    ) {
        let config =
            config_with_quick_switch(quick_switch_workspace, quick_switch_workspace_backward);
        let quick_switch_modifiers = config
            .keybinds()
            .quick_switch_command_modifiers()
            .expect("quick switch should have a direct binding");
        let mut state = state_with_workspaces(&["main", "issue", "docs"]);
        state.keybinds = config.keybinds();
        state.workspaces[1].test_add_tab(Some("logs"));
        state.workspaces[2].test_add_tab(Some("logs"));
        state.switch_workspace(2);
        state.switch_workspace(0);

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_quick_switch_workspace_from(&terminal_runtimes);
        (state, terminal_runtimes, quick_switch_modifiers)
    }

    fn selected_workspace_picker_ws_idx(
        state: &AppState,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> usize {
        let rows = state.workspace_picker_rows_from(terminal_runtimes);
        rows[state.workspace_picker.selected].ws_idx
    }

    #[test]
    fn opening_workspace_picker_selects_active_workspace() {
        let mut state = app_with_workspaces(&["one", "two"]);
        state.active = Some(1);

        state.open_workspace_picker();

        assert_eq!(state.mode, Mode::WorkspacePicker);
        assert_eq!(
            state.workspace_picker_rows()[state.workspace_picker.selected].ws_idx,
            1
        );
    }
    #[test]
    fn workspace_picker_filters_workspace_names_only() {
        let mut state = app_with_workspaces(&["one", "issue"]);
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_manual_label("weekly review".into());

        state.open_workspace_picker();
        state.workspace_picker.query = "weekly".into();
        assert!(state.workspace_picker_rows().is_empty());

        state.workspace_picker.query = "ie".into();
        let rows = state.workspace_picker_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ws_idx, 1);
    }
    #[test]
    fn workspace_picker_empty_state_has_placeholder_preview() {
        let mut state = AppState::test_new();

        state.open_workspace_picker();

        assert_eq!(state.mode, Mode::WorkspacePicker);
        assert!(matches!(
            state.workspace_picker.preview,
            WorkspacePickerPreview::Empty { ref message } if message == "no workspaces"
        ));
    }
    #[test]
    fn workspace_picker_preview_handles_missing_runtime() {
        let mut state = app_with_workspaces(&["one"]);

        state.open_workspace_picker();

        assert!(matches!(
            state.workspace_picker.preview,
            WorkspacePickerPreview::Empty { ref message } if message == "no pane content"
        ));
    }
    #[test]
    fn workspace_picker_shows_branch_name_for_grouped_child() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);
        // Clear custom name so the workspace is auto-named — this is the case
        // where grouped_child_display_label substitutes the branch name.
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());

        state.open_workspace_picker();
        let rows = state.workspace_picker_rows();

        // The grouped child should display the branch name (without "worktree/" prefix),
        // matching the sidebar's grouped_child_display_label behavior.
        let child_row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child_row.label, "issue-137");
    }
    #[test]
    fn workspace_picker_shows_cwd_name_for_standalone_workspace() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        // No worktree_space set — standalone workspace.
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let raw_label = state.workspaces[1].display_name_from(&state.terminals, &terminal_runtimes);

        state.open_workspace_picker();
        let rows = state.workspace_picker_rows();

        // Standalone workspace — no branch substitution, label is CWD-derived.
        let row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(row.label, raw_label);
        assert_ne!(row.label, "issue-137");
    }
    #[test]
    fn workspace_picker_keeps_custom_name_for_grouped_child() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());
        state.workspaces[1].custom_name = Some("my-custom-name".into());

        state.open_workspace_picker();
        let rows = state.workspace_picker_rows();

        let child_row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child_row.label, "my-custom-name");
    }
    #[test]
    fn workspace_picker_shows_cwd_name_for_linked_only_group() {
        // Two linked worktrees with no parent — should NOT form a parentless group.
        let mut state = app_with_workspaces(&["issue", "review"]);
        mark_linked_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);
        state.workspaces[0].custom_name = None;
        state.workspaces[1].custom_name = None;
        state.workspaces[0].cached_git_branch = Some("worktree/issue-137".into());
        state.workspaces[1].cached_git_branch = Some("worktree/review-42".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let raw0 = state.workspaces[0].display_name_from(&state.terminals, &terminal_runtimes);
        let raw1 = state.workspaces[1].display_name_from(&state.terminals, &terminal_runtimes);

        state.open_workspace_picker();
        let rows = state.workspace_picker_rows();

        // Without a parent worktree, these are not grouped children —
        // the CWD-derived name should be used unchanged.
        let row0 = rows.iter().find(|r| r.ws_idx == 0).unwrap();
        assert_eq!(row0.label, raw0);
        assert_ne!(row0.label, "issue-137");
        let row1 = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(row1.label, raw1);
        assert_ne!(row1.label, "review-42");
    }
    #[test]
    fn quick_switch_uses_mru_order_and_preselects_previous_workspace() {
        let mut state = app_with_workspaces(&["one", "two", "three"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.switch_workspace(1);
        state.switch_workspace(2);

        state.open_quick_switch_workspace_from(&terminal_runtimes);
        let rows = state.workspace_picker_rows();

        assert_eq!(rows[0].ws_idx, 2);
        assert_eq!(rows[1].ws_idx, 1);
        assert_eq!(rows[2].ws_idx, 0);
        assert_eq!(state.workspace_picker.selected, 1);
        assert_eq!(
            state.workspace_picker.mode,
            WorkspacePickerMode::QuickSwitch
        );
    }
    #[test]
    fn quick_switch_tab_cycles_workspace_rows_only() {
        let mut state = app_with_workspaces(&["one", "two", "three"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.switch_workspace(1);
        state.switch_workspace(2);
        state.open_quick_switch_workspace_from(&terminal_runtimes);

        state.cycle_quick_switch_workspace_from(&terminal_runtimes, 1);

        let selected = &state.workspace_picker_rows()[state.workspace_picker.selected];
        assert_eq!(selected.ws_idx, 0);
        assert!(!selected.is_tab);
    }
    #[test]
    fn quick_switch_can_expand_and_select_workspace_tab() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
        state.switch_workspace(1);
        state.switch_workspace(0);
        state.open_quick_switch_workspace_from(&terminal_runtimes);

        state.expand_selected_workspace_picker_workspace_from(&terminal_runtimes);
        state.move_workspace_picker_selection_from(&terminal_runtimes, 2);

        let selected = state.workspace_picker_rows()[state.workspace_picker.selected].clone();
        assert_eq!(
            selected.target,
            WorkspacePickerTarget::Tab {
                ws_idx: 1,
                tab_idx: second_tab
            }
        );
        assert!(state.accept_workspace_picker_selection_from(&terminal_runtimes));
        assert_eq!(state.active, Some(1));
        assert_eq!(state.workspaces[1].active_tab_index(), second_tab);
    }
    #[test]
    fn workspace_picker_typing_filters_and_enter_switches_workspace() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_picker_from(&terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
        );
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.active, Some(1));
        assert_eq!(state.mode, Mode::Terminal);
    }
    #[test]
    fn workspace_picker_escape_closes_without_switching() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_picker_from(&terminal_runtimes);
        state.move_workspace_picker_selection_from(&terminal_runtimes, 1);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.active, Some(0));
        assert_eq!(state.mode, Mode::Terminal);
    }
    #[test]
    fn quick_switch_search_toggle_returns_to_quick_switch_on_escape() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_quick_switch_workspace_from(&terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty()),
        );
        assert_eq!(
            state.workspace_picker.mode,
            WorkspacePickerMode::QuickSwitchSearch
        );

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(
            state.workspace_picker.mode,
            WorkspacePickerMode::QuickSwitch
        );
        assert_eq!(state.mode, Mode::WorkspacePicker);
    }
    #[test]
    fn quick_switch_accepts_control_modified_commands_while_shortcut_is_held() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.workspaces[1].test_add_tab(Some("logs"));
        state.switch_workspace(1);
        state.switch_workspace(0);
        state.open_quick_switch_workspace_from(&terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
        );
        assert!(state
            .workspace_picker_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == 1 && row.is_tab && row.label == "logs"));

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        assert!(
            state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab
        );

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        );
        assert!(
            !state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab
        );

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );
        assert!(!state
            .workspace_picker_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == 1 && row.is_tab));

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            state.workspace_picker.mode,
            WorkspacePickerMode::QuickSwitchSearch
        );
    }
    #[test]
    fn quick_switch_cycle_and_commands_follow_configured_direct_binding() {
        let cases = [
            (
                "ctrl+tab",
                KeyCode::Tab,
                KeyModifiers::CONTROL,
                KeyCode::Tab,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            (
                "cmd+tab",
                KeyCode::Tab,
                KeyModifiers::SUPER,
                KeyCode::Tab,
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
            ),
            (
                "alt+tab",
                KeyCode::Tab,
                KeyModifiers::ALT,
                KeyCode::Tab,
                KeyModifiers::ALT | KeyModifiers::SHIFT,
            ),
            (
                "super+tab",
                KeyCode::Tab,
                KeyModifiers::SUPER,
                KeyCode::Tab,
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
            ),
            (
                "cmd+f13",
                KeyCode::F(13),
                KeyModifiers::SUPER,
                KeyCode::F(13),
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
            ),
            (
                "ctrl+f13",
                KeyCode::F(13),
                KeyModifiers::CONTROL,
                KeyCode::F(13),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ];

        for (binding, forward_code, forward_modifiers, backward_code, backward_modifiers) in cases {
            let (mut state, terminal_runtimes, command_modifiers) =
                state_with_quick_switch_binding(binding, None);
            let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(forward_code, forward_modifiers),
            );
            assert_ne!(
                selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
                initial_ws,
                "{binding} should cycle forward"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(backward_code, backward_modifiers),
            );
            assert_eq!(
                selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
                initial_ws,
                "{binding} should cycle backward"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('l'), command_modifiers),
            );
            assert!(
                state
                    .workspace_picker_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == initial_ws && row.is_tab && row.label == "logs"),
                "{binding} should expand with the configured modifier"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('j'), command_modifiers),
            );
            assert!(
                state.workspace_picker_rows_from(&terminal_runtimes)
                    [state.workspace_picker.selected]
                    .is_tab,
                "{binding} should move down with the configured modifier"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('k'), command_modifiers),
            );
            assert!(
                !state.workspace_picker_rows_from(&terminal_runtimes)
                    [state.workspace_picker.selected]
                    .is_tab,
                "{binding} should move up with the configured modifier"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('h'), command_modifiers),
            );
            assert!(
                !state
                    .workspace_picker_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == initial_ws && row.is_tab),
                "{binding} should collapse with the configured modifier"
            );

            handle_workspace_picker_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('s'), command_modifiers),
            );
            assert_eq!(
                state.workspace_picker.mode,
                WorkspacePickerMode::QuickSwitchSearch,
                "{binding} should enter search with the configured modifier"
            );
        }
    }
    #[test]
    fn quick_switch_backward_cycle_uses_explicit_override_when_set() {
        let (mut state, terminal_runtimes, _) =
            state_with_quick_switch_binding("cmd+f13", Some("cmd+f14"));
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(13), KeyModifiers::SUPER),
        );
        let after_forward = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(13), KeyModifiers::SUPER | KeyModifiers::SHIFT),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            after_forward,
            "derived backward shortcut should not apply when an override is set"
        );

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(14), KeyModifiers::SUPER),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            initial_ws
        );
    }
    #[test]
    fn quick_switch_shift_press_cycles_backward() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        // Cycle forward first
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );
        let after_forward = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        // Shift press while Ctrl held cycles backward
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press while modifier held should cycle backward"
        );
    }
    #[test]
    fn quick_shift_press_with_right_shift_cycles_backward() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::RightShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Right Shift press should also cycle backward"
        );
    }
    #[test]
    fn quick_switch_command_chars_work_with_shift_held() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        // 'l' with CONTROL | SHIFT should still expand (using .contains())
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('l'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            state
                .workspace_picker_rows_from(&terminal_runtimes)
                .iter()
                .any(|row| row.ws_idx == initial_ws && row.is_tab && row.label == "logs"),
            "expand command should work when Shift is held alongside modifier"
        );

        // 'j' with CONTROL | SHIFT should still move down
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('j'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab,
            "move down command should work when Shift is held alongside modifier"
        );

        // 'k' with CONTROL | SHIFT should still move up
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            !state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab,
            "move up command should work when Shift is held alongside modifier"
        );

        // 'h' with CONTROL | SHIFT should still collapse
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('h'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            !state
                .workspace_picker_rows_from(&terminal_runtimes)
                .iter()
                .any(|row| row.ws_idx == initial_ws && row.is_tab),
            "collapse command should work when Shift is held alongside modifier"
        );

        // 's' with CONTROL | SHIFT should still enter search
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            state.workspace_picker.mode,
            WorkspacePickerMode::QuickSwitchSearch,
            "search command should work when Shift is held alongside modifier"
        );
    }
    #[test]
    fn quick_switch_shift_press_with_backward_override_still_works() {
        // Shift-press should cycle backward even when an explicit backward override is set
        let (mut state, terminal_runtimes, _) =
            state_with_quick_switch_binding("ctrl+tab", Some("ctrl+shift+tab"));
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        // Cycle forward first
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );
        let after_forward = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        // Shift press should still cycle backward (native overlay, not configurable)
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press should cycle backward even with explicit backward override"
        );
    }
    #[test]
    fn quick_switch_shift_press_without_modifier_is_noop() {
        // Shift press without the quick-switch modifier held should not cycle
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_picker_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press without quick-switch modifier should be a no-op"
        );
    }
    #[test]
    fn quick_switch_shift_press_wraps_from_first_workspace() {
        // Picker starts at first non-active workspace in MRU order (workspace 2).
        // Cycle backward once to reach workspace 0, then again to wrap around.
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let start_ws = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);

        // First backward: from start_ws → workspace 0
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        let after_one = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);
        assert_eq!(after_one, 0, "first backward should land at workspace 0");

        // Second backward: wrap from workspace 0 to last in MRU order
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        let after_wrap = selected_workspace_picker_ws_idx(&state, &terminal_runtimes);
        assert_ne!(
            after_wrap, start_ws,
            "wrap-around should land on a different workspace"
        );
        assert_ne!(
            after_wrap, after_one,
            "wrap-around should move from workspace 0"
        );
    }
    #[test]
    fn quick_switch_arrow_keys_work_with_shift_held() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);

        // Expand workspace so tab rows are visible for arrow navigation
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('l'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );

        // Down arrow with CONTROL | SHIFT should still move down
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert!(
            state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab,
            "Down arrow should work when Shift is held alongside modifier"
        );

        // Up arrow with CONTROL | SHIFT should still move up
        handle_workspace_picker_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert!(
            !state.workspace_picker_rows_from(&terminal_runtimes)[state.workspace_picker.selected]
                .is_tab,
            "Up arrow should work when Shift is held alongside modifier"
        );
    }
    #[test]
    fn quick_switch_shift_release_does_not_accept() {
        let (state, _terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);

        // Releasing Shift while Ctrl is held should NOT trigger accept
        let accepted = quick_switch_modifier_release_matches(
            &state.keybinds.quick_switch_workspace,
            TerminalKey::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::empty(),
            ),
        );
        assert!(
            !accepted,
            "Shift release should not trigger quick-switch accept"
        );
    }

    #[test]
    fn render_row_shows_state_dot_for_blocked_workspace() {
        let app = AppState::test_new();
        let row = WorkspacePickerRow {
            target: WorkspacePickerTarget::Workspace { ws_idx: 0 },
            ws_idx: 0,
            depth: 0,
            label: "test".to_string(),
            meta: "".to_string(),
            is_current: true,
            expanded: false,
            is_tab: false,
            state: crate::detect::AgentState::Blocked,
            seen: false,
        };
        let area = Rect::new(0, 0, 20, 1);
        let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        // Span layout: [" ", dot, " ", title]
        // Non-quick-switch, not selected: " ● test" → dot at position 1
        assert_eq!(buf[(1, 0)].symbol(), "●");
    }

    #[test]
    fn render_row_shows_state_dot_for_idle_seen_workspace() {
        let app = AppState::test_new();
        let row = WorkspacePickerRow {
            target: WorkspacePickerTarget::Workspace { ws_idx: 0 },
            ws_idx: 0,
            depth: 0,
            label: "test".to_string(),
            meta: "".to_string(),
            is_current: false,
            expanded: false,
            is_tab: false,
            state: crate::detect::AgentState::Idle,
            seen: true,
        };
        let area = Rect::new(0, 0, 20, 1);
        let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        // Span layout: [" ", dot, " ", title]
        // Non-quick-switch, not selected: " ○ test" → dot at position 1
        assert_eq!(buf[(1, 0)].symbol(), "○");
    }

    #[test]
    fn render_row_shows_quick_switch_indent() {
        let mut app = AppState::test_new();
        app.workspace_picker.mode = WorkspacePickerMode::QuickSwitch;
        let row = WorkspacePickerRow {
            target: WorkspacePickerTarget::Workspace { ws_idx: 0 },
            ws_idx: 0,
            depth: 1,
            label: "ws".to_string(),
            meta: "".to_string(),
            is_current: false,
            expanded: false,
            is_tab: false,
            state: crate::detect::AgentState::Working,
            seen: false,
        };
        let area = Rect::new(0, 0, 20, 1);
        let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        // Quick-switch workspace row, depth=1: "   ▸ ● ws"
        // Position 3 is caret "▸" (after leading space + 2-char indent)
        assert_eq!(buf[(3, 0)].symbol(), "▸");
    }

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
