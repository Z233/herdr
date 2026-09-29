use super::*;
use crate::api::schema::PaneTextPoint;

const LABELS: &str = "fjdkslgha;rueiwotyqpvbcnxmzFJDKSLGHARUEIWOTYQPVBCNXMZ";

pub(super) enum CopyModeInitialAction {
    EasyMotion,
    ScrollUp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EasyMotionState {
    pub(super) query: String,
    pub(super) labels: Vec<(char, PaneTextPoint)>,
    endpoint_id: ClientEndpointId,
    connection_generation: Option<u64>,
    boot_id: String,
    session_generation: u64,
    content_revision: u64,
    geometry: (u16, u16),
    viewport_top: u32,
    cursor: PaneTextPoint,
    selection: Option<ClientCopySelection>,
}

impl ClientShellState {
    pub(super) fn enter_copy_mode_with_initial_action(
        &mut self,
        action: CopyModeInitialAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.enter_copy_mode(outcome) {
            return false;
        }
        match action {
            CopyModeInitialAction::EasyMotion => self.begin_copy_easymotion(),
            CopyModeInitialAction::ScrollUp => self.move_copy_page(-1, true, outcome),
        }
        outcome.repaint = true;
        true
    }

    pub(super) fn begin_copy_easymotion(&mut self) {
        let (Some(copy), Some(surface)) = (self.copy_mode.as_mut(), self.pane_surface.as_ref())
        else {
            return;
        };
        copy.easymotion = Some(EasyMotionState {
            query: String::new(),
            labels: Vec::new(),
            endpoint_id: self.active_endpoint_id.clone(),
            connection_generation: self.active_snapshot_generation,
            boot_id: surface.boot_id.clone(),
            session_generation: self.copy_session_generation,
            content_revision: copy.content_revision,
            geometry: copy.geometry,
            viewport_top: viewport_top(copy),
            cursor: copy.cursor,
            selection: copy.selection,
        });
    }

    pub(super) fn route_copy_easymotion_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(mut motion) = self
            .copy_mode
            .as_mut()
            .and_then(|copy| copy.easymotion.take())
        else {
            return false;
        };
        outcome.repaint = true;
        let Some(copy) = self.copy_mode.as_ref() else {
            return true;
        };
        if motion.endpoint_id != self.active_endpoint_id
            || motion.connection_generation != self.active_snapshot_generation
            || motion.session_generation != self.copy_session_generation
            || motion.content_revision != copy.content_revision
            || motion.geometry != copy.geometry
            || motion.viewport_top != viewport_top(copy)
            || motion.cursor != copy.cursor
            || motion.selection != copy.selection
            || self.pane_surface.as_ref().is_none_or(|surface| {
                surface.boot_id != motion.boot_id
                    || !surface.panes.iter().any(|pane| {
                        pane.pane_id == copy.pane_id
                            && pane.content_revision == motion.content_revision
                            && (pane.inner_rect.width, pane.inner_rect.height) == motion.geometry
                    })
            })
        {
            return true;
        }
        let Some(ch) = crate::copy_mode::copy_mode_command_char(key.clone()) else {
            return true;
        };
        if motion.query.chars().count() == 2 {
            if let Some((_, point)) = motion.labels.iter().find(|(label, _)| *label == ch) {
                if let Some(copy) = self.copy_mode.as_mut() {
                    copy.cursor = *point;
                }
                self.sync_copy_selection();
                return true;
            }
        } else {
            motion.query.push(ch);
            if motion.query.chars().count() == 2 {
                if let Some(surface) = self.pane_surface.as_ref() {
                    motion.labels = find_matches(surface, copy, &motion.query);
                }
            }
        }
        if let Some(copy) = self.copy_mode.as_mut() {
            copy.easymotion = Some(motion);
        }
        true
    }
}

fn viewport_top(copy: &ClientCopyModeState) -> u32 {
    copy.max_offset_from_bottom
        .saturating_sub(copy.offset_from_bottom)
        .min(u32::MAX as usize) as u32
}

fn find_matches(
    surface: &PaneSurfaceFrame,
    copy: &ClientCopyModeState,
    query: &str,
) -> Vec<(char, PaneTextPoint)> {
    let Some(pane) = surface
        .panes
        .iter()
        .find(|pane| pane.pane_id == copy.pane_id)
    else {
        return Vec::new();
    };
    let Some(buffer) = surface.frame.to_ratatui_buffer() else {
        return Vec::new();
    };
    let mut chars = query.chars();
    let (Some(first), Some(second)) = (chars.next(), chars.next()) else {
        return Vec::new();
    };
    let sensitive = first.is_uppercase() || second.is_uppercase();
    let equal = |actual: char, target: char| {
        if sensitive {
            actual == target
        } else {
            actual.to_lowercase().eq(target.to_lowercase())
        }
    };
    let mut labels = LABELS.chars();
    let mut matches = Vec::new();
    let rect = pane.inner_rect;
    for row in 0..rect.height {
        let mut previous = None;
        let mut col = 0;
        while col < rect.width {
            let Some(cell) = buffer.cell((rect.x.saturating_add(col), rect.y.saturating_add(row)))
            else {
                break;
            };
            for ch in cell.symbol().chars() {
                if let Some((prev, start)) = previous {
                    if equal(prev, first) && equal(ch, second) {
                        let Some(label) = labels.next() else {
                            return matches;
                        };
                        matches.push((
                            label,
                            PaneTextPoint {
                                row: viewport_top(copy).saturating_add(u32::from(row)),
                                col: start,
                            },
                        ));
                    }
                }
                previous = Some((ch, col));
            }
            col = col.saturating_add(
                u16::try_from(cell.symbol().width())
                    .unwrap_or(u16::MAX)
                    .max(1),
            );
        }
    }
    matches
}
