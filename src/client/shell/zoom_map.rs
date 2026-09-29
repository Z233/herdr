use super::*;
use crate::api::schema::{PaneLayoutRect, PaneLayoutSnapshot};

pub(super) const WIDTH: u16 = 9;

#[derive(Default)]
pub(super) struct ZoomMapState {
    layout: Option<PaneLayoutSnapshot>,
    slots: [Option<String>; 5],
    pub(super) requested_at: Option<std::time::Instant>,
}

impl ZoomMapState {
    pub(super) fn set_layout(&mut self, layout: PaneLayoutSnapshot) {
        use crate::layout::NavDirection::{Down, Left, Right, Up};
        self.slots = [
            neighbor(&layout, Up),
            neighbor(&layout, Left),
            Some(layout.focused_pane_id.clone()),
            neighbor(&layout, Right),
            neighbor(&layout, Down),
        ];
        self.layout = Some(layout);
    }

    pub(super) fn available(&self, snapshot: &ClientShellSnapshot) -> bool {
        self.layout.as_ref().is_some_and(|layout| {
            layout.zoomed
                && snapshot.focused_tab_id.as_deref() == Some(layout.tab_id.as_str())
                && snapshot.focused_pane_id.as_deref() == Some(layout.focused_pane_id.as_str())
                && snapshot
                    .tabs
                    .iter()
                    .any(|tab| tab.tab_id == layout.tab_id && tab.zoomed)
        })
    }

    pub(super) fn render(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        endpoints: &[ClientShellEndpoint],
        endpoint_id: &ClientEndpointId,
        config: &ClientShellConfig,
        hits: &mut ShellHitMap,
    ) {
        if area.width < WIDTH || area.height == 0 {
            return;
        }
        let palette = &config.palette;
        super::render::put_text(
            buffer,
            area.x,
            area.y,
            WIDTH,
            "         ",
            Style::default().bg(palette.panel_bg),
        );
        for ((offset, label), pane_id) in [(0, "k"), (2, "h"), (3, "[@]"), (6, "l"), (8, "j")]
            .into_iter()
            .zip(&self.slots)
        {
            let Some(pane_id) = pane_id else {
                continue;
            };
            let status = super::render::endpoint_pane_agent_status(endpoints, endpoint_id, pane_id)
                .unwrap_or(crate::api::schema::AgentStatus::Unknown);
            let rect = Rect::new(area.x + offset, area.y, label.len() as u16, 1);
            super::render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                label,
                Style::default()
                    .fg(status_color(status, palette))
                    .bg(palette.panel_bg)
                    .add_modifier(if offset == 3 {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
            hits.zoom_map_panes.push((rect, pane_id.clone()));
        }
    }
}

fn neighbor(layout: &PaneLayoutSnapshot, direction: crate::layout::NavDirection) -> Option<String> {
    use crate::layout::NavDirection::{Down, Left, Right, Up};
    let focused = layout
        .panes
        .iter()
        .find(|pane| pane.pane_id == layout.focused_pane_id)?;
    let axes = |rect: PaneLayoutRect| match direction {
        Left | Right => (
            u32::from(rect.x),
            u32::from(rect.width),
            u32::from(rect.y),
            u32::from(rect.height),
        ),
        Up | Down => (
            u32::from(rect.y),
            u32::from(rect.height),
            u32::from(rect.x),
            u32::from(rect.width),
        ),
    };
    let (start, len, cross, cross_len) = axes(focused.rect);
    layout
        .panes
        .iter()
        .enumerate()
        .filter_map(|(index, pane)| {
            if pane.pane_id == focused.pane_id {
                return None;
            }
            let (other, other_len, other_cross, other_cross_len) = axes(pane.rect);
            let gap = match direction {
                Left | Up => start.checked_sub(other + other_len)?,
                Right | Down => other.checked_sub(start + len)?,
            };
            let overlap = (cross + cross_len)
                .min(other_cross + other_cross_len)
                .saturating_sub(cross.max(other_cross));
            if overlap == 0 {
                return None;
            }
            let center_distance =
                (cross * 2 + cross_len).abs_diff(other_cross * 2 + other_cross_len);
            Some((
                (gap, std::cmp::Reverse(overlap), center_distance, index),
                &pane.pane_id,
            ))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, pane_id)| pane_id.clone())
}
