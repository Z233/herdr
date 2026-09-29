use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};

pub(super) fn two_endpoints() -> (ClientShellState, ClientEndpointId) {
    let config: Config = toml::from_str("[keys]\nworkspace_switcher = 'alt+tab'\n").unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let profile = SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Remote".into(),
        target: "test@localhost".into(),
        session: "test".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let mut projection = snapshot();
    projection.boot_id = "remote-boot".into();
    projection.workspaces[0].label = "Remote directory".into();
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    state.compose(106, 24).unwrap();
    (state, remote)
}

#[test]
fn fork_merge_hold_switcher_qualifies_duplicate_ids_and_selects_only_on_modifier_release() {
    let (mut state, remote) = two_endpoints();
    let opened = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Tab,
        KeyModifiers::ALT,
    ))]);
    assert!(opened.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
    state.compose(106, 24).unwrap();
    let targets: Vec<_> = state
        .hits
        .navigator_rows
        .iter()
        .map(|(_, target)| target)
        .collect();
    assert!(targets.iter().any(|target| matches!(target,
        ClientNavigatorTarget::Workspace { endpoint_id, workspace_id }
            if *endpoint_id == ClientEndpointId::Local && workspace_id == "ws_1"
    )));
    assert!(targets.iter().any(|target| matches!(target,
        ClientNavigatorTarget::Workspace { endpoint_id, workspace_id }
            if *endpoint_id == remote && workspace_id == "ws_1"
    )));
    let tab_up = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::Tab, KeyModifiers::ALT)
            .with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(
        tab_up.actions.is_empty(),
        "Tab release does not commit while Alt is held"
    );
    let released = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
            KeyModifiers::empty(),
        )
        .with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(matches!(released.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }]
            if *endpoint_id == remote && id == "ws_1"
    ));
    assert!(state.overlay.is_none());
    assert_eq!(
        state.workspace_mru.len(),
        1,
        "unacknowledged remote activation is not MRU"
    );
}

#[test]
fn fork_merge_hold_switcher_rejects_restarted_endpoint_and_escape_cancels() {
    let (mut state, remote) = two_endpoints();
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Tab,
        KeyModifiers::ALT,
    ))]);
    let mut restarted = snapshot();
    restarted.boot_id = "replacement-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(restarted));
    let released = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
            KeyModifiers::empty(),
        )
        .with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(released.actions.is_empty());
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
}

#[test]
fn fork_merge_remote_directory_search_reports_errors_and_rejects_old_queries() {
    let (mut state, remote) = two_endpoints();
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Machine {
        endpoint_id: remote.clone(),
    });
    navigator.search_focused = true;
    navigator.query.insert("project");
    let first = state.handle_raw_events(Vec::new());
    let request = first
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint {
                endpoint_id,
                boot_id,
                request,
            } if *endpoint_id == remote
                && boot_id == "remote-boot"
                && matches!(
                    request.method,
                    crate::api::schema::Method::WorkspaceSearch(_)
                ) =>
            {
                Some(request)
            }
            _ => None,
        })
        .expect("directory query is routed to the selected remote endpoint");
    let first_id = request.id.clone();
    state.handle_endpoint_result(
        "remote-boot",
        &first_id,
        Err(ClientShellEndpointError {
            code: Some("workspace_search_unavailable".into()),
            message: "zoxide is not installed on Remote".into(),
        }),
    );
    let frame = state.compose(106, 24).unwrap();
    assert!(frame_rows(&frame)
        .join("\n")
        .contains("zoxide is not installed"));
    state.handle_input_bytes(b"2");
    state.handle_endpoint_result(
        "remote-boot",
        &first_id,
        Ok(crate::api::schema::ResponseResult::WorkspaceSearch {
            candidates: vec![crate::api::schema::WorkspaceSearchCandidate {
                shown_path: "/stale".into(),
                canonical_path: "/stale".into(),
                score: 1.0,
            }],
        }),
    );
    assert!(!frame_rows(&state.compose(106, 24).unwrap())
        .join("\n")
        .contains("/stale"));
    state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
    let outcome = state.handle_input_bytes(b"3");
    assert!(outcome.actions.is_empty());
    let frame = frame_rows(&state.compose(106, 24).unwrap()).join("\n");
    assert!(
        frame.contains("offline") || frame.contains("not ready"),
        "{frame}"
    );
}

fn select_directory(
    state: &mut ClientShellState,
    endpoint_id: &ClientEndpointId,
    path: &str,
) -> String {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Directory {
        endpoint_id: endpoint_id.clone(),
        shown_path: path.into(),
        canonical_path: path.into(),
    });
    let outcome = state.handle_raw_events(Vec::new());
    outcome
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. }
                if matches!(
                    request.method,
                    crate::api::schema::Method::WorkspaceDirectoryPreview(_)
                ) =>
            {
                Some(request.id.clone())
            }
            _ => None,
        })
        .expect("selected directory starts endpoint preview")
}

