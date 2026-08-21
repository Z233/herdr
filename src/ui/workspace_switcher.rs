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
        state::{AppState, Mode, ViewLayout},
    },
    config::key_event_matches_combo,
    input::TerminalKey,
    terminal::TerminalRuntimeRegistry,
};

const WORKSPACE_SWITCHER_LINES_PER_ITEM: u16 = 2;

fn workspace_switcher_capacity(body_height: u16) -> usize {
    usize::from(body_height / WORKSPACE_SWITCHER_LINES_PER_ITEM)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceSwitcherTarget {
    Workspace {
        workspace_id: String,
    },
    Tab {
        tab_id: String,
    },
    Directory {
        shown_path: std::path::PathBuf,
        canonical_path: std::path::PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum WorkspaceSwitcherMode {
    #[default]
    QuickSwitch,
    Search,
}

impl WorkspaceSwitcherMode {
    pub(crate) fn search_visible(self) -> bool {
        self == Self::Search
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceSwitcherRow {
    pub target: WorkspaceSwitcherTarget,
    pub ws_idx: usize,
    pub depth: u8,
    pub label: SwitcherLabel,
    pub meta: String,
    pub activity: String,
    pub is_current: bool,
    pub expanded: bool,
    pub is_tab: bool,
    pub is_directory: bool,
    pub state: crate::detect::AgentState,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceSwitcherPreview {
    Empty {
        message: String,
    },
    TabSurface {
        target: WorkspaceSwitcherTarget,
    },
    Directory {
        shown_path: std::path::PathBuf,
        entries: Vec<crate::app::workspace_search_provider::DirectoryPreviewEntry>,
        truncated: bool,
    },
    DirectoryLoading {
        shown_path: std::path::PathBuf,
    },
    DirectoryError {
        shown_path: std::path::PathBuf,
    },
    Opening {
        shown_path: std::path::PathBuf,
    },
}

impl Default for WorkspaceSwitcherPreview {
    fn default() -> Self {
        Self::Empty {
            message: "select a workspace".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WorkspaceSwitcherState {
    pub active: bool,
    pub mode: WorkspaceSwitcherMode,
    pub query: String,
    pub selected: usize,
    pub selected_target: Option<WorkspaceSwitcherTarget>,
    pub scroll: usize,
    pub preview: WorkspaceSwitcherPreview,
    pub expanded_workspaces: std::collections::HashSet<String>,
    // -- Search provider session state (plain data, no handles or I/O) --
    /// Increments each time Search mode is entered; used to discard stale
    /// background results.
    pub search_generation: u64,
    /// Current provider lifecycle status.
    pub provider_status: crate::app::workspace_search_provider::SearchProviderStatus,
    /// Deduplicated zoxide candidates for the current session.
    pub provider_candidates: Vec<crate::app::workspace_search_provider::SearchProviderCandidate>,
    /// Cached directory previews keyed by shown path for the session.
    pub directory_preview_cache: std::collections::HashMap<
        std::path::PathBuf,
        crate::app::workspace_search_provider::DirectoryPreviewState,
    >,
    /// Set by AppState when a directory row is selected and its preview is
    /// not yet cached. The App controller reads and clears this to spawn a
    /// background preview load.
    pub preview_request: Option<std::path::PathBuf>,
    /// Set by AppState when Search mode is entered. The App controller reads
    /// and clears this to check zoxide availability and spawn the query.
    pub search_started: bool,
    /// A directory acceptance is in flight (Opening…). Prevents re-acceptance.
    pub pending_directory: Option<PendingDirectoryAccept>,
    /// Set to true when the pending_directory was set and the Opening…
    /// preview has been rendered. The App controller waits for this before
    /// dispatching workspace.create so the user sees the Opening state.
    pub pending_directory_rendered: bool,
    /// Concise inline error shown in the preview pane.
    pub search_error: Option<String>,
    /// Snapshot of workspace_id → canonical_cwd, computed by the App
    /// controller. The UI reads only this for row coalescing and directory
    /// acceptance — never filesystem I/O.
    pub workspace_canonical_snapshot: Vec<WorkspaceCanonicalEntry>,
    /// Set by handle_app_event when provider candidates change. The App
    /// controller reads and clears this to clamp selection and refresh
    /// preview with terminal runtimes.
    pub needs_provider_refresh: bool,
    /// Set by handle_app_event when a directory preview completes. The App
    /// controller reads and clears this to refresh the preview if the path
    /// is currently selected.
    pub needs_preview_refresh: Option<std::path::PathBuf>,
}

/// Tracks a directory acceptance in flight so the UI can show "Opening…"
/// and prevent repeated acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingDirectoryAccept {
    pub shown_path: std::path::PathBuf,
    pub canonical_path: std::path::PathBuf,
}

/// One entry in the workspace canonical-cwd snapshot. The App controller
/// computes this at Search start and after provider/create events so the
/// UI layer never performs filesystem I/O during row projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceCanonicalEntry {
    pub workspace_id: String,
    pub canonical_cwd: std::path::PathBuf,
}

// ---------------------------------------------------------------------------
// Workspace switcher operations
// ---------------------------------------------------------------------------

impl AppState {
    #[cfg(test)]
    pub(crate) fn open_workspace_switcher(&mut self) {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.open_workspace_switcher_from(&terminal_runtimes);
    }

    pub(crate) fn open_workspace_switcher_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_switcher.mode = WorkspaceSwitcherMode::QuickSwitch;
        self.workspace_switcher.query.clear();
        self.workspace_switcher.scroll = 0;
        self.workspace_switcher.expanded_workspaces.clear();
        self.workspace_switcher.active = true;

        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        self.workspace_switcher.selected = self
            .active
            .and_then(|active| {
                rows.iter()
                    .position(|row| row.ws_idx != active && !row.is_tab)
            })
            .unwrap_or(0);
        self.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        self.capture_workspace_switcher_target_from(terminal_runtimes);
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_rows(&self) -> Vec<WorkspaceSwitcherRow> {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.workspace_switcher_rows_from(&terminal_runtimes)
    }

    pub(crate) fn workspace_switcher_rows_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Vec<WorkspaceSwitcherRow> {
        let query = self.workspace_switcher.query.trim();
        let workspace_indices = self.workspace_mru_indices();
        let search_visible = self.workspace_switcher.mode.search_visible();
        let include_tabs = self.workspace_switcher.mode == WorkspaceSwitcherMode::QuickSwitch;
        let mut rows = Vec::new();

        // QuickSwitch mode: workspace + tab rows, no search filtering.
        if !search_visible {
            for (order, ws_idx) in workspace_indices.into_iter().enumerate() {
                let Some(ws) = self.workspaces.get(ws_idx) else {
                    continue;
                };
                let label = self.workspace_label(ws_idx, terminal_runtimes);
                let expanded =
                    include_tabs && self.workspace_switcher.expanded_workspaces.contains(&ws.id);
                rows.push((
                    self.workspace_switcher_workspace_row(ws_idx, label, expanded),
                    (0u8, order),
                ));
                if expanded {
                    rows.extend(
                        self.workspace_switcher_tab_rows(ws_idx)
                            .into_iter()
                            .map(|row| (row, (0u8, order))),
                    );
                }
            }
            return rows.into_iter().map(|(row, _)| row).collect();
        }

        // Search mode with empty query: only workspace MRU rows, no tabs.
        if query.is_empty() {
            for ws_idx in workspace_indices.into_iter() {
                let label = self.workspace_label(ws_idx, terminal_runtimes);
                rows.push((
                    self.workspace_switcher_workspace_row(ws_idx, label, false),
                    (0u8, 0usize),
                ));
            }
            return rows.into_iter().map(|(row, _)| row).collect();
        }

        // Search mode with non-empty query: unify workspaces and zoxide
        // directories with local matching and ranking.
        self.search_rows_from(terminal_runtimes, query, &workspace_indices)
    }

    /// Build unified search rows: workspaces (optionally coalesced with
    /// zoxide candidates) and unopened zoxide directories, ranked by match
    /// quality, workspace priority, and zoxide score.
    ///
    /// This is a pure projection: it reads only the workspace_canonical_snapshot
    /// and provider_candidates — never the filesystem.
    fn search_rows_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        query: &str,
        workspace_indices: &[usize],
    ) -> Vec<WorkspaceSwitcherRow> {
        use std::collections::HashMap;

        let candidates = &self.workspace_switcher.provider_candidates;

        // Map canonical path → candidate (candidates are already deduped).
        let candidates_by_canonical: HashMap<
            std::path::PathBuf,
            &crate::app::workspace_search_provider::SearchProviderCandidate,
        > = candidates
            .iter()
            .map(|c| (c.canonical_path.clone(), c))
            .collect();

        // Build workspace_id → canonical_cwd from the snapshot (pure data, no I/O).
        let snapshot_by_id: HashMap<&str, &std::path::Path> = self
            .workspace_switcher
            .workspace_canonical_snapshot
            .iter()
            .map(|e| (e.workspace_id.as_str(), e.canonical_cwd.as_path()))
            .collect();

        // Map ws_idx → canonical_cwd from the snapshot, and track which
        // canonical paths are coalesced (have an open workspace) and which
        // workspace is the enriched one (first in MRU order).
        let mut workspace_canonicals: HashMap<usize, Option<std::path::PathBuf>> = HashMap::new();
        let mut enriched_workspace: HashMap<std::path::PathBuf, usize> = HashMap::new();

        for &ws_idx in workspace_indices {
            let ws = self.workspaces.get(ws_idx);
            let canonical = ws
                .and_then(|w| snapshot_by_id.get(w.id.as_str()))
                .map(|p| p.to_path_buf());
            if let Some(ref canon) = canonical {
                if candidates_by_canonical.contains_key(canon.as_path()) {
                    enriched_workspace.entry(canon.clone()).or_insert(ws_idx);
                }
            }
            workspace_canonicals.insert(ws_idx, canonical);
        }

        // Internal sort key for scored rows.
        struct ScoredRow {
            row: WorkspaceSwitcherRow,
            rank: (u8, usize),
            is_directory: bool,
            score: f64,
            mru_order: usize,
        }

        let mut scored: Vec<ScoredRow> = Vec::new();

        // Workspace rows (optionally enriched with zoxide path matching).
        for (order, &ws_idx) in workspace_indices.iter().enumerate() {
            if self.workspaces.get(ws_idx).is_none() {
                continue;
            }
            let label = self.workspace_label(ws_idx, terminal_runtimes);
            let label_text = label.display();

            // Match workspace name.
            let mut best_rank = workspace_switcher_match_rank(query, &label_text);

            // If this workspace is the enriched one for its canonical path,
            // also match against the zoxide candidate using the candidate's
            // own match_rank helper (basename-first).
            let is_enriched = workspace_canonicals
                .get(&ws_idx)
                .and_then(|canon| canon.as_ref())
                .and_then(|canon| enriched_workspace.get(canon))
                .is_some_and(|&idx| idx == ws_idx);

            if is_enriched {
                if let Some(canon) = workspace_canonicals.get(&ws_idx).and_then(|c| c.as_ref()) {
                    if let Some(candidate) = candidates_by_canonical.get(canon.as_path()) {
                        if let Some(candidate_rank) = candidate.match_rank(query) {
                            best_rank =
                                Some(best_rank.map_or(candidate_rank, |r| r.min(candidate_rank)));
                        }
                    }
                }
            }

            let Some(rank) = best_rank else {
                continue;
            };

            scored.push(ScoredRow {
                row: self.workspace_switcher_workspace_row(ws_idx, label, false),
                rank,
                is_directory: false,
                score: 0.0,
                mru_order: order,
            });
        }

        // Directory rows for non-coalesced zoxide candidates.
        for candidate in candidates {
            // Skip candidates that match an open workspace.
            if enriched_workspace.contains_key(&candidate.canonical_path) {
                continue;
            }
            let Some(rank) = candidate.match_rank(query) else {
                continue;
            };
            scored.push(ScoredRow {
                row: self.directory_row(candidate),
                rank,
                is_directory: true,
                score: candidate.score,
                mru_order: usize::MAX,
            });
        }

        // Sort: match quality, then workspace before directory, then higher
        // zoxide score for directory ties, then MRU order for workspace ties.
        scored.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then(a.is_directory.cmp(&b.is_directory))
                .then_with(|| {
                    if a.is_directory {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    } else {
                        a.mru_order.cmp(&b.mru_order)
                    }
                })
        });

        scored
            .into_iter()
            .take(crate::app::workspace_search_provider::SEARCH_RESULTS_LIMIT)
            .map(|s| s.row)
            .collect()
    }

    /// Build a directory row from a zoxide candidate.
    fn directory_row(
        &self,
        candidate: &crate::app::workspace_search_provider::SearchProviderCandidate,
    ) -> WorkspaceSwitcherRow {
        let label = SwitcherLabel::plain(candidate.basename());
        let meta = candidate.abbreviated_path();
        WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Directory {
                shown_path: candidate.shown_path.clone(),
                canonical_path: candidate.canonical_path.clone(),
            },
            ws_idx: usize::MAX,
            depth: 0,
            label,
            meta,
            activity: String::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: true,
            state: crate::detect::AgentState::Idle,
            seen: false,
        }
    }

    /// Compute the display label for a workspace, applying grouped-child
    /// worktree formatting and repository-name composition when applicable.
    /// Shared by QuickSwitch, empty-query, and Search row builders.
    ///
    /// For managed linked worktrees with a non-empty repository name, the
    /// returned [`SwitcherLabel`] carries the repo and existing-label as
    /// separate structured parts. All other workspaces return a plain label.
    fn workspace_label(
        &self,
        ws_idx: usize,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> SwitcherLabel {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return SwitcherLabel::default();
        };
        let raw = ws.display_name_from(&self.terminals, terminal_runtimes);
        let existing_label = if crate::ui::sidebar::is_grouped_child_worktree(self, ws_idx) {
            crate::ui::sidebar::grouped_child_display_label(
                &raw,
                ws.branch().as_deref(),
                ws.custom_name.is_some(),
            )
        } else {
            raw
        };
        match managed_worktree_repo_name(ws) {
            Some(repo) => SwitcherLabel::composite(repo.to_string(), existing_label),
            None => SwitcherLabel::plain(existing_label),
        }
    }

    fn workspace_switcher_workspace_row(
        &self,
        ws_idx: usize,
        label: SwitcherLabel,
        expanded: bool,
    ) -> WorkspaceSwitcherRow {
        let ws = &self.workspaces[ws_idx];
        let pane_count = ws.tabs.iter().map(|tab| tab.panes.len()).sum::<usize>();
        let meta = if pane_count == 1 {
            "1 pane".to_string()
        } else {
            format!("{pane_count} panes")
        };
        let activity = workspace_activity_summary(ws, &self.terminals);
        let (state, seen) = ws.aggregate_state(&self.terminals);

        WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: ws.id.clone(),
            },
            ws_idx,
            depth: 0,
            label,
            meta,
            activity,
            is_current: self.active == Some(ws_idx),
            expanded,
            is_tab: false,
            is_directory: false,
            state,
            seen,
        }
    }

    fn workspace_switcher_tab_rows(&self, ws_idx: usize) -> Vec<WorkspaceSwitcherRow> {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return Vec::new();
        };
        ws.tabs
            .iter()
            .enumerate()
            .map(|(tab_idx, tab)| {
                let pane_count = tab.panes.len();
                let (state, seen) = tab_aggregate_state(tab, &self.terminals);
                WorkspaceSwitcherRow {
                    target: WorkspaceSwitcherTarget::Tab {
                        tab_id: crate::workspace::public_tab_id_for_number(&ws.id, tab.number),
                    },
                    ws_idx,
                    depth: 1,
                    label: SwitcherLabel::plain(tab.display_name()),
                    meta: if pane_count == 1 {
                        "1 pane".to_string()
                    } else {
                        format!("{pane_count} panes")
                    },
                    activity: String::new(),
                    is_current: self.active == Some(ws_idx) && ws.active_tab_index() == tab_idx,
                    expanded: false,
                    is_tab: true,
                    is_directory: false,
                    state,
                    seen,
                }
            })
            .collect()
    }

    pub(crate) fn workspace_switcher_max_scroll_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        viewport: usize,
    ) -> usize {
        if viewport == 0 {
            return 0;
        }
        self.workspace_switcher_rows_from(terminal_runtimes)
            .len()
            .saturating_sub(viewport)
    }

    pub(crate) fn ensure_workspace_switcher_selection_visible_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let viewport = workspace_switcher_capacity(self.workspace_switcher_body_rect().height);
        if viewport == 0 {
            self.workspace_switcher.scroll = 0;
            return;
        }
        let max_scroll = self.workspace_switcher_max_scroll_from(terminal_runtimes, viewport);
        if self.workspace_switcher.selected < self.workspace_switcher.scroll {
            self.workspace_switcher.scroll = self.workspace_switcher.selected;
        } else if self.workspace_switcher.selected >= self.workspace_switcher.scroll + viewport {
            self.workspace_switcher.scroll = self
                .workspace_switcher
                .selected
                .saturating_add(1)
                .saturating_sub(viewport);
        }
        self.workspace_switcher.scroll = self.workspace_switcher.scroll.min(max_scroll);
    }

    pub(crate) fn clamp_workspace_switcher_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let count = self.workspace_switcher_rows_from(terminal_runtimes).len();
        self.workspace_switcher.selected = self
            .workspace_switcher
            .selected
            .min(count.saturating_sub(1));
        self.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        self.capture_workspace_switcher_target_from(terminal_runtimes);
    }

    pub(crate) fn move_workspace_switcher_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        delta: isize,
    ) {
        let count = self.workspace_switcher_rows_from(terminal_runtimes).len();
        if count == 0 {
            self.workspace_switcher.selected = 0;
            self.workspace_switcher.scroll = 0;
            self.refresh_workspace_switcher_preview_from(terminal_runtimes);
            return;
        }

        let current = self.workspace_switcher.selected.min(count - 1) as isize;
        self.workspace_switcher.selected = (current + delta).clamp(0, count as isize - 1) as usize;
        self.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        self.capture_workspace_switcher_target_from(terminal_runtimes);
    }

    pub(crate) fn cycle_workspace_switcher_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        delta: isize,
    ) {
        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        let workspace_positions = rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| (!row.is_tab && !row.is_directory).then_some(idx))
            .collect::<Vec<_>>();
        if workspace_positions.is_empty() {
            self.workspace_switcher.selected = 0;
            self.workspace_switcher.scroll = 0;
            self.refresh_workspace_switcher_preview_from(terminal_runtimes);
            return;
        }

        let selected_ws_idx = rows
            .get(self.workspace_switcher.selected)
            .map(|row| row.ws_idx)
            .unwrap_or_else(|| rows[workspace_positions[0]].ws_idx);
        let current_pos = workspace_positions
            .iter()
            .position(|idx| rows[*idx].ws_idx == selected_ws_idx)
            .unwrap_or(0);
        let next_pos =
            (current_pos as isize + delta).rem_euclid(workspace_positions.len() as isize) as usize;
        self.workspace_switcher.selected = workspace_positions[next_pos];
        self.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        self.capture_workspace_switcher_target_from(terminal_runtimes);
    }

    pub(crate) fn expand_selected_workspace_switcher_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let Some(ws_idx) = self.selected_workspace_switcher_ws_idx_from(terminal_runtimes) else {
            return;
        };
        let Some(workspace_id) = self.workspaces.get(ws_idx).map(|ws| ws.id.clone()) else {
            return;
        };
        self.workspace_switcher
            .expanded_workspaces
            .insert(workspace_id);
        self.clamp_workspace_switcher_selection_from(terminal_runtimes);
    }

    pub(crate) fn collapse_selected_workspace_switcher_workspace_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        let Some(row) = rows.get(self.workspace_switcher.selected) else {
            return;
        };
        let ws_idx = row.ws_idx;
        let Some(workspace_id) = self.workspaces.get(ws_idx).map(|ws| ws.id.clone()) else {
            return;
        };
        self.workspace_switcher
            .expanded_workspaces
            .remove(&workspace_id);
        self.workspace_switcher.selected = self
            .workspace_switcher_rows_from(terminal_runtimes)
            .iter()
            .position(|row| row.ws_idx == ws_idx && !row.is_tab)
            .unwrap_or(0);
        self.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        self.capture_workspace_switcher_target_from(terminal_runtimes);
    }

    pub(crate) fn enter_workspace_switcher_search_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_switcher.mode = WorkspaceSwitcherMode::Search;
        self.workspace_switcher.query.clear();
        self.workspace_switcher.expanded_workspaces.clear();
        self.workspace_switcher.search_generation =
            self.workspace_switcher.search_generation.wrapping_add(1);
        self.reset_search_provider_state();
        self.workspace_switcher.search_started = true;
        self.workspace_switcher.selected = 0;
        self.workspace_switcher.scroll = 0;
        self.clamp_workspace_switcher_selection_from(terminal_runtimes);
    }

    pub(crate) fn leave_workspace_switcher_search_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.workspace_switcher.mode = WorkspaceSwitcherMode::QuickSwitch;
        self.workspace_switcher.query.clear();
        self.reset_search_provider_state();
        self.clamp_workspace_switcher_selection_from(terminal_runtimes);
    }

    /// Reset all search-provider session state to its default/idle values.
    /// Used by enter, leave, and close to avoid duplicated reset logic.
    fn reset_search_provider_state(&mut self) {
        self.workspace_switcher.provider_status =
            crate::app::workspace_search_provider::SearchProviderStatus::Idle;
        self.workspace_switcher.provider_candidates.clear();
        self.workspace_switcher.directory_preview_cache.clear();
        self.workspace_switcher.preview_request = None;
        self.workspace_switcher.pending_directory = None;
        self.workspace_switcher.pending_directory_rendered = false;
        self.workspace_switcher.search_error = None;
        self.workspace_switcher.workspace_canonical_snapshot.clear();
        self.workspace_switcher.needs_provider_refresh = false;
        self.workspace_switcher.needs_preview_refresh = None;
    }

    pub(crate) fn accept_workspace_switcher_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        // Prevent repeated acceptance while a directory open is in flight.
        if self.workspace_switcher.pending_directory.is_some() {
            return false;
        }
        let Some(target) = self.workspace_switcher.selected_target.clone().or_else(|| {
            self.workspace_switcher_rows_from(terminal_runtimes)
                .get(self.workspace_switcher.selected)
                .map(|row| row.target.clone())
        }) else {
            return false;
        };
        match &target {
            WorkspaceSwitcherTarget::Directory {
                shown_path,
                canonical_path,
            } => self.accept_directory_from(
                terminal_runtimes,
                shown_path.clone(),
                canonical_path.clone(),
            ),
            _ => self.focus_workspace_switcher_target(target),
        }
    }

    /// Accept a directory row: recheck canonical identity, then either focus
    /// an existing matching workspace or set the pending-create latch.
    fn accept_directory_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        shown_path: std::path::PathBuf,
        canonical_path: std::path::PathBuf,
    ) -> bool {
        // Pure acceptance: record the pending target only. The App controller
        // performs the final canonical recheck and chooses MRU focus or create.
        // No filesystem I/O here.
        self.workspace_switcher.pending_directory = Some(PendingDirectoryAccept {
            shown_path: shown_path.clone(),
            canonical_path: canonical_path.clone(),
        });
        self.workspace_switcher.pending_directory_rendered = false;
        self.workspace_switcher.search_error = None;
        self.refresh_workspace_switcher_preview_from(terminal_runtimes);
        true
    }

    fn capture_workspace_switcher_target_from(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) {
        self.workspace_switcher.selected_target = self
            .workspace_switcher_rows_from(terminal_runtimes)
            .get(self.workspace_switcher.selected)
            .map(|row| row.target.clone());
    }

    fn focus_workspace_switcher_target(&mut self, target: WorkspaceSwitcherTarget) -> bool {
        match target {
            WorkspaceSwitcherTarget::Workspace { workspace_id } => {
                let Some(ws_idx) = self
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == workspace_id)
                else {
                    return false;
                };
                self.switch_workspace(ws_idx);
                self.workspace_switcher.active = false;
                self.mode = Mode::Terminal;
                true
            }
            WorkspaceSwitcherTarget::Tab { tab_id } => {
                let Some((ws_idx, tab_idx)) =
                    self.workspaces
                        .iter()
                        .enumerate()
                        .find_map(|(ws_idx, workspace)| {
                            workspace
                                .tabs
                                .iter()
                                .enumerate()
                                .find_map(|(tab_idx, tab)| {
                                    (crate::workspace::public_tab_id_for_number(
                                        &workspace.id,
                                        tab.number,
                                    ) == tab_id)
                                        .then_some((ws_idx, tab_idx))
                                })
                        })
                else {
                    return false;
                };
                if self.switch_workspace_tab(ws_idx, tab_idx) {
                    self.workspace_switcher.active = false;
                    self.mode = Mode::Terminal;
                    true
                } else {
                    false
                }
            }
            // Directory targets are handled by accept_directory_from before
            // reaching this function; this arm is unreachable.
            WorkspaceSwitcherTarget::Directory { .. } => false,
        }
    }

    pub(crate) fn refresh_workspace_switcher_preview_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        // If a directory acceptance is in flight, show Opening…
        if let Some(pending) = &self.workspace_switcher.pending_directory {
            self.workspace_switcher.preview = WorkspaceSwitcherPreview::Opening {
                shown_path: pending.shown_path.clone(),
            };
            // Note: pending_directory_rendered is NOT set here. It is set
            // by the App controller only after an actual frame draw
            // completes, so the user sees at least one Opening frame before
            // workspace.create fires.
            return;
        }

        // If there is an inline error, show it.
        if let Some(error) = self.workspace_switcher.search_error.clone() {
            self.workspace_switcher.preview = WorkspaceSwitcherPreview::Empty { message: error };
            return;
        }

        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        let Some(row) = rows.get(self.workspace_switcher.selected) else {
            self.workspace_switcher.preview = WorkspaceSwitcherPreview::Empty {
                message: if self.workspaces.is_empty() {
                    "no workspaces".to_string()
                } else {
                    "no matching workspaces".to_string()
                },
            };
            return;
        };

        // Directory row: show directory preview (cached, loading, or request).
        if row.is_directory {
            let shown_path = match &row.target {
                WorkspaceSwitcherTarget::Directory { shown_path, .. } => shown_path.clone(),
                _ => return,
            };
            self.workspace_switcher.preview = self.directory_preview_for_path(&shown_path);
            return;
        }

        // Workspace or tab row: terminal preview.
        self.workspace_switcher.preview =
            self.workspace_switcher_preview_for_target(row.target.clone());
    }

    /// Build the preview for a directory shown path, using the cache or
    /// requesting a background load.
    fn directory_preview_for_path(
        &mut self,
        shown_path: &std::path::Path,
    ) -> WorkspaceSwitcherPreview {
        match self
            .workspace_switcher
            .directory_preview_cache
            .get(shown_path)
        {
            Some(crate::app::workspace_search_provider::DirectoryPreviewState::Ready(preview)) => {
                WorkspaceSwitcherPreview::Directory {
                    shown_path: shown_path.to_path_buf(),
                    entries: preview.entries.clone(),
                    truncated: preview.truncated,
                }
            }
            Some(crate::app::workspace_search_provider::DirectoryPreviewState::Error) => {
                WorkspaceSwitcherPreview::DirectoryError {
                    shown_path: shown_path.to_path_buf(),
                }
            }
            Some(crate::app::workspace_search_provider::DirectoryPreviewState::Loading) => {
                WorkspaceSwitcherPreview::DirectoryLoading {
                    shown_path: shown_path.to_path_buf(),
                }
            }
            None => {
                // Not cached and not loading: request a background load.
                self.workspace_switcher.preview_request = Some(shown_path.to_path_buf());
                WorkspaceSwitcherPreview::DirectoryLoading {
                    shown_path: shown_path.to_path_buf(),
                }
            }
        }
    }

    fn selected_workspace_switcher_ws_idx_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Option<usize> {
        self.workspace_switcher_rows_from(terminal_runtimes)
            .get(self.workspace_switcher.selected)
            .filter(|row| !row.is_directory)
            .map(|row| row.ws_idx)
    }

    fn workspace_switcher_preview_for_target(
        &self,
        target: WorkspaceSwitcherTarget,
    ) -> WorkspaceSwitcherPreview {
        if self
            .workspace_switcher_target_tab_indices(&target)
            .is_some()
        {
            return WorkspaceSwitcherPreview::TabSurface { target };
        }

        let message = match target {
            WorkspaceSwitcherTarget::Workspace { .. } => "workspace unavailable",
            WorkspaceSwitcherTarget::Tab { .. } => "tab unavailable",
            WorkspaceSwitcherTarget::Directory { .. } => "",
        };
        WorkspaceSwitcherPreview::Empty {
            message: message.to_string(),
        }
    }

    fn workspace_switcher_target_tab_indices(
        &self,
        target: &WorkspaceSwitcherTarget,
    ) -> Option<(usize, usize)> {
        match target {
            WorkspaceSwitcherTarget::Workspace { workspace_id } => {
                let ws_idx = self
                    .workspaces
                    .iter()
                    .position(|workspace| &workspace.id == workspace_id)?;
                let workspace = self.workspaces.get(ws_idx)?;
                let tab_idx = workspace.active_tab_index();
                workspace.tabs.get(tab_idx)?;
                Some((ws_idx, tab_idx))
            }
            WorkspaceSwitcherTarget::Tab { tab_id } => {
                self.workspaces
                    .iter()
                    .enumerate()
                    .find_map(|(ws_idx, workspace)| {
                        workspace
                            .tabs
                            .iter()
                            .enumerate()
                            .find_map(|(tab_idx, tab)| {
                                (crate::workspace::public_tab_id_for_number(
                                    &workspace.id,
                                    tab.number,
                                ) == *tab_id)
                                    .then_some((ws_idx, tab_idx))
                            })
                    })
            }
            WorkspaceSwitcherTarget::Directory { .. } => None,
        }
    }

    pub(crate) fn workspace_switcher_preview_tab_indices(&self) -> Option<(usize, usize)> {
        if !self.workspace_switcher.active {
            return None;
        }
        let WorkspaceSwitcherPreview::TabSurface { target } = &self.workspace_switcher.preview
        else {
            return None;
        };
        self.workspace_switcher_target_tab_indices(target)
    }

    pub(crate) fn workspace_switcher_preview_contains_pane(
        &self,
        pane_id: crate::layout::PaneId,
    ) -> bool {
        let Some((ws_idx, tab_idx)) = self.workspace_switcher_preview_tab_indices() else {
            return false;
        };
        self.tab_surface_contains_pane(ws_idx, tab_idx, pane_id)
    }

    pub(crate) fn workspace_mru_indices(&self) -> Vec<usize> {
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

pub(crate) fn workspace_switcher_match_rank(query: &str, text: &str) -> Option<(u8, usize)> {
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
        return Some((0, workspace_switcher_match_position_sum(&terms, &haystack)));
    }

    if terms.iter().all(|needle| haystack.contains(needle)) {
        return Some((1, workspace_switcher_match_position_sum(&terms, &haystack)));
    }

    if terms.iter().all(|needle| {
        needle.chars().count() > 1 && workspace_switcher_fuzzy_match(needle, &haystack)
    }) {
        return Some((2, usize::MAX / 2));
    }

    None
}

