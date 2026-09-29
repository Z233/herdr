use super::*;
use crate::api::schema::{Method, ResponseResult};
use crossterm::event::{KeyEventKind, KeyModifiers, ModifierKeyCode};

#[derive(Debug, Default)]
pub(super) struct NavigatorState {
    pub(super) preview: super::navigator_preview::NavigatorPreview,
    pub(super) hold: Option<NavigatorHold>,
    pub(super) scopes: HashMap<ClientEndpointId, (String, Option<u64>)>,
    pub(super) directory: NavigatorDirectory,
}

#[derive(Debug)]
pub(super) struct NavigatorHold {
    pub(super) modifiers: KeyModifiers,
    pub(super) targets: Vec<WorkspaceNavigationTarget>,
}

#[derive(Debug, Default)]
pub(super) struct NavigatorDirectory {
    pub(super) endpoint_id: Option<ClientEndpointId>,
    pub(super) query: String,
    pub(super) generation: u64,
    pub(super) candidates: Vec<crate::api::schema::WorkspaceSearchCandidate>,
    pub(super) preview_path: Option<String>,
    pub(super) preview: Option<crate::api::schema::WorkspaceDirectoryPreview>,
    pub(super) error: Option<String>,
    pub(super) loading: bool,
    pub(super) creating: bool,
}

#[derive(Debug)]
pub(super) enum NavigatorRequestKind {
    Query,
    Preview(String),
    Create,
}

#[derive(Debug)]
pub(super) struct NavigatorRequest {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) boot_id: String,
    pub(super) connection_generation: Option<u64>,
    pub(super) query_generation: u64,
    pub(super) kind: NavigatorRequestKind,
}

impl ClientNavigatorTarget {
    pub(super) fn endpoint_id(&self) -> &ClientEndpointId {
        match self {
            Self::Machine { endpoint_id }
            | Self::Workspace { endpoint_id, .. }
            | Self::Tab { endpoint_id, .. }
            | Self::Pane { endpoint_id, .. }
            | Self::Directory { endpoint_id, .. } => endpoint_id,
        }
    }
}

impl ClientShellState {
    pub(super) fn record_presented_workspace(&mut self) {
        let Some(target) = self.focused_navigation_target() else {
            return;
        };
        if self.workspace_mru.front() == Some(&target) {
            return;
        }
        self.workspace_mru
            .retain(|entry| !entry.matches(&target.endpoint_id, &target.workspace_id));
        self.workspace_mru.push_front(target);
        self.workspace_mru.truncate(256);
    }

    pub(super) fn capture_navigator_scopes(
        &self,
    ) -> HashMap<ClientEndpointId, (String, Option<u64>)> {
        self.endpoints
            .iter()
            .filter_map(|endpoint| {
                Some((
                    endpoint.endpoint_id.clone(),
                    (
                        endpoint.snapshot.as_ref()?.boot_id.clone(),
                        endpoint.snapshot_generation,
                    ),
                ))
            })
            .collect()
    }

