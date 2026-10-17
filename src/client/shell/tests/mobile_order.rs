use super::*;

#[test]
#[ignore = "manual mobile switcher composition scaling profile"]
fn mobile_switcher_render_scale_profile() {
    for count in [1, 15] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut projected = snapshot();
        let workspace = projected.workspaces[0].clone();
        let tab = projected.tabs[0].clone();
        let pane = projected.panes[0].clone();
        projected.workspaces = (1..=count)
            .map(|number| ClientShellWorkspace {
                workspace_id: format!("ws_{number}"),
                active_tab_id: format!("tab_{number}"),
                number,
                focused: number == 1,
                ..workspace.clone()
            })
            .collect();
        projected.tabs = (1..=count)
            .map(|number| ClientShellTab {
                workspace_id: format!("ws_{number}"),
                tab_id: format!("tab_{number}"),
                number,
                focused: number == 1,
                ..tab.clone()
            })
            .collect();
        projected.panes = (1..=count)
            .map(|number| ClientShellPane {
                workspace_id: format!("ws_{number}"),
                tab_id: format!("tab_{number}"),
                pane_id: format!("pane_{number}"),
                focused: number == 1,
                ..pane.clone()
            })
            .collect();
        state.set_snapshot(Box::new(projected));
        for number in (1..=count).rev() {
            present(
                &mut state,
                &ClientEndpointId::Local,
                &format!("ws_{number}"),
            );
        }
        state.mode = ClientShellMode::Navigate;
        state.navigate_workspace_id = state.focused_navigation_target();
        for _ in 0..20 {
            std::hint::black_box(state.compose(45, 60).unwrap());
        }
        let mut samples = Vec::new();
        for _ in 0..1000 {
            let start = std::time::Instant::now();
            std::hint::black_box(state.compose(45, 60).unwrap());
            samples.push(start.elapsed());
        }
        samples.sort();
        eprintln!("mobile switcher 45x60: {count} populated workspaces/panes median_us={:.1} p95_us={:.1}",
            samples[500].as_secs_f64() * 1_000_000.0, samples[950].as_secs_f64() * 1_000_000.0);
    }
}

fn projection() -> ClientShellSnapshot {
    let mut projected = snapshot();
    for number in 2..=3 {
        let mut workspace = projected.workspaces[0].clone();
        workspace.workspace_id = format!("ws_{number}");
        workspace.number = number;
        workspace.label = format!("space-{number}");
        workspace.branch = Some(format!("worktree/branch-{number}"));
        workspace.focused = false;
        projected.workspaces.push(workspace);
    }
    for (index, linked) in [(0, false), (2, true)] {
        projected.workspaces[index].worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: linked,
        });
    }
    projected
}

fn present(state: &mut ClientShellState, endpoint: &ClientEndpointId, workspace: &str) {
    let mut projected = state
        .endpoints
        .iter()
        .find(|entry| &entry.endpoint_id == endpoint)
        .unwrap()
        .snapshot
        .as_deref()
        .unwrap()
        .clone();
    projected.revision += 1;
    projected.focused_workspace_id = Some(workspace.into());
    for entry in &mut projected.workspaces {
        entry.focused = entry.workspace_id == workspace;
    }
    let mut presented = surface();
    presented.boot_id = projected.boot_id.clone();
    presented.projection_revision = projected.revision;
    presented.surface_revision = projected.revision;
    state.set_endpoint_snapshot(endpoint, Box::new(projected));
    assert!(state.activate_endpoint_projection(endpoint));
    state.set_pane_surface(presented);
}

fn federated() -> (ClientShellState, ClientEndpointId) {
    let (mut state, remote) = super::fork_navigator::two_endpoints();
    for endpoint in [&ClientEndpointId::Local, &remote] {
        state.retire_endpoint(endpoint);
        state.set_endpoint_status(endpoint, ClientEndpointStatus::Online);
    }
    state.set_snapshot(Box::new(projection()));
    let mut projected = projection();
    projected.boot_id = "remote-boot".into();
    projected.workspaces[0].label = "Remote directory".into();
    state.set_endpoint_snapshot(&remote, Box::new(projected));
    present(&mut state, &ClientEndpointId::Local, "ws_3");
    present(&mut state, &remote, "ws_2");
    present(&mut state, &ClientEndpointId::Local, "ws_1");
    (state, remote)
}

