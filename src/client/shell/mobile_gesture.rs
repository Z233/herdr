use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

#[derive(Clone, PartialEq, Eq)]
struct MobileSelection {
    target: ClientMobileTarget,
    boot_id: String,
    generation: Option<u64>,
}

pub(super) struct MobileSwitchGesture {
    anchor: MobileSelection,
    selected: MobileSelection,
    origin: Option<(u16, u16)>,
    horizontal_origin: (u16, u16),
    effective: bool,
}

impl ClientShellState {
    pub(super) fn arm_mobile_gesture(&mut self) {
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
                effective: false,
            })
        });
    }

    fn mobile_selection(&self, target: ClientMobileTarget) -> Option<MobileSelection> {
        let endpoint_id = match &target {
            ClientMobileTarget::Workspace { endpoint_id, .. }
            | ClientMobileTarget::Tab { endpoint_id, .. } => endpoint_id,
            _ => return None,
        };
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)?;
        let snapshot = endpoint.snapshot.as_deref()?;
        let exists = match &target {
            ClientMobileTarget::Workspace { workspace_id, .. } => snapshot
                .workspaces
                .iter()
                .any(|workspace| &workspace.workspace_id == workspace_id),
            ClientMobileTarget::Tab { tab_id, .. } => {
                snapshot.tabs.iter().any(|tab| &tab.tab_id == tab_id)
            }
            _ => false,
        };
        exists.then(|| MobileSelection {
            target,
            boot_id: snapshot.boot_id.clone(),
            generation: endpoint.snapshot_generation,
        })
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
            _ => None,
        }
    }

    fn mobile_navigation_targets(&self) -> Vec<ClientMobileTarget> {
        let mut targets = Vec::new();
        for endpoint in super::aggregate_navigation::cached_endpoint_snapshots(&self.endpoints) {
            for entry in super::render::workspace_entries(endpoint.snapshot, &HashSet::new()) {
                let Some(workspace) = endpoint.snapshot.workspaces.get(entry.index) else {
                    continue;
                };
                targets.push(ClientMobileTarget::Workspace {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    workspace_id: workspace.workspace_id.clone(),
                });
                if self
                    .mobile_expanded_groups
                    .contains(&(endpoint.endpoint_id.clone(), workspace.workspace_id.clone()))
                {
                    targets.extend(
                        endpoint
                            .snapshot
                            .tabs
                            .iter()
                            .filter(|tab| tab.workspace_id == workspace.workspace_id)
                            .map(|tab| ClientMobileTarget::Tab {
                                endpoint_id: endpoint.endpoint_id.clone(),
                                tab_id: tab.tab_id.clone(),
                            }),
                    );
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
                        self.mobile_expanded_groups.insert(group);
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
            } else {
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
                        }
                    }
                }
            }
        } else if mouse.kind == MouseEventKind::Drag(MouseButton::Left) && point.1 >= 3 {
            gesture.origin = Some(point);
            gesture.horizontal_origin = point;
        }
        if gesture.effective {
            self.mobile_drag = Some(gesture.selected.target.clone());
            if let Some((endpoint_id, workspace_id)) = self.mobile_parent(&gesture.selected.target)
            {
                self.navigate_workspace_id = self.navigation_target(&endpoint_id, &workspace_id);
                self.reveal_mobile_workspace = true;
            }
        }
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            self.mobile_drag = None;
            if gesture.effective {
                self.activate_mobile_target(Some(gesture.selected.target), outcome);
            }
        } else {
            self.mobile_hold = Some(gesture);
        }
        outcome.repaint = true;
        true
    }
}