    pub(super) fn navigator_target_is_current(&self, target: &ClientNavigatorTarget) -> bool {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return false;
        };
        let endpoint_id = target.endpoint_id();
        self.endpoints.iter().any(|endpoint| {
            &endpoint.endpoint_id == endpoint_id
                && endpoint.status == ClientEndpointStatus::Online
                && endpoint.snapshot.as_ref().is_some_and(|snapshot| {
                    navigator.fork.scopes.get(endpoint_id)
                        == Some(&(snapshot.boot_id.clone(), endpoint.snapshot_generation))
                })
        })
    }

    pub(super) fn open_hold_navigator(&mut self, backward: bool) {
        let mut targets = Vec::new();
        for endpoint in &self.endpoints {
            if endpoint.status != ClientEndpointStatus::Online {
                continue;
            }
            let Some(snapshot) = endpoint.snapshot.as_ref() else {
                continue;
            };
            for workspace in &snapshot.workspaces {
                if let Some(target) =
                    self.navigation_target(&endpoint.endpoint_id, &workspace.workspace_id)
                {
                    targets.push(target);
                }
            }
        }
        targets.sort_by_key(|target| {
            self.workspace_mru
                .iter()
                .position(|recent| recent == target)
                .unwrap_or(usize::MAX)
        });
        self.open_navigator_overlay();
        if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
            navigator.search_focused = false;
            navigator.expanded_workspaces.clear();
            navigator.selected = targets
                .first()
                .map(|target| ClientNavigatorTarget::Workspace {
                    endpoint_id: target.endpoint_id.clone(),
                    workspace_id: target.workspace_id.clone(),
                });
            navigator.fork.hold = Some(NavigatorHold {
                modifiers: KeyModifiers::empty(),
                targets,
            });
        }
        self.cycle_hold_navigator(if backward { -1 } else { 1 });
    }

    fn cycle_hold_navigator(&mut self, delta: isize) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let Some(hold) = navigator.fork.hold.as_ref() else {
            return;
        };
        if hold.targets.is_empty() {
            return;
        }
        let parent = navigator
            .selected
            .as_ref()
            .and_then(|target| navigator_workspace(&self.endpoints, target));
        let current = hold
            .targets
            .iter()
            .position(|target| {
                parent.as_ref() == Some(&(target.endpoint_id.clone(), target.workspace_id.clone()))
            })
            .unwrap_or_default();
        let index = (current as isize + delta).rem_euclid(hold.targets.len() as isize) as usize;
        let target = &hold.targets[index];
        navigator.selected = Some(ClientNavigatorTarget::Workspace {
            endpoint_id: target.endpoint_id.clone(),
            workspace_id: target.workspace_id.clone(),
        });
    }

    fn expand_hold_workspace(&mut self, expand: bool) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let Some(parent) = navigator
            .selected
            .as_ref()
            .and_then(|target| navigator_workspace(&self.endpoints, target))
        else {
            return;
        };
        if expand {
            navigator.expanded_workspaces.insert(parent);
        } else {
            navigator.expanded_workspaces.remove(&parent);
            navigator.selected = Some(ClientNavigatorTarget::Workspace {
                endpoint_id: parent.0,
                workspace_id: parent.1,
            });
        }
    }

    pub(super) fn route_hold_navigator_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return false;
        };
        let Some(hold) = navigator.fork.hold.as_ref() else {
            return false;
        };
        if key.kind == KeyEventKind::Release {
            let released = match key.code {
                KeyCode::Modifier(ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt) => {
                    KeyModifiers::ALT
                }
                KeyCode::Modifier(ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl) => {
                    KeyModifiers::CONTROL
                }
                KeyCode::Modifier(ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper) => {
                    KeyModifiers::SUPER
                }
                KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift) => {
                    KeyModifiers::SHIFT
                }
                KeyCode::Modifier(ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper) => {
                    KeyModifiers::HYPER
                }
                KeyCode::Modifier(ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta) => {
                    KeyModifiers::META
                }
                _ => KeyModifiers::empty(),
            };
            if !hold.modifiers.is_empty()
                && (!key.modifiers.contains(hold.modifiers) || released.intersects(hold.modifiers))
            {
                self.accept_navigator_selection(outcome);
                return true;
            }
            return true;
        }
        let command = key.modifiers.is_empty()
            || !hold.modifiers.is_empty() && key.modifiers.contains(hold.modifiers);
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL
        {
            self.overlay = None;
        } else if key.code == KeyCode::Enter {
            self.accept_navigator_selection(outcome);
        } else if self
            .config
            .keybinds
            .keybinds
            .workspace_switcher_backward
            .matches_direct_key(key)
        {
            self.cycle_hold_navigator(-1);
        } else if self
            .config
            .keybinds
            .keybinds
            .workspace_switcher
            .matches_direct_key(key)
        {
            self.cycle_hold_navigator(1);
        } else if command
            && matches!(
                key.code,
                KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
            )
        {
            self.cycle_hold_navigator(-1);
        } else if command && matches!(key.code, KeyCode::Right | KeyCode::Char('l' | 'L')) {
            self.expand_hold_workspace(true);
        } else if command && matches!(key.code, KeyCode::Left | KeyCode::Char('h' | 'H')) {
            self.expand_hold_workspace(false);
        } else if command && matches!(key.code, KeyCode::Down | KeyCode::Char('j' | 'J')) {
            self.move_navigator_selection(1);
        } else if command && matches!(key.code, KeyCode::Up | KeyCode::Char('k' | 'K')) {
            self.move_navigator_selection(-1);
        } else if key.code == KeyCode::PageDown {
            self.move_navigator_selection(self.hits.navigator_rows.len().max(1) as isize);
        } else if key.code == KeyCode::PageUp {
            self.move_navigator_selection(-(self.hits.navigator_rows.len().max(1) as isize));
        } else if key.code == KeyCode::Home {
            self.move_navigator_selection(isize::MIN / 2);
        } else if key.code == KeyCode::End {
            self.move_navigator_selection(isize::MAX / 2);
        } else if command && matches!(key.code, KeyCode::Char('/' | 's' | 'S')) {
            if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                navigator.fork.directory.endpoint_id = navigator
                    .selected
                    .as_ref()
                    .map(|target| target.endpoint_id().clone());
                navigator.fork.hold = None;
                navigator.search_focused = true;
            }
        } else {
            return true;
        }
        outcome.repaint = true;
        true
    }

    pub(super) fn refresh_navigator_directory(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return;
        };
        if navigator.fork.hold.is_some() || navigator.fork.directory.creating {
            return;
        }
        let query = navigator.query.as_str().to_owned();
        let endpoint_id = navigator
            .selected
            .as_ref()
            .map(ClientNavigatorTarget::endpoint_id)
            .cloned()
            .or_else(|| navigator.fork.directory.endpoint_id.clone())
            .unwrap_or_else(|| self.active_endpoint_id.clone());
        let changed = navigator.fork.directory.query != query
            || navigator.fork.directory.endpoint_id.as_ref() != Some(&endpoint_id);
        if changed {
            let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
                return;
            };
            let directory = &mut navigator.fork.directory;
            directory.query = query.clone();
            directory.endpoint_id = Some(endpoint_id.clone());
            directory.generation = self.next_request_id;
            directory.candidates.clear();
            directory.preview = None;
            directory.preview_path = None;
            directory.error = None;
            directory.loading = !query.trim().is_empty();
            let generation = directory.generation;
            self.pending_requests.retain(|_, pending| !matches!(&pending.kind,
                PendingEndpointKind::Navigator(request) if !matches!(request.kind, NavigatorRequestKind::Create)
            ));
            if query.trim().is_empty() {
                return;
            }
            self.request_navigator_method(
                endpoint_id,
                generation,
                NavigatorRequestKind::Query,
                Method::WorkspaceSearch(crate::api::schema::WorkspaceSearchParams { query }),
                outcome,
            );
            return;
        }
        let selected = navigator.selected.clone();
        let generation = navigator.fork.directory.generation;
        if let Some(ClientNavigatorTarget::Directory {
            endpoint_id,
            canonical_path,
            ..
        }) = selected
        {
            if navigator.fork.directory.preview_path.as_ref() == Some(&canonical_path) {
                return;
            }
            if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                navigator.fork.directory.preview_path = Some(canonical_path.clone());
                navigator.fork.directory.preview = None;
                navigator.fork.directory.error = None;
            }
            self.pending_requests.retain(|_, pending| !matches!(&pending.kind,
                PendingEndpointKind::Navigator(request) if matches!(request.kind, NavigatorRequestKind::Preview(_))
            ));
            self.request_navigator_method(
                endpoint_id,
                generation,
                NavigatorRequestKind::Preview(canonical_path.clone()),
                Method::WorkspaceDirectoryPreview(
                    crate::api::schema::WorkspaceDirectoryPreviewParams {
                        path: canonical_path,
                    },
                ),
                outcome,
            );
        }
    }

    fn request_navigator_method(
        &mut self,
        endpoint_id: ClientEndpointId,
        query_generation: u64,
        kind: NavigatorRequestKind,
        method: Method,
        outcome: &mut ClientShellInput,
    ) {
        let method_name = crate::api::api_method_name(&method).to_owned();
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id);
        let error = match endpoint {
            None => Some("Selected endpoint is unavailable".to_owned()),
            Some(endpoint) if endpoint.status != ClientEndpointStatus::Online => {
                Some(format!("{} is offline", endpoint.label))
            }
            Some(endpoint)
                if endpoint
                    .methods
                    .as_ref()
                    .is_some_and(|methods| !methods.contains(&method_name)) =>
            {
                Some(format!(
                    "{} does not support {method_name}; update that endpoint",
                    endpoint.label
                ))
            }
            _ => None,
        };
        if let Some(error) = error {
            self.set_navigator_directory_error(error);
            outcome.repaint = true;
            return;
        }
        let Some(endpoint) = endpoint else {
            return;
        };
        let Some(snapshot) = endpoint.snapshot.as_ref() else {
            self.set_navigator_directory_error("Endpoint snapshot is unavailable".into());
            return;
        };
        let boot_id = snapshot.boot_id.clone();
        let connection_generation = endpoint.snapshot_generation;
        let request_id = format!("client-shell:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: boot_id.clone(),
                method_name,
                confirmation_workspace_id: None,
                kind: PendingEndpointKind::Navigator(NavigatorRequest {
                    endpoint_id: endpoint_id.clone(),
                    boot_id: boot_id.clone(),
                    connection_generation,
                    query_generation,
                    kind,
                }),
            },
        );
        outcome.actions.push(ClientShellAction::Endpoint {
            endpoint_id,
            boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
    }

    fn set_navigator_directory_error(&mut self, message: String) {
        if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
            navigator.fork.directory.error = Some(message);
            navigator.fork.directory.loading = false;
            navigator.fork.directory.creating = false;
        }
    }

    pub(super) fn complete_navigator_request(
        &mut self,
        request: NavigatorRequest,
        response_boot_id: &str,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let current = self.endpoints.iter().any(|endpoint| {
            endpoint.endpoint_id == request.endpoint_id
                && endpoint.status == ClientEndpointStatus::Online
                && endpoint.snapshot_generation == request.connection_generation
                && endpoint.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.boot_id == request.boot_id && snapshot.boot_id == response_boot_id
                })
        });
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return (false, Vec::new());
        };
        if !current
            || navigator.fork.directory.generation != request.query_generation
            || navigator.fork.directory.endpoint_id.as_ref() != Some(&request.endpoint_id)
        {
            return (false, Vec::new());
        }
        if let NavigatorRequestKind::Preview(path) = &request.kind {
            if navigator.fork.directory.preview_path.as_ref() != Some(path)
                || !matches!(&navigator.selected, Some(ClientNavigatorTarget::Directory { canonical_path, .. }) if canonical_path == path)
            {
                return (false, Vec::new());
            }
        }
        let directory = &mut navigator.fork.directory;
        let mut outcome = ClientShellInput::default();
        match (request.kind, result) {
            (NavigatorRequestKind::Query, Ok(ResponseResult::WorkspaceSearch { candidates })) => {
                directory.candidates = candidates;
                directory.loading = false;
                directory.error = None;
            }
            (
                NavigatorRequestKind::Preview(path),
                Ok(ResponseResult::WorkspaceDirectoryPreview { preview }),
            ) => {
                if directory.preview_path.as_ref() != Some(&path) || preview.canonical_path != path
                {
                    return (false, Vec::new());
                }
                directory.preview = Some(preview);
            }
            (
                NavigatorRequestKind::Create,
                Ok(ResponseResult::WorkspaceCreated { workspace, .. }),
            ) => {
                let endpoint_id = request.endpoint_id;
                self.overlay = None;
                self.focus_or_activate(
                    endpoint_id,
                    ClientEndpointFocusTarget::Workspace(workspace.workspace_id),
                    &mut outcome,
                );
            }
            (_, Err(error)) => {
                self.set_navigator_directory_error(error.message);
                return (true, Vec::new());
            }
            _ => {
                self.set_navigator_directory_error(
                    "Endpoint returned an unexpected directory result".into(),
                );
                return (true, Vec::new());
            }
        }
        self.refresh_navigator_directory(&mut outcome);
        (true, outcome.actions)
    }

    pub(super) fn create_navigator_workspace(
        &mut self,
        endpoint_id: ClientEndpointId,
        path: String,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        if navigator.fork.directory.creating {
            return;
        }
        navigator.fork.directory.creating = true;
        navigator.fork.directory.error = None;
        let generation = navigator.fork.directory.generation;
        self.request_navigator_method(
            endpoint_id,
            generation,
            NavigatorRequestKind::Create,
            Method::WorkspaceCreate(crate::api::schema::WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: Some(path),
                focus: false,
                label: None,
                env: Default::default(),
            }),
            outcome,
        );
        outcome.repaint = true;
    }

    pub(crate) fn accepts_background_endpoint_result(
        &self,
        endpoint_id: &ClientEndpointId,
        request_id: &str,
    ) -> bool {
        self.pending_requests
            .get(request_id)
            .is_some_and(|pending| match &pending.kind {
                PendingEndpointKind::Navigator(request) => &request.endpoint_id == endpoint_id,
                PendingEndpointKind::NavigatorPreview(request) => {
                    &request.endpoint_id == endpoint_id
                }
                _ => false,
            })
    }
}

