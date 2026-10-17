use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

const MOBILE_EDGE_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, PartialEq, Eq)]
struct MobileSelection {
    target: ClientMobileTarget,
    boot_id: String,
    generation: Option<u64>,
    workspace_id: String,
}

pub(super) struct MobileSwitchGesture {
    anchor: MobileSelection,
    selected: MobileSelection,
    origin: Option<(u16, u16)>,
    horizontal_origin: (u16, u16),
    last_point: Option<(u16, u16)>,
    effective: bool,
    edge: Option<(isize, Instant)>,
}

impl ClientShellState {
    fn mobile_edge_direction(&self, point: (u16, u16)) -> Option<isize> {
        if self.overlay.is_some()
            || self.hits.mobile_close.is_empty()
            || self.hits.mobile_max_scroll == 0
        {
            return None;
        }
        let top = self.hits.mobile_close.bottom().saturating_add(1);
        let bottom = self
            .hits
            .mobile_targets
            .iter()
            .map(|(rect, _)| rect.bottom())
            .max()?;
        let (content, _) = self.hits.mobile_targets.first()?;
        if point.0 < content.x || point.0 >= content.right() || point.1 < top || point.1 >= bottom {
            return None;
        }
        if point.1 <= top.saturating_add(1) {
            Some(-1)
        } else if point.1 >= bottom.saturating_sub(2) {
            Some(1)
        } else {
            None
        }
    }

    fn project_mobile_gesture(&mut self, gesture: &MobileSwitchGesture) {
        if gesture.effective {
            self.mobile_drag = Some(gesture.selected.target.clone());
            if let Some((endpoint_id, workspace_id)) = self.mobile_parent(&gesture.selected.target)
            {
                self.navigate_workspace_id = self.navigation_target(&endpoint_id, &workspace_id);
                self.reveal_mobile_workspace = true;
            }
        }
    }

    pub(crate) fn tick_mobile_gesture(&mut self, now: Instant) -> bool {
        let Some((direction, deadline)) =
            self.mobile_hold.as_ref().and_then(|gesture| gesture.edge)
        else {
            return false;
        };
        if now < deadline {
            return false;
        }
        self.reconcile_mobile_gesture();
        let Some(mut gesture) = self.mobile_hold.take() else {
            return true;
        };
        if gesture
            .last_point
            .and_then(|point| self.mobile_edge_direction(point))
            != Some(direction)
        {
            gesture.edge = None;
            self.mobile_hold = Some(gesture);
            return false;
        }
        gesture.edge = Some((direction, now + MOBILE_EDGE_INTERVAL));
        let targets = self.mobile_navigation_targets();
        let selected = targets
            .iter()
            .position(|target| *target == gesture.selected.target)
            .and_then(|mut index| loop {
                index = index.checked_add_signed(direction)?;
                let target = targets.get(index)?;
                if let Some(selected) = self.mobile_selection(target.clone()) {
                    break Some(selected);
                }
            });
        let changed = selected.is_some();
        if let Some(selected) = selected {
            gesture.selected = selected;
            gesture.anchor = gesture.selected.clone();
            gesture.origin = gesture.last_point;
            if let Some(point) = gesture.last_point {
                gesture.horizontal_origin = point;
            }
            gesture.effective = true;
            self.project_mobile_gesture(&gesture);
        }
        self.mobile_hold = Some(gesture);
        changed
    }

    pub(super) fn arm_mobile_gesture(&mut self) {
        self.prepare_mobile_switcher_order();
        self.mobile_drag = None;
        self.mobile_hold = self.focused_navigation_target().and_then(|target| {
            let selected = self.mobile_selection(ClientMobileTarget::Workspace {
                endpoint_id: target.endpoint_id,
                workspace_id: target.workspace_id,
            })?;
            Some(MobileSwitchGesture {
                anchor: selected.clone(),
                selected,
                origin: None,
                horizontal_origin: (0, 0),
                last_point: None,
                effective: false,
                edge: None,
            })
        });
    }