fn artifact(name: &str, frame: &FrameData) {
    if let Ok(directory) = std::env::var("HERDR_MOBILE_ARTIFACT_DIR") {
        let buffer = frame.to_ratatui_buffer().unwrap();
        let mut proof = frame_rows(frame).join("\n");
        proof.push_str("\n\nCell styles (x,y: fg/bg/modifiers):\n");
        for y in 0..frame.height {
            for x in 0..frame.width {
                let cell = &buffer[(x, y)];
                if !cell.symbol().trim().is_empty() {
                    proof.push_str(&format!(
                        "{x},{y}: {:?}/{:?}/{:?} {}\n",
                        cell.fg,
                        cell.bg,
                        cell.modifier,
                        cell.symbol()
                    ));
                }
            }
        }
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{name}.txt")),
            proof,
        )
        .unwrap();
    }
}

fn visible_order(state: &mut ClientShellState) -> Vec<(ClientEndpointId, String)> {
    state.mode = ClientShellMode::Navigate;
    state.mobile_switcher_scroll = 0;
    state.compose(45, 60).unwrap();
    state
        .hits
        .mobile_targets
        .iter()
        .filter_map(|(_, target)| match target {
            ClientMobileTarget::Workspace {
                endpoint_id,
                workspace_id,
            } => Some((endpoint_id.clone(), workspace_id.clone())),
            _ => None,
        })
        .collect()
}

fn reopened_order(state: &mut ClientShellState) -> Vec<(ClientEndpointId, String)> {
    state.mode = ClientShellMode::Terminal;
    state.compose(45, 60).unwrap();
    visible_order(state)
}

fn workspace_rect(state: &ClientShellState, endpoint: &ClientEndpointId, workspace: &str) -> Rect {
    state
        .hits
        .mobile_targets
        .iter()
        .find_map(|(rect, target)| {
            matches!(target, ClientMobileTarget::Workspace { endpoint_id, workspace_id }
            if endpoint_id == endpoint && workspace_id == workspace)
            .then_some(*rect)
        })
        .unwrap()
}

fn binding(state: &mut ClientShellState, action: crate::input::KeybindAction) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
    outcome
}

#[test]
fn mobile_order_local_label_is_subdued_only_when_a_remote_is_saved() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.mode = ClientShellMode::Navigate;
    let frame = state.compose(45, 30).unwrap();
    artifact("local-only", &frame);
    let rect = workspace_rect(&state, &ClientEndpointId::Local, "ws_1");
    let rows = frame_rows(&frame);
    assert!(rows[usize::from(rect.y)].contains("client-shell"));
    assert!(!rows[usize::from(rect.y)].contains("Local"));
    assert!(!rows[usize::from(rect.y + 1)].contains("Local"));

    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Local".into(),
        target: "test@localhost".into(),
        session: "test".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    for status in [
        ClientEndpointStatus::Reconnecting,
        ClientEndpointStatus::Connecting,
        ClientEndpointStatus::Online,
        ClientEndpointStatus::Attention,
    ] {
        state.set_endpoint_status(&remote, status);
        let frame = state.compose(45, 35).unwrap();
        artifact(&format!("saved-{status:?}"), &frame);
        let rect = workspace_rect(&state, &ClientEndpointId::Local, "ws_1");
        let rows = frame_rows(&frame);
        assert_eq!(rect.height, 2);
        assert!(!rows[usize::from(rect.y)].contains("Local"));
        assert!(rows[usize::from(rect.y + 1)].contains("main · tab 1 · Local"));
        let buffer = frame.to_ratatui_buffer().unwrap();
        let (x, y) = cell_symbol_position(
            &frame,
            Rect::new(rect.x, rect.y + 1, rect.width, 1),
            "Local",
        );
        assert_eq!(buffer[(x, y)].fg, state.config.palette.overlay0);
        assert!(!buffer[(x, y)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD));
    }
    let mut projected = snapshot();
    projected.revision += 1;
    projected.workspaces[0].branch =
        Some("a-very-long-branch-name-that-fills-the-detail-line".into());
    state.set_snapshot(Box::new(projected));
    let mut presented = surface();
    presented.projection_revision = 2;
    presented.surface_revision = 2;
    state.set_pane_surface(presented);
    let frame = state.compose(30, 35).unwrap();
    artifact("truncated-detail", &frame);
    let rect = workspace_rect(&state, &ClientEndpointId::Local, "ws_1");
    let rows = frame_rows(&frame);
    assert!(rows[usize::from(rect.y + 1)].contains("a-very-long-branch-name"));
    assert!(!rows[usize::from(rect.y + 1)].contains("Local"));
    assert_eq!(rect.height, 2);
}