pub(super) fn hold_rows(
    endpoints: &[ClientShellEndpoint],
    navigator: &ClientNavigatorOverlay,
) -> Option<Vec<ClientNavigatorRow>> {
    let hold = navigator.fork.hold.as_ref()?;
    let mut rows = Vec::new();
    for target in &hold.targets {
        let Some(endpoint) = endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == target.endpoint_id)
        else {
            continue;
        };
        let Some(snapshot) = endpoint.snapshot.as_ref() else {
            continue;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == target.workspace_id)
        else {
            continue;
        };
        let stale = endpoint.status != ClientEndpointStatus::Online
            || navigator.fork.scopes.get(&endpoint.endpoint_id)
                != Some(&(snapshot.boot_id.clone(), endpoint.snapshot_generation));
        rows.push(ClientNavigatorRow {
            depth: 0,
            label: workspace.label.clone(),
            meta: endpoint.label.clone(),
            status: Some(workspace.agent_status),
            stale,
            current: false,
            target: ClientNavigatorTarget::Workspace {
                endpoint_id: target.endpoint_id.clone(),
                workspace_id: target.workspace_id.clone(),
            },
        });
        if navigator
            .expanded_workspaces
            .contains(&(endpoint.endpoint_id.clone(), workspace.workspace_id.clone()))
        {
            rows.extend(
                snapshot
                    .tabs
                    .iter()
                    .filter(|tab| tab.workspace_id == workspace.workspace_id)
                    .map(|tab| ClientNavigatorRow {
                        depth: 1,
                        label: tab.label.clone(),
                        meta: String::new(),
                        status: Some(tab.agent_status),
                        stale,
                        current: false,
                        target: ClientNavigatorTarget::Tab {
                            endpoint_id: endpoint.endpoint_id.clone(),
                            tab_id: tab.tab_id.clone(),
                        },
                    }),
            );
        }
    }
    Some(rows)
}