fn open_directory_search(state: &mut ClientShellState, endpoint_id: &ClientEndpointId) -> String {
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Machine {
        endpoint_id: endpoint_id.clone(),
    });
    navigator.search_focused = true;
    navigator.query.insert("project");
    let outcome = state.handle_raw_events(Vec::new());
    outcome
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. }
                if matches!(
                    request.method,
                    crate::api::schema::Method::WorkspaceSearch(_)
                ) =>
            {
                Some(request.id.clone())
            }
            _ => None,
        })
        .expect("directory query")
}

#[test]
fn fork_merge_directory_preview_ignores_old_errors_and_cancellation_does_not_start_work() {
    let (mut state, remote) = two_endpoints();
    let query = open_directory_search(&mut state, &remote);
    state.handle_endpoint_result(
        "remote-boot",
        &query,
        Ok(crate::api::schema::ResponseResult::WorkspaceSearch {
            candidates: ["/first/project", "/second/project"]
                .into_iter()
                .map(|path| crate::api::schema::WorkspaceSearchCandidate {
                    shown_path: path.into(),
                    canonical_path: path.into(),
                    score: 1.0,
                })
                .collect(),
        }),
    );
    let first = select_directory(&mut state, &remote, "/first/project");
    let second = select_directory(&mut state, &remote, "/second/project");
    assert!(
        !state.accepts_background_endpoint_result(&remote, &first),
        "superseded directory previews must retire their pending result scope"
    );
    state.handle_endpoint_result(
        "remote-boot",
        &second,
        Ok(
            crate::api::schema::ResponseResult::WorkspaceDirectoryPreview {
                preview: crate::api::schema::WorkspaceDirectoryPreview {
                    canonical_path: "/second/project".into(),
                    entries: vec![crate::api::schema::WorkspaceDirectoryEntry {
                        name: "CURRENT_PREVIEW".into(),
                        is_dir: false,
                    }],
                    truncated: false,
                },
            },
        ),
    );
    let result = state.handle_endpoint_result(
        "remote-boot",
        &first,
        Err(ClientShellEndpointError {
            code: Some("directory_preview_failed".into()),
            message: "OBSOLETE_PREVIEW_ERROR".into(),
        }),
    );
    assert!(!result.0);
    assert!(result.1.is_empty());
    let frame = frame_rows(&state.compose(106, 24).unwrap()).join("\n");
    assert!(frame.contains("CURRENT_PREVIEW"), "{frame}");
    assert!(!frame.contains("OBSOLETE_PREVIEW_ERROR"), "{frame}");

    let query = open_directory_search(&mut state, &remote);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Directory {
        endpoint_id: remote.clone(),
        shown_path: "/second/project".into(),
        canonical_path: "/second/project".into(),
    });
    state.cancel_endpoint_request(&query);
}

#[test]
fn fork_merge_reopened_navigator_rejects_the_previous_search_response() {
    let (mut state, remote) = two_endpoints();
    let old = open_directory_search(&mut state, &remote);
    state.handle_input_bytes(b"\x1b");
    let current = open_directory_search(&mut state, &remote);
    assert_ne!(old, current);
    let result = state.handle_endpoint_result(
        "remote-boot",
        &old,
        Ok(crate::api::schema::ResponseResult::WorkspaceSearch {
            candidates: vec![crate::api::schema::WorkspaceSearchCandidate {
                shown_path: "/OBSOLETE_RESULT".into(),
                canonical_path: "/OBSOLETE_RESULT".into(),
                score: 1.0,
            }],
        }),
    );
    assert!(!result.0);
    assert!(result.1.is_empty());
    assert!(!frame_rows(&state.compose(106, 24).unwrap())
        .join("\n")
        .contains("OBSOLETE_RESULT"));
}

#[test]
fn fork_merge_reopened_navigator_rejects_a_previous_creation_result() {
    let (mut state, remote) = two_endpoints();
    open_directory_search(&mut state, &remote);
    let mut outcome = ClientShellInput::default();
    state.create_navigator_workspace(remote.clone(), "/project".into(), &mut outcome);
    let request_id = outcome
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. }
                if matches!(
                    request.method,
                    crate::api::schema::Method::WorkspaceCreate(_)
                ) =>
            {
                Some(request.id.clone())
            }
            _ => None,
        })
        .expect("workspace creation request");
    state.handle_input_bytes(b"\x1b");
    open_directory_search(&mut state, &remote);
    let (changed, actions) = state.handle_endpoint_result(
        "remote-boot",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("workspace_create_failed".into()),
            message: "OBSOLETE_CREATE_ERROR".into(),
        }),
    );
    assert!(!changed);
    assert!(actions.is_empty());
    let frame = frame_rows(&state.compose(106, 24).unwrap()).join("\n");
    assert!(!frame.contains("OBSOLETE_CREATE_ERROR"), "{frame}");
}

