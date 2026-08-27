use crossterm::event::KeyCode;

use crate::{
    app::state::{
        AppState, EasyMotionMatch, EasyMotionState, EASYMOTION_LABELS, EASYMOTION_MAX_MATCHES,
    },
    input::TerminalKey,
    terminal::TerminalRuntimeRegistry,
};

use super::{CopyModeNotice, FrozenCopyView};

impl AppState {
    pub(crate) fn begin_copy_mode_easymotion(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) {
        let Some(copy_mode) = self.copy_mode.as_ref() else {
            return;
        };
        let pane_id = copy_mode.pane_id;
        let mut captured = false;
        if self.fork_features.frozen_copy_view.is_none() {
            let Some(info) = self.pane_info_by_id(pane_id).cloned() else {
                return;
            };
            let snapshot = self
                .active
                .and_then(|ws_idx| {
                    self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
                })
                .and_then(|runtime| {
                    runtime.visible_cell_snapshot(info.inner_rect.width, info.inner_rect.height)
                });
            let Some(snapshot) = snapshot else {
                self.fork_features.easymotion = None;
                self.fork_features.copy_mode_notice = Some(CopyModeNotice::SnapshotFailed);
                return;
            };
            self.fork_features.frozen_copy_view = Some(FrozenCopyView {
                pane_id,
                cells: std::sync::Arc::new(snapshot),
            });
            captured = true;
        }

        if captured {
            if let (Some(frozen), Some(copy_mode)) = (
                self.fork_features.frozen_copy_view.as_ref(),
                self.copy_mode.as_mut(),
            ) {
                if !copy_mode.search.query.is_empty() {
                    let previous = copy_mode
                        .search
                        .current
                        .and_then(|index| copy_mode.search.matches.get(index).copied());
                    let matches = frozen.cells.search_text_matches(
                        &copy_mode.search.query,
                        copy_mode.search.query.chars().any(char::is_uppercase),
                    );
                    copy_mode.search.current = previous.and_then(|previous| {
                        matches.iter().position(|candidate| {
                            candidate.start == previous.start && candidate.end == previous.end
                        })
                    });
                    copy_mode.search.matches = matches;
                }
            }
        }

        let anchor_outside = self
            .selection
            .as_ref()
            .zip(self.fork_features.frozen_copy_view.as_ref())
            .is_some_and(|(selection, frozen)| {
                selection.pane_id != pane_id
                    || !frozen.cells.contains_absolute_cell(
                        selection.anchor_cell().0,
                        selection.anchor_cell().1,
                    )
            });
        if anchor_outside {
            self.clear_copy_mode_selection();
            self.fork_features.copy_mode_notice = Some(CopyModeNotice::SelectionOutsideSnapshot);
        } else if self.fork_features.copy_mode_notice == Some(CopyModeNotice::SnapshotFailed) {
            self.fork_features.copy_mode_notice = None;
        }
        self.fork_features.easymotion = Some(EasyMotionState::new());
    }

    fn cancel_copy_mode_easymotion(&mut self) {
        self.fork_features.easymotion = None;
    }

    pub(crate) fn handle_copy_mode_easymotion_key(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        key: TerminalKey,
    ) {
        if key.code == KeyCode::Esc {
            self.cancel_copy_mode_easymotion();
            return;
        }

        let Some(ch) = super::copy_mode_command_char(key) else {
            return;
        };
        if ch == 'q' {
            self.cancel_copy_mode_easymotion();
            return;
        }

        let Some(mut copy_mode) = self.copy_mode.clone() else {
            return;
        };
        let Some(mut easymotion) = self.fork_features.easymotion else {
            return;
        };

        if easymotion.target().is_some() {
            if let Some(target) = easymotion
                .labels
                .iter()
                .take(usize::from(easymotion.label_count))
                .flatten()
                .find(|target| target.label == ch)
                .copied()
            {
                copy_mode.cursor_row = target.row;
                copy_mode.cursor_col = target.col;
                self.fork_features.easymotion = None;
                self.copy_mode = Some(copy_mode);
                self.sync_copy_mode_selection(terminal_runtimes);
            }
            return;
        }

        easymotion.push_query_char(ch);
        if self.fork_features.copy_mode_notice == Some(CopyModeNotice::SelectionOutsideSnapshot) {
            self.fork_features.copy_mode_notice = None;
        }
        let query_complete = easymotion.target().is_some();
        self.fork_features.easymotion = Some(easymotion);
        self.copy_mode = Some(copy_mode);

        if query_complete {
            self.rebuild_copy_mode_easymotion_matches(terminal_runtimes);
        }
    }

