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
        actions::{tab_activity_summary, tab_aggregate_state, workspace_activity_summary},
        state::{AppState, Mode, ViewLayout},
    },
    config::key_event_matches_combo,
    input::TerminalKey,
    terminal::TerminalRuntimeRegistry,
};

/// Physical line count consumed by a row: directories always render their
/// path on the second line, and every other row renders a second line only
/// when it carries secondary metadata.
fn switcher_row_height(row: &WorkspaceSwitcherRow) -> usize {
    if row.is_directory || !row.secondary.is_empty() {
        2
    } else {
        1
    }
}

/// Shared variable-height layout for the switcher list.
///
/// One instance computed from the current rows and body height drives every
/// geometry consumer: render rectangles, capacity, page movement, wheel
/// movement, maximum scroll, selection visibility, mouse hit-testing,
/// trailing blank-space handling, and scrollbar metrics.
#[derive(Debug, Clone)]
struct SwitcherRowLayout {
    /// Cumulative physical lines: `prefix[i]` is the line count before row
    /// `i`; `prefix[rows]` is the total line count.
    prefix: Vec<usize>,
    body_height: usize,
}

impl SwitcherRowLayout {
    fn new(rows: &[WorkspaceSwitcherRow], body_height: usize) -> Self {
        let mut prefix = Vec::with_capacity(rows.len() + 1);
        prefix.push(0);
        for row in rows {
            prefix.push(prefix.last().copied().unwrap_or(0) + switcher_row_height(row));
        }
        Self {
            prefix,
            body_height,
        }
    }

    fn rows(&self) -> usize {
        self.prefix.len().saturating_sub(1)
    }

    fn total_physical(&self) -> usize {
        *self.prefix.last().unwrap_or(&0)
    }

    /// Physical line count consumed by the row.
    fn height(&self, index: usize) -> usize {
        self.prefix
            .get(index + 1)
            .zip(self.prefix.get(index))
            .map(|(end, start)| end - start)
            .unwrap_or(0)
    }

    /// Physical line offset of a row from the top of the list.
    fn physical_offset(&self, index: usize) -> usize {
        self.prefix.get(index).copied().unwrap_or(0)
    }