#[test]
fn mobile_order_is_flat_global_mru_with_stable_unknown_fallback() {
    let (mut state, remote) = federated();
    let local = ClientEndpointId::Local;
    assert_eq!(
        visible_order(&mut state),
        vec![
            (local.clone(), "ws_1".into()),
            (remote.clone(), "ws_2".into()),
            (local.clone(), "ws_3".into()),
            (local, "ws_2".into()),
            (remote.clone(), "ws_1".into()),
            (remote.clone(), "ws_3".into()),
        ]
    );
    let frame = state.compose(45, 60).unwrap();
    artifact("global-mru", &frame);
    let rows = frame_rows(&frame);
    for (endpoint, workspace) in [(ClientEndpointId::Local, "ws_3"), (remote.clone(), "ws_3")] {
        let rect = workspace_rect(&state, &endpoint, workspace);
        assert!(rows[usize::from(rect.y)].contains("space-3"));
        assert!(!rows[usize::from(rect.y)].contains('└'));
        assert!(!rows[usize::from(rect.y)].contains("branch-3"));
    }
    let rect = workspace_rect(&state, &remote, "ws_1");
    assert!(rows[usize::from(rect.y)].contains("Remote · Remote directory"));
    state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
    let frame = state.compose(45, 60).unwrap();
    artifact("stale-remote", &frame);
    let rect = workspace_rect(&state, &remote, "ws_1");
    let buffer = frame.to_ratatui_buffer().unwrap();
    let (x, y) = cell_symbol_position(&frame, rect, "Remote");
    assert_eq!(buffer[(x, y)].fg, state.config.palette.overlay0);
    assert!(buffer[(x, y)]
        .modifier
        .contains(ratatui::style::Modifier::DIM));
    assert!(frame_rows(&frame)[usize::from(rect.y + 1)].contains("reconnecting"));
    let mut outcome = ClientShellInput::default();
    state.activate_mobile_target(
        Some(ClientMobileTarget::Workspace {
            endpoint_id: remote,
            workspace_id: "ws_1".into(),
        }),
        &mut outcome,
    );
    assert!(outcome.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn mobile_order_changes_only_after_presentation_and_rejects_old_identity_recency() {
    let (mut state, remote) = federated();
    let before = visible_order(&mut state);
    let mut outcome = ClientShellInput::default();
    state.activate_mobile_target(
        Some(ClientMobileTarget::Workspace {
            endpoint_id: remote.clone(),
            workspace_id: "ws_3".into(),
        }),
        &mut outcome,
    );
    assert!(!outcome.actions.is_empty());
    assert_eq!(visible_order(&mut state), before);
    let mut projected = state
        .endpoints
        .iter()
        .find(|entry| entry.endpoint_id == remote)
        .unwrap()
        .snapshot
        .as_deref()
        .unwrap()
        .clone();
    projected.revision += 1;
    projected.focused_workspace_id = Some("ws_3".into());
    state.set_endpoint_snapshot(&remote, Box::new(projected));
    assert!(state.activate_endpoint_projection(&remote));
    assert_eq!(
        state.workspace_mru.front().unwrap().endpoint_id,
        ClientEndpointId::Local
    );
    let mut presented = surface();
    presented.boot_id = "remote-boot".into();
    presented.projection_revision = state.snapshot.as_deref().unwrap().revision;
    state.set_pane_surface(presented);
    assert_eq!(
        reopened_order(&mut state)[0],
        (remote.clone(), "ws_3".into())
    );
    present(&mut state, &ClientEndpointId::Local, "ws_1");
    for (generation, boot) in [(1, "remote-boot"), (2, "replacement-boot")] {
        let mut replacement = projection();
        replacement.boot_id = boot.into();
        state.set_endpoint_snapshot_for_generation(&remote, generation, Box::new(replacement));
        let order = reopened_order(&mut state);
        assert_eq!(
            &order[3..],
            &[
                (remote.clone(), "ws_1".into()),
                (remote.clone(), "ws_2".into()),
                (remote.clone(), "ws_3".into())
            ]
        );
    }
}

#[test]
fn mobile_order_direction_actions_and_drag_follow_displayed_sequence() {
    use crate::input::KeybindAction;
    let (mut state, remote) = federated();
    let order = visible_order(&mut state);
    state.navigate_workspace_id = state.focused_navigation_target();
    for (endpoint, workspace) in order.iter().skip(1) {
        let outcome = state.handle_input_bytes(b"\x1b[B");
        assert!(outcome.actions.is_empty());
        assert_eq!(
            state.navigate_workspace_id,
            state.navigation_target(endpoint, workspace)
        );
    }
    state.handle_input_bytes(b"\x1b[A");
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&remote, "ws_1")
    );
    state.mode = ClientShellMode::Terminal;
    let next = binding(&mut state, KeybindAction::NextWorkspace);
    assert!(
        matches!(next.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }] if *endpoint_id == remote && id == "ws_2")
    );
    let previous = binding(&mut state, KeybindAction::PreviousWorkspace);
    assert!(
        matches!(previous.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }] if *endpoint_id == remote && id == "ws_3")
    );
    let indexed = binding(&mut state, KeybindAction::SwitchWorkspace(1));
    assert!(
        matches!(indexed.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }] if *endpoint_id == remote && id == "ws_2")
    );
    state.compose(45, 60).unwrap();
    let switch = state.hits.mobile_switch;
    let mouse = |kind, row| {
        RawInputEvent::Mouse(MouseEvent {
            kind,
            column: switch.x,
            row,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![mouse(
        MouseEventKind::Down(MouseButton::Left),
        switch.y,
    )]);
    state.handle_raw_events(vec![mouse(MouseEventKind::Drag(MouseButton::Left), 10)]);
    state.handle_raw_events(vec![mouse(MouseEventKind::Drag(MouseButton::Left), 12)]);
    assert!(
        matches!(state.mobile_drag.as_ref(), Some(ClientMobileTarget::Workspace { endpoint_id, workspace_id }) if *endpoint_id == remote && workspace_id == "ws_2")
    );
    let release = state.handle_raw_events(vec![mouse(MouseEventKind::Up(MouseButton::Left), 12)]);
    assert!(
        matches!(release.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }] if *endpoint_id == remote && id == "ws_2")
    );
}