    fn rebuild_copy_mode_easymotion_matches(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) {
        let Some(pane_id) = self.copy_mode.as_ref().map(|copy_mode| copy_mode.pane_id) else {
            return;
        };
        let Some(mut easymotion) = self.fork_features.easymotion else {
            return;
        };
        let Some((first, second)) = easymotion.target() else {
            return;
        };
        let Some(info) = self.pane_info_by_id(pane_id).cloned() else {
            self.cancel_copy_mode(terminal_runtimes);
            return;
        };

        easymotion.labels = [None; EASYMOTION_MAX_MATCHES];
        easymotion.label_count = 0;
        easymotion.case_sensitive = easymotion_query_is_case_sensitive(first, second);

        for row in 0..info.inner_rect.height {
            let Some(text) = self.copy_mode_visible_row_text(terminal_runtimes, row) else {
                continue;
            };
            append_easymotion_row_matches(&text, row, first, second, &mut easymotion);
            if usize::from(easymotion.label_count) >= EASYMOTION_MAX_MATCHES {
                break;
            }
        }

        self.fork_features.easymotion = Some(easymotion);
    }
}

pub(crate) fn append_easymotion_row_matches(
    text: &str,
    row: u16,
    first: char,
    second: char,
    easymotion: &mut EasyMotionState,
) {
    let mut chars = text.chars().peekable();
    let mut col = 0u16;

    while let Some(ch) = chars.next() {
        if usize::from(easymotion.label_count) >= EASYMOTION_MAX_MATCHES {
            break;
        }

        if let Some(next_ch) = chars.peek().copied() {
            let matches = easymotion_chars_equal(ch, first, easymotion.case_sensitive)
                && easymotion_chars_equal(next_ch, second, easymotion.case_sensitive);
            if matches {
                let label_idx = usize::from(easymotion.label_count);
                if let Some(label) = easymotion_label_at(label_idx) {
                    easymotion.labels[label_idx] = Some(EasyMotionMatch { label, row, col });
                    easymotion.label_count = easymotion.label_count.saturating_add(1);
                }
            }
        }

        col = col.saturating_add(char_cell_width(ch));
    }
}

fn char_cell_width(ch: char) -> u16 {
    u16::from(crate::ghostty::unicode_codepoint_width(ch as u32)).max(1)
}

fn easymotion_label_at(index: usize) -> Option<char> {
    if index >= EASYMOTION_MAX_MATCHES {
        return None;
    }
    EASYMOTION_LABELS.chars().nth(index)
}

fn easymotion_query_is_case_sensitive(first: char, second: char) -> bool {
    first.is_uppercase() || second.is_uppercase()
}

fn easymotion_chars_equal(actual: char, target: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        actual == target
    } else {
        actual.to_lowercase().eq(target.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easymotion_match_columns_follow_copy_mode_widths() {
        let prefix = "界\u{301}\u{fe0f}";
        let mut easymotion = EasyMotionState::new();

        append_easymotion_row_matches(&format!("{prefix}th"), 0, 't', 'h', &mut easymotion);

        let expected_col = prefix.chars().fold(0u16, |col, ch| {
            col.saturating_add(u16::from(crate::ghostty::unicode_codepoint_width(ch as u32)).max(1))
        });
        assert_eq!(
            easymotion.labels[0].expect("matching label").col,
            expected_col
        );
    }
}
