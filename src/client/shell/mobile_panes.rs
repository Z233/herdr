use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct PaneNavigationTarget {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) pane_id: String,
    boot_id: String,
    generation: Option<u64>,
}

pub(super) type MobilePaneOrder = HashMap<(ClientEndpointId, String), Vec<PaneNavigationTarget>>;

fn endpoint_panes(endpoint: &ClientShellEndpoint) -> MobilePaneOrder {
    let Some(snapshot) = endpoint.snapshot.as_deref() else {
        return HashMap::new();
    };
    let workspaces = snapshot
        .workspaces
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .collect::<HashSet<_>>();
    let mut panes_by_tab = HashMap::new();
    for pane in &snapshot.panes {
        panes_by_tab
            .entry(pane.tab_id.as_str())
            .or_insert_with(Vec::new)
            .push(pane);
    }
    let mut groups = HashMap::new();
    for tab in &snapshot.tabs {
        if !workspaces.contains(tab.workspace_id.as_str()) {
            continue;
        }
        for pane in panes_by_tab.get(tab.tab_id.as_str()).into_iter().flatten() {
            if pane.workspace_id != tab.workspace_id {
                continue;
            }
            groups
                .entry((endpoint.endpoint_id.clone(), tab.workspace_id.clone()))
                .or_insert_with(Vec::new)
                .push(PaneNavigationTarget {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    pane_id: pane.pane_id.clone(),
                    boot_id: snapshot.boot_id.clone(),
                    generation: endpoint.snapshot_generation,
                });
        }
    }
    groups
}

impl ClientShellState {
    pub(super) fn prepare_mobile_switcher_order(&mut self) {
        if !self.mobile_layout_active()
            || (self.mode != ClientShellMode::Navigate && !self.mobile_switcher_suspended)
        {
            self.mobile_workspace_order = None;
            self.mobile_pane_order = None;
            self.mobile_hold = None;
            self.mobile_drag = None;
            return;
        }
        if self.mobile_workspace_order.is_none() {
            self.mobile_workspace_order = Some(self.mobile_ordered_workspace_targets());
        }
        if self.mobile_pane_order.is_none() {
            let mut groups = self
                .endpoints
                .iter()
                .flat_map(endpoint_panes)
                .collect::<MobilePaneOrder>();
            let ranks = self
                .pane_mru
                .iter()
                .enumerate()
                .map(|(rank, target)| (target, rank))
                .collect::<HashMap<_, _>>();
            for panes in groups.values_mut() {
                panes.sort_by_key(|pane| ranks.get(pane).copied().unwrap_or(usize::MAX));
            }
            self.mobile_pane_order = Some(groups);
        }
    }

    pub(super) fn reconcile_mobile_panes(&mut self, endpoint_id: &ClientEndpointId) {
        if self.pane_mru.is_empty() && self.mobile_pane_order.is_none() {
            self.reconcile_mobile_gesture();
            return;
        }
        let groups = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .map(endpoint_panes)
            .unwrap_or_default();
        let live = groups.values().flatten().collect::<HashSet<_>>();
        self.pane_mru
            .retain(|target| &target.endpoint_id != endpoint_id || live.contains(target));
        if let Some(order) = self.mobile_pane_order.as_mut() {
            order.retain(|(id, workspace), _| {
                id != endpoint_id || groups.contains_key(&(id.clone(), workspace.clone()))
            });
            for (group, panes) in groups {
                let current = order.entry(group).or_default();
                let live = panes.iter().collect::<HashSet<_>>();
                current.retain(|target| live.contains(target));
                let retained = current.iter().cloned().collect::<HashSet<_>>();
                current.extend(
                    panes
                        .into_iter()
                        .filter(|target| !retained.contains(target)),
                );
            }
        }
        self.reconcile_mobile_gesture();
    }

    pub(super) fn record_presented_pane(&mut self) {
        let (Some(snapshot), Some(surface)) =
            (self.snapshot.as_deref(), self.pane_surface.as_ref())
        else {
            return;
        };
        if surface.boot_id != snapshot.boot_id
            || surface.projection_revision != snapshot.revision
            || self.pane_surface_generation != self.active_snapshot_generation
            || !self.endpoint_is_online(&self.active_endpoint_id)
        {
            return;
        }
        let Some(pane_id) = snapshot.focused_pane_id.as_deref() else {
            return;
        };
        if !surface
            .panes
            .iter()
            .any(|pane| pane.pane_id == pane_id && pane.focused)
        {
            return;
        }
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return;
        };
        if snapshot.focused_workspace_id.as_deref() != Some(&pane.workspace_id)
            || snapshot.focused_tab_id.as_deref() != Some(&pane.tab_id)
            || !snapshot
                .tabs
                .iter()
                .any(|tab| tab.tab_id == pane.tab_id && tab.workspace_id == pane.workspace_id)
            || !snapshot
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == pane.workspace_id)
        {
            return;
        }
        let target = PaneNavigationTarget {
            endpoint_id: self.active_endpoint_id.clone(),
            pane_id: pane.pane_id.clone(),
            boot_id: snapshot.boot_id.clone(),
            generation: self.active_snapshot_generation,
        };
        if self.pane_mru.front() != Some(&target) {
            self.pane_mru.retain(|entry| entry != &target);
            self.pane_mru.push_front(target);
        }
    }
}