    fn mobile_selection(&self, target: ClientMobileTarget) -> Option<MobileSelection> {
        let endpoint_id = match &target {
            ClientMobileTarget::Workspace { endpoint_id, .. }
            | ClientMobileTarget::Tab { endpoint_id, .. }
            | ClientMobileTarget::Pane { endpoint_id, .. } => endpoint_id,
            _ => return None,
        };
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)?;
        if endpoint.status != ClientEndpointStatus::Online {
            return None;
        }
        let snapshot = endpoint.snapshot.as_deref()?;
        let workspace_id = match &target {
            ClientMobileTarget::Workspace { workspace_id, .. } => workspace_id.clone(),
            ClientMobileTarget::Tab { tab_id, .. } => snapshot
                .tabs
                .iter()
                .find(|tab| &tab.tab_id == tab_id)?
                .workspace_id
                .clone(),
            ClientMobileTarget::Pane { pane_id, .. } => {
                let pane = snapshot
                    .panes
                    .iter()
                    .find(|pane| &pane.pane_id == pane_id)?;
                let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == pane.tab_id)?;
                if tab.workspace_id != pane.workspace_id {
                    return None;
                }
                pane.workspace_id.clone()
            }
            _ => return None,
        };
        snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == workspace_id)
            .then(|| MobileSelection {
                target,
                boot_id: snapshot.boot_id.clone(),
                generation: endpoint.snapshot_generation,
                workspace_id,
            })
    }

    pub(super) fn reconcile_mobile_gesture(&mut self) {
        if self.mobile_hold.as_ref().is_some_and(|gesture| {
            self.mobile_selection(gesture.anchor.target.clone())
                .as_ref()
                != Some(&gesture.anchor)
                || self
                    .mobile_selection(gesture.selected.target.clone())
                    .as_ref()
                    != Some(&gesture.selected)
        }) {
            self.mobile_hold = None;
            self.mobile_drag = None;
        }
    }

    fn mobile_parent(&self, target: &ClientMobileTarget) -> Option<(ClientEndpointId, String)> {
        match target {
            ClientMobileTarget::Workspace {
                endpoint_id,
                workspace_id,
            } => Some((endpoint_id.clone(), workspace_id.clone())),
            ClientMobileTarget::Tab {
                endpoint_id,
                tab_id,
            } => {
                let snapshot = self
                    .endpoints
                    .iter()
                    .find(|endpoint| &endpoint.endpoint_id == endpoint_id)?
                    .snapshot
                    .as_deref()?;
                let tab = snapshot.tabs.iter().find(|tab| &tab.tab_id == tab_id)?;
                Some((endpoint_id.clone(), tab.workspace_id.clone()))
            }
            ClientMobileTarget::Pane { endpoint_id, .. } => {
                let selected = self.mobile_selection(target.clone())?;
                Some((endpoint_id.clone(), selected.workspace_id))
            }
            _ => None,
        }
    }

    fn mobile_navigation_targets(&self) -> Vec<ClientMobileTarget> {
        let mut targets = Vec::new();
        for target in self.mobile_ordered_workspace_targets() {
            let Some(endpoint) =
                super::aggregate_navigation::cached_endpoint_snapshots(&self.endpoints)
                    .find(|endpoint| endpoint.endpoint_id == &target.endpoint_id)
            else {
                continue;
            };
            targets.push(ClientMobileTarget::Workspace {
                endpoint_id: endpoint.endpoint_id.clone(),
                workspace_id: target.workspace_id.clone(),
            });
            if self
                .mobile_expanded_groups
                .contains(&(endpoint.endpoint_id.clone(), target.workspace_id.clone()))
            {
                if let Some(panes) = self.mobile_pane_order.as_ref().and_then(|order| {
                    order.get(&(endpoint.endpoint_id.clone(), target.workspace_id.clone()))
                }) {
                    targets.extend(panes.iter().map(|pane| ClientMobileTarget::Pane {
                        endpoint_id: pane.endpoint_id.clone(),
                        pane_id: pane.pane_id.clone(),
                    }));
                }
            }
        }
        targets
    }

    pub(super) fn handle_mobile_gesture(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(mut gesture) = self.mobile_hold.take() else {
            return false;
        };
        if self
            .mobile_selection(gesture.anchor.target.clone())
            .as_ref()
            != Some(&gesture.anchor)
            || self
                .mobile_selection(gesture.selected.target.clone())
                .as_ref()
                != Some(&gesture.selected)
        {
            self.mobile_drag = None;
            outcome.repaint = true;
            return true;
        }
        if mouse.kind == MouseEventKind::Moved {
            self.mobile_hold = Some(gesture);
            return true;
        }
        if !matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) {
            self.mobile_drag = None;
            return false;
        }
        let point = (mouse.column, mouse.row);
        if let Some(origin) = gesture.origin {
            let dx = i32::from(point.0) - i32::from(gesture.horizontal_origin.0);
            let dy = i32::from(point.1) - i32::from(gesture.horizontal_origin.1);
            if dx.abs() >= 4 && dx.abs() > 2 * dy.abs() {
                if let Some((endpoint_id, workspace_id)) =
                    self.mobile_parent(&gesture.selected.target)
                {
                    let group = (endpoint_id.clone(), workspace_id.clone());
                    if dx > 0 {
                        if matches!(
                            gesture.selected.target,
                            ClientMobileTarget::Workspace { .. }
                        ) {
                            self.mobile_expanded_groups.insert(group);
                        }
                    } else {
                        self.mobile_expanded_groups.remove(&group);
                        if let Some(parent) = self.mobile_selection(ClientMobileTarget::Workspace {
                            endpoint_id,
                            workspace_id,
                        }) {
                            gesture.selected = parent;
                        }
                    }
                    gesture.anchor = gesture.selected.clone();
                    gesture.origin = Some(point);
                    gesture.horizontal_origin = point;
                    gesture.effective = true;
                }
            } else if gesture.last_point.is_none_or(|last| {
                (i32::from(point.1) - i32::from(origin.1)) / 2
                    != (i32::from(last.1) - i32::from(origin.1)) / 2
            }) {
                // A refresh must not turn an unchanged release position into another pane.
                let targets = self.mobile_navigation_targets();
                if let Some(index) = targets
                    .iter()
                    .position(|target| *target == gesture.anchor.target)
                {
                    let steps = (i32::from(point.1) - i32::from(origin.1)) / 2;
                    let next = index
                        .saturating_add_signed(steps as isize)
                        .min(targets.len().saturating_sub(1));
                    if let Some(target) = targets
                        .get(next)
                        .filter(|target| **target != gesture.selected.target)
                    {
                        if let Some(selected) = self.mobile_selection(target.clone()) {
                            gesture.selected = selected;
                            gesture.effective = true;
                            gesture.horizontal_origin = point;
                        } else {
                            self.mobile_drag = None;
                            outcome.repaint = true;
                            return true;
                        }
                    }
                }
            }
        } else if mouse.kind == MouseEventKind::Drag(MouseButton::Left) && point.1 >= 3 {
            gesture.origin = Some(point);
            gesture.horizontal_origin = point;
        }
        gesture.last_point = Some(point);
        self.project_mobile_gesture(&gesture);
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            self.mobile_drag = None;
            if gesture.effective {
                self.activate_mobile_target(Some(gesture.selected.target), outcome);
            }
        } else {
            let direction = gesture
                .origin
                .and_then(|_| self.mobile_edge_direction(point));
            if gesture.edge.map(|(direction, _)| direction) != direction {
                gesture.edge =
                    direction.map(|direction| (direction, Instant::now() + MOBILE_EDGE_INTERVAL));
            }
            self.mobile_hold = Some(gesture);
        }
        outcome.repaint = true;
        true
    }
}