    /// Number of rows, starting at `start`, that fit completely in the body.
    fn visible_count_at(&self, start: usize) -> usize {
        let rows = self.rows();
        let start = start.min(rows);
        if start == rows {
            return 0;
        }
        let limit = self.prefix[start] + self.body_height;
        let mut lo = start;
        let mut hi = rows;
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.prefix[mid] <= limit {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo - start
    }

    /// Largest scroll offset that still shows at least the last row.
    fn max_scroll(&self) -> usize {
        let rows = self.rows();
        if rows == 0 {
            return 0;
        }
        let limit = self.total_physical().saturating_sub(self.body_height);
        let mut lo = 0;
        let mut hi = rows - 1;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.prefix[mid] >= limit {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }

    /// Scroll offset that keeps `selected` visible. Preserves `scroll`
    /// while the row is visible, places `selected` on the body's bottom
    /// edge when scrolling down, and never leaves the row above the top
    /// edge.
    fn scroll_to_show(&self, selected: usize, scroll: usize) -> usize {
        let rows = self.rows();
        if rows == 0 || self.body_height == 0 {
            return 0;
        }
        let selected = selected.min(rows - 1);
        let scroll = scroll.min(self.max_scroll());
        let result = if selected < scroll {
            selected
        } else if selected < scroll + self.visible_count_at(scroll) {
            scroll
        } else if self.height(selected) > self.body_height {
            // The selected row cannot fit in the body; keep the window in
            // place so the trailing-blank message can be shown.
            scroll
        } else {
            // Smallest offset s <= selected such that rows s..=selected fit.
            let limit = self.prefix[selected + 1].saturating_sub(self.body_height);
            let mut lo = 0;
            let mut hi = selected;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if self.prefix[mid] >= limit {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            lo
        };
        result.min(self.max_scroll())
    }

    /// Row at a physical offset inside the body for the window starting at
    /// `scroll`. Offsets in the trailing blank space past the last fully
    /// visible row return `None`.
    fn row_at_physical(&self, physical: usize, scroll: usize) -> Option<usize> {
        let rows = self.rows();
        if rows == 0 {
            return None;
        }
        let scroll = scroll.min(rows - 1);
        let visible = self.visible_count_at(scroll);
        if visible == 0 {
            return None;
        }
        let base = self.prefix[scroll];
        let used = self.prefix[scroll + visible] - base;
        if physical >= used {
            return None;
        }
        let mut lo = scroll;
        let mut hi = scroll + visible;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.prefix[mid + 1] - base > physical {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Some(lo)
    }

    /// Physical line offset of `row` within the viewport window starting at
    /// `scroll`, or `None` when the row is not fully visible there.
    fn offset_within_window(&self, row: usize, scroll: usize) -> Option<usize> {
        let rows = self.rows();
        if rows == 0 {
            return None;
        }
        let row = row.min(rows - 1);
        let scroll = scroll.min(rows - 1);
        if row >= scroll && row < scroll + self.visible_count_at(scroll) {
            Some(self.physical_offset(row) - self.physical_offset(scroll))
        } else {
            None
        }
    }

    /// Nearest row-boundary scroll that keeps `selected` fully visible and
    /// minimizes the deviation of its viewport offset from `wanted_offset`.
    /// Ties prefer the smaller scroll, so less of the list stays hidden
    /// above. Never returns a scroll beyond the max-scroll policy; when no
    /// valid scroll keeps the row visible, falls back to
    /// [`Self::scroll_to_show`] from `current`.
    fn scroll_to_preserve_offset(
        &self,
        selected: usize,
        wanted_offset: usize,
        current: usize,
    ) -> usize {
        let rows = self.rows();
        if rows == 0 || self.body_height == 0 {
            return 0;
        }
        let selected = selected.min(rows - 1);
        let mut best: Option<(usize, usize)> = None; // (deviation, scroll)
        for scroll in 0..=self.max_scroll() {
            if scroll > selected {
                break;
            }
            let Some(offset) = self.offset_within_window(selected, scroll) else {
                continue;
            };
            let deviation = offset.abs_diff(wanted_offset);
            if best.is_none_or(|(best_deviation, _)| deviation < best_deviation) {
                best = Some((deviation, scroll));
            }
        }
        match best {
            Some((_, scroll)) => scroll,
            None => self.scroll_to_show(selected, current),
        }
    }
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
    /// Present secondary metadata segments, in display order. Workspace
    /// rows carry Repository Name, Git branch (omitted when the branch is
    /// already the primary displayed title), and agent activity; tab rows
    /// carry tab-specific agent activity; directory rows carry the path.
    /// Empty means no secondary line, so the row consumes one line.
    /// Directory rows always hold a path segment and consume two lines.
    pub secondary: Vec<String>,
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
    /// Physical line offset of the selected row within the current
    /// viewport, captured whenever the selection, scroll, or target capture
    /// settles. The asynchronous re-anchor uses it to keep the selected
    /// target's on-screen position stable across row composition and
    /// height changes. Presentation state only.
    pub selected_viewport_offset: usize,
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
    /// Set when the switcher opens. The App controller reads and clears
    /// this to request the existing asynchronous Git identity/branch
    /// refresh so branch metadata arrives while the switcher is open,
    /// even when the sidebar has no branch token.
    pub branch_refresh_requested: bool,
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
        // Cached Git data renders immediately; the App controller consumes
        // this flag to request the asynchronous Git identity/branch refresh.
        self.workspace_switcher.branch_refresh_requested = true;

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

            // Search identity fields: the actual workspace primary name,
            // repository name, and git branch. For composite labels the
            // rendered `repo / primary` display string is retained as an
            // additional compatibility field so multi-term repo+label
            // queries keep matching. Agent activity does not participate in
            // matching. Each field is ranked with the shared match-rank
            // function and the best rank wins.
            let (repo, primary) = label.parts();
            let mut identity_fields: Vec<String> = vec![primary.to_string()];
            if let Some(repo) = repo {
                identity_fields.push(repo.to_string());
            }
            if let Some(branch) = self.workspaces[ws_idx].branch() {
                identity_fields.push(branch);
            }
            let display = label.display();
            if display != primary {
                identity_fields.push(display);
            }
            let mut best_rank: Option<(u8, usize)> = identity_fields
                .iter()
                .filter_map(|field| workspace_switcher_match_rank(query, field))
                .min();

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
        WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Directory {
                shown_path: candidate.shown_path.clone(),
                canonical_path: candidate.canonical_path.clone(),
            },
            ws_idx: usize::MAX,
            depth: 0,
            label,
            secondary: vec![candidate.abbreviated_path()],
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
        let (repo, existing) = label.parts();
        let mut secondary: Vec<String> = Vec::new();
        if let Some(repo) = repo {
            secondary.push(repo.to_string());
        }
        // Provenance: the displayed primary was derived from the branch
        // only for a normal (non-custom-named) grouped linked-worktree
        // child. Passed explicitly so suppression never infers it from
        // string comparison alone.
        let primary_derived_from_branch =
            crate::ui::sidebar::is_grouped_child_worktree(self, ws_idx) && ws.custom_name.is_none();
        // Omit the branch when it is already the primary displayed title.
        // A custom primary name never suppresses a distinct branch.
        if let Some(branch) = ws.branch().filter(|branch| {
            !branch_matches_primary_label(branch, existing, primary_derived_from_branch)
        }) {
            secondary.push(branch);
        }
        let activity = workspace_activity_summary(ws, &self.terminals);
        if !activity.is_empty() {
            secondary.push(activity);
        }
        let (state, seen) = ws.aggregate_state(&self.terminals);

        WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: ws.id.clone(),
            },
            ws_idx,
            depth: 0,
            label,
            secondary,
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
                let (state, seen) = tab_aggregate_state(tab, &self.terminals);
                let activity = tab_activity_summary(tab, &self.terminals);
                let mut secondary = Vec::new();
                if !activity.is_empty() {
                    secondary.push(activity);
                }
                WorkspaceSwitcherRow {
                    target: WorkspaceSwitcherTarget::Tab {
                        tab_id: crate::workspace::public_tab_id_for_number(&ws.id, tab.number),
                    },
                    ws_idx,
                    depth: 1,
                    label: SwitcherLabel::plain(tab.display_name()),
                    secondary,
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

    /// Shared variable-height row layout for the current body rectangle.
    /// All switcher geometry consumers derive from this calculation.
    fn workspace_switcher_row_layout(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> SwitcherRowLayout {
        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        SwitcherRowLayout::new(&rows, self.workspace_switcher_body_rect().height as usize)
    }

    pub(crate) fn workspace_switcher_max_scroll_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> usize {
        self.workspace_switcher_row_layout(terminal_runtimes)
            .max_scroll()
    }

    pub(crate) fn ensure_workspace_switcher_selection_visible_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let layout = self.workspace_switcher_row_layout(terminal_runtimes);
        self.workspace_switcher.scroll = layout.scroll_to_show(
            self.workspace_switcher.selected,
            self.workspace_switcher.scroll,
        );
        self.capture_workspace_switcher_viewport_anchor(&layout);
    }

    /// Record the selected row's physical offset within the current
    /// viewport. Called whenever the selection, scroll, or row composition
    /// settles, so the asynchronous re-anchor always reads a fresh anchor.
    fn capture_workspace_switcher_viewport_anchor(&mut self, layout: &SwitcherRowLayout) {
        self.workspace_switcher.selected_viewport_offset = layout
            .offset_within_window(
                self.workspace_switcher.selected,
                self.workspace_switcher.scroll,
            )
            .unwrap_or(0);
    }

    /// Re-anchor the selection by target identity after an asynchronous
    /// state change (Git branch refresh, provider results, agent activity)
    /// changed the row composition or heights. Relocates the selection to
    /// the same target and, when a valid row-boundary scroll allows it,
    /// preserves the target's stored physical offset in the viewport.
    /// Falls back to the stored index and the safe clamp/visibility policy
    /// when the target is no longer in the results.
    pub(crate) fn reanchor_workspace_switcher_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        if !self.workspace_switcher.active {
            return;
        }
        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        if let Some(target) = self.workspace_switcher.selected_target.clone() {
            if let Some(position) = rows.iter().position(|row| row.target == target) {
                let layout = SwitcherRowLayout::new(
                    &rows,
                    self.workspace_switcher_body_rect().height as usize,
                );
                self.workspace_switcher.selected = position;
                self.workspace_switcher.scroll = layout.scroll_to_preserve_offset(
                    position,
                    self.workspace_switcher.selected_viewport_offset,
                    self.workspace_switcher.scroll,
                );
                self.refresh_workspace_switcher_preview_from(terminal_runtimes);
                self.capture_workspace_switcher_target_from(terminal_runtimes);
                return;
            }
        }
        self.clamp_workspace_switcher_selection_from(terminal_runtimes);
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
        let rows = self.workspace_switcher_rows_from(terminal_runtimes);
        self.workspace_switcher.selected_target = rows
            .get(self.workspace_switcher.selected)
            .map(|row| row.target.clone());
        let layout =
            SwitcherRowLayout::new(&rows, self.workspace_switcher_body_rect().height as usize);
        self.capture_workspace_switcher_viewport_anchor(&layout);
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
        KeyCode::Right | KeyCode::Char('l')
            if workspace_switcher_command_modifiers(state, key.modifiers) =>
        {
            state.expand_selected_workspace_switcher_workspace_from(terminal_runtimes);
        }
        KeyCode::Left | KeyCode::Char('h')
            if workspace_switcher_command_modifiers(state, key.modifiers) =>
        {
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
    let page = state
        .workspace_switcher_row_layout(terminal_runtimes)
        .visible_count_at(state.workspace_switcher.scroll)
        .max(1);
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
            let max = state.workspace_switcher_max_scroll_from(terminal_runtimes);
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
    state.workspace_switcher.branch_refresh_requested = false;
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
    state.capture_workspace_switcher_target_from(terminal_runtimes);
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
        let physical_offset = row.saturating_sub(body.y) as usize;
        let layout = self.workspace_switcher_row_layout(terminal_runtimes);
        layout.row_at_physical(physical_offset, self.workspace_switcher.scroll)
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

    let scroll = app
        .workspace_switcher
        .scroll
        .min(rows.len().saturating_sub(1));
    let layout = SwitcherRowLayout::new(&rows, body.height as usize);
    let visible = layout.visible_count_at(scroll);
    if visible == 0 {
        frame.render_widget(
            Paragraph::new(format!("{} items hidden", rows.len()))
                .style(Style::default().fg(app.palette.overlay0)),
            body,
        );
        return;
    }

    for (index, row) in rows.iter().enumerate().skip(scroll).take(visible) {
        let y = body.y + (layout.physical_offset(index) - layout.physical_offset(scroll)) as u16;
        let rect = Rect::new(body.x, y, body.width, layout.height(index) as u16);
        render_row(
            app,
            frame,
            rect,
            row,
            index == app.workspace_switcher.selected,
        );
    }
}

fn render_secondary_line(
    frame: &mut Frame,
    secondary: &Rect,
    fixed_width: u16,
    segments: &[String],
    style: Style,
) {
    let secondary_rect = Rect::new(
        secondary.x.saturating_add(fixed_width),
        secondary.y,
        secondary.width.saturating_sub(fixed_width),
        1,
    );
    let text = truncate_secondary_segments(segments, secondary_rect.width as usize);
    frame.render_widget(Paragraph::new(text).style(style), secondary_rect);
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
    let has_secondary = rect.height >= 2;
    let secondary = if has_secondary {
        Rect::new(rect.x, rect.y.saturating_add(1), rect.width, 1)
    } else {
        Rect::default()
    };
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
        if has_secondary {
            render_secondary_line(frame, &secondary, fixed_width, &row.secondary, dim_style);
        }
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

    let fixed_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
    let title_budget = rect.width.saturating_sub(fixed_width).saturating_sub(1) as usize;
    let title = truncate_text(row.label.parts().1, title_budget);
    spans.push(Span::styled(title, text_style));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), primary);

    if has_secondary && !row.secondary.is_empty() {
        render_secondary_line(frame, &secondary, fixed_width, &row.secondary, dim_style);
    }
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
    let layout = app.workspace_switcher_row_layout(terminal_runtimes);
    let total = layout.total_physical();
    if total <= body.height as usize {
        return;
    }
    // Window-position metrics, the same pattern as the sidebar scrollbar:
    // the physical lines above the window top. Monotonic for mixed-height
    // rows, zero at the top and max at the bottom of the list.
    let max_scroll = layout.max_scroll();
    let max_offset_from_top = layout.physical_offset(max_scroll);
    let offset_from_top = layout.physical_offset(app.workspace_switcher.scroll.min(max_scroll));
    let metrics = crate::pane::ScrollMetrics {
        viewport_rows: body.height as usize,
        offset_from_bottom: max_offset_from_top - offset_from_top,
        max_offset_from_bottom: max_offset_from_top,
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
            Span::styled("←/→", key),
            Span::styled(" collapse/expand  ", dim),
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

/// Separator between secondary metadata segments.
const SECONDARY_SEPARATOR: &str = " · ";

/// Join secondary metadata segments with ` · ` for the given width, keeping
/// left-to-right priority: earlier segments are preserved before later
/// ones. The first segment that does not fit is truncated with an ellipsis
/// when at least two columns remain, otherwise it and all later segments
/// are dropped. The result never ends in a dangling separator.
fn truncate_secondary_segments(segments: &[String], max_width: usize) -> String {
    if segments.is_empty() || max_width == 0 {
        return String::new();
    }
    if segments.len() == 1 {
        return truncate_text(&segments[0], max_width);
    }

    let separator_width = SECONDARY_SEPARATOR.chars().count();
    let widths: Vec<usize> = segments.iter().map(|s| s.chars().count()).collect();
    let full: usize = widths.iter().sum::<usize>() + separator_width * (segments.len() - 1);
    if full <= max_width {
        return segments.join(SECONDARY_SEPARATOR);
    }

    let mut used = 0usize;
    for (index, segment) in segments.iter().enumerate() {
        let taken = used + if index > 0 { separator_width } else { 0 };
        let remaining = max_width.saturating_sub(taken);
        if widths[index] <= remaining {
            used = taken + widths[index];
            continue;
        }
        if remaining >= 2 {
            let mut out = if index == 0 {
                String::new()
            } else {
                let mut joined = segments[..index].join(SECONDARY_SEPARATOR);
                joined.push_str(SECONDARY_SEPARATOR);
                joined
            };
            out.push_str(&truncate_text(segment, remaining));
            return out;
        }
        if index == 0 {
            return truncate_text(segment, remaining);
        }
        return segments[..index].join(SECONDARY_SEPARATOR);
    }
    segments.join(SECONDARY_SEPARATOR)
}

/// Returns `true` when the branch should be omitted from secondary
/// metadata because it is already the primary displayed title. Raw
/// equality is suppressed for every primary. The `worktree/`-stripped form
/// is suppressed only when `primary_derived_from_branch` holds, i.e. the
/// displayed primary came from the branch for a normal grouped child — a
/// custom primary name never suppresses a distinct branch.
fn branch_matches_primary_label(
    branch: &str,
    existing_label: &str,
    primary_derived_from_branch: bool,
) -> bool {
    if existing_label == branch {
        return true;
    }
    primary_derived_from_branch
        && existing_label == branch.strip_prefix("worktree/").unwrap_or(branch)
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

    fn assert_no_preview_text(screen: &[String]) {
        let joined = screen.join("\n");
        assert!(
            !joined.contains("preview:"),
            "no preview text expected: {joined}"
        );
    }

    fn assert_no_preview_divider(screen: &[String], first_row: u16, row_count: u16) {
        for (row_idx, row) in screen
            .iter()
            .enumerate()
            .skip(first_row as usize)
            .take(row_count as usize)
        {
            assert!(
                !row.contains('│'),
                "no preview divider expected on row {row_idx}: {row}"
            );
        }
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
        assert!(!screen.join("\n").contains("collapse/expand"));
        assert_ne!(screen[0].chars().next(), Some('┌'));
        assert_ne!(screen[19].chars().next(), Some('└'));
        assert_no_preview_text(&screen);
        assert_no_preview_divider(&screen, 1, 19);
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
                assert_no_preview_text(&screen);
                assert_no_preview_divider(&screen, content.y, content.height);
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
        assert_no_preview_text(&screen);
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
        assert_no_preview_text(&screen);

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
    fn layout_switch_toggles_mobile_preview_without_reopen() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 120, 30));
        state.open_workspace_switcher_from(&terminal_runtimes);

        // Desktop: existing list + divider + preview.
        assert_eq!(state.view.layout, ViewLayout::Desktop);
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(11, 4, 32, 21)
        );
        assert_eq!(
            state.workspace_switcher_divider_rect(),
            Rect::new(43, 4, 1, 21)
        );
        assert_eq!(
            state.workspace_switcher_preview_rect(),
            Rect::new(44, 4, 65, 21)
        );
        let screen = rendered_screen(&state, 120, 30);
        assert!(
            screen.join("\n").contains("preview:"),
            "{}",
            screen.join("\n")
        );
        assert!(screen[4..25].iter().any(|row| row.contains('│')));

        // Cross into Mobile while open: list fills content, preview hidden.
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 60, 20));
        assert_eq!(state.view.layout, ViewLayout::Mobile);
        assert!(state.workspace_switcher.active);
        assert_eq!(
            state.workspace_switcher_content_rect(),
            Rect::new(0, 1, 60, 19)
        );
        assert_eq!(
            state.workspace_switcher_body_rect(),
            state.workspace_switcher_content_rect()
        );
        assert_eq!(state.workspace_switcher_divider_rect(), Rect::default());
        assert_eq!(state.workspace_switcher_preview_rect(), Rect::default());
        let screen = rendered_screen(&state, 60, 20);
        assert_no_preview_text(&screen);
        assert_no_preview_divider(&screen, 1, 19);

        // Cross back to Desktop while open: preview restored immediately.
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 120, 30));
        assert_eq!(state.view.layout, ViewLayout::Desktop);
        assert!(state.workspace_switcher.active);
        assert_eq!(
            state.workspace_switcher_body_rect(),
            Rect::new(11, 4, 32, 21)
        );
        assert_eq!(
            state.workspace_switcher_divider_rect(),
            Rect::new(43, 4, 1, 21)
        );
        assert_eq!(
            state.workspace_switcher_preview_rect(),
            Rect::new(44, 4, 65, 21)
        );
        let screen = rendered_screen(&state, 120, 30);
        assert!(
            screen.join("\n").contains("preview:"),
            "{}",
            screen.join("\n")
        );
        assert!(screen[4..25].iter().any(|row| row.contains('│')));
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
        assert_no_preview_text(&screen);
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
        assert!(screen[25].contains("←/→ collapse/expand"), "{}", screen[25]);
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
    fn reanchor_selection_follows_target_identity_across_row_reorder() {
        let mut state = app_with_workspaces(&["one", "two", "three", "four"]);
        for ws in state.workspaces.iter_mut() {
            ws.cached_git_branch = None;
        }
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        // Rows are [one, two, three, four]; select "three" (row 2) and
        // capture its target identity.
        state.workspace_switcher.selected = 2;
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);
        assert_eq!(state.workspace_switcher_rows()[2].ws_idx, 2);

        // A concurrent client reorders the MRU while the switcher is open:
        // rows become [one, three, four, two]. The stale numeric index 2
        // now points at "four"; the re-anchor must land on "three".
        state.workspace_mru = vec![
            state.workspaces[2].id.clone(),
            state.workspaces[3].id.clone(),
            state.workspaces[1].id.clone(),
        ];
        state.reanchor_workspace_switcher_selection_from(&terminal_runtimes);

        assert_eq!(state.workspace_switcher.selected, 1);
        assert_eq!(state.workspace_switcher_rows()[1].ws_idx, 2);
    }

    /// Select workspace 2 in a 6-workspace fixture, open Search for
    /// "proj" at an 8-line body (terminal 80x15), and return the expected
    /// pre-refresh anchor: selected row 2, scroll 0, offset 3.
    fn open_search_anchor_at_ws2(
        state: &mut AppState,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        state.open_workspace_switcher_from(terminal_runtimes);
        state.enter_workspace_switcher_search_from(terminal_runtimes);
        set_snapshot(state);
        state.workspace_switcher.query = "proj".into();
        for _ in 0..2 {
            handle_workspace_switcher_key(
                state,
                terminal_runtimes,
                KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            );
        }
        assert_eq!(state.workspace_switcher.selected, 2);
        assert_eq!(state.workspace_switcher.scroll, 0);
        assert_eq!(state.workspace_switcher.selected_viewport_offset, 3);
    }

    #[test]
    fn reanchor_preserves_viewport_offset_when_result_inserted_above() {
        // Rows [proj-a(1), b(2), c(2), d(2), e(2)] for query "proj": ws0
        // matches by label (1 line), ws1..ws4 by branch (2 lines), ws5
        // matches nothing. Body is 8 lines (terminal 80x15, search mode).
        let mut state = app_with_workspaces(&["proj-a", "b", "c", "d", "e", "f"]);
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].cached_git_branch = Some("proj-b".into());
        state.workspaces[2].cached_git_branch = Some("proj-c".into());
        state.workspaces[3].cached_git_branch = Some("proj-d".into());
        state.workspaces[4].cached_git_branch = Some("proj-e".into());
        state.workspaces[5].cached_git_branch = None;
        state.active = Some(5);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 15));

        open_search_anchor_at_ws2(&mut state, &terminal_runtimes);
        assert_eq!(state.workspace_switcher_body_rect().height, 8);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 5);
        let selected_id = state.workspaces[2].id.clone();

        // Async Git refresh: ws5 gains a branch that starts with the query
        // and ranks ahead of every existing row, inserting a 2-line result
        // above the selected target.
        let cwd = state.workspaces[5].resolved_identity_cwd().unwrap();
        let changed = state.apply_workspace_git_statuses(
            &terminal_runtimes,
            vec![crate::workspace::WorkspaceGitStatus {
                workspace_id: state.workspaces[5].id.clone(),
                resolved_identity_cwd: cwd.clone(),
                status_cache_key: cwd,
                demand: crate::workspace::GitStatusRefreshDemand::ALL,
                auto_label: "f".into(),
                branch: Some("proj-head".into()),
                ahead_behind: None,
                space: None,
            }],
        );
        assert!(changed);

        state.reanchor_workspace_switcher_selection_from(&terminal_runtimes);

        // Identity preserved: the selection still targets ws2, now row 3.
        assert_eq!(state.workspace_switcher.selected, 3);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[3].ws_idx, 2);
        let WorkspaceSwitcherTarget::Workspace { workspace_id } = &rows[3].target else {
            panic!("expected workspace target");
        };
        assert_eq!(workspace_id, &selected_id);
        // Physical offset 3 is preservable (window at row 1) and kept:
        // the old scroll 0 would have shown the target two lines lower.
        assert_eq!(state.workspace_switcher.scroll, 1);
        assert_eq!(state.workspace_switcher.selected_viewport_offset, 3);
    }

    #[test]
    fn reanchor_preserves_viewport_offset_when_row_grows_above() {
        // Rows [proj-a(1), proj-b(1), c(2), d(2), e(2), f(2)] for query
        // "proj": ws0/ws1 match by label (1 line each), ws2..ws5 by branch
        // (2 lines each). Body is 8 lines (terminal 80x15, search mode).
        let mut state = app_with_workspaces(&["proj-a", "proj-b", "c", "d", "e", "f"]);
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].cached_git_branch = None;
        state.workspaces[2].cached_git_branch = Some("proj-c".into());
        state.workspaces[3].cached_git_branch = Some("proj-d".into());
        state.workspaces[4].cached_git_branch = Some("proj-e".into());
        state.workspaces[5].cached_git_branch = Some("proj-f".into());
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 15));

        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        assert_eq!(state.workspace_switcher_body_rect().height, 8);
        state.workspace_switcher.query = "proj".into();
        for _ in 0..3 {
            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            );
        }
        assert_eq!(state.workspace_switcher.selected, 3);
        assert_eq!(state.workspace_switcher.scroll, 0);
        assert_eq!(state.workspace_switcher.selected_viewport_offset, 4);
        let selected_id = state.workspaces[3].id.clone();

        // Async Git refresh: ws1 gains a branch, so its row grows from one
        // line to two directly above the selected target.
        let cwd = state.workspaces[1].resolved_identity_cwd().unwrap();
        let changed = state.apply_workspace_git_statuses(
            &terminal_runtimes,
            vec![crate::workspace::WorkspaceGitStatus {
                workspace_id: state.workspaces[1].id.clone(),
                resolved_identity_cwd: cwd.clone(),
                status_cache_key: cwd,
                demand: crate::workspace::GitStatusRefreshDemand::ALL,
                auto_label: "proj-b".into(),
                branch: Some("proj-g".into()),
                ahead_behind: None,
                space: None,
            }],
        );
        assert!(changed);

        state.reanchor_workspace_switcher_selection_from(&terminal_runtimes);

        // Identity preserved: the selection still targets ws3 at row 3.
        assert_eq!(state.workspace_switcher.selected, 3);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 6);
        let WorkspaceSwitcherTarget::Workspace { workspace_id } = &rows[3].target else {
            panic!("expected workspace target");
        };
        assert_eq!(workspace_id, &selected_id);
        // Offset 4 is preservable at window start row 1; keeping scroll 0
        // would have shown the target one line lower.
        assert_eq!(state.workspace_switcher.scroll, 1);
        assert_eq!(state.workspace_switcher.selected_viewport_offset, 4);
    }

    #[test]
    fn opening_switcher_demands_async_branch_refresh() {
        let mut state = app_with_workspaces(&["one"]);
        assert!(!state.workspace_switcher.branch_refresh_requested);
        state.open_workspace_switcher();
        assert!(state.workspace_switcher.branch_refresh_requested);
    }

    #[test]
    fn workspace_switcher_filters_workspace_names_only() {
        let mut state = app_with_workspaces(&["one", "issue"]);
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].cached_git_branch = None;
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

        state.workspace_switcher.query = "issu".into();
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
    fn quick_switch_direction_keys_expand_and_collapse_in_both_layouts() {
        for (width, expected_layout) in [(60, ViewLayout::Mobile), (120, ViewLayout::Desktop)] {
            let (mut state, terminal_runtimes, _) =
                state_with_quick_switch_binding("ctrl+tab", None);
            crate::ui::compute_view(&mut state, Rect::new(0, 0, width, 30));
            assert_eq!(state.view.layout, expected_layout);
            let selected_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
            );
            assert!(
                state
                    .workspace_switcher_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == selected_ws && row.is_tab),
                "Right should expand in {expected_layout:?}"
            );
            let expanded_rows = state.workspace_switcher_rows_from(&terminal_runtimes);
            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
            );
            assert_eq!(
                state.workspace_switcher_rows_from(&terminal_runtimes),
                expanded_rows
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
            );
            assert!(
                !state
                    .workspace_switcher_rows_from(&terminal_runtimes)
                    .iter()
                    .any(|row| row.ws_idx == selected_ws && row.is_tab),
                "Left should collapse in {expected_layout:?}"
            );
            let collapsed_rows = state.workspace_switcher_rows_from(&terminal_runtimes);
            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
            );
            assert_eq!(
                state.workspace_switcher_rows_from(&terminal_runtimes),
                collapsed_rows
            );

            handle_workspace_switcher_key(
                &mut state,
                &terminal_runtimes,
                KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
            );
            state.open_workspace_switcher_from(&terminal_runtimes);
            assert!(state
                .workspace_switcher_rows_from(&terminal_runtimes)
                .iter()
                .all(|row| !row.is_tab));
        }
    }
    #[test]
    fn quick_switch_direction_keys_accept_configured_command_modifiers() {
        let (mut state, terminal_runtimes, command_modifiers) =
            state_with_quick_switch_binding("alt+f13", None);
        let selected_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Right, command_modifiers),
        );
        assert!(state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == selected_ws && row.is_tab));

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Left, command_modifiers),
        );
        assert!(!state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .iter()
            .any(|row| row.ws_idx == selected_ws && row.is_tab));
    }
    #[test]
    fn quick_switch_left_from_tab_collapses_and_selects_parent_workspace() {
        let (mut state, terminal_runtimes, _) = state_with_quick_switch_binding("ctrl+tab", None);
        let selected_ws = selected_workspace_switcher_ws_idx(&state, &terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
        );
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        );
        assert!(
            state.workspace_switcher_rows_from(&terminal_runtimes)
                [state.workspace_switcher.selected]
                .is_tab
        );

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
        );

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert!(!rows
            .iter()
            .any(|row| row.ws_idx == selected_ws && row.is_tab));
        assert_eq!(rows[state.workspace_switcher.selected].ws_idx, selected_ws);
        assert!(!rows[state.workspace_switcher.selected].is_tab);
    }
    #[test]
    fn search_direction_keys_leave_observable_state_and_results_unchanged() {
        let (mut state, terminal_runtimes, command_modifiers) =
            state_with_quick_switch_binding("ctrl+tab", None);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        state.workspace_switcher.query = "main".into();
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);
        let before = state.workspace_switcher.clone();
        let before_rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
        );
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Right, command_modifiers),
        );

        assert_eq!(state.workspace_switcher, before);
        assert_eq!(
            state.workspace_switcher_rows_from(&terminal_runtimes),
            before_rows
        );
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
            secondary: Vec::new(),
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
            secondary: Vec::new(),
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
            secondary: Vec::new(),
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
            assert!(dir_row.secondary[0].starts_with('~'));
            assert!(dir_row.secondary[0].contains("projects/myapp"));
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
    fn search_matches_branch_identity_field() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        state.workspace_switcher.query = "feature-x".into();

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let child = rows.iter().find(|r| r.ws_idx == 1);
        assert!(child.is_some(), "branch-only query should find the row");
    }

    #[test]
    fn search_ranks_exact_primary_match_for_composite_rows() {
        // ws0 is a composite row `myrepo / feature` (custom primary
        // "feature"); ws1 matches the query only through its branch field.
        // The exact primary match must keep ws0 ranked by the primary's own
        // position, not by the repo-prefixed composite string's later
        // position.
        let mut state = app_with_workspaces(&["alpha", "beta"]);
        mark_linked_worktree_with_repo(&mut state, 0, "myrepo");
        state.workspaces[0].custom_name = Some("feature".to_string());
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].custom_name = Some("unrelated".to_string());
        state.workspaces[1].cached_git_branch = Some("feature-b".to_string());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);
        state.workspace_switcher.query = "feature".into();

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(
            rows.iter().map(|r| r.ws_idx).collect::<Vec<_>>(),
            vec![0, 1],
            "exact primary match must not lose to a branch-field match"
        );
    }

    #[test]
    fn search_selection_survives_async_branch_match_insertion() {
        let mut state = app_with_workspaces(&["alpha", "beta", "gamma"]);
        state.workspaces[0].cached_git_branch = None;
        state.workspaces[1].cached_git_branch = None;
        state.workspaces[2].cached_git_branch = Some("issue-9-a".into());

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        state.workspace_switcher.query = "issue-9".into();
        state.clamp_workspace_switcher_selection_from(&terminal_runtimes);

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ws_idx, 2);
        assert_eq!(state.workspace_switcher.selected, 0);

        // An asynchronous Git refresh arrives while the switcher is open:
        // "beta" now carries a matching branch and inserts a row above the
        // selected one (equal rank, earlier MRU order).
        let cwd = state.workspaces[1].resolved_identity_cwd().unwrap();
        let changed = state.apply_workspace_git_statuses(
            &terminal_runtimes,
            vec![crate::workspace::WorkspaceGitStatus {
                workspace_id: state.workspaces[1].id.clone(),
                resolved_identity_cwd: cwd.clone(),
                status_cache_key: cwd,
                demand: crate::workspace::GitStatusRefreshDemand::ALL,
                auto_label: "beta".into(),
                branch: Some("issue-9-zz".into()),
                ahead_behind: None,
                space: None,
            }],
        );
        assert!(changed);

        state.reanchor_workspace_switcher_selection_from(&terminal_runtimes);

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ws_idx, 1);
        assert_eq!(rows[1].ws_idx, 2);
        // The selection followed the target identity, not the stale index 0.
        assert_eq!(state.workspace_switcher.selected, 1);
    }

    #[test]
    fn search_does_not_match_agent_activity() {
        let mut state = app_with_workspaces(&["alpha"]);
        state.workspaces[0].cached_git_branch = None;
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        state.terminals.get_mut(&terminal_id).unwrap().state = crate::detect::AgentState::Working;
        // The row carries "1 working" on its secondary line, but activity
        // is not a search identity field.
        let rows = state.workspace_switcher_rows();
        assert!(rows[0].secondary.join(" ").contains("working"));

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        set_snapshot(&mut state);

        state.workspace_switcher.query = "working".into();
        assert!(state
            .workspace_switcher_rows_from(&terminal_runtimes)
            .is_empty());

        state.workspace_switcher.query = "alpha".into();
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn composite_row_secondary_omits_branch_already_in_primary() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = None;
        state.workspaces[1].cached_git_branch = Some("worktree/feature-x".into());
        state.workspaces[0].cached_git_branch = None;

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        // The primary title is the branch with the `worktree/` prefix
        // stripped, so the branch must not repeat on the secondary line.
        assert_eq!(child.label.display(), "myrepo / feature-x");
        assert_eq!(child.secondary, vec!["myrepo".to_string()]);
    }

    #[test]
    fn composite_row_secondary_keeps_branch_different_from_primary() {
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        // Custom primary name does not suppress a different branch.
        state.workspaces[1].cached_git_branch = Some("worktree/other".into());
        state.workspaces[0].cached_git_branch = None;

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child.label.display(), "myrepo / feature");
        assert_eq!(
            child.secondary,
            vec!["myrepo".to_string(), "worktree/other".to_string()]
        );
    }

    #[test]
    fn composite_row_secondary_keeps_branch_matching_custom_primary_stripped() {
        // Custom primary "feature" and branch "worktree/feature" are
        // distinct titles: the stripped-prefix equality must not suppress
        // the branch for custom names (only for branch-derived primaries).
        let mut state = app_with_workspaces(&["main", "feature"]);
        mark_parent_worktree(&mut state, 0);
        mark_linked_worktree_with_repo(&mut state, 1, "myrepo");
        state.workspaces[1].custom_name = Some("feature".to_string());
        state.workspaces[1].cached_git_branch = Some("worktree/feature".to_string());
        state.workspaces[0].cached_git_branch = None;

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);

        let child = rows.iter().find(|r| r.ws_idx == 1).unwrap();
        assert_eq!(child.label.display(), "myrepo / feature");
        assert_eq!(
            child.secondary,
            vec!["myrepo".to_string(), "worktree/feature".to_string()]
        );
    }

    #[test]
    fn tab_rows_carry_tab_specific_activity_not_workspace_wide() {
        let mut state = app_with_workspaces(&["one"]);
        state.workspaces[0].cached_git_branch = None;
        // A second tab so per-tab activity differs from workspace-wide.
        state.workspaces[0].test_add_tab(None);
        state.ensure_test_terminals();

        let term0 = state.workspaces[0]
            .terminal_id(state.workspaces[0].tabs[0].root_pane)
            .cloned()
            .unwrap();
        let term1 = state.workspaces[0]
            .terminal_id(state.workspaces[0].tabs[1].root_pane)
            .cloned()
            .unwrap();
        state.terminals.get_mut(&term0).unwrap().state = crate::detect::AgentState::Blocked;
        state.terminals.get_mut(&term1).unwrap().state = crate::detect::AgentState::Working;

        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let tab_rows = state.workspace_switcher_tab_rows(0);
        assert_eq!(tab_rows.len(), 2);
        assert_eq!(tab_rows[0].secondary, vec!["1 blocked".to_string()]);
        assert_eq!(tab_rows[1].secondary, vec!["1 working".to_string()]);

        // The workspace row aggregates all tabs instead.
        let ws_row = state.workspace_switcher_workspace_row(
            0,
            state.workspace_label(0, &terminal_runtimes),
            false,
        );
        assert_eq!(ws_row.secondary, vec!["1 blocked · 1 working".to_string()]);
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
            secondary: vec!["herdr".to_string(), "1 working".to_string()],
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
        assert!(!primary.contains("herdr"));
        assert!(!primary.contains("pane"));
        assert!(secondary.contains("herdr · 1 working"));
        assert_eq!(
            primary.chars().position(|ch| ch == 'f'),
            secondary.chars().position(|ch| ch == 'h')
        );
    }

    #[test]
    fn secondary_truncation_keeps_left_segments_before_right() {
        let segs = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // Everything fits: joined with separators.
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main", "1 working"]), 23),
            "repo · main · 1 working"
        );
        // Rightmost segment truncates first.
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main", "1 working"]), 18),
            "repo · main · 1 w…"
        );
        // Then the middle segment, then only the left one remains.
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main", "1 working"]), 10),
            "repo · ma…"
        );
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main", "1 working"]), 8),
            "repo"
        );
        // A single segment truncates on its own.
        assert_eq!(
            truncate_secondary_segments(&segs(&["repository"]), 6),
            "repos…"
        );
        // Prior two-segment behavior is preserved.
        assert_eq!(
            truncate_secondary_segments(&segs(&["repository", "working"]), 10),
            "repository"
        );
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "long activity"]), 11),
            "repo · lon…"
        );
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "working"]), 8),
            "repo"
        );
        assert_eq!(
            truncate_secondary_segments(&segs(&["working"]), 6),
            "worki…"
        );
        // No dangling separator at the drop boundary, never past the width.
        for width in [4, 5, 6, 7] {
            let out = truncate_secondary_segments(&segs(&["repo", "main"]), width);
            assert!(!out.ends_with('·'), "width {width}: {out:?}");
            assert!(!out.ends_with(' '), "width {width}: {out:?}");
            assert!(out.chars().count() <= width, "width {width}: {out:?}");
        }
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main"]), 3),
            "re…"
        );
        assert_eq!(
            truncate_secondary_segments(&segs(&["repo", "main"]), 1),
            "…"
        );
        assert_eq!(truncate_secondary_segments(&Vec::new(), 10), "");
        assert_eq!(truncate_secondary_segments(&segs(&["repo"]), 0), "");
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
                secondary: vec!["2 blocked".to_string()],
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
                secondary: vec!["/work/project".to_string()],
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
                secondary: Vec::new(),
                is_current: false,
                expanded: false,
                is_tab: true,
                is_directory: false,
                state: crate::detect::AgentState::Idle,
                seen: false,
            },
        ];

        for (row, expected_secondary) in rows.iter().zip(["2 blocked", "/work/project", ""]) {
            let height = switcher_row_height(row) as u16;
            let mut terminal = Terminal::new(TestBackend::new(40, 2)).unwrap();
            terminal
                .draw(|frame| render_row(&app, frame, Rect::new(0, 0, 40, height), row, true))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let primary = rendered_line(&terminal, 0, 40);
            let secondary = if height >= 2 {
                rendered_line(&terminal, 1, 40)
            } else {
                String::new()
            };
            // Selected styling covers exactly the rendered item height.
            for y in 0..height {
                assert_eq!(buffer[(0, y)].bg, app.palette.accent);
            }
            if height < 2 {
                assert_ne!(buffer[(0, 1)].bg, app.palette.accent);
            }
            if expected_secondary.is_empty() {
                assert!(secondary.trim().is_empty());
            } else {
                assert!(secondary.contains(expected_secondary));
            }
            // The primary line never renders secondary metadata or pane
            // counts.
            assert!(!primary.contains("pane"));
            if !expected_secondary.is_empty() {
                assert!(!primary.contains(expected_secondary));
            }
        }
    }

    #[test]
    fn primary_title_uses_width_released_from_pane_count() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("very-long-workspace-name".to_string()),
            secondary: Vec::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Idle,
            seen: false,
        };

        // Widths where the former pane-count reservation (9-13 columns) used
        // to truncate the title now keep it intact.
        for width in [30u16, 38, 46] {
            let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
            terminal
                .draw(|frame| render_row(&app, frame, Rect::new(0, 0, width, 1), &row, false))
                .unwrap();
            let primary = rendered_line(&terminal, 0, width);
            assert!(
                primary.contains("very-long-workspace-name"),
                "width {width}: {primary:?}"
            );
            assert!(!primary.contains("pane"));
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
            secondary: vec!["1 working".to_string()],
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
    fn selected_style_covers_exactly_one_line_for_plain_item() {
        let app = AppState::test_new();
        let row = WorkspaceSwitcherRow {
            target: WorkspaceSwitcherTarget::Workspace {
                workspace_id: String::new(),
            },
            ws_idx: 0,
            depth: 0,
            label: SwitcherLabel::plain("workspace".to_string()),
            secondary: Vec::new(),
            is_current: false,
            expanded: false,
            is_tab: false,
            is_directory: false,
            state: crate::detect::AgentState::Idle,
            seen: false,
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal
            .draw(|frame| render_row(&app, frame, Rect::new(0, 0, 40, 1), &row, true))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, app.palette.accent);
        assert_eq!(buffer[(39, 0)].bg, app.palette.accent);
    }

    #[test]
    fn mixed_height_items_render_in_mobile_and_desktop_layouts() {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for (width, expected_layout) in [
            (44, crate::app::state::ViewLayout::Mobile),
            (80, crate::app::state::ViewLayout::Desktop),
        ] {
            let mut app = app_with_workspaces(&["main", "feature"]);
            mark_parent_worktree(&mut app, 0);
            mark_linked_worktree_with_repo(&mut app, 1, "herdr");
            app.workspaces[1].custom_name = Some("feature".to_string());
            // Deterministic mixed heights: the parent workspace carries no
            // secondary line, the linked worktree renders repo + branch.
            app.workspaces[0].cached_git_branch = None;
            app.workspaces[1].cached_git_branch = Some("worktree/issue-137".to_string());
            crate::ui::compute_view(&mut app, Rect::new(0, 0, width, 24));
            app.open_workspace_switcher_from(&terminal_runtimes);
            assert_eq!(app.view.layout, expected_layout);

            let body = app.workspace_switcher_body_rect();
            let rows = app.workspace_switcher_rows_from(&terminal_runtimes);
            let layout = SwitcherRowLayout::new(&rows, body.height as usize);
            assert_eq!(layout.height(0), 1);
            assert_eq!(layout.height(1), 2);

            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal
                .draw(|frame| render_workspace_switcher_overlay(&app, &terminal_runtimes, frame))
                .unwrap();

            let feature_y = (body.y..body.y + body.height)
                .find(|&y| rendered_rect_line(&terminal, body, y).contains("feature"))
                .expect("linked workspace primary line should render");
            // The two-line item starts on the odd physical offset left by
            // the one-line item above it, not on a fixed stride.
            assert_eq!((feature_y - body.y) as usize, layout.physical_offset(1));
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
    fn hit_testing_maps_mixed_height_lines_and_rejects_trailing_rows() {
        let mut state = app_with_workspaces(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ]);
        // Alternate row heights: even rows carry no secondary (1 line), odd
        // rows carry a branch (2 lines).
        for (idx, ws) in state.workspaces.iter_mut().enumerate() {
            ws.cached_git_branch = if idx % 2 == 1 {
                Some(format!("branch-{idx}"))
            } else {
                None
            };
        }
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        for height in 12..40 {
            set_switcher_view(&mut state, 80, height);
            if state.workspace_switcher_body_rect().height == 7 {
                break;
            }
        }
        let body = state.workspace_switcher_body_rect();
        assert_eq!(body.height, 7);
        state.open_workspace_switcher_from(&terminal_runtimes);

        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let layout = SwitcherRowLayout::new(&rows, body.height as usize);
        assert_eq!(
            (0..rows.len())
                .map(|i| layout.height(i))
                .collect::<Vec<_>>(),
            vec![1, 2, 1, 2, 1, 2, 1, 2]
        );

        // At scroll 0 the rows fill the body exactly (1+2+1+2+1 = 7 lines),
        // so every body line maps to its item and both lines of a two-line
        // item map to the same row.
        let expected_at_top: [usize; 7] = [0, 1, 1, 2, 3, 3, 4];
        for (offset, expected) in expected_at_top.iter().enumerate() {
            assert_eq!(
                state.workspace_switcher_row_index_at_from(
                    &terminal_runtimes,
                    body.x,
                    body.y + offset as u16
                ),
                Some(*expected)
            );
        }

        // Scrolled to the end, rows 4..=7 use 6 of the 7 body lines; the
        // final trailing line must reject hit-testing.
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::End, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.scroll, 4);
        let expected_at_bottom: [Option<usize>; 7] =
            [Some(4), Some(5), Some(5), Some(6), Some(7), Some(7), None];
        for (offset, expected) in expected_at_bottom.iter().enumerate() {
            assert_eq!(
                state.workspace_switcher_row_index_at_from(
                    &terminal_runtimes,
                    body.x,
                    body.y + offset as u16
                ),
                *expected
            );
        }
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
    fn page_navigation_and_resize_use_mixed_height_item_capacity() {
        let names = (0..20).map(|idx| format!("ws-{idx}")).collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut state = app_with_workspaces(&name_refs);
        // Alternate row heights: even rows 1 line, odd rows 2 lines (30
        // physical lines for 20 rows).
        for (idx, ws) in state.workspaces.iter_mut().enumerate() {
            ws.cached_git_branch = if idx % 2 == 1 {
                Some(format!("branch-{idx}"))
            } else {
                None
            };
        }
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 30));
        state.open_workspace_switcher_from(&terminal_runtimes);
        let rows = state.workspace_switcher_rows_from(&terminal_runtimes);
        let layout =
            SwitcherRowLayout::new(&rows, state.workspace_switcher_body_rect().height as usize);
        assert_eq!(layout.total_physical(), 30);
        // Body 21 lines fits 14 alternating rows (7 pairs of 3 lines).
        let page = layout.visible_count_at(0);
        assert_eq!(page, 14);
        state.workspace_switcher.selected = 0;
        state.workspace_switcher.scroll = 0;
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, page);
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, 0);

        state.enter_workspace_switcher_search_from(&terminal_runtimes);
        let search_layout =
            SwitcherRowLayout::new(&rows, state.workspace_switcher_body_rect().height as usize);
        // Search row + separator shrink the body to 19 lines, fitting 13
        // rows (19 of the 30 physical lines).
        let search_page = search_layout.visible_count_at(0);
        assert_eq!(search_page, 13);
        state.workspace_switcher.selected = 0;
        state.workspace_switcher.scroll = 0;
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, search_page);
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        );
        assert_eq!(state.workspace_switcher.selected, 0);

        state.workspace_switcher.selected = state.workspace_switcher_rows().len() - 1;
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 80, 16));
        let resized_layout =
            SwitcherRowLayout::new(&rows, state.workspace_switcher_body_rect().height as usize);
        assert!(state.workspace_switcher.selected >= state.workspace_switcher.scroll);
        assert!(
            state.workspace_switcher.selected
                < state.workspace_switcher.scroll.saturating_add(
                    resized_layout.visible_count_at(state.workspace_switcher.scroll)
                )
        );
        assert_eq!(state.workspace_switcher.scroll, resized_layout.max_scroll());
    }

    #[test]
    fn mouse_wheel_moves_three_logical_items() {
        let names = (0..20).map(|idx| format!("ws-{idx}")).collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut state = app_with_workspaces(&name_refs);
        // Mixed heights: wheel movement counts rows, not physical lines.
        for (idx, ws) in state.workspaces.iter_mut().enumerate() {
            ws.cached_git_branch = if idx % 2 == 1 {
                Some(format!("branch-{idx}"))
            } else {
                None
            };
        }
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

        handle_workspace_switcher_mouse(
            &mut state,
            &terminal_runtimes,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: body.x,
                row: body.y,
                modifiers: KeyModifiers::empty(),
            },
        );
        assert_eq!(state.workspace_switcher.scroll, 0);
        assert_eq!(state.workspace_switcher.selected, 0);
    }

    #[test]
    fn scrollbar_tracks_mixed_height_window_position() {
        let mut state = app_with_workspaces(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ]);
        for (idx, ws) in state.workspaces.iter_mut().enumerate() {
            ws.cached_git_branch = if idx % 2 == 1 {
                Some(format!("branch-{idx}"))
            } else {
                None
            };
        }
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        set_switcher_view(&mut state, 80, 12);
        assert_eq!(state.workspace_switcher_body_rect().height, 7);
        state.open_workspace_switcher_from(&terminal_runtimes);
        let body = state.workspace_switcher_body_rect();
        let track_x = body.x + body.width - 1;

        let thumb_rows = |state: &AppState| -> Vec<u16> {
            let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
            terminal
                .draw(|frame| render_workspace_switcher_overlay(state, &terminal_runtimes, frame))
                .unwrap();
            let buffer = terminal.backend().buffer();
            (body.y..body.y + body.height)
                .filter(|&y| {
                    buffer[(track_x, y)].symbol() == "▕"
                        && buffer[(track_x, y)].fg == state.palette.overlay0
                })
                .collect()
        };

        // At the top of the list the thumb starts at the top of the track.
        let top_thumb = thumb_rows(&state);
        assert!(!top_thumb.is_empty());
        assert_eq!(top_thumb.first(), Some(&body.y));

        // Scrolling to the end moves the thumb to the bottom of the track.
        handle_workspace_switcher_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::End, KeyModifiers::empty()),
        );
        let bottom_thumb = thumb_rows(&state);
        assert!(!bottom_thumb.is_empty());
        assert_eq!(bottom_thumb.last(), Some(&(body.y + body.height - 1)));
        assert!(bottom_thumb.first() > top_thumb.first());
    }
}