fn navigator_workspace(
    endpoints: &[ClientShellEndpoint],
    target: &ClientNavigatorTarget,
) -> Option<(ClientEndpointId, String)> {
    let endpoint_id = target.endpoint_id();
    let snapshot = endpoints
        .iter()
        .find(|endpoint| &endpoint.endpoint_id == endpoint_id)?
        .snapshot
        .as_ref()?;
    let workspace_id = match target {
        ClientNavigatorTarget::Workspace { workspace_id, .. } => workspace_id.clone(),
        ClientNavigatorTarget::Tab { tab_id, .. } => snapshot
            .tabs
            .iter()
            .find(|tab| &tab.tab_id == tab_id)?
            .workspace_id
            .clone(),
        _ => return None,
    };
    Some((endpoint_id.clone(), workspace_id))
}

pub(super) fn append_directory_rows(
    rows: &mut Vec<ClientNavigatorRow>,
    navigator: &ClientNavigatorOverlay,
) {
    let directory = &navigator.fork.directory;
    let Some(endpoint_id) = directory.endpoint_id.as_ref() else {
        return;
    };
    rows.extend(
        directory
            .candidates
            .iter()
            .map(|candidate| ClientNavigatorRow {
                depth: 0,
                label: candidate.shown_path.clone(),
                meta: "create workspace".into(),
                status: None,
                stale: false,
                current: false,
                target: ClientNavigatorTarget::Directory {
                    endpoint_id: endpoint_id.clone(),
                    shown_path: candidate.shown_path.clone(),
                    canonical_path: candidate.canonical_path.clone(),
                },
            }),
    );
}