#[test]
fn mobile_order_does_not_change_desktop_grouping_or_endpoint_order() {
    let (mut state, remote) = federated();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.focused_navigation_target();
    let frame = state.compose(106, 40).unwrap();
    artifact("desktop-unchanged", &frame);
    let order: Vec<_> = state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.endpoint_id.clone(), hit.workspace_id.clone()))
        .collect();
    assert_eq!(
        order,
        vec![
            (ClientEndpointId::Local, "ws_1".into()),
            (ClientEndpointId::Local, "ws_3".into()),
            (ClientEndpointId::Local, "ws_2".into()),
            (remote.clone(), "ws_1".into()),
            (remote.clone(), "ws_3".into()),
            (remote, "ws_2".into()),
        ]
    );
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_3")
    );
}

#[test]
fn workspace_recency_batch_admission_preserves_order_and_ignores_output() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let local = ClientEndpointId::Local;
    state.set_snapshot(Box::new(projection()));
    present(&mut state, &local, "ws_1");
    let baseline = visible_order(&mut state);
    let mut added = state.snapshot.as_deref().unwrap().clone();
    added.revision += 1;
    for number in 4..=5 {
        added.workspaces.push(ClientShellWorkspace {
            workspace_id: format!("ws_{number}"),
            number,
            label: format!("space-{number}"),
            focused: false,
            ..added.workspaces[0].clone()
        });
    }
    state.set_snapshot(Box::new(added));
    present(&mut state, &local, "ws_1");
    assert_eq!(visible_order(&mut state), baseline);
    let expected = ["ws_4", "ws_5", "ws_1", "ws_2", "ws_3"].map(|id| (local.clone(), id.into()));
    assert_eq!(reopened_order(&mut state), expected);
    present(&mut state, &local, "ws_1");
    assert_eq!(reopened_order(&mut state), expected);
    let unchanged = state.snapshot.as_ref().unwrap().clone();
    state.set_snapshot(unchanged);
    assert_eq!(reopened_order(&mut state), expected);
    present(&mut state, &local, "ws_2");
    assert_eq!(visible_order(&mut state), expected);
    assert_eq!(
        reopened_order(&mut state),
        ["ws_2", "ws_4", "ws_5", "ws_1", "ws_3"].map(|id| (local.clone(), id.into()))
    );
    let mut removed = state.snapshot.as_deref().unwrap().clone();
    removed.revision += 1;
    removed
        .workspaces
        .retain(|workspace| workspace.workspace_id != "ws_4");
    state.set_snapshot(Box::new(removed));
    present(&mut state, &local, "ws_2");
    assert_eq!(
        visible_order(&mut state),
        ["ws_2", "ws_5", "ws_1", "ws_3"].map(|id| (local.clone(), id.into()))
    );
}