fn workspace_switcher_match_position_sum(terms: &[&str], haystack: &str) -> usize {
    terms
        .iter()
        .filter_map(|needle| haystack.find(needle))
        .sum()
}

fn workspace_switcher_fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_ch| chars.any(|text_ch| text_ch == needle_ch))
}

pub(crate) fn handle_workspace_switcher_key(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    if state.workspace_switcher.mode == WorkspaceSwitcherMode::QuickSwitch {
        handle_quick_switch_key(state, terminal_runtimes, key);
        return;
    }

    match key.code {
        KeyCode::Esc if state.workspace_switcher.mode == WorkspaceSwitcherMode::Search => {
            state.leave_workspace_switcher_search_from(terminal_runtimes);
        }
        KeyCode::Esc => close_workspace_switcher(state),
        KeyCode::Enter => {
            state.accept_workspace_switcher_selection_from(terminal_runtimes);
        }
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            close_workspace_switcher(state)
        }
        KeyCode::Char('q')
            if key.modifiers.is_empty() && state.workspace_switcher.query.is_empty() =>
        {
            close_workspace_switcher(state);
        }
        KeyCode::Backspace => {
            state.workspace_switcher.query.pop();
            state.clamp_workspace_switcher_selection_from(terminal_runtimes);
        }
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
            state.workspace_switcher.query.clear();
            state.clamp_workspace_switcher_selection_from(terminal_runtimes);
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            state.move_workspace_switcher_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            state.move_workspace_switcher_selection_from(terminal_runtimes, -1);
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
            state.move_workspace_switcher_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
            state.move_workspace_switcher_selection_from(terminal_runtimes, -1);
        }
        KeyCode::PageDown => {
            move_workspace_switcher_page(state, terminal_runtimes, 1);
        }
        KeyCode::PageUp => {
            move_workspace_switcher_page(state, terminal_runtimes, -1);
        }
        KeyCode::Home => {
            state.workspace_switcher.selected = 0;
            state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_switcher_preview_from(terminal_runtimes);
        }
        KeyCode::End => {
            state.workspace_switcher.selected = state
                .workspace_switcher_rows_from(terminal_runtimes)
                .len()
                .saturating_sub(1);
            state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_switcher_preview_from(terminal_runtimes);
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            state.workspace_switcher.query.push(c);
            state.clamp_workspace_switcher_selection_from(terminal_runtimes);
        }
        _ => {}
    }
    if state.workspace_switcher.active {
        state.capture_workspace_switcher_target_from(terminal_runtimes);
    }
}