#[test]
fn fork_merge_hold_switcher_uses_the_modifiers_of_each_explicit_binding() {
    for (key, modifiers, released_modifier) in [
        (
            KeyCode::Char('q'),
            KeyModifiers::ALT,
            crossterm::event::ModifierKeyCode::LeftAlt,
        ),
        (
            KeyCode::Char('p'),
            KeyModifiers::SUPER,
            crossterm::event::ModifierKeyCode::LeftSuper,
        ),
    ] {
        let (mut state, remote) = two_endpoints();
        let config: Config = toml::from_str("[keys]\nworkspace_switcher = ['ctrl+tab', 'alt+q']\nworkspace_switcher_backward = 'super+p'\n").unwrap();
        state.config = ClientShellConfig::from_config(&config);
        let opened = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(key, modifiers),
        )]);
        assert!(opened.actions.is_empty());
        let key_up = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(key, modifiers)
                .with_kind(crossterm::event::KeyEventKind::Release),
        )]);
        assert!(
            key_up.actions.is_empty(),
            "primary key release while its modifier remains held"
        );
        assert!(state.overlay.is_some());
        let accepted = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(
                KeyCode::Modifier(released_modifier),
                KeyModifiers::empty(),
            )
            .with_kind(crossterm::event::KeyEventKind::Release),
        )]);
        assert!(
            matches!(accepted.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, .. }] if endpoint_id == &remote)
        );
    }
}

fn navigator_target(state: &ClientShellState) -> ClientNavigatorTarget {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    navigator.selected.clone().expect("selected target")
}

#[test]
fn fork_merge_hold_navigation_expands_tabs_and_tab_shortcut_cycles_only_workspaces() {
    let (mut state, remote) = two_endpoints();
    let mut projection = snapshot();
    projection.boot_id = "remote-boot".into();
    let mut tab = projection.tabs[0].clone();
    tab.tab_id = "tab_2".into();
    tab.label = "remote logs".into();
    tab.focused = false;
    projection.tabs.push(tab);
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    state.handle_input_bytes(b"\x1b[9;3u");
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
    ))]);
    let frame = frame_rows(&state.compose(106, 24).unwrap()).join("\n");
    assert!(frame.contains("remote logs"), "{frame}");
    for _ in 0..2 {
        state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('j'),
            KeyModifiers::ALT,
        ))]);
    }
    assert_eq!(
        navigator_target(&state),
        ClientNavigatorTarget::Tab {
            endpoint_id: remote.clone(),
            tab_id: "tab_2".into()
        }
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Tab,
        KeyModifiers::ALT,
    ))]);
    assert_eq!(
        navigator_target(&state),
        ClientNavigatorTarget::Workspace {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into()
        }
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Tab,
        KeyModifiers::ALT,
    ))]);
    for _ in 0..2 {
        state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('j'),
            KeyModifiers::ALT,
        ))]);
    }
    let accepted = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
            KeyModifiers::empty(),
        )
        .with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(
        matches!(accepted.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Tab(id)) }]
        if endpoint_id == &remote && id == "tab_2")
    );
}

#[test]
fn fork_merge_hold_shift_press_collapse_and_search_keep_endpoint_identity() {
    let (mut state, remote) = two_endpoints();
    state.handle_input_bytes(b"\x1b[9;3u");
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Modifier(crossterm::event::ModifierKeyCode::RightShift),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ))]);
    assert_eq!(
        navigator_target(&state),
        ClientNavigatorTarget::Workspace {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into()
        }
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Tab,
        KeyModifiers::ALT,
    ))]);
    for key in ['l', 'j', 'h'] {
        state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char(key),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ))]);
    }
    assert_eq!(
        navigator_target(&state),
        ClientNavigatorTarget::Workspace {
            endpoint_id: remote.clone(),
            workspace_id: "ws_1".into()
        }
    );
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    assert!(!navigator
        .expanded_workspaces
        .contains(&(remote.clone(), "ws_1".into())));
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('s'),
        KeyModifiers::ALT,
    ))]);
    let release = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
            KeyModifiers::empty(),
        )
        .with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(release.actions.is_empty());
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    assert!(navigator.search_focused);
    assert!(navigator.fork.hold.is_none());
    let typed = state.handle_input_bytes(b"project");
    assert!(typed.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { endpoint_id, request, .. }
        if endpoint_id == &remote && matches!(request.method, crate::api::schema::Method::WorkspaceSearch(_)))));
}
