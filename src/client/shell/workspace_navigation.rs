use super::*;

/// A client-only preview. Snapshot identity prevents Enter from using a reused workspace ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WorkspaceNavigationTarget {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) workspace_id: String,
    boot_id: String,
    generation: Option<u64>,
}

/// Display-only continuity while a direct focus request awaits its authoritative snapshot.
pub(super) struct PendingWorkspaceHighlight {
    pub(super) target: WorkspaceNavigationTarget,
    pub(super) request_id: String,
    expires_at: std::time::Instant,
}

impl WorkspaceNavigationTarget {
    pub(super) fn matches(&self, endpoint_id: &ClientEndpointId, workspace_id: &str) -> bool {
        &self.endpoint_id == endpoint_id && self.workspace_id == workspace_id
    }
}

impl ClientShellState {
    pub(super) fn reconcile_workspace_snapshot(
        &mut self,
        endpoint_id: &ClientEndpointId,
        generation: Option<u64>,
        snapshot: &ClientShellSnapshot,
    ) {
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        else {
            return;
        };
        let previous_ids = endpoint
            .snapshot
            .as_deref()
            .filter(|previous| {
                endpoint.status == ClientEndpointStatus::Online
                    && endpoint.snapshot_generation == generation
                    && previous.boot_id == snapshot.boot_id
            })
            .map(|previous| {
                previous
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.workspace_id.as_str())
                    .collect::<HashSet<_>>()
            });
        let live_ids = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.as_str())
            .collect::<HashSet<_>>();
        let retained = |target: &WorkspaceNavigationTarget| {
            &target.endpoint_id != endpoint_id
                || (target.boot_id == snapshot.boot_id
                    && target.generation == generation
                    && live_ids.contains(target.workspace_id.as_str()))
        };
        self.workspace_mru.retain(retained);
        if let Some(order) = self.mobile_workspace_order.as_mut() {
            order.retain(retained);
        }
        if let Some(previous_ids) = previous_ids {
            for workspace in snapshot
                .workspaces
                .iter()
                .rev()
                .filter(|workspace| !previous_ids.contains(workspace.workspace_id.as_str()))
            {
                self.workspace_mru.push_front(WorkspaceNavigationTarget {
                    endpoint_id: endpoint_id.clone(),
                    workspace_id: workspace.workspace_id.clone(),
                    boot_id: snapshot.boot_id.clone(),
                    generation,
                });
            }
            self.workspace_mru.truncate(256);
        }
    }

    pub(super) fn mobile_ordered_workspace_targets(&self) -> Vec<WorkspaceNavigationTarget> {
        if let Some(order) = self.mobile_workspace_order.as_ref() {
            return order.clone();
        }
        let mut targets = Vec::new();
        for endpoint in &self.endpoints {
            let Some(snapshot) = endpoint.snapshot.as_deref() else {
                continue;
            };
            targets.extend(
                snapshot
                    .workspaces
                    .iter()
                    .map(|workspace| WorkspaceNavigationTarget {
                        endpoint_id: endpoint.endpoint_id.clone(),
                        workspace_id: workspace.workspace_id.clone(),
                        boot_id: snapshot.boot_id.clone(),
                        generation: endpoint.snapshot_generation,
                    }),
            );
        }
        targets.sort_by_key(|target| {
            self.workspace_mru
                .iter()
                .position(|recent| recent == target)
                .unwrap_or(usize::MAX)
        });
        targets
    }

    pub(super) fn handle_mobile_workspace_action(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction;
        if !self.mobile_layout_active()
            || self.snapshot.is_none()
            || self.pane_surface.is_none()
            || !matches!(
                action,
                KeybindAction::SwitchWorkspace(_)
                    | KeybindAction::PreviousWorkspace
                    | KeybindAction::NextWorkspace
            )
        {
            return false;
        }
        let targets = self
            .mobile_ordered_workspace_targets()
            .into_iter()
            .filter(|target| {
                matches!(action, KeybindAction::SwitchWorkspace(_))
                    || self.endpoint_is_online(&target.endpoint_id)
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return true;
        }
        let focused = self.focused_navigation_target();
        let current = targets
            .iter()
            .position(|target| Some(target) == focused.as_ref());
        let next = match (action, current) {
            (KeybindAction::SwitchWorkspace(index), _) => index,
            (KeybindAction::PreviousWorkspace, Some(index)) => {
                (index + targets.len() - 1) % targets.len()
            }
            (KeybindAction::NextWorkspace, Some(index)) => (index + 1) % targets.len(),
            (KeybindAction::PreviousWorkspace, None) => targets.len() - 1,
            _ => 0,
        };
        if let Some(target) = targets
            .get(next)
            .filter(|target| self.navigation_target_valid(target))
        {
            self.focus_or_activate(
                target.endpoint_id.clone(),
                ClientEndpointFocusTarget::Workspace(target.workspace_id.clone()),
                outcome,
            );
        }
        true
    }

    pub(crate) fn tick_workspace_highlight(&mut self, now: std::time::Instant) -> bool {
        if self
            .pending_workspace_highlight
            .as_ref()
            .is_some_and(|pending| now >= pending.expires_at)
        {
            self.pending_workspace_highlight = None;
            return true;
        }
        false
    }

    pub(super) fn reconcile_pending_workspace_highlight(&mut self) {
        if self
            .pending_workspace_highlight
            .as_ref()
            .is_some_and(|pending| {
                pending.target.endpoint_id != self.active_endpoint_id
                    || !self.navigation_target_valid(&pending.target)
                    || self.snapshot.as_deref().is_some_and(|snapshot| {
                        snapshot.focused_workspace_id.as_deref()
                            == Some(pending.target.workspace_id.as_str())
                    })
            })
        {
            self.pending_workspace_highlight = None;
        }
    }

    pub(super) fn navigation_target(
        &self,
        endpoint_id: &ClientEndpointId,
        workspace_id: &str,
    ) -> Option<WorkspaceNavigationTarget> {
        let endpoint = self
            .endpoints
            .iter()
            .find(|entry| &entry.endpoint_id == endpoint_id)?;
        let snapshot = endpoint.snapshot.as_deref()?;
        Some(WorkspaceNavigationTarget {
            endpoint_id: endpoint_id.clone(),
            workspace_id: workspace_id.to_owned(),
            boot_id: snapshot.boot_id.clone(),
            generation: endpoint.snapshot_generation,
        })
    }

    pub(super) fn focused_navigation_target(&self) -> Option<WorkspaceNavigationTarget> {
        let workspace_id = self.snapshot.as_deref()?.focused_workspace_id.as_deref()?;
        self.navigation_target(&self.active_endpoint_id, workspace_id)
    }

    pub(super) fn navigation_target_valid(&self, target: &WorkspaceNavigationTarget) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.endpoint_id == target.endpoint_id
                && endpoint.status == ClientEndpointStatus::Online
                && endpoint.snapshot_generation == target.generation
                && endpoint.snapshot.as_deref().is_some_and(|snapshot| {
                    snapshot.boot_id == target.boot_id
                        && snapshot
                            .workspaces
                            .iter()
                            .any(|workspace| workspace.workspace_id == target.workspace_id)
                })
        })
    }

    pub(super) fn workspace_preview_action_blocked(&self) -> bool {
        self.navigate_workspace_id.as_ref().is_some_and(|target| {
            target.endpoint_id != self.active_endpoint_id || !self.navigation_target_valid(target)
        })
    }

    pub(super) fn move_navigate_workspace(&mut self, delta: isize) {
        let mobile = self.mobile_layout_active();
        let surface_available = self.snapshot.is_some() && self.pane_surface.is_some();
        let empty_collapsed_groups = HashSet::new();
        let mut targets = if mobile && surface_available {
            self.mobile_ordered_workspace_targets()
                .into_iter()
                .filter(|target| self.endpoint_is_online(&target.endpoint_id))
                .collect()
        } else {
            let mut targets = Vec::new();
            for endpoint in &self.endpoints {
                if endpoint.status != ClientEndpointStatus::Online {
                    continue;
                }
                let Some(snapshot) = endpoint.snapshot.as_deref() else {
                    continue;
                };
                let entries = if self.sidebar_collapsed && !mobile && surface_available {
                    snapshot
                        .workspaces
                        .iter()
                        .enumerate()
                        .map(|(index, _)| WorkspaceEntry {
                            index,
                            indented: false,
                            last_child: false,
                        })
                        .collect()
                } else {
                    let collapsed_groups = self
                        .collapsed_groups_for_endpoint(&endpoint.endpoint_id)
                        .unwrap_or(&empty_collapsed_groups);
                    render::workspace_entries(snapshot, collapsed_groups)
                };
                for entry in entries {
                    targets.push(WorkspaceNavigationTarget {
                        endpoint_id: endpoint.endpoint_id.clone(),
                        workspace_id: snapshot.workspaces[entry.index].workspace_id.clone(),
                        boot_id: snapshot.boot_id.clone(),
                        generation: endpoint.snapshot_generation,
                    });
                }
            }
            targets
        };
        if targets.is_empty() {
            return;
        }
        let current = self
            .navigate_workspace_id
            .as_ref()
            .and_then(|selected| targets.iter().position(|target| target == selected));
        let next = match current {
            Some(current) if mobile => {
                (current as isize + delta).clamp(0, targets.len() as isize - 1) as usize
            }
            Some(current) => (current as isize + delta).rem_euclid(targets.len() as isize) as usize,
            None if delta < 0 => targets.len() - 1,
            None => 0,
        };
        let target = targets.swap_remove(next);
        self.collapsed_endpoints.remove(&target.endpoint_id);
        if self.endpoints.len() == 1 && !mobile {
            self.reveal_workspace(&target.workspace_id);
        }
        self.navigate_workspace_id = Some(target);
        self.reveal_mobile_workspace = mobile;
        self.reveal_navigation_workspace =
            !mobile || self.snapshot.is_none() || self.pane_surface.is_none();
    }

    pub(super) fn accept_navigate_workspace(&mut self, outcome: &mut ClientShellInput) {
        let Some(target) = self.navigate_workspace_id.clone() else {
            self.mode = self.copy_or_terminal_mode();
            outcome.repaint = true;
            return;
        };
        if !self.navigation_target_valid(&target) {
            self.receive_endpoint_unavailable(
                "Workspace is no longer available; select a connected workspace".into(),
            );
            outcome.repaint = true;
            return;
        }
        let action_index = outcome.actions.len();
        if self.focus_or_activate(
            target.endpoint_id.clone(),
            ClientEndpointFocusTarget::Workspace(target.workspace_id.clone()),
            outcome,
        ) {
            // Endpoint handoffs already retain their coherent source frame in the runtime.
            // Only retain a highlight if a focus request was actually enqueued.
            if let Some(ClientShellAction::Endpoint { request, .. }) =
                outcome.actions.get(action_index)
            {
                self.pending_workspace_highlight = Some(PendingWorkspaceHighlight {
                    target,
                    request_id: request.id.clone(),
                    // A later focus can be coalesced with this one before a snapshot is sent.
                    expires_at: std::time::Instant::now() + std::time::Duration::from_secs(1),
                });
                self.reconcile_pending_workspace_highlight();
            }
            self.mode = ClientShellMode::Terminal;
            self.navigate_workspace_id = None;
        }
        outcome.repaint = true;
    }
}