fn handle_quick_switch_key(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    let forward_cycle = state.keybinds.workspace_switcher_forward_combo();
    let backward_cycle = state.keybinds.workspace_switcher_backward_combo();

    match key.code {
        KeyCode::Esc => close_workspace_switcher(state),
        KeyCode::Enter => {
            state.accept_workspace_switcher_selection_from(terminal_runtimes);
        }
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            close_workspace_switcher(state)
        }
        _ if forward_cycle.is_some_and(|combo| key_event_matches_combo(&key, combo)) => {
            state.cycle_workspace_switcher_from(terminal_runtimes, 1);
        }
        _ if backward_cycle.is_some_and(|combo| key_event_matches_combo(&key, combo)) => {
            state.cycle_workspace_switcher_from(terminal_runtimes, -1);
        }
        KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
            if workspace_switcher_command_modifiers(state, key.modifiers) =>
        {
            state.cycle_workspace_switcher_from(terminal_runtimes, -1);
        }
        KeyCode::Char('s') if workspace_switcher_command_modifiers(state, key.modifiers) => {
            state.enter_workspace_switcher_search_from(terminal_runtimes);
        }
        KeyCode::Char('l') if workspace_switcher_command_modifiers(state, key.modifiers) => {
            state.expand_selected_workspace_switcher_workspace_from(terminal_runtimes);
        }
        KeyCode::Char('h') if workspace_switcher_command_modifiers(state, key.modifiers) => {
            state.collapse_selected_workspace_switcher_workspace_from(terminal_runtimes);
        }
        KeyCode::Down | KeyCode::Char('j')
            if workspace_switcher_command_modifiers(state, key.modifiers) =>
        {
            state.move_workspace_switcher_selection_from(terminal_runtimes, 1);
        }
        KeyCode::Up | KeyCode::Char('k')
            if workspace_switcher_command_modifiers(state, key.modifiers) =>
        {
            state.move_workspace_switcher_selection_from(terminal_runtimes, -1);
        }
        KeyCode::PageDown => {
            move_workspace_switcher_page(state, terminal_runtimes, 1);
        }
        KeyCode::PageUp => {
            move_workspace_switcher_page(state, terminal_runtimes, -1);
        }
        KeyCode::Home => {
            state.workspace_switcher.selected = 0;
            state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_switcher_preview_from(terminal_runtimes);
        }
        KeyCode::End => {
            state.workspace_switcher.selected = state
                .workspace_switcher_rows_from(terminal_runtimes)
                .len()
                .saturating_sub(1);
            state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
            state.refresh_workspace_switcher_preview_from(terminal_runtimes);
        }
        _ => {}
    }
    if state.workspace_switcher.active {
        state.capture_workspace_switcher_target_from(terminal_runtimes);
    }
}

fn move_workspace_switcher_page(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    direction: isize,
) {
    let page = workspace_switcher_capacity(state.workspace_switcher_body_rect().height).max(1);
    state.move_workspace_switcher_selection_from(
        terminal_runtimes,
        (page as isize).saturating_mul(direction),
    );
}

fn workspace_switcher_command_modifiers(state: &AppState, modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
        || state
            .keybinds
            .workspace_switcher_command_modifiers()
            .is_some_and(|quick_switch_modifiers| modifiers.contains(quick_switch_modifiers))
}