#[test]
fn workspace_recency_reconnect_and_boot_snapshots_are_baselines() {
    let (mut state, remote) = federated();
    let local = ClientEndpointId::Local;
    let baseline = visible_order(&mut state);
    let mut reconnected = projection();
    reconnected.boot_id = "remote-boot".into();
    reconnected.revision += 10;
    reconnected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_4".into(),
        number: 4,
        label: "remote-existing".into(),
        focused: false,
        ..reconnected.workspaces[0].clone()
    });
    state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
    state.set_endpoint_snapshot_for_generation(&remote, 1, Box::new(reconnected.clone()));
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let expected = [
        (local.clone(), "ws_1"),
        (local.clone(), "ws_3"),
        (local.clone(), "ws_2"),
        (remote.clone(), "ws_1"),
        (remote.clone(), "ws_2"),
        (remote.clone(), "ws_3"),
        (remote.clone(), "ws_4"),
    ]
    .map(|(endpoint, id)| (endpoint, id.into()));
    assert_ne!(baseline, expected);
    assert_eq!(reopened_order(&mut state), expected);
    reconnected.revision += 1;
    reconnected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_5".into(),
        number: 5,
        label: "remote-new".into(),
        focused: false,
        ..reconnected.workspaces[0].clone()
    });
    state.set_endpoint_snapshot_for_generation(&remote, 1, Box::new(reconnected.clone()));
    assert_eq!(visible_order(&mut state), expected);
    assert_eq!(
        reopened_order(&mut state)[0],
        (remote.clone(), "ws_5".into())
    );
    reconnected.boot_id = "replacement-boot".into();
    reconnected.revision = 1;
    reconnected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_6".into(),
        number: 6,
        label: "replacement-existing".into(),
        focused: false,
        ..reconnected.workspaces[0].clone()
    });
    state.set_endpoint_snapshot_for_generation(&remote, 1, Box::new(reconnected));
    let order = reopened_order(&mut state);
    assert_eq!(order[0], (local, "ws_1".into()));
    assert_eq!(order.last(), Some(&(remote, "ws_6".into())));
}