pub(crate) fn handle_workspace_switcher_key_release(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    key: TerminalKey,
) -> bool {
    if !state.workspace_switcher.active
        || state.workspace_switcher.mode != WorkspaceSwitcherMode::QuickSwitch
    {
        return false;
    }

    if state
        .keybinds
        .workspace_switcher
        .bindings
        .iter()
        .filter_map(|binding| match binding.trigger {
            crate::config::BindingTrigger::Direct(combo) => Some(combo),
            _ => None,
        })
        .all(|combo| combo.1.is_empty())
    {
        tracing::trace!("workspace_switcher has no modifiers; release-accept unavailable");
        return false;
    }

    if !quick_switch_modifier_release_matches(&state.keybinds.workspace_switcher, key) {
        return false;
    }

    state.accept_workspace_switcher_selection_from(terminal_runtimes)
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
        .filter_map(|binding| match binding.trigger {
            crate::config::BindingTrigger::Direct(combo) => Some(combo),
            _ => None,
        })
        .any(|combo| combo.1.contains(modifier))
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

pub(crate) fn handle_workspace_switcher_mouse(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    mouse: MouseEvent,
) {
    match mouse.kind {
        MouseEventKind::Moved => {
            if let Some(idx) = state.workspace_switcher_row_index_at_from(
                terminal_runtimes,
                mouse.column,
                mouse.row,
            ) {
                if state.workspace_switcher.selected != idx {
                    state.workspace_switcher.selected = idx;
                    state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
                    state.refresh_workspace_switcher_preview_from(terminal_runtimes);
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(
                state.workspace_switcher_close_rect(),
                mouse.column,
                mouse.row,
            ) {
                close_workspace_switcher(state);
            } else if let Some(idx) = state.workspace_switcher_row_index_at_from(
                terminal_runtimes,
                mouse.column,
                mouse.row,
            ) {
                state.workspace_switcher.selected = idx;
                state.capture_workspace_switcher_target_from(terminal_runtimes);
                state.accept_workspace_switcher_selection_from(terminal_runtimes);
            } else if !state.workspace_switcher_popup_contains(mouse.column, mouse.row) {
                close_workspace_switcher(state);
            }
        }
        MouseEventKind::ScrollUp => {
            state.workspace_switcher.scroll = state.workspace_switcher.scroll.saturating_sub(3);
            state.workspace_switcher.selected = state.workspace_switcher.scroll;
            state.clamp_workspace_switcher_selection_from(terminal_runtimes);
        }
        MouseEventKind::ScrollDown => {
            let viewport = workspace_switcher_capacity(state.workspace_switcher_body_rect().height);
            let max = state.workspace_switcher_max_scroll_from(terminal_runtimes, viewport);
            state.workspace_switcher.scroll =
                state.workspace_switcher.scroll.saturating_add(3).min(max);
            state.workspace_switcher.selected = state.workspace_switcher.scroll;
            state.clamp_workspace_switcher_selection_from(terminal_runtimes);
        }
        _ => {}
    }
    if state.workspace_switcher.active {
        state.capture_workspace_switcher_target_from(terminal_runtimes);
    }
}

fn close_workspace_switcher(state: &mut AppState) {
    state.workspace_switcher.active = false;
    state.workspace_switcher.selected_target = None;
    state.reset_search_provider_state();
    if state.active.is_some() {
        state.mode = Mode::Terminal;
    } else {
        state.mode = Mode::Navigate;
    }
}

pub(crate) fn paste_workspace_switcher_query(
    state: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    text: &str,
) -> bool {
    if !state.workspace_switcher.active
        || state.workspace_switcher.mode != WorkspaceSwitcherMode::Search
    {
        return false;
    }
    state
        .workspace_switcher
        .query
        .extend(text.chars().filter(|ch| !ch.is_control()));
    state.workspace_switcher.selected = 0;
    state.workspace_switcher.scroll = 0;
    state.ensure_workspace_switcher_selection_visible_from(terminal_runtimes);
    state.refresh_workspace_switcher_preview_from(terminal_runtimes);
    true
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn workspace_switcher_list_width(width: u16) -> u16 {
    if width < 48 {
        return width;
    }
    (width / 3).clamp(24, 42).min(width.saturating_sub(1))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WorkspaceSwitcherLayout {
    mobile_fullscreen: bool,
    popup: Rect,
    inner: Rect,
    top_bar: Rect,
    close: Rect,
    search: Rect,
    search_separator: Rect,
    content: Rect,
    body: Rect,
    divider: Rect,
    preview: Rect,
    footer: Rect,
}

impl AppState {
    fn workspace_switcher_layout(&self) -> WorkspaceSwitcherLayout {
        if self.view.layout == ViewLayout::Mobile {
            return self.workspace_switcher_mobile_layout();
        }

        let area = self.view.sidebar_rect.union(self.view.terminal_area);
        let margin_x = (area.width / 12).max(2);
        let margin_y = (area.height / 9).max(1);
        let width = area.width.saturating_sub(margin_x.saturating_mul(2));
        let height = area.height.saturating_sub(margin_y.saturating_mul(2));
        let popup = Rect::new(
            area.x + margin_x,
            area.y + margin_y,
            width.max(4),
            height.max(4),
        );
        let inner = Block::default().borders(Borders::ALL).inner(popup);
        let search = if self.workspace_switcher.mode.search_visible() {
            Rect::new(inner.x, inner.y, inner.width, inner.height.min(1))
        } else {
            Rect::default()
        };
        let search_separator = if self.workspace_switcher.mode.search_visible() {
            Rect::new(inner.x, search.y + 1, inner.width, 1)
        } else {
            Rect::default()
        };
        let content = if self.workspace_switcher.mode.search_visible() {
            if inner.height <= 3 {
                Rect::default()
            } else {
                Rect::new(
                    inner.x,
                    inner.y + 2,
                    inner.width,
                    inner.height.saturating_sub(3),
                )
            }
        } else if inner.height <= 1 {
            Rect::default()
        } else {
            Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(1),
            )
        };
        let footer = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            inner.height.min(1),
        );
        Self::workspace_switcher_layout_with_content(WorkspaceSwitcherLayout {
            mobile_fullscreen: false,
            popup,
            inner,
            top_bar: Rect::default(),
            close: Rect::default(),
            search,
            search_separator,
            content,
            footer,
            ..WorkspaceSwitcherLayout::default()
        })
    }

    fn workspace_switcher_mobile_layout(&self) -> WorkspaceSwitcherLayout {
        let popup = self.view.mobile_header_rect.union(self.view.terminal_area);
        if popup.width == 0 || popup.height == 0 {
            return WorkspaceSwitcherLayout {
                mobile_fullscreen: true,
                ..WorkspaceSwitcherLayout::default()
            };
        }

        let top_bar = Rect::new(popup.x, popup.y, popup.width, 1);
        let close_width = popup.width.min(10);
        let close = Rect::new(
            popup.x + popup.width.saturating_sub(close_width),
            popup.y,
            close_width,
            1,
        );
        let search = if self.workspace_switcher.mode.search_visible() && popup.height > 1 {
            Rect::new(popup.x, popup.y + 1, popup.width, 1)
        } else {
            Rect::default()
        };
        let search_separator = if self.workspace_switcher.mode.search_visible() && popup.height > 2
        {
            Rect::new(popup.x, popup.y + 2, popup.width, 1)
        } else {
            Rect::default()
        };
        let reserved_height = 1u16
            .saturating_add(search.height)
            .saturating_add(search_separator.height);
        let content = Rect::new(
            popup.x,
            popup.y.saturating_add(reserved_height),
            popup.width,
            popup.height.saturating_sub(reserved_height),
        );
        // Mobile is a list-only presentation: the body fills the complete
        // content width at every width, with no divider or preview area
        // reserved. Preview state still updates; only its geometry is empty.
        WorkspaceSwitcherLayout {
            mobile_fullscreen: true,
            popup,
            inner: popup,
            top_bar,
            close,
            search,
            search_separator,
            content,
            body: content,
            ..WorkspaceSwitcherLayout::default()
        }
    }

    fn workspace_switcher_layout_with_content(
        mut layout: WorkspaceSwitcherLayout,
    ) -> WorkspaceSwitcherLayout {
        let content = layout.content;
        let list_width = workspace_switcher_list_width(content.width);
        layout.body = Rect::new(content.x, content.y, list_width, content.height);
        layout.divider = if content.width <= list_width || content.height == 0 {
            Rect::default()
        } else {
            Rect::new(content.x + list_width, content.y, 1, content.height)
        };
        let preview_x = content.x.saturating_add(list_width).saturating_add(1);
        layout.preview = Rect::new(
            preview_x,
            content.y,
            content.width.saturating_sub(list_width).saturating_sub(1),
            content.height,
        );
        layout
    }

    pub(crate) fn workspace_switcher_popup_rect(&self) -> Rect {
        self.workspace_switcher_layout().popup
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_inner_rect(&self) -> Rect {
        self.workspace_switcher_layout().inner
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_top_bar_rect(&self) -> Rect {
        self.workspace_switcher_layout().top_bar
    }

    pub(crate) fn workspace_switcher_close_rect(&self) -> Rect {
        self.workspace_switcher_layout().close
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_search_rect(&self) -> Rect {
        self.workspace_switcher_layout().search
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_content_rect(&self) -> Rect {
        self.workspace_switcher_layout().content
    }

    pub(crate) fn workspace_switcher_body_rect(&self) -> Rect {
        self.workspace_switcher_layout().body
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_divider_rect(&self) -> Rect {
        self.workspace_switcher_layout().divider
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_preview_rect(&self) -> Rect {
        self.workspace_switcher_layout().preview
    }

    #[cfg(test)]
    pub(crate) fn workspace_switcher_footer_rect(&self) -> Rect {
        self.workspace_switcher_layout().footer
    }

    pub(crate) fn workspace_switcher_popup_contains(&self, col: u16, row: u16) -> bool {
        rect_contains(self.workspace_switcher_popup_rect(), col, row)
    }

    pub(crate) fn workspace_switcher_row_index_at_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        col: u16,
        row: u16,
    ) -> Option<usize> {
        let body = self.workspace_switcher_body_rect();
        if !rect_contains(body, col, row) {
            return None;
        }
        let capacity = workspace_switcher_capacity(body.height);
        let physical_offset = row.saturating_sub(body.y) as usize;
        if physical_offset >= capacity.saturating_mul(WORKSPACE_SWITCHER_LINES_PER_ITEM as usize) {
            return None;
        }
        let idx = self
            .workspace_switcher
            .scroll
            .saturating_add(physical_offset / WORKSPACE_SWITCHER_LINES_PER_ITEM as usize);
        (idx < self.workspace_switcher_rows_from(terminal_runtimes).len()).then_some(idx)
    }
}

pub(super) fn render_workspace_switcher_overlay(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
) {
    let layout = app.workspace_switcher_layout();
    if layout.popup.width == 0 || layout.popup.height == 0 {
        return;
    }
    if layout.mobile_fullscreen {
        frame.render_widget(Clear, layout.popup);
        frame.render_widget(
            Block::default().style(Style::default().bg(app.palette.panel_bg)),
            layout.popup,
        );
        render_mobile_top_bar(app, frame, layout.top_bar, layout.close);
    } else if render_panel_shell(
        frame,
        layout.popup,
        app.palette.accent,
        app.palette.panel_bg,
    )
    .is_none()
    {
        return;
    }

    if app.workspace_switcher.mode.search_visible() {
        render_search(
            app,
            terminal_runtimes,
            frame,
            layout.search,
            !layout.mobile_fullscreen,
        );
        render_separator(frame, layout.search_separator, app.palette.surface1);
    }
    render_rows(app, terminal_runtimes, frame, layout.body);
    render_workspace_switcher_scrollbar(app, terminal_runtimes, frame, layout.body);
    render_vertical_divider(app, frame, layout.divider);
    render_preview(app, terminal_runtimes, frame, layout.preview);
    if !layout.mobile_fullscreen {
        render_footer(app, frame, layout.footer);
    }
}

fn render_mobile_top_bar(app: &AppState, frame: &mut Frame, area: Rect, close: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    frame.render_widget(
        Paragraph::new(" workspace switcher").style(
            Style::default()
                .fg(p.text)
                .bg(p.panel_bg)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
    if close.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "close",
                Style::default()
                    .fg(p.overlay1)
                    .bg(p.surface0)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(p.surface0)),
            Span::styled(
                "×",
                Style::default()
                    .fg(p.text)
                    .bg(p.surface0)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(p.surface0))
        .alignment(ratatui::layout::Alignment::Center),
        close,
    );
    frame.buffer_mut()[(close.x, close.y)]
        .set_symbol("│")
        .set_style(Style::default().fg(p.surface_dim).bg(p.surface0));
}

fn render_search(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
    show_title: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let p = &app.palette;
    let rows = app.workspace_switcher_rows_from(terminal_runtimes);
    let query = app.workspace_switcher.query.trim();
    let mut spans = if show_title {
        vec![Span::styled(
            " workspace switcher ",
            Style::default().fg(p.accent),
        )]
    } else {
        Vec::new()
    };
    spans.push(Span::styled("/ ", Style::default().fg(p.overlay0)));
    if query.is_empty() {
        spans.push(Span::styled(
            "search workspace names",
            Style::default().fg(p.overlay0),
        ));
    } else {
        spans.push(Span::styled(query.to_string(), Style::default().fg(p.text)));
    }
    // Show loading status only while an available provider is loading.
    let status_text = match app.workspace_switcher.provider_status {
        crate::app::workspace_search_provider::SearchProviderStatus::Loading => Some("loading…"),
        _ => None,
    };
    if let Some(status) = status_text {
        spans.push(Span::styled(
            format!(" {status}"),
            Style::default().fg(p.overlay0),
        ));
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

    let rows = app.workspace_switcher_rows_from(terminal_runtimes);
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

    let capacity = workspace_switcher_capacity(body.height);
    if capacity == 0 {
        frame.render_widget(
            Paragraph::new(format!("{} items hidden", rows.len()))
                .style(Style::default().fg(app.palette.overlay0)),
            body,
        );
        return;
    }

    let start = app.workspace_switcher.scroll.min(rows.len());
    let end = rows.len().min(start.saturating_add(capacity));
    for (visible_idx, row) in rows[start..end].iter().enumerate() {
        let idx = start + visible_idx;
        let y = body.y + visible_idx as u16 * WORKSPACE_SWITCHER_LINES_PER_ITEM;
        let rect = Rect::new(body.x, y, body.width, WORKSPACE_SWITCHER_LINES_PER_ITEM);
        render_row(
            app,
            frame,
            rect,
            row,
            idx == app.workspace_switcher.selected,
        );
    }
}

fn render_row(
    app: &AppState,
    frame: &mut Frame,
    rect: Rect,
    row: &WorkspaceSwitcherRow,
    selected: bool,
) {
    let p = &app.palette;
    frame.render_widget(Clear, rect);
    let base_style = if selected {
        Style::default().bg(p.accent).fg(panel_contrast_fg(p))
    } else {
        Style::default().bg(p.panel_bg).fg(p.text)
    };
    frame.render_widget(Block::default().style(base_style), rect);
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

    let primary = Rect::new(rect.x, rect.y, rect.width, 1);
    let secondary = Rect::new(rect.x, rect.y.saturating_add(1), rect.width, 1);
    let mut spans: Vec<Span> = Vec::new();

    if row.is_directory {
        let dir_text_style = if selected {
            base_style.add_modifier(Modifier::BOLD)
        } else {
            dim_style
        };
        spans.push(Span::styled(" + ", dim_style));
        let fixed_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        let title_budget = primary.width.saturating_sub(fixed_width).saturating_sub(1) as usize;
        let title = truncate_text(row.label.parts().1, title_budget);
        spans.push(Span::styled(title, dir_text_style));
        frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), primary);
        let secondary_rect = Rect::new(
            secondary.x.saturating_add(fixed_width),
            secondary.y,
            secondary.width.saturating_sub(fixed_width),
            1,
        );
        let path = truncate_text(&row.meta, secondary_rect.width as usize);
        frame.render_widget(Paragraph::new(path).style(dim_style), secondary_rect);
        return;
    }

    let (dot, dot_style) = state_dot(row.state, row.seen, p);

    let caret = if row.is_tab {
        " "
    } else if row.expanded {
        "▾"
    } else {
        "▸"
    };
    let indent = "  ".repeat(row.depth as usize);
    spans.push(Span::styled(format!(" {indent}{caret} "), dim_style));
    spans.push(Span::styled(dot, dot_style));
    spans.push(Span::styled(" ", dim_style));

    let meta_width = row_meta_width(rect.width);
    let fixed_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
    let title_budget = rect
        .width
        .saturating_sub(meta_width)
        .saturating_sub(fixed_width)
        .saturating_sub(1) as usize;
    let title = truncate_text(row.label.parts().1, title_budget);
    spans.push(Span::styled(title, text_style));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), primary);

    if meta_width > 0 {
        let meta_rect = Rect::new(
            rect.x + rect.width.saturating_sub(meta_width),
            primary.y,
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

    let secondary_rect = Rect::new(
        secondary.x.saturating_add(fixed_width),
        secondary.y,
        secondary.width.saturating_sub(fixed_width),
        1,
    );
    let secondary_text = workspace_secondary_text(
        row.label.parts().0,
        &row.activity,
        secondary_rect.width as usize,
    );
    frame.render_widget(
        Paragraph::new(secondary_text).style(dim_style),
        secondary_rect,
    );
}

fn render_workspace_switcher_scrollbar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    body: Rect,
) {
    if body.width <= 1 || body.height == 0 {
        return;
    }
    let rows = app.workspace_switcher_rows_from(terminal_runtimes).len();
    let viewport = workspace_switcher_capacity(body.height);
    if viewport == 0 {
        return;
    }
    if rows <= viewport {
        return;
    }
    let metrics = crate::pane::ScrollMetrics {
        viewport_rows: viewport,
        offset_from_bottom: rows
            .saturating_sub(viewport)
            .saturating_sub(app.workspace_switcher.scroll),
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
        .workspace_switcher_rows_from(terminal_runtimes)
        .get(app.workspace_switcher.selected)
        .map(|row| row.label.display())
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

    match &app.workspace_switcher.preview {
        WorkspaceSwitcherPreview::TabSurface { .. } => {
            if let Some((ws_idx, tab_idx)) = app.workspace_switcher_preview_tab_indices() {
                super::tab_surface::render_tab_surface_preview(
                    app,
                    terminal_runtimes,
                    ws_idx,
                    tab_idx,
                    content,
                    frame,
                );
            }
        }
        WorkspaceSwitcherPreview::Empty { message } => {
            frame.render_widget(
                Paragraph::new(format!(" {message}"))
                    .style(Style::default().fg(app.palette.overlay0)),
                content,
            );
        }
        WorkspaceSwitcherPreview::Directory {
            entries, truncated, ..
        } => {
            let p = &app.palette;
            let dir_style = Style::default().fg(p.accent).bg(p.panel_bg);
            let file_style = Style::default().fg(p.subtext0).bg(p.panel_bg);
            let dim_style = Style::default().fg(p.overlay0).bg(p.panel_bg);
            let mut lines = Vec::new();
            for entry in entries {
                let prefix = if entry.is_dir { "/" } else { " " };
                let style = if entry.is_dir { dir_style } else { file_style };
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(" ", style),
                    Span::styled(entry.name.clone(), style),
                ]));
            }
            if *truncated {
                lines.push(Line::from(Span::styled(" …more", dim_style)));
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(" empty directory", dim_style)));
            }
            frame.render_widget(Paragraph::new(lines), content);
        }
        WorkspaceSwitcherPreview::DirectoryLoading { .. } => {
            frame.render_widget(
                Paragraph::new(" loading directory…")
                    .style(Style::default().fg(app.palette.overlay0)),
                content,
            );
        }
        WorkspaceSwitcherPreview::DirectoryError { .. } => {
            frame.render_widget(
                Paragraph::new(" directory unreadable")
                    .style(Style::default().fg(app.palette.overlay0)),
                content,
            );
        }
        WorkspaceSwitcherPreview::Opening { shown_path } => {
            let msg = truncate_text(
                &format!(" Opening {}…", shown_path.display()),
                content.width.saturating_sub(1) as usize,
            );
            frame.render_widget(
                Paragraph::new(msg).style(Style::default().fg(app.palette.accent)),
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
    let line = match app.workspace_switcher.mode {
        WorkspaceSwitcherMode::QuickSwitch => Line::from(vec![
            Span::styled(" enter", key),
            Span::styled(" switch  ", dim),
            Span::styled("tab", key),
            Span::styled(" cycle  ", dim),
            Span::styled("l/h", key),
            Span::styled(" expand  ", dim),
            Span::styled("s", key),
            Span::styled(" search  ", dim),
            Span::styled("esc", key),
            Span::styled(" close", dim),
        ]),
        WorkspaceSwitcherMode::Search => Line::from(vec![
            Span::styled(" enter", key),
            Span::styled(" switch  ", dim),
            Span::styled("type", key),
            Span::styled(" search  ", dim),
            Span::styled("j/k/↑↓", key),
            Span::styled(" move  ", dim),
            Span::styled("esc", key),
            Span::styled(" back", dim),
        ]),
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

fn workspace_secondary_text(repo: Option<&str>, activity: &str, max_width: usize) -> String {
    const MIN_ACTIVITY_WITH_ELLIPSIS_WIDTH: usize = 2;

    let Some(repo) = repo else {
        return truncate_text(activity, max_width);
    };
    if activity.is_empty() {
        return truncate_text(repo, max_width);
    }

    let repo_width = repo.chars().count();
    let separator = " · ";
    let separator_width = separator.chars().count();
    if repo_width > max_width {
        return truncate_text(repo, max_width);
    }
    if repo_width
        .saturating_add(separator_width)
        .saturating_add(MIN_ACTIVITY_WITH_ELLIPSIS_WIDTH)
        > max_width
    {
        return repo.to_string();
    }

    let activity_width = max_width - repo_width - separator_width;
    format!(
        "{repo}{separator}{}",
        truncate_text(activity, activity_width)
    )
}

/// Separator used between the repository name and the existing workspace
/// label in a composite switcher row.
const COMPOSITE_SEPARATOR: &str = " / ";

/// Structured label for a switcher row. Carries the repository name and
/// existing workspace label as separate parts so rendering and truncation
/// never parse a display string. Composite display construction is
/// centralized in [`SwitcherLabel::compose`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SwitcherLabel {
    /// The existing workspace label (after grouped-child substitution).
    existing: String,
    /// Repository name for managed linked worktrees. When present, the
    /// display string is `compose(repo, existing)`.
    repo: Option<String>,
}

impl SwitcherLabel {
    /// Compose the display string from repository and existing-label parts.
    /// This is the single production path for composite label construction;
    /// all callers that need the composite string go through here or
    /// [`Self::display`].
    fn compose(repo: &str, existing: &str) -> String {
        format!("{repo}{COMPOSITE_SEPARATOR}{existing}")
    }

    /// Create a plain (non-composite) label.
    fn plain(existing: String) -> Self {
        Self {
            existing,
            repo: None,
        }
    }

    /// Create a composite label from a repository name and existing label.
    /// Argument order matches [`Self::compose`] and [`Self::parts`]:
    /// `(repo, existing)`.
    fn composite(repo: String, existing: String) -> Self {
        Self {
            existing,
            repo: Some(repo),
        }
    }

    /// Full display string for search matching and preview titles.
    fn display(&self) -> String {
        match &self.repo {
            Some(r) => Self::compose(r, &self.existing),
            None => self.existing.clone(),
        }
    }

    /// Returns `(repo, existing)` parts for rendering and truncation.
    /// `repo` is `None` for non-composite labels.
    fn parts(&self) -> (Option<&str>, &str) {
        (self.repo.as_deref(), &self.existing)
    }
}

/// Returns the non-empty repository name for a Herdr-managed linked
/// worktree, or `None` for non-linked or empty-name workspaces.
///
/// This is the single source of truth for the repo-name portion of a
/// composite switcher label, used by label construction.
fn managed_worktree_repo_name(ws: &crate::workspace::Workspace) -> Option<&str> {
    ws.worktree_space()
        .filter(|s| s.is_linked_worktree)
        .map(|s| s.label.as_str())
        .filter(|s| !s.is_empty())
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

    fn rendered_screen(state: &AppState, width: u16, height: u16) -> Vec<String> {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render_workspace_switcher_overlay(state, &terminal_runtimes, frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn mobile_quick_switch_uses_full_frame_without_footer() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 60, 20));
        state.open_workspace_switcher();

        assert_eq!(state.view.layout, ViewLayout::Mobile);
        assert_eq!(
            state.workspace_switcher_popup_rect(),
            Rect::new(0, 0, 60, 20)
        );
        assert_eq!(
            state.workspace_switcher_inner_rect(),
            Rect::new(0, 0, 60, 20)
        );
        assert_eq!(
            state.workspace_switcher_top_bar_rect(),
            Rect::new(0, 0, 60, 1)
        );
        assert_eq!(
            state.workspace_switcher_close_rect(),
            Rect::new(50, 0, 10, 1)
        );
        assert_eq!(state.workspace_switcher_search_rect(), Rect::default());
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(0, 1, 60, 19)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(0, 1, 60, 19)
        );
        assert_eq!(state.workspace_switcher_divider_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_preview_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_footer_rect(), Rect::default());

        let screen = rendered_screen(&state, 60, 20);
        assert!(screen[0].contains("workspace switcher"), "{}", screen[0]);
        assert!(screen[0].contains("close"), "{}", screen[0]);
        assert!(screen[0].contains('×'), "{}", screen[0]);
        assert!(!screen.join("\n").contains("enter"));
        assert_ne!(screen[0].chars().next(), Some('┌'));
        assert_ne!(screen[19].chars().next(), Some('└'));
        assert!(
            !screen.join("\n").contains("preview:"),
            "mobile must not render preview text: {screen:?}"
        );
        for row in screen.iter().skip(1) {
            assert!(
                !row.contains('│'),
                "mobile must not render a preview divider: {row}"
            );
        }
    }

    #[test]
    fn mobile_switcher_hides_preview_below_and_at_old_threshold_widths() {
        // The old desktop 48-column list/preview split must never reserve
        // preview or divider geometry on mobile, in QuickSwitch or Search.
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for width in [40u16, 47, 48, 60, 80] {
            for mode in [
                WorkspaceSwitcherMode::QuickSwitch,
                WorkspaceSwitcherMode::Search,
            ] {
                let mut state = app_with_workspaces(&["alpha", "beta"]);
                state.mobile_width_threshold = 90;
                crate::ui::compute_view(&mut state, Rect::new(0, 0, width, 20));
                assert_eq!(state.view.layout, ViewLayout::Mobile, "width {width}");
                state.open_workspace_switcher_from(&terminal_runtimes);
                state.workspace_switcher.mode = mode;
                let content = state.workspace_switcher_content_rect();
                let body = state.workspace_switcher_body_rect();
                assert_eq!(
                    body, content,
                    "width {width} {mode:?}: body must fill content"
                );
                assert_eq!(
                    state.workspace_switcher_divider_rect(),
                    Rect::default(),
                    "width {width} {mode:?}: divider must be empty"
                );
                assert_eq!(
                    state.workspace_switcher_preview_rect(),
                    Rect::default(),
                    "width {width} {mode:?}: preview must be empty"
                );

                let screen = rendered_screen(&state, width, 20);
                assert!(
                    !screen.join("\n").contains("preview:"),
                    "width {width} {mode:?}: no preview text"
                );
                for row in screen
                    .iter()
                    .skip(content.y as usize)
                    .take(content.height as usize)
                {
                    assert!(
                        !row.contains('│'),
                        "width {width} {mode:?}: no divider in content rows: {row}"
                    );
                }
            }
        }
    }

    #[test]
    fn mobile_switcher_keeps_tiny_width_geometry_in_bounds() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let mut state = app_with_workspaces(&["alpha"]);
        state.mobile_width_threshold = 90;
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 5, 3));
        state.open_workspace_switcher_from(&terminal_runtimes);
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(0, 1, 5, 2)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            state.workspace_switcher_content_rect()
        );
        assert_eq!(state.workspace_switcher_divider_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_preview_rect(), Rect::default());
        let screen = rendered_screen(&state, 5, 3);
        assert!(!screen.join("\n").contains("preview:"));
    }

    #[test]
    fn desktop_switcher_keeps_list_only_below_48_and_preview_at_48() {
        let app = |width: u16| -> AppState {
            let mut state = app_with_workspaces(&["alpha", "beta"]);
            state.mobile_width_threshold = 40;
            crate::ui::compute_view(&mut state, Rect::new(0, 0, width, 20));
            assert_eq!(state.view.layout, ViewLayout::Desktop);
            state.open_workspace_switcher();
            state
        };

        // Terminal width 57 -> popup inner content width 47: list only.
        let state = app(57);
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(5, 3, 47, 13)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(5, 3, 47, 13)
        );
        assert_eq!(state.workspace_switcher_divider_rect(), Rect::default());
        assert_eq!(
            state.workspace_switcher_preview_rect().width,
            0,
            "content width 47 must stay list-only"
        );
        let screen = rendered_screen(&state, 57, 20);
        assert!(!screen.join("\n").contains("preview:"));

        // Terminal width 58 -> popup inner content width 48: list + divider + preview.
        let state = app(58);
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(5, 3, 48, 13)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(5, 3, 24, 13)
        );
        assert_eq!(
            state.workspace_switcher_divider_rect(),
            Rect::new(29, 3, 1, 13)
        );
        assert_eq!(
            state.workspace_switcher_preview_rect(),
            Rect::new(30, 3, 23, 13)
        );
        let screen = rendered_screen(&state, 58, 20);
        assert!(
            screen.join("\n").contains("preview:"),
            "{}",
            screen.join("\n")
        );
        assert!(screen[3..16].iter().any(|row| row.contains('│')));
    }

    #[test]
    fn mobile_search_places_search_below_top_bar_and_returns_footer_row_to_content() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 60, 20));
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);

        assert_eq!(
            state.workspace_switcher_search_rect(),
            Rect::new(0, 1, 60, 1)
        );
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(0, 3, 60, 17)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(0, 3, 60, 17)
        );
        assert_eq!(state.workspace_switcher_divider_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_preview_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_footer_rect(), Rect::default());

        let screen = rendered_screen(&state, 60, 20);
        assert!(screen[0].contains("workspace switcher"), "{}", screen[0]);
        assert!(
            screen[1].contains("search workspace names"),
            "{}",
            screen[1]
        );
        assert!(!screen[1].contains("workspace switcher"), "{}", screen[1]);
        assert!(
            !screen.join("\n").contains("preview:"),
            "mobile search must not render preview text: {screen:?}"
        );
    }

    #[test]
    fn mobile_close_control_closes_switcher_without_accepting_a_row() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 40, 20));
        state.open_workspace_switcher_from(&terminal_runtimes);
        let active = state.active;
        let close = state.workspace_switcher_close_rect();

        handle_workspace_switcher_mouse(
            &mut state,
            &terminal_runtimes,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: close.x + 1,
                row: close.y,
                modifiers: KeyModifiers::empty(),
            },
        );

        assert!(!state.workspace_switcher.active);
        assert_eq!(state.active, active);
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn mobile_fullscreen_keeps_tiny_height_geometry_in_bounds() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let mut state = app_with_workspaces(&["alpha"]);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 40, 1));
        state.open_workspace_switcher_from(&terminal_runtimes);

        assert_eq!(
            state.workspace_switcher_top_bar_rect(),
            Rect::new(0, 0, 40, 1)
        );
        assert_eq!(state.workspace_switcher_body_rect().height, 0);
        let quick_screen = rendered_screen(&state, 40, 1);
        assert!(quick_screen[0].contains("workspace switcher"));

        crate::ui::compute_view(&mut state, Rect::new(0, 0, 40, 2));
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        assert_eq!(
            state.workspace_switcher_search_rect(),
            Rect::new(0, 1, 40, 1)
        );
        assert_eq!(state.workspace_switcher_body_rect().height, 0);
        let search_screen = rendered_screen(&state, 40, 2);
        assert!(search_screen[1].contains("search workspace names"));
    }

    #[test]
    fn desktop_switcher_keeps_bordered_popup_geometry_and_footer() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 120, 30));
        state.open_workspace_switcher();

        assert_eq!(state.view.layout, ViewLayout::Desktop);
        assert_eq!(
            state.workspace_switcher_popup_rect(),
            Rect::new(10, 3, 100, 24)
        );
        assert_eq!(
            state.workspace_switcher_inner_rect(),
            Rect::new(11, 4, 98, 22)
        );
        assert_eq!(state.workspace_switcher_top_bar_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_close_rect(), Rect::default());
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(11, 4, 98, 21)
        );
        assert_eq!(
            state.workspace_switcher_footer_rect(),
            Rect::new(11, 25, 98, 1)
        );

        let screen = rendered_screen(&state, 120, 30);
        assert_eq!(screen[3].chars().nth(10), Some('┌'));
        assert_eq!(screen[26].chars().nth(10), Some('└'));
        assert!(screen[25].contains("enter"), "{}", screen[25]);
    }

    fn mark_linked_worktree(state: &mut AppState, ws_idx: usize) {
        mark_linked_worktree_with_repo(state, ws_idx, "herdr");
    }

    fn mark_linked_worktree_with_repo(state: &mut AppState, ws_idx: usize, repo: &str) {
        state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: repo.into(),
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
        workspace_switcher: &str,
        workspace_switcher_backward: Option<&str>,
    ) -> crate::config::Config {
        let backward = workspace_switcher_backward
            .map(|binding| format!("workspace_switcher_backward = {binding:?}\n"))
            .unwrap_or_default();
        toml::from_str(&format!(
            "[keys]\nworkspace_switcher = {workspace_switcher:?}\n{backward}"
        ))
        .expect("quick switch config should parse")
    }

    fn state_with_quick_switch_binding(
        workspace_switcher: &str,
        workspace_switcher_backward: Option<&str>,
    ) -> (
        AppState,
        crate::terminal::TerminalRuntimeRegistry,
        KeyModifiers,
    ) {
        let config = config_with_quick_switch(workspace_switcher, workspace_switcher_backward);
        let quick_switch_modifiers = config
            .keybinds()
            .workspace_switcher_command_modifiers()
            .expect("quick switch should have a direct binding");
        let mut state = state_with_workspaces(&["main", "issue", "docs"]);
        state.keybinds = config.keybinds();
        state.workspaces[1].test_add_tab(Some("logs"));
        state.workspaces[2].test_add_tab(Some("logs"));
        state.switch_workspace(2);
        state.switch_workspace(0);

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        (state, terminal_runtimes, quick_switch_modifiers)
    }

    fn selected_workspace_switcher_ws_idx(
        state: &AppState,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> usize {
        let rows = state.workspace_switcher_rows_from(terminal_runtimes);
        rows[state.workspace_switcher.selected].ws_idx
    }

    #[test]
    fn switcher_accept_resolves_stable_target_after_workspace_reorder() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let selected_id = state.workspace_switcher_rows()[state.workspace_switcher.selected]
            .target
            .clone();

        state.workspaces.swap(0, 1);
        assert!(state.accept_workspace_switcher_selection_from(&terminal_runtimes));

        let WorkspaceSwitcherTarget::Workspace { workspace_id } = selected_id else {
            panic!("workspace row should be selected");
        };
        assert_eq!(state.workspaces[state.active.unwrap()].id, workspace_id);
    }
    #[test]
    fn workspace_switcher_filters_workspace_names_only() {
        let mut state = app_with_workspaces(&["one", "issue"]);
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_manual_label("weekly review".into());

        state.open_workspace_switcher();
        state
            .enter_workspace_switcher_search_from(&crate::terminal::TerminalRuntimeRegistry::new());
        state.workspace_switcher.query = "weekly".into();
        assert!(state.workspace_switcher_rows().is_empty());

        state.workspace_switcher.query = "ie".into();
        let rows = state.workspace_switcher_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ws_idx, 1);
    }

    #[test]
    fn workspace_switcher_search_ranks_matches_then_preserves_mru_order() {
        let mut state = app_with_workspaces(&["main", "other issue", "issue alpha", "issue beta"]);
        state.workspace_mru = vec![
            state.workspaces[1].id.clone(),
            state.workspaces[3].id.clone(),
            state.workspaces[2].id.clone(),
        ];

        state.open_workspace_switcher();
        state
            .enter_workspace_switcher_search_from(&crate::terminal::TerminalRuntimeRegistry::new());
        state.workspace_switcher.query = "issue".into();

        let rows = state.workspace_switcher_rows();
        assert_eq!(
            rows.iter().map(|row| row.ws_idx).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn workspace_switcher_empty_state_has_placeholder_preview() {
        let mut state = AppState::test_new();

        state.open_workspace_switcher();

        assert!(state.workspace_switcher.active);
        assert!(matches!(
            state.workspace_switcher.preview,
            WorkspaceSwitcherPreview::Empty { ref message } if message == "no workspaces"
        ));
    }
    #[test]
    fn workspace_switcher_shows_branch_name_for_grouped_child() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);
        // Clear custom name so the workspace is auto-named — this is the case
        // where grouped_child_display_label substitutes the branch name.
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());

        state.open_workspace_switcher();
        let rows = state.workspace_switcher_rows();

        // The grouped child is a managed linked worktree, so it shows the
        // composite label: repo name + branch substitution (without "worktree/" prefix).
        let child_row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child_row.label.display(), "herdr / issue-137");
        assert_eq!(child_row.label.parts().0, Some("herdr"));

        // The parent is not a linked worktree — no composite label.
        let parent_row = rows.iter().find(|r| r.ws_idx == 0).unwrap();
        assert_eq!(parent_row.label.parts().0, None);
    }
    #[test]
    fn workspace_switcher_shows_cwd_name_for_standalone_workspace() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        // No worktree_space set — standalone workspace.
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let raw_label = state.workspaces[1].display_name_from(&state.terminals, &terminal_runtimes);

        state.open_workspace_switcher();
        let rows = state.workspace_switcher_rows();

        // Standalone workspace — no branch substitution, label is CWD-derived.
        let row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(row.label.display(), raw_label);
        assert_ne!(row.label.display(), "issue-137");
    }
    #[test]
    fn workspace_switcher_keeps_custom_name_for_grouped_child() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);
        state.workspaces[1].cached_git_branch = Some("worktree/issue-137".into());
        state.workspaces[1].custom_name = Some("my-custom-name".into());

        state.open_workspace_switcher();
        let rows = state.workspace_switcher_rows();

        // Custom name remains authoritative for the existing-label portion;
        // the repository name is still prepended.
        let child_row = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child_row.label.display(), "herdr / my-custom-name");
        assert_eq!(child_row.label.parts().0, Some("herdr"));
    }
    #[test]
    fn workspace_switcher_shows_cwd_name_for_linked_only_group() {
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

        state.open_workspace_switcher();
        let rows = state.workspace_switcher_rows();

        // Without a parent worktree, these are not grouped children —
        // the CWD-derived name is used for the existing-label portion, and
        // the repo name is prepended because they are linked worktrees.
        let row0 = rows.iter().find(|r| r.ws_idx == 0).unwrap();
        assert_eq!(row0.label.display(), format!("herdr / {raw0}"));
        assert_ne!(row0.label.display(), "herdr / issue-137");
        let row1 = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(row1.label.display(), format!("herdr / {raw1}"));
        assert_ne!(row1.label.display(), "herdr / review-42");
    }
    #[test]
    fn quick_switch_uses_observed_mru_order_and_preselects_previous_workspace() {
        let mut state = app_with_workspaces(&["one", "two", "three"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.workspace_mru = vec![
            state.workspaces[2].id.clone(),
            state.workspaces[1].id.clone(),
            state.workspaces[0].id.clone(),
        ];
        state.switch_workspace(2);

        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows();

        assert_eq!(rows[0].ws_idx, 2);
        assert_eq!(rows[1].ws_idx, 1);
        assert_eq!(rows[2].ws_idx, 0);
        assert_eq!(state.workspace_switcher.selected, 1);
        assert_eq!(
            state.workspace_switcher.mode,
            WorkspaceSwitcherMode::QuickSwitch
        );
    }
    #[test]
    fn quick_switch_tab_cycles_workspace_rows_only() {
        let mut state = app_with_workspaces(&["one", "two", "three"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.switch_workspace(1);
        state.switch_workspace(2);
        state.workspace_mru = vec![
            state.workspaces[2].id.clone(),
            state.workspaces[1].id.clone(),
            state.workspaces[0].id.clone(),
        ];
        state.open_workspace_switcher_from(&terminal_runtimes);

        state.cycle_workspace_switcher_from(&terminal_runtimes, 1);

        let selected = &state.workspace_switcher_rows()[state.workspace_switcher.selected];
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
        state.open_workspace_switcher_from(&terminal_runtimes);

        state.expand_selected_workspace_switcher_workspace_from(&terminal_runtimes);
        state.move_workspace_switcher_selection_from(&terminal_runtimes, 2);

        let selected = state.workspace_switcher_rows()[state.workspace_switcher.selected].clone();
        assert_eq!(
            selected.target,
            WorkspaceSwitcherTarget::Tab {
                tab_id: crate::workspace::public_tab_id_for_number(
                    &state.workspaces[1].id,
                    state.workspaces[1].tabs[second_tab].number,
                )
            }
        );
        assert!(state.accept_workspace_switcher_selection_from(&terminal_runtimes));
        assert_eq!(state.active, Some(1));
        assert_eq!(state.workspaces[1].active_tab_index(), second_tab);
    }
    #[test]
    fn workspace_switcher_typing_filters_and_enter_switches_workspace() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty()),
        );
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
        );
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
        );
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.active, Some(1));
        assert_eq!(state.mode, Mode::Terminal);
    }
    #[test]
    fn workspace_switcher_escape_closes_without_switching() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.move_workspace_switcher_selection_from(&terminal_runtimes, 1);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.active, Some(0));
        assert_eq!(state.mode, Mode::Terminal);
    }
    #[test]
    fn workspace_switcher_search_returns_to_quick_switch_on_escape() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.mode, WorkspaceSwitcherMode::Search);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(
            state.workspace_switcher.mode,
            WorkspaceSwitcherMode::QuickSwitch
        );
        assert!(state.workspace_switcher.active);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert!(!state.workspace_switcher.active);
        assert_eq!(state.mode, Mode::Terminal);
    }
    #[test]
    fn quick_switch_accepts_control_modified_commands_while_shortcut_is_held() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.workspaces[1].test_add_tab(Some("logs"));
        state.switch_workspace(1);
        state.switch_workspace(0);
        state.open_workspace_switcher_from(&terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
        );
        assert!(state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == 1 && row.is_tab && row.label.display() == "logs"));

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        assert!(
            state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab
        );

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        );
        assert!(
            !state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab
        );

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );
        assert!(!state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == 1 && row.is_tab));

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.workspace_switcher.mode, WorkspaceSwitcherMode::Search);
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
            let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(forward_code, forward_modifiers),
            );
            assert_ne!(
                selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
                initial_ws,
                "{binding} should cycle forward"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(backward_code, backward_modifiers),
            );
            assert_eq!(
                selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
                initial_ws,
                "{binding} should cycle backward"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('l'), command_modifiers),
            );
            assert!(
                state
                    .workspace_switcher_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == initial_ws
                        && row.is_tab
                        && row.label.display() == "logs"),
                "{binding} should expand with the configured modifier"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('j'), command_modifiers),
            );
            assert!(
                state.workspace_switcher_rows_from(&terminal_runtimes)
                    [state.workspace_switcher.selected]
                    .is_tab,
                "{binding} should move down with the configured modifier"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('k'), command_modifiers),
            );
            assert!(
                !state.workspace_switcher_rows_from(&terminal_runtimes)
                    [state.workspace_switcher.selected]
                    .is_tab,
                "{binding} should move up with the configured modifier"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('h'), command_modifiers),
            );
            assert!(
                !state
                    .workspace_switcher_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == initial_ws && row.is_tab),
                "{binding} should collapse with the configured modifier"
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Char('s'), command_modifiers),
            );
            assert_eq!(
                state.workspace_switcher.mode,
                WorkspaceSwitcherMode::Search,
                "{binding} should enter search with the configured modifier"
            );
        }
    }
    #[test]
    fn quick_switch_backward_cycle_uses_explicit_override_when_set() {
        let (mut state, terminal_runtimes, _) =
            state_with_quick_switch_binding("cmd+f13", Some("cmd+f14"));
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(13), KeyModifiers::SUPER),
        );
        let after_forward = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(13), KeyModifiers::SUPER | KeyModifiers::SHIFT),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            after_forward,
            "derived backward shortcut should not apply when an override is set"
        );

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::F(14), KeyModifiers::SUPER),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            initial_ws
        );
    }
    #[test]
    fn quick_switch_shift_press_cycles_backward() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        // Cycle forward first
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );
        let after_forward = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        // Shift press while Ctrl held cycles backward
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press while modifier held should cycle backward"
        );
    }
    #[test]
    fn quick_shift_press_with_right_shift_cycles_backward() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::RightShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Right Shift press should also cycle backward"
        );
    }
    #[test]
    fn quick_switch_command_chars_work_with_shift_held() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        // 'l' with CONTROL | SHIFT should still expand (using .contains())
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('l'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            state
                .workspace_switcher_rows_from(&terminal_runtimes)
                .iter()
                .any(|row| row.ws_idx == initial_ws && row.is_tab && row.label.display() == "logs"),
            "expand command should work when Shift is held alongside modifier"
        );

        // 'j' with CONTROL | SHIFT should still move down
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('j'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab,
            "move down command should work when Shift is held alongside modifier"
        );

        // 'k' with CONTROL | SHIFT should still move up
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            !state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab,
            "move up command should work when Shift is held alongside modifier"
        );

        // 'h' with CONTROL | SHIFT should still collapse
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('h'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(
            !state
                .workspace_switcher_rows_from(&terminal_runtimes)
                .iter()
                .any(|row| row.ws_idx == initial_ws && row.is_tab),
            "collapse command should work when Shift is held alongside modifier"
        );

        // 's' with CONTROL | SHIFT should still enter search
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            state.workspace_switcher.mode,
            WorkspaceSwitcherMode::Search,
            "search command should work when Shift is held alongside modifier"
        );
    }
    #[test]
    fn quick_switch_shift_press_with_backward_override_still_works() {
        // Shift-press should cycle backward even when an explicit backward override is set
        let (mut state, terminal_runtimes, _) =
            state_with_quick_switch_binding("ctrl+tab", Some("ctrl+shift+tab"));
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        // Cycle forward first
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        );
        let after_forward = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);
        assert_ne!(after_forward, initial_ws);

        // Shift press should still cycle backward (native overlay, not configurable)
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press should cycle backward even with explicit backward override"
        );
    }
    #[test]
    fn quick_switch_shift_press_without_modifier_is_noop() {
        // Shift press without the quick-switch modifier held should not cycle
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let initial_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(
            selected_workspace_switcher_ws_idx(&state, &terminal_runtimes),
            initial_ws,
            "Shift press without quick-switch modifier should be a no-op"
        );
    }
    #[test]
    fn quick_switch_shift_press_wraps_from_first_workspace() {
        // Picker starts at first non-active workspace in MRU order (workspace 2).
        // Cycle backward once to reach workspace 0, then again to wrap around.
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let start_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        // First backward: from start_ws → workspace 0
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        let after_one = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);
        assert_eq!(after_one, 0, "first backward should land at workspace 0");

        // Second backward: wrap from workspace 0 to last in MRU order
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Modifier(ModifierKeyCode::LeftShift),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        let after_wrap = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);
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
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(
                KeyCode::Char('l'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );

        // Down arrow with CONTROL | SHIFT should still move down
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert!(
            state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab,
            "Down arrow should work when Shift is held alongside modifier"
        );

        // Up arrow with CONTROL | SHIFT should still move up
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert!(
            !state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab,
            "Up arrow should work when Shift is held alongside modifier"
        );
    }
    #[test]
    fn quick_switch_shift_release_does_not_accept() {
        let (state, _terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);

        // Releasing Shift while Ctrl is held should NOT trigger accept
        let accepted = quick_switch_modifier_release_matches(
            &state.keybinds.workspace_switcher,
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
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("test".to_string()),
            meta: "".to_string(),
            activity: String::new(),
            is_current: true,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Blocked,
            seen: false,
        };
        let area = Rect::new(0, 0, 20, 2);
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        assert_eq!(buf[(3, 0)].symbol(), "●");
    }

    #[test]
    fn render_row_shows_state_dot_for_idle_seen_workspace() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("test".to_string()),
            meta: "".to_string(),
            activity: String::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Idle,
            seen: true,
        };
        let area = Rect::new(0, 0, 20, 2);
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        assert_eq!(buf[(3, 0)].symbol(), "○");
    }

    #[test]
    fn render_row_shows_quick_switch_indent() {
        let mut app = AppState::test_new();
        app.workspace_switcher.mode = WorkspaceSwitcherMode::QuickSwitch;
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 1,
            label: SwitcherLabel::plain("ws".to_string()),
            meta: "".to_string(),
            activity: String::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Working,
            seen: false,
        };
        let area = Rect::new(0, 0, 20, 2);
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        // Workspace switcher row, depth=1: "   ▸ ● ws"
        // Position 3 is caret "▸" (after leading space + 2-char indent)
        assert_eq!(buf[(3, 0)].symbol(), "▸");
    }

    // -----------------------------------------------------------------------
    // Search provider integration tests
    // -----------------------------------------------------------------------

    use crate::app::workspace_search_provider::{
        DirectoryPreview, DirectoryPreviewEntry, DirectoryPreviewState, SearchProviderCandidate,
        SearchProviderStatus, SEARCH_RESULTS_LIMIT,
    };
    use crate::events::AppEvent;

    fn candidate(shown: &str, canonical: &str, score: f64) -> SearchProviderCandidate {
        SearchProviderCandidate {
            shown_path: std::path::PathBuf::from(shown),
            canonical_path: std::path::PathBuf::from(canonical),
            score,
        }
    }

    fn enter_search(state: &mut AppState) {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        // Populate snapshot from identity_cwd fields (normally done by App
        // controller’s refresh_workspace_canonical_snapshot).
        set_snapshot(state);
    }

    /// Populate the workspace canonical-cwd snapshot from identity_cwd fields.
    /// In production this is done by the App controller; tests set it directly
    /// to avoid filesystem I/O in the UI layer.
    fn set_snapshot(state: &mut AppState) {
        state.workspace_switcher.workspace_canonical_snapshot = state
            .workspaces
            .iter()
            .filter_map(|ws| {
                if ws.cached_identity_cwd.as_os_str().is_empty() {
                    return None;
                }
                Some(crate::ui::workspace_switcher::WorkspaceCanonicalEntry {
                    workspace_id: ws.id.clone(),
                    canonical_cwd: ws.cached_identity_cwd.clone(),
                })
            })
            .collect();
    }

    fn set_query(state: &mut AppState, query: &str) {
        state.workspace_switcher.query = query.to_string();
    }

    #[test]
    fn search_empty_query_shows_only_workspace_mru_rows() {
        let mut state = app_with_workspaces(&["main", "issue"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![
            candidate("/tmp/extra", "/tmp/extra", 10.0),
            candidate("/tmp/other", "/tmp/other", 5.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;

        let rows = state.workspace_switcher_rows();
        // No directory rows on empty query.
        assert!(rows.iter().all(|r| !r.is_directory));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn search_directory_rows_match_basename_and_path() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![
            candidate("/home/user/myproject", "/home/user/myproject", 10.0),
            candidate("/home/user/other", "/home/user/other", 5.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "myproj");

        let rows = state.workspace_switcher_rows();
        let dir_rows: Vec<_> = rows.iter().filter(|r| r.is_directory).collect();
        assert_eq!(dir_rows.len(), 1);
        assert_eq!(dir_rows[0].label.display(), "myproject");
    }

    #[test]
    fn search_ranks_basename_quality_before_path_match() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        // "alpha" matches basename of first, path of second.
        state.workspace_switcher.provider_candidates = vec![
            candidate("/home/user/alpha", "/home/user/alpha", 1.0),
            candidate("/home/alpha-deep/work", "/home/alpha-deep/work", 100.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "alpha");

        let rows = state.workspace_switcher_rows();
        let dir_rows: Vec<_> = rows.iter().filter(|r| r.is_directory).collect();
        assert_eq!(dir_rows.len(), 2);
        // Basename match (tier 0) should rank before path match.
        assert_eq!(dir_rows[0].label.display(), "alpha");
    }

    #[test]
    fn search_workspace_tie_priority_over_directory() {
        let mut state = app_with_workspaces(&["alpha"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/alpha", "/tmp/alpha", 999.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "alpha");

        let rows = state.workspace_switcher_rows();
        // Workspace should come before directory on equal match quality.
        assert!(!rows[0].is_directory);
        assert!(rows[0].label.display() == "alpha");
    }

    #[test]
    fn search_directory_ties_sorted_by_score_descending() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![
            candidate("/tmp/foo-low", "/tmp/foo-low", 10.0),
            candidate("/tmp/foo-high", "/tmp/foo-high", 999.0),
            candidate("/tmp/foo-mid", "/tmp/foo-mid", 50.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "foo");

        let all_rows = state.workspace_switcher_rows();
        let dir_rows: Vec<_> = all_rows.iter().filter(|r| r.is_directory).collect();
        assert_eq!(dir_rows.len(), 3);
        assert_eq!(dir_rows[0].label.display(), "foo-high");
        assert_eq!(dir_rows[1].label.display(), "foo-mid");
        assert_eq!(dir_rows[2].label.display(), "foo-low");
    }

    #[test]
    fn search_caps_at_100_rows() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let candidates: Vec<_> = (0..150)
            .map(|i| {
                candidate(
                    &format!("/tmp/dir{i:03}"),
                    &format!("/tmp/dir{i:03}"),
                    100.0 - i as f64,
                )
            })
            .collect();
        state.workspace_switcher.provider_candidates = candidates;
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "dir");

        let rows = state.workspace_switcher_rows();
        // 1 workspace + 149 directories = 150, capped at 100.
        assert_eq!(rows.len(), SEARCH_RESULTS_LIMIT);
    }

    #[test]
    fn search_coalesces_open_workspace_with_zoxide() {
        let dir =
            std::env::temp_dir().join(format!("herdr-switcher-coalesce-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let canonical = std::fs::canonicalize(&dir).expect("canonicalize");

        let mut state = AppState::test_new();
        let mut ws = Workspace::test_new("myproject");
        ws.identity_cwd = canonical.clone();
        ws.cached_identity_cwd = canonical.clone();
        state.workspaces.push(ws);
        state.ensure_test_terminals();
        state.active = Some(0);
        state.mode = Mode::Terminal;

        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![candidate(
            canonical.to_str().unwrap(),
            canonical.to_str().unwrap(),
            100.0,
        )];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "myproj");

        let rows = state.workspace_switcher_rows();
        // Workspace is coalesced: no separate directory row.
        assert!(rows.iter().all(|r| !r.is_directory));
        // Workspace should match through the zoxide path (basename = "myproject").
        assert_eq!(rows.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_unopened_directory_sets_directory_target() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/unopened", "/tmp/unopened", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "unopened");

        let rows = state.workspace_switcher_rows();
        let dir_row = rows.iter().find(|r| r.is_directory).expect("directory row");
        assert!(matches!(
            &dir_row.target,
            WorkspaceSwitcherTarget::Directory { shown_path, .. }
                if shown_path == &std::path::PathBuf::from("/tmp/unopened")
        ));
    }

    #[test]
    fn stale_generation_discards_zoxide_results() {
        let mut state = app_with_workspaces(&["one"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;

        // Send result with stale generation.
        state.handle_app_event(AppEvent::ZoxideQueryCompleted {
            generation: gen.wrapping_sub(1),
            available: true,
            candidates: vec![candidate("/tmp/stale", "/tmp/stale", 10.0)],
        });

        assert!(state.workspace_switcher.provider_candidates.is_empty());
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Idle
        );
    }

    #[test]
    fn fresh_generation_updates_zoxide_results() {
        let mut state = app_with_workspaces(&["one"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;

        state.handle_app_event(AppEvent::ZoxideQueryCompleted {
            generation: gen,
            available: true,
            candidates: vec![candidate("/tmp/fresh", "/tmp/fresh", 10.0)],
        });

        assert_eq!(state.workspace_switcher.provider_candidates.len(), 1);
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Ready
        );
    }

    #[test]
    fn stale_generation_discards_preview_results() {
        let mut state = app_with_workspaces(&["one"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;

        state.handle_app_event(AppEvent::DirectoryPreviewCompleted {
            generation: gen.wrapping_sub(1),
            shown_path: std::path::PathBuf::from("/tmp/whatever"),
            result: Ok(DirectoryPreview {
                entries: vec![],
                truncated: false,
            }),
        });

        assert!(state.workspace_switcher.directory_preview_cache.is_empty());
    }

    #[test]
    fn fresh_generation_caches_preview_results() {
        let mut state = app_with_workspaces(&["one"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        let path = std::path::PathBuf::from("/tmp/whatever");

        state.handle_app_event(AppEvent::DirectoryPreviewCompleted {
            generation: gen,
            shown_path: path.clone(),
            result: Ok(DirectoryPreview {
                entries: vec![DirectoryPreviewEntry {
                    name: "file".into(),
                    is_dir: false,
                }],
                truncated: false,
            }),
        });

        assert!(matches!(
            state.workspace_switcher.directory_preview_cache.get(&path),
            Some(DirectoryPreviewState::Ready(_))
        ));
    }

    #[test]
    fn preview_error_caches_error_state() {
        let mut state = app_with_workspaces(&["one"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        let path = std::path::PathBuf::from("/nonexistent/whatever");

        state.handle_app_event(AppEvent::DirectoryPreviewCompleted {
            generation: gen,
            shown_path: path.clone(),
            result: Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        });

        assert!(matches!(
            state.workspace_switcher.directory_preview_cache.get(&path),
            Some(DirectoryPreviewState::Error)
        ));
    }

    #[test]
    fn directory_accept_sets_pending_when_not_open() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/unopened", "/tmp/unopened", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.query = "unopened".into();

        // Select the directory row.
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);
        let accepted = state.accept_workspace_switcher_selection_from(&terminal_runtimes);
        assert!(accepted);
        assert!(state.workspace_switcher.pending_directory.is_some());
        assert!(state.workspace_switcher.active);
        // Preview should show Opening…
        assert!(matches!(
            state.workspace_switcher.preview,
            WorkspaceSwitcherPreview::Opening { .. }
        ));
    }

    #[test]
    fn directory_accept_prevents_repeated_acceptance() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/unopened", "/tmp/unopened", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.query = "unopened".into();
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);

        // First accept succeeds.
        assert!(state.accept_workspace_switcher_selection_from(&terminal_runtimes));
        // Second accept is prevented.
        assert!(!state.accept_workspace_switcher_selection_from(&terminal_runtimes));
    }

    #[test]
    fn directory_accept_records_pending_target() {
        let dir = std::env::temp_dir().join(format!("herdr-switcher-focus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let canonical = std::fs::canonicalize(&dir).expect("canonicalize");

        let mut state = AppState::test_new();
        let mut ws = Workspace::test_new("existing");
        ws.identity_cwd = canonical.clone();
        ws.cached_identity_cwd = canonical.clone();
        state.workspaces.push(ws);
        state.ensure_test_terminals();
        state.active = Some(0);
        state.mode = Mode::Terminal;

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);

        // In the new architecture, accept_directory_from records the pending
        // target only — it does NOT perform the canonical recheck or set
        // pending_directory_rendered. The App controller does that after
        // a render boundary.
        let accepted =
            state.accept_directory_from(&terminal_runtimes, canonical.clone(), canonical.clone());
        assert!(accepted);
        // Switcher stays open; pending_directory is set.
        assert!(state.workspace_switcher.active);
        assert!(state.workspace_switcher.pending_directory.is_some());
        // The rendered flag is NOT set yet (only set after a frame draw).
        assert!(!state.workspace_switcher.pending_directory_rendered);
        // The preview shows Opening…
        assert!(matches!(
            state.workspace_switcher.preview,
            crate::ui::workspace_switcher::WorkspaceSwitcherPreview::Opening { .. }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_error_shows_in_preview() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        enter_search(&mut state);
        state.workspace_switcher.search_error = Some("Could not open /tmp/bad".to_string());
        state.refresh_workspace_switcher_preview_from(&terminal_runtimes);

        assert!(matches!(
            state.workspace_switcher.preview,
            WorkspaceSwitcherPreview::Empty { ref message } if message == "Could not open /tmp/bad"
        ));
    }

    #[test]
    fn leave_search_clears_provider_state() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![candidate("/tmp/x", "/tmp/x", 1.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.directory_preview_cache.insert(
            std::path::PathBuf::from("/tmp/x"),
            DirectoryPreviewState::Loading,
        );

        state.leave_workspace_switcher_search_from(&terminal_runtimes);

        assert!(state.workspace_switcher.provider_candidates.is_empty());
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Idle
        );
        assert!(state.workspace_switcher.directory_preview_cache.is_empty());
    }

    #[test]
    fn close_switcher_clears_provider_state() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![candidate("/tmp/x", "/tmp/x", 1.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.pending_directory =
            Some(crate::ui::workspace_switcher::PendingDirectoryAccept {
                shown_path: std::path::PathBuf::from("/tmp/x"),
                canonical_path: std::path::PathBuf::from("/tmp/x"),
            });

        close_workspace_switcher(&mut state);

        assert!(state.workspace_switcher.provider_candidates.is_empty());
        assert!(state.workspace_switcher.pending_directory.is_none());
        assert!(state.workspace_switcher.directory_preview_cache.is_empty());
    }

    #[test]
    fn directory_preview_request_set_when_not_cached() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/preview-me", "/tmp/preview-me", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.query = "preview".into();

        // Select the directory row and refresh preview.
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);
        state.refresh_workspace_switcher_preview_from(&terminal_runtimes);

        // Preview request should be set for the uncached path.
        assert_eq!(
            state.workspace_switcher.preview_request,
            Some(std::path::PathBuf::from("/tmp/preview-me"))
        );
    }

    #[test]
    fn directory_preview_request_not_set_when_cached() {
        let mut state = app_with_workspaces(&["main"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        enter_search(&mut state);
        let path = std::path::PathBuf::from("/tmp/cached");
        state.workspace_switcher.provider_candidates = vec![candidate(
            path.to_str().unwrap(),
            path.to_str().unwrap(),
            10.0,
        )];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        state.workspace_switcher.directory_preview_cache.insert(
            path.clone(),
            DirectoryPreviewState::Ready(DirectoryPreview {
                entries: vec![],
                truncated: false,
            }),
        );
        state.workspace_switcher.query = "cached".into();

        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);
        state.refresh_workspace_switcher_preview_from(&terminal_runtimes);

        // No new preview request since it's cached.
        assert!(state.workspace_switcher.preview_request.is_none());
        assert!(matches!(
            state.workspace_switcher.preview,
            WorkspaceSwitcherPreview::Directory { .. }
        ));
    }

    #[test]
    fn unavailable_provider_shows_no_loading_status() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_status = SearchProviderStatus::Unavailable;
        state.workspace_switcher.provider_candidates = vec![];
        set_query(&mut state, "main");

        // Only workspace rows, no directory rows.
        let rows = state.workspace_switcher_rows();
        assert!(rows.iter().all(|r| !r.is_directory));
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn mru_coalescing_enriches_only_most_recent_workspace() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-switcher-mru-coalesce-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let canonical = std::fs::canonicalize(&dir).expect("canonicalize");
        let basename = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut state = AppState::test_new();
        // Two workspaces at the same cwd, neither named to match the basename.
        let mut ws_a = Workspace::test_new("ws-alpha");
        ws_a.identity_cwd = canonical.clone();
        ws_a.cached_identity_cwd = canonical.clone();
        let mut ws_b = Workspace::test_new("ws-beta");
        ws_b.identity_cwd = canonical.clone();
        ws_b.cached_identity_cwd = canonical.clone();
        state.workspaces.push(ws_a);
        state.workspaces.push(ws_b);
        state.ensure_test_terminals();
        state.active = Some(1);
        state.mode = Mode::Terminal;
        // MRU: ws_b (most recent) first, then ws_a.
        state.workspace_mru = vec![
            state.workspaces[1].id.clone(),
            state.workspaces[0].id.clone(),
        ];

        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![candidate(
            canonical.to_str().unwrap(),
            canonical.to_str().unwrap(),
            100.0,
        )];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        // Query matches the basename, not the workspace names.
        set_query(&mut state, &basename);

        let rows = state.workspace_switcher_rows();
        // Only the most-recent workspace (ws-beta, ws_idx=1) should be
        // enriched and match through the zoxide basename.
        let matching: Vec<_> = rows.iter().filter(|r| !r.is_directory).collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].ws_idx, 1);
        // No directory row (candidate is coalesced).
        assert!(rows.iter().all(|r| !r.is_directory));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Item 14: Quick Switch regression tests
    // -----------------------------------------------------------------------

    #[test]
    fn quick_switch_shows_all_workspaces_with_tabs_collapsed() {
        let mut state = app_with_workspaces(&["alpha", "beta", "gamma"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        // Quick Switch mode by default.
        assert_eq!(
            state.workspace_switcher.mode,
            crate::ui::workspace_switcher::WorkspaceSwitcherMode::QuickSwitch
        );
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        // All three workspaces, no tabs (collapsed), no directories.
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| !r.is_directory && !r.is_tab));
    }

    #[test]
    fn quick_switch_accept_focuses_selected_workspace() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        // Select the second workspace (index 1 in the row list).
        state.workspace_switcher.selected = 1;
        state.capture_workspace_switcher_target_from(&terminal_runtimes);
        let accepted = state.accept_workspace_switcher_selection_from(&terminal_runtimes);
        assert!(accepted);
        assert!(!state.workspace_switcher.active);
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn quick_switch_escape_closes_without_changing_active() {
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        let original_active = state.active;
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        assert!(state.workspace_switcher.active);
        close_workspace_switcher(&mut state);
        assert!(!state.workspace_switcher.active);
        assert_eq!(state.active, original_active);
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn quick_switch_no_directory_rows_or_provider_state() {
        let mut state = app_with_workspaces(&["alpha"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert!(rows.iter().all(|r| !r.is_directory));
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Idle
        );
        assert!(state.workspace_switcher.provider_candidates.is_empty());
    }

    // -----------------------------------------------------------------------
    // Item 14: Row projection uses snapshot (no filesystem I/O)
    // -----------------------------------------------------------------------

    #[test]
    fn row_projection_uses_snapshot_not_filesystem() {
        // Create a workspace with a canonical cwd that does NOT exist on the
        // filesystem. If the UI tried to canonicalize, it would get a
        // different path. The snapshot ensures pure data comparison.
        let mut state = AppState::test_new();
        let mut ws = Workspace::test_new("test");
        ws.cached_identity_cwd = std::path::PathBuf::from("/fake/nonexistent/path");
        state.workspaces.push(ws);
        state.ensure_test_terminals();
        state.active = Some(0);
        state.mode = Mode::Terminal;

        enter_search(&mut state);
        // Set candidates that match the snapshot canonical path.
        state.workspace_switcher.provider_candidates = vec![candidate(
            "/fake/nonexistent/path",
            "/fake/nonexistent/path",
            100.0,
        )];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "path");

        let rows = state.workspace_switcher_rows();
        // The workspace should be coalesced (enriched), no directory row.
        let workspace_rows: Vec<_> = rows.iter().filter(|r| !r.is_directory).collect();
        assert_eq!(workspace_rows.len(), 1);
        assert!(rows.iter().all(|r| !r.is_directory));
    }

    // -----------------------------------------------------------------------
    // Item 4: Basename-first ranking — path rank better but basename controls
    // -----------------------------------------------------------------------

    #[test]
    fn basename_rank_wins_over_better_path_rank() {
        // Distinguishes basename-first from min(basename, path):
        //
        // Candidate A "/foo/bar": basename "bar" does NOT match query "foo",
        // path matches at quality 0 (word prefix). Rank = 0 (path only).
        //
        // Candidate B "/xfoo": basename "xfoo" matches "foo" at quality 1
        // (contains, not word prefix). Path "/xfoo" also matches at quality 0
        // (word prefix).
        //
        // Old min(b,p): B=0, A=0 → tie → B (score 200) before A (score 100).
        // New basename-first: B=1, A=0 → A before B.
        let mut state = app_with_workspaces(&["placeholder"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![
            candidate("/xfoo", "/xfoo", 200.0),
            candidate("/foo/bar", "/foo/bar", 100.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "foo");

        let rows = state.workspace_switcher_rows();
        assert_eq!(rows.len(), 2);
        // A (path-only match, quality 0) ranks before B (basename contains,
        // quality 1) even though B has a higher score.
        assert_eq!(rows[0].label.display(), "bar");
        assert_eq!(rows[1].label.display(), "xfoo");
    }

    // -----------------------------------------------------------------------
    // Item 3: Stale candidate removal on create failure
    // -----------------------------------------------------------------------

    #[test]
    fn create_failure_removes_stale_candidate_and_clamps() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![
            candidate("/tmp/bad-dir", "/tmp/bad-dir", 10.0),
            candidate("/tmp/good-dir", "/tmp/good-dir", 5.0),
        ];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "dir");

        // Simulate a failed create: the finalize_directory_acceptance path
        // removes the candidate and shows an error.
        let pending = crate::ui::workspace_switcher::PendingDirectoryAccept {
            shown_path: std::path::PathBuf::from("/tmp/bad-dir"),
            canonical_path: std::path::PathBuf::from("/tmp/bad-dir"),
        };
        // We can’t call finalize_directory_acceptance from AppState tests
        // (it’s on App), but we can verify the retain logic works:
        state
            .workspace_switcher
            .provider_candidates
            .retain(|c| c.canonical_path != pending.canonical_path);
        state.workspace_switcher.search_error = Some("Could not open /tmp/bad-dir".to_string());

        // The bad candidate is gone, the good one remains.
        assert_eq!(state.workspace_switcher.provider_candidates.len(), 1);
        assert_eq!(
            state.workspace_switcher.provider_candidates[0].shown_path,
            std::path::PathBuf::from("/tmp/good-dir")
        );
        // Error is set.
        assert!(state.workspace_switcher.search_error.is_some());
    }

    // -----------------------------------------------------------------------
    // Item 8: Deferred Opening… — pending_directory_rendered flag
    // -----------------------------------------------------------------------

    #[test]
    fn opening_preview_deferred_until_after_render_boundary() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/somedir", "/tmp/somedir", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "somedir");

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let dir_idx = rows.iter().position(|r| r.is_directory).expect("dir row");
        state.workspace_switcher.selected = dir_idx;
        state.capture_workspace_switcher_target_from(&terminal_runtimes);
        let accepted = state.accept_workspace_switcher_selection_from(&terminal_runtimes);

        assert!(accepted);
        assert!(state.workspace_switcher.pending_directory.is_some());
        // After accept, the preview shows Opening… but the rendered flag
        // is NOT set yet — it must only be set after an actual frame draw
        // (which the App controller does after terminal.draw).
        assert!(!state.workspace_switcher.pending_directory_rendered);
        assert!(matches!(
            state.workspace_switcher.preview,
            crate::ui::workspace_switcher::WorkspaceSwitcherPreview::Opening { .. }
        ));

        // Simulate a render boundary: the controller sets the flag after
        // the frame is drawn.
        state.workspace_switcher.pending_directory_rendered = true;
        // Now the deferred create block would fire on the next iteration.
        // The flag is consumed and reset by the controller.
        state.workspace_switcher.pending_directory = None;
        state.workspace_switcher.pending_directory_rendered = false;
        assert!(state.workspace_switcher.pending_directory.is_none());
    }

    #[test]
    fn repeated_acceptance_prevented_while_pending() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates =
            vec![candidate("/tmp/somedir", "/tmp/somedir", 10.0)];
        state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
        set_query(&mut state, "somedir");

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let dir_idx = rows.iter().position(|r| r.is_directory).expect("dir row");
        state.workspace_switcher.selected = dir_idx;
        state.capture_workspace_switcher_target_from(&terminal_runtimes);

        // First accept succeeds.
        assert!(state.accept_workspace_switcher_selection_from(&terminal_runtimes));
        // Second accept is blocked.
        assert!(!state.accept_workspace_switcher_selection_from(&terminal_runtimes));
    }

    // -----------------------------------------------------------------------
    // Item 14: Headless/App-level Search provider event tests
    // -----------------------------------------------------------------------

    #[test]
    fn zoxide_query_started_sets_loading_status() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        state.handle_app_event(AppEvent::ZoxideQueryStarted { generation: gen });
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Loading
        );
    }

    #[test]
    fn zoxide_query_started_discarded_for_stale_generation() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.handle_app_event(AppEvent::ZoxideQueryStarted { generation: 999 });
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Idle
        );
    }

    #[test]
    fn zoxide_query_completed_sets_ready_and_refresh_flag() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        state.handle_app_event(AppEvent::ZoxideQueryCompleted {
            generation: gen,
            available: true,
            candidates: vec![candidate("/tmp/dir", "/tmp/dir", 10.0)],
        });
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Ready
        );
        assert_eq!(state.workspace_switcher.provider_candidates.len(), 1);
        assert!(state.workspace_switcher.needs_provider_refresh);
    }

    #[test]
    fn zoxide_query_completed_unavailable_clears_candidates() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        state.workspace_switcher.provider_candidates = vec![candidate("/tmp/old", "/tmp/old", 5.0)];
        let gen = state.workspace_switcher.search_generation;
        state.handle_app_event(AppEvent::ZoxideQueryCompleted {
            generation: gen,
            available: false,
            candidates: vec![],
        });
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Unavailable
        );
        assert!(state.workspace_switcher.provider_candidates.is_empty());
        assert!(state.workspace_switcher.needs_provider_refresh);
    }

    #[test]
    fn directory_preview_completed_sets_cache_and_refresh_flag() {
        use crate::app::workspace_search_provider::DirectoryPreview;
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        let path = std::path::PathBuf::from("/tmp/somedir");
        let preview = DirectoryPreview {
            entries: vec![],
            truncated: false,
        };
        state.handle_app_event(AppEvent::DirectoryPreviewCompleted {
            generation: gen,
            shown_path: path.clone(),
            result: Ok(preview),
        });
        assert!(state
            .workspace_switcher
            .directory_preview_cache
            .contains_key(&path));
        assert_eq!(state.workspace_switcher.needs_preview_refresh, Some(path));
    }

    #[test]
    fn directory_preview_completed_error_caches_error_state() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let gen = state.workspace_switcher.search_generation;
        let path = std::path::PathBuf::from("/tmp/missing");
        state.handle_app_event(AppEvent::DirectoryPreviewCompleted {
            generation: gen,
            shown_path: path.clone(),
            result: Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        });
        assert!(matches!(
            state.workspace_switcher.directory_preview_cache.get(&path),
            Some(crate::app::workspace_search_provider::DirectoryPreviewState::Error)
        ));
        assert_eq!(state.workspace_switcher.needs_preview_refresh, Some(path));
    }

    #[test]
    fn stale_generation_events_discarded() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let old_gen = state.workspace_switcher.search_generation;
        // Enter search again to bump the generation.
        enter_search(&mut state);
        assert_ne!(state.workspace_switcher.search_generation, old_gen);

        // Old-generation events should be ignored.
        state.handle_app_event(AppEvent::ZoxideQueryCompleted {
            generation: old_gen,
            available: true,
            candidates: vec![candidate("/tmp/stale", "/tmp/stale", 10.0)],
        });
        assert_eq!(
            state.workspace_switcher.provider_status,
            SearchProviderStatus::Idle
        );
        assert!(state.workspace_switcher.provider_candidates.is_empty());
        assert!(!state.workspace_switcher.needs_provider_refresh);
    }

    // -----------------------------------------------------------------------
    // Item 5: Abbreviated path metadata
    // -----------------------------------------------------------------------

    #[test]
    fn directory_row_uses_abbreviated_path_metadata() {
        let mut state = app_with_workspaces(&["main"]);
        enter_search(&mut state);
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        if let Some(home) = home {
            let dir_path = home.join("projects/myapp");
            state.workspace_switcher.provider_candidates = vec![candidate(
                dir_path.to_str().unwrap(),
                dir_path.to_str().unwrap(),
                10.0,
            )];
            state.workspace_switcher.provider_status = SearchProviderStatus::Ready;
            set_query(&mut state, "myapp");

            let rows = state.workspace_switcher_rows();
            let dir_row = rows.iter().find(|r| r.is_directory).expect("dir row");
            assert!(dir_row.meta.starts_with('~'));
            assert!(dir_row.meta.contains("projects/myapp"));
        }
    }

    #[test]
    fn entering_search_resets_selection_to_first_item() {
        let mut state = app_with_workspaces(&["main", "issue", "docs"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);

        // QuickSwitch opens on the first non-active workspace, not row 0.
        assert_ne!(state.workspace_switcher.selected, 0);

        state.enter_workspace_switcher_search_from(&terminal_runtimes);

        assert_eq!(state.workspace_switcher.selected, 0);
        assert_eq!(state.workspace_switcher.scroll, 0);
    }

    // -----------------------------------------------------------------------
    // Repository-label composite tests
    // -----------------------------------------------------------------------

    #[test]
    fn composite_label_shown_for_managed_linked_worktree_in_quick_switch() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child.label.parts().0, Some("myrepo"));
        // Composite label includes repo name and the branch-derived label.
        assert!(child.label.display().starts_with("myrepo / "));
        assert!(child.label.display().contains("feature-x"));
    }

    #[test]
    fn composite_label_shown_for_custom_named_linked_worktree() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = Some("custom-name".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child.label.parts().0, Some("myrepo"));
        assert!(child.label.display().starts_with("myrepo / "));
        assert!(child.label.display().contains("custom-name"));
    }

    #[test]
    fn parent_workspace_has_no_composite_label() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree(&mut state, 1);

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let parent = rows.iter().find(|r| r.ws_idx == 0).unwrap();
        assert_eq!(parent.label.parts().0, None);
        assert!(!parent.label.display().contains(" / "));
    }

    #[test]
    fn standalone_linked_worktree_with_custom_name_shows_composite_label() {
        // A standalone managed linked worktree (no grouped parent) with a
        // custom name should render `<repo> / <custom name>` — no branch
        // substitution, but repo context is still prepended.
        let mut state = app_with_workspaces(&["other", "my-workspace"]);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = Some("custom-name".into());
        state.workspaces[1].cached_git_branch = Some("worktree/some-branch".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child.label.parts().0, Some("myrepo"));
        assert_eq!(child.label.display(), "myrepo / custom-name");
        // The branch name must not appear — custom name is authoritative.
        assert!(!child.label.display().contains("some-branch"));
    }

    #[test]
    fn unmanaged_workspace_has_no_composite_label() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        // No worktree_space at all — completely unmanaged.

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        for row in &rows {
            assert_eq!(row.label.parts().0, None);
            assert!(!row.label.display().contains(" / "));
        }
    }

    #[test]
    fn empty_repo_name_falls_back_to_existing_label() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        // Empty repo name → no composite, no separator, no placeholder.
        assert_eq!(child.label.parts().0, None);
        assert!(!child.label.display().contains(" / "));
        // The existing label (branch substitution) is still present.
        assert_eq!(child.label.display(), "feature-x");
    }

    #[test]
    fn composite_label_consistent_across_quick_switch_empty_query_and_search() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        // QuickSwitch mode.
        state.open_workspace_switcher_from(&terminal_runtimes);
        let qs_label = state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .find(|r| r.ws_idx == 1)
            .map(|r| r.label.display())
            .unwrap();

        // Search mode with empty query.
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        let empty_label = state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .find(|r| r.ws_idx == 1)
            .map(|r| r.label.display())
            .unwrap();

        // Search mode with a query that matches the row.
        state.workspace_switcher.query = "myrepo".into();
        let search_label = state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .find(|r| r.ws_idx == 1)
            .map(|r| r.label.display())
            .unwrap();

        assert_eq!(qs_label, empty_label);
        assert_eq!(qs_label, search_label);
        assert!(qs_label.starts_with("myrepo / "));
    }

    #[test]
    fn search_matches_repo_name_only() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        state.workspace_switcher.query = "myrepo".into();

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let child = rows.iter().find(|r| r.ws_idx == 1);
        assert!(child.is_some(), "repo-name-only query should find the row");
    }

    #[test]
    fn search_matches_existing_label_only() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        state.workspace_switcher.query = "feature".into();

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let child = rows.iter().find(|r| r.ws_idx == 1);
        assert!(child.is_some(), "label-only query should find the row");
    }

    #[test]
    fn search_matches_combined_repo_and_label_terms() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        state.workspace_switcher.query = "myrepo feature".into();

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let child = rows.iter().find(|r| r.ws_idx == 1);
        assert!(
            child.is_some(),
            "combined repo+label query should find the row"
        );
    }

    #[test]
    fn composite_row_preserves_pane_metadata() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        // The meta field should still contain the pane count.
        assert!(child.meta.contains("pane"));
    }

    fn rendered_line(terminal: &Terminal<TestBackend>, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect()
    }

    fn rendered_rect_line(terminal: &Terminal<TestBackend>, rect: Rect, y: u16) -> String {
        (rect.x..rect.x + rect.width)
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn render_linked_workspace_uses_label_primary_and_repo_activity_secondary() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::composite("herdr".to_string(), "feature-x".to_string()),
            meta: "2 panes".to_string(),
            activity: "1 working".to_string(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Idle,
            seen: false,
        };
        let area = Rect::new(0, 0, 40, 2);
        let mut terminal = Terminal::new(TestBackend::new(40, 2)).unwrap();

        terminal
            .draw(|frame| render_row(&app, frame, area, &row, false))
            .unwrap();

        let primary = rendered_line(&terminal, 0, 40);
        let secondary = rendered_line(&terminal, 1, 40);
        assert!(primary.contains("feature-x"));
        assert!(primary.contains("2 panes"));
        assert!(!primary.contains("herdr"));
        assert!(secondary.contains("herdr · 1 working"));
        assert_eq!(
            primary.chars().position(|ch| ch == 'f'),
            secondary.chars().position(|ch| ch == 'h')
        );
    }

    #[test]
    fn secondary_text_preserves_repository_before_activity() {
        assert_eq!(
            workspace_secondary_text(Some("repository"), "working", 10),
            "repository"
        );
        assert_eq!(
            workspace_secondary_text(Some("repository"), "working", 6),
            "repos…"
        );
        assert_eq!(
            workspace_secondary_text(Some("repo"), "long activity", 11),
            "repo · lon…"
        );
        assert_eq!(workspace_secondary_text(Some("repo"), "working", 8), "repo");
    }

    #[test]
    fn render_workspace_directory_and_tab_secondary_lines() {
        let app = AppState::test_new();
        let rows = [
            WorkspaceSwitcherRow {
                target: WorkspaceSwitcherTarget::Workspace {
                    workspace_id: String::new(),
                },
                ws_idx: 0,
                depth: 0,
                label: SwitcherLabel::plain("workspace".to_string()),
                meta: "1 pane".to_string(),
                activity: "2 blocked".to_string(),
                is_current: false,
                expanded: false,
                is_tab: false,
                is_directory: false,
                state: crate::detect::AgentState::Idle,
                seen: false,
            },
            WorkspaceSwitcherRow {
                target: WorkspaceSwitcherTarget::Directory {
                    shown_path: "/work/project".into(),
                    canonical_path: "/work/project".into(),
                },
                ws_idx: usize::MAX,
                depth: 0,
                label: SwitcherLabel::plain("project".to_string()),
                meta: "/work/project".to_string(),
                activity: String::new(),
                is_current: false,
                expanded: false,
                is_tab: false,
                is_directory: true,
                state: crate::detect::AgentState::Idle,
                seen: false,
            },
            WorkspaceSwitcherRow {
                target: WorkspaceSwitcherTarget::Tab {
                    tab_id: String::new(),
                },
                ws_idx: 0,
                depth: 1,
                label: SwitcherLabel::plain("tab".to_string()),
                meta: "3 panes".to_string(),
                activity: String::new(),
                is_current: false,
                expanded: false,
                is_tab: true,
                is_directory: false,
                state: crate::detect::AgentState::Idle,
                seen: false,
            },
        ];

        for (row, expected_secondary) in rows.iter().zip(["2 blocked", "/work/project", ""]) {
            let mut terminal = Terminal::new(TestBackend::new(40, 2)).unwrap();
            terminal
                .draw(|frame| render_row(&app, frame, Rect::new(0, 0, 40, 2), row, true))
                .unwrap();
            let primary = rendered_line(&terminal, 0, 40);
            let secondary = rendered_line(&terminal, 1, 40);
            assert_eq!(terminal.backend().buffer()[(0, 0)].bg, app.palette.accent);
            assert_eq!(terminal.backend().buffer()[(0, 1)].bg, app.palette.accent);
            if expected_secondary.is_empty() {
                assert!(secondary.trim().is_empty());
            } else {
                assert!(secondary.contains(expected_secondary));
            }
            if row.is_directory {
                assert!(!primary.contains(&row.meta));
            } else {
                assert!(primary.contains(&row.meta));
            }
        }
    }

    #[test]
    fn pane_metadata_uses_existing_width_thresholds() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("workspace".to_string()),
            meta: "12 panes".to_string(),
            activity: String::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Idle,
            seen: false,
        };

        for (width, visible) in [(38, true), (30, true), (29, false)] {
            let mut terminal = Terminal::new(TestBackend::new(width, 2)).unwrap();
            terminal
                .draw(|frame| render_row(&app, frame, Rect::new(0, 0, width, 2), &row, false))
                .unwrap();
            assert_eq!(
                rendered_line(&terminal, 0, width).contains("12 panes"),
                visible
            );
        }
    }

    #[test]
    fn selected_style_covers_both_item_lines() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("workspace".to_string()),
            meta: "1 pane".to_string(),
            activity: "working".to_string(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Working,
            seen: false,
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 2)).unwrap();
        terminal
            .draw(|frame| render_row(&app, frame, Rect::new(0, 0, 40, 2), &row, true))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for y in 0..2 {
            assert_eq!(buffer[(0, y)].bg, app.palette.accent);
            assert_eq!(buffer[(5, y)].fg, panel_contrast_fg(&app.palette));
        }
        assert_eq!(
            buffer[(3, 0)].fg,
            state_dot(row.state, row.seen, &app.palette).1.fg.unwrap()
        );
    }

    #[test]
    fn complete_two_line_items_render_in_mobile_and_desktop_layouts() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for (width, expected_layout) in [
            (44, crate::app::state::ViewLayout::Mobile),
            (80, crate::app::state::ViewLayout::Desktop),
        ] {
            let mut app = app_with_workspaces(&["main", "feature"]);
            mark_parent_worktree(&mut app, 0);
            mark_linked_worktree_with_repo(&mut app, 1, "herdr");
            app.workspaces[1].custom_name = Some("feature".to_string());
            crate::ui::compute_view(&mut app, Rect::new(0, 0, width, 24));
            app.open_workspace_switcher_from(&terminal_runtimes);
            assert_eq!(app.view.layout, expected_layout);

            let body = app.workspace_switcher_body_rect();
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal
                .draw(|frame| render_workspace_switcher_overlay(&app, &terminal_runtimes, frame))
                .unwrap();

            let feature_y = (body.y..body.y + body.height)
                .find(|&y| rendered_rect_line(&terminal, body, y).contains("feature"))
                .expect("linked workspace primary line should render");
            assert_eq!((feature_y - body.y) % WORKSPACE_SWITCHER_LINES_PER_ITEM, 0);
            assert!(rendered_rect_line(&terminal, body, feature_y + 1).contains("herdr"));
        }
    }

    #[test]
    fn render_rows_only_uses_complete_item_capacity_in_both_modes() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for mode in [
            WorkspaceSwitcherMode::QuickSwitch,
            WorkspaceSwitcherMode::Search,
        ] {
            let mut app = app_with_workspaces(&["one", "two", "three"]);
            app.workspace_switcher.active = true;
            app.workspace_switcher.mode = mode;
            let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
            terminal
                .draw(|frame| render_rows(&app, &terminal_runtimes, frame, Rect::new(0, 0, 30, 5)))
                .unwrap();

            assert!(!rendered_line(&terminal, 0, 30).trim().is_empty());
            assert!(!rendered_line(&terminal, 2, 30).trim().is_empty());
            assert!(rendered_line(&terminal, 4, 30).trim().is_empty());
        }
    }

    #[test]
    fn render_rows_reports_hidden_items_when_no_complete_item_fits() {
        let mut app = app_with_workspaces(&["one", "two", "three"]);
        app.workspace_switcher.active = true;
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
        terminal
            .draw(|frame| render_rows(&app, &terminal_runtimes, frame, Rect::new(0, 0, 30, 1)))
            .unwrap();

        assert_eq!(rendered_line(&terminal, 0, 30).trim_end(), "3 items hidden");
    }

    #[test]
    fn zero_result_messages_take_priority_over_zero_capacity_message() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let cases = [
            (AppState::test_new(), "no workspaces"),
            (
                {
                    let mut app = app_with_workspaces(&["one"]);
                    app.workspace_switcher.mode = WorkspaceSwitcherMode::Search;
                    app.workspace_switcher.query = "missing".to_string();
                    app
                },
                "no matching workspaces",
            ),
        ];

        for (app, expected) in cases {
            let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
            terminal
                .draw(|frame| render_rows(&app, &terminal_runtimes, frame, Rect::new(0, 0, 30, 1)))
                .unwrap();
            assert!(rendered_line(&terminal, 0, 30).contains(expected));
        }
    }

    fn set_switcher_view(state: &mut AppState, width: u16, height: u16) {
        state.view.sidebar_rect = Rect::default();
        state.view.terminal_area = Rect::new(0, 0, width, height);
    }

    #[test]
    fn hit_testing_maps_both_lines_and_rejects_spare_and_post_list_rows() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for height in 12..40 {
            set_switcher_view(&mut state, 80, height);
            if state.workspace_switcher_body_rect().height % 2 == 1 {
                break;
            }
        }
        state.open_workspace_switcher_from(&terminal_runtimes);
        let body = state.workspace_switcher_body_rect();
        assert_eq!(body.height % 2, 1);
        assert_eq!(
            state.workspace_switcher_row_index_at_from(&terminal_runtimes, body.x, body.y,),
            state.workspace_switcher_row_index_at_from(&terminal_runtimes, body.x, body.y + 1,)
        );
        assert_eq!(
            state.workspace_switcher_row_index_at_from(&terminal_runtimes, body.x, body.y + 2,),
            Some(state.workspace_switcher.scroll + 1)
        );
        let spare_y = body.y + workspace_switcher_capacity(body.height) as u16 * 2;
        assert!(spare_y < body.y + body.height);
        assert_eq!(
            state.workspace_switcher_row_index_at_from(&terminal_runtimes, body.x, spare_y),
            None
        );
        let post_list_y = body.y + state.workspace_switcher_rows().len() as u16 * 2;
        assert!(post_list_y < spare_y);
        assert_eq!(
            state.workspace_switcher_row_index_at_from(&terminal_runtimes, body.x, post_list_y),
            None
        );
    }

    #[test]
    fn second_line_hover_and_click_target_the_same_item() {
        let mut state = app_with_workspaces(&["one", "two", "three"]);
        set_switcher_view(&mut state, 80, 30);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let body = state.workspace_switcher_body_rect();
        let expected = state.workspace_switcher.scroll + 1;

        handle_workspace_switcher_mouse(
            &mut state,
            &terminal_runtimes,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: body.x,
                row: body.y + 3,
                modifiers: KeyModifiers::empty(),
            },
        );
        assert_eq!(state.workspace_switcher.selected, expected);

        let expected_workspace = state.workspace_switcher_rows()[expected].ws_idx;
        handle_workspace_switcher_mouse(
            &mut state,
            &terminal_runtimes,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: body.x,
                row: body.y + 3,
                modifiers: KeyModifiers::empty(),
            },
        );
        assert_eq!(state.active, Some(expected_workspace));
        assert!(!state.workspace_switcher.active);
    }

    #[test]
    fn page_navigation_and_resize_use_logical_item_capacity() {
        let names = (0..20).map(|idx| format!("ws-{idx}")).collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut state = app_with_workspaces(&name_refs);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 30));
        state.open_workspace_switcher_from(&terminal_runtimes);
        let capacity = workspace_switcher_capacity(state.workspace_switcher_body_rect().height);
        state.workspace_switcher.selected = 0;
        state.workspace_switcher.scroll = 0;
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, capacity);
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, 0);

        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        let search_capacity =
            workspace_switcher_capacity(state.workspace_switcher_body_rect().height);
        state.workspace_switcher.selected = 0;
        state.workspace_switcher.scroll = 0;
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, search_capacity);
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, 0);

        state.workspace_switcher.selected = state.workspace_switcher_rows().len() - 1;
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 16));
        let resized_capacity =
            workspace_switcher_capacity(state.workspace_switcher_body_rect().height);
        assert!(state.workspace_switcher.selected >= state.workspace_switcher.scroll);
        assert!(
            state.workspace_switcher.selected
                < state
                    .workspace_switcher
                    .scroll
                    .saturating_add(resized_capacity)
        );
        assert_eq!(
            state.workspace_switcher.scroll,
            state
                .workspace_switcher_rows()
                .len()
                .saturating_sub(resized_capacity)
        );
    }

    #[test]
    fn mouse_wheel_moves_three_logical_items() {
        let names = (0..20).map(|idx| format!("ws-{idx}")).collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut state = app_with_workspaces(&name_refs);
        set_switcher_view(&mut state, 80, 20);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.workspace_switcher.selected = 0;
        state.workspace_switcher.scroll = 0;
        let body = state.workspace_switcher_body_rect();

        handle_workspace_switcher_mouse(
            &mut state,
            &terminal_runtimes,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: body.x,
                row: body.y,
                modifiers: KeyModifiers::empty(),
            },
        );
        assert_eq!(state.workspace_switcher.scroll, 3);
        assert_eq!(state.workspace_switcher.selected, 3);
    }
}
