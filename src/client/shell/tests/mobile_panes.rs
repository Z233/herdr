use super::*;

fn projection() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.tabs[0].label = "build".into();
    projected.tabs[0].custom_label = true;
    let mut second = projected.tabs[0].clone();
    second.tab_id = "tab_2".into();
    second.label = "2".into();
    second.custom_label = false;
    second.focused = false;
    projected.tabs.push(second);
    let pane = projected.panes[0].clone();
    projected.panes = (1..=4)
        .map(|number| ClientShellPane {
            pane_id: format!("pane_{number}"),
            tab_id: format!("tab_{}", if number <= 2 { 1 } else { 2 }),
            label: (number == 1).then(|| "named-shell".into()),
            focused: number == 1,
            ..pane.clone()
        })
        .collect();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_3".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_2".into(),
        name: Some("named-agent".into()),
        display_agent: Some("pi".into()),
        agent: Some("pi".into()),
        title: Some("agent-title".into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Blocked,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    projected
}

fn state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projection()));
    state.set_pane_surface(surface());
    state
}

fn event(state: &mut ClientShellState, kind: MouseEventKind, x: u16, y: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn open(state: &mut ClientShellState) {
    state.compose(45, 60).unwrap();
    let rect = state.hits.mobile_switch;
    assert!(!rect.is_empty(), "switcher must be closed before opening");
    event(
        state,
        MouseEventKind::Down(MouseButton::Left),
        rect.x,
        rect.y,
    );
    state.compose(45, 60).unwrap();
    event(state, MouseEventKind::Drag(MouseButton::Left), 5, 20);
}

fn edge_scroll_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut projected = projection();
    let workspace = projected.workspaces[0].clone();
    for number in 2..=20 {
        projected.workspaces.push(ClientShellWorkspace {
            workspace_id: format!("ws_{number}"),
            label: format!("workspace-{number}"),
            number,
            focused: false,
            ..workspace.clone()
        });
    }
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(45, 14).unwrap();
    let button = state.hits.mobile_switch;
    event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        button.x,
        button.y,
    );
    state.compose(45, 14).unwrap();
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 7);
    state
}

fn edge_frame(state: &mut ClientShellState, name: &str) {
    let frame = state.compose(45, 14).unwrap();
    if let Ok(directory) = std::env::var("HERDR_MOBILE_ARTIFACT_DIR") {
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{name}.txt")),
            frame_rows(&frame).join("\n"),
        )
        .unwrap();
    }
}

#[test]
fn mobile_edge_scroll_down_then_up_same_hold_and_exact_release() {
    let mut state = edge_scroll_state();
    edge_frame(&mut state, "edge-before");
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 13);
    let start = std::time::Instant::now();
    assert!(!state.tick_mobile_gesture(start));
    for step in 1..=24 {
        state.tick_mobile_gesture(start + std::time::Duration::from_millis(step * 200));
        edge_frame(&mut state, "edge-bottom");
    }
    assert!(state.mobile_switcher_scroll > 0);
    assert!(
        matches!(state.mobile_drag.as_ref(), Some(ClientMobileTarget::Workspace { workspace_id, .. }) if workspace_id == "ws_20")
    );
    let bottom_scroll = state.mobile_switcher_scroll;
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_secs(6)));
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 3);
    let up_start = start + std::time::Duration::from_secs(7);
    for step in 1..=24 {
        state.tick_mobile_gesture(up_start + std::time::Duration::from_millis(step * 200));
        edge_frame(&mut state, "edge-top");
    }
    assert!(state.mobile_switcher_scroll < bottom_scroll);
    assert!(!state.tick_mobile_gesture(up_start + std::time::Duration::from_secs(6)));
    let selected = state.mobile_drag.clone().expect("held selection");
    assert!(
        matches!(&selected, ClientMobileTarget::Workspace { workspace_id, .. } if workspace_id == "ws_1")
    );
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, target)| target == &selected));
    let released = event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 3);
    assert!(released.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { endpoint_id: ClientEndpointId::Local, request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_1")
    )));
    assert!(!state.tick_mobile_gesture(up_start + std::time::Duration::from_secs(10)));
}

#[test]
fn mobile_edge_scroll_stops_in_middle_and_release_keeps_selection() {
    let mut state = edge_scroll_state();
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 13);
    let start = std::time::Instant::now();
    assert!(state.tick_mobile_gesture(start + std::time::Duration::from_millis(200)));
    edge_frame(&mut state, "edge-one-step");
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_millis(201)));
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 7);
    edge_frame(&mut state, "edge-middle");
    let selected = state.mobile_drag.clone().unwrap();
    let scroll = state.mobile_switcher_scroll;
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_secs(10)));
    assert_eq!(state.mobile_drag.as_ref(), Some(&selected));
    assert_eq!(state.mobile_switcher_scroll, scroll);
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, target)| target == &selected));
    let released = event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 7);
    let ClientMobileTarget::Workspace { workspace_id, .. } = selected else {
        panic!("workspace selection")
    };
    assert!(released.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { endpoint_id: ClientEndpointId::Local, request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == workspace_id)
    )));
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_secs(11)));
}

#[test]
fn mobile_edge_scroll_rejects_restarted_endpoint_and_preserves_pane_expansion() {
    let mut state = edge_scroll_state();
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 7);
    edge_frame(&mut state, "edge-expanded");
    assert_eq!(leaves(&mut state, "edge-expanded-leaves"), labels());
    state.compose(45, 14).unwrap();
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 13, 13);
    let start = std::time::Instant::now();
    assert!(state.tick_mobile_gesture(start + std::time::Duration::from_millis(200)));
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.boot_id = "restarted".into();
    refresh(&mut state, projected);
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_secs(1)));
    let released = event(&mut state, MouseEventKind::Up(MouseButton::Left), 13, 13);
    assert!(released.actions.is_empty());
}

#[test]
fn mobile_edge_scroll_skips_cached_offline_workspace_in_both_directions() {
    use crate::client::endpoint::{ProfileId, SavedSshEndpoint};

    let (mut state, offline) = super::fork_navigator::two_endpoints();
    let profiles = [
        SavedSshEndpoint {
            id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            label: "Offline".into(),
            target: "offline@localhost".into(),
            session: "test".into(),
            enabled: true,
        },
        SavedSshEndpoint {
            id: ProfileId::parse("1123456789abcdef0123456789abcdef").unwrap(),
            label: "Online".into(),
            target: "online@localhost".into(),
            session: "test".into(),
            enabled: true,
        },
    ];
    let online = ClientEndpointId::Ssh(profiles[1].id.clone());
    state.set_endpoint_catalog(&profiles);
    state.set_endpoint_status(&offline, ClientEndpointStatus::Reconnecting);
    state.set_endpoint_status(&online, ClientEndpointStatus::Online);
    let mut projected = snapshot();
    projected.boot_id = "third-boot".into();
    state.set_endpoint_snapshot(&online, Box::new(projected));
    state.compose(45, 60).unwrap();
    let button = state.hits.mobile_switch;
    event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        button.x,
        button.y,
    );
    state.compose(45, 60).unwrap();
    let displayed_endpoints: Vec<_> = state
        .hits
        .mobile_targets
        .iter()
        .filter_map(|(_, target)| match target {
            ClientMobileTarget::Workspace { endpoint_id, .. } => Some(endpoint_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        displayed_endpoints,
        [ClientEndpointId::Local, offline, online.clone()]
    );
    state.compose(45, 6).unwrap();
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 5);
    let start = std::time::Instant::now();
    assert!(state.tick_mobile_gesture(start + std::time::Duration::from_millis(200)));
    state.compose(45, 6).unwrap();
    let remote_target = ClientMobileTarget::Workspace {
        endpoint_id: online,
        workspace_id: "ws_1".into(),
    };
    assert_eq!(state.mobile_drag.as_ref(), Some(&remote_target));
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, target)| target == &remote_target));
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_millis(400)));
    // One row avoids a direct vertical step while moving into the top edge band.
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 4);
    assert!(state.tick_mobile_gesture(start + std::time::Duration::from_millis(600)));
    state.compose(45, 6).unwrap();
    let local_target = ClientMobileTarget::Workspace {
        endpoint_id: ClientEndpointId::Local,
        workspace_id: "ws_1".into(),
    };
    assert_eq!(state.mobile_drag.as_ref(), Some(&local_target));
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, target)| target == &local_target));
    assert!(!state.tick_mobile_gesture(start + std::time::Duration::from_millis(800)));
    let released = event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 4);
    assert!(released.actions.iter().any(|action| matches!(action,
        ClientShellAction::ActivateEndpoint { endpoint_id: ClientEndpointId::Local, target: Some(ClientEndpointFocusTarget::Workspace(id)) } if id == "ws_1"
    )));
}

fn expand(state: &mut ClientShellState) {
    event(state, MouseEventKind::Drag(MouseButton::Left), 9, 20);
}

fn leaves(state: &mut ClientShellState, name: &str) -> Vec<String> {
    let frame = state.compose(45, 60).unwrap();
    if let Ok(directory) = std::env::var("HERDR_MOBILE_ARTIFACT_DIR") {
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{name}.txt")),
            frame_rows(&frame).join("\n"),
        )
        .unwrap();
    }
    frame_rows(&frame)
        .iter()
        .filter_map(|row| {
            row.split_once("└─ ").map(|(_, label)| {
                label
                    .trim()
                    .strip_prefix("● ")
                    .unwrap_or(label.trim())
                    .to_owned()
            })
        })
        .collect()
}

fn labels() -> Vec<String> {
    [
        "build · named-shell · 1",
        "build · terminal · 2",
        "named-agent · 1",
        "terminal · 2",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn refresh(state: &mut ClientShellState, mut projected: ClientShellSnapshot) {
    projected.revision = state.snapshot.as_ref().unwrap().revision + 1;
    let mut presented = surface();
    presented.boot_id = projected.boot_id.clone();
    presented.projection_revision = projected.revision;
    // A coherent non-focused surface allows composition, but proves no pane use.
    presented.panes.clear();
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(presented);
}

fn present(state: &mut ClientShellState, pane_id: &str) {
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.revision += 1;
    let pane = projected
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .unwrap();
    projected.focused_pane_id = Some(pane_id.into());
    projected.focused_tab_id = Some(pane.tab_id.clone());
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == pane_id;
    }
    let mut presented = surface();
    presented.boot_id = projected.boot_id.clone();
    presented.projection_revision = projected.revision;
    presented.surface_revision = projected.revision;
    presented.panes[0].pane_id = pane_id.into();
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(presented);
}

fn close(state: &mut ClientShellState) {
    event(state, MouseEventKind::Up(MouseButton::Left), 9, 20);
    state.compose(45, 60).unwrap();
    if state.mode == ClientShellMode::Navigate {
        let rect = state.hits.mobile_close;
        event(
            state,
            MouseEventKind::Down(MouseButton::Left),
            rect.x,
            rect.y,
        );
        state.compose(45, 60).unwrap();
    }
}

fn assert_focus(result: &ClientShellInput, endpoint: &ClientEndpointId, pane: &str) {
    assert!(result.actions.iter().any(|action| match action {
        ClientShellAction::Endpoint { endpoint_id, request, .. } => {
            endpoint_id == endpoint
                && matches!(&request.method, crate::api::schema::Method::PaneFocus(params) if params.pane_id == pane)
        }
        ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Pane(id)) } => {
            endpoint_id == endpoint && id == pane
        }
        _ => false,
    }), "exact pane focus missing: {:?}", result.actions);
}

#[test]
fn mobile_panes_status_icons_follow_config_and_survive_narrow_stale_rows() {
    use crate::config::StatusIndicatorStyle;

    for indicators in [StatusIndicatorStyle::Dots, StatusIndicatorStyle::Symbols] {
        for status in [
            AgentStatus::Working,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            let mut state = state();
            state.config.status_indicators = indicators;
            let mut projected = projection();
            projected.agents[0].name = Some("named-agent-with-a-long-name".into());
            if status == AgentStatus::Done {
                projected.agents[0].agent_status = AgentStatus::Working;
                refresh(&mut state, projected.clone());
                projected.agents[0].state_change_seq += 1;
            }
            projected.agents[0].agent_status = status;
            refresh(&mut state, projected);
            open(&mut state);
            expand(&mut state);
            for stale in [false, true] {
                if stale {
                    state.set_endpoint_status(
                        &ClientEndpointId::Local,
                        ClientEndpointStatus::Reconnecting,
                    );
                }
                for width in [45, 20, 12] {
                    let frame = state.compose(width, 60).unwrap();
                    let buffer = frame.to_ratatui_buffer().unwrap();
                    let pane_rows = state
                        .hits
                        .mobile_targets
                        .iter()
                        .filter_map(|(rect, target)| match target {
                            ClientMobileTarget::Pane { pane_id, .. } => Some((rect, pane_id)),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(pane_rows.len(), 4);
                    for (rect, pane_id) in pane_rows {
                        let x = rect.x + 8;
                        let cell = &buffer[(x, rect.y)];
                        if pane_id == "pane_3" {
                            assert_eq!(cell.symbol(), status_icon(status, indicators));
                            assert_eq!(
                                cell.fg,
                                if stale {
                                    state.config.palette.overlay0
                                } else {
                                    status_color(status, &state.config.palette)
                                }
                            );
                            assert_eq!(cell.modifier.contains(Modifier::DIM), stale);
                            assert_eq!(buffer[(x + 1, rect.y)].symbol(), " ");
                        } else {
                            assert!(!["●", "○", "·", "×", "◐", "✓"].contains(&cell.symbol()));
                        }
                        assert!(frame_rows(&frame)[usize::from(rect.y + 1)].contains("/"));
                    }
                    if let Ok(directory) = std::env::var("HERDR_MOBILE_ARTIFACT_DIR") {
                        std::fs::write(
                            std::path::Path::new(&directory).join(format!(
                                "status-{indicators:?}-{status:?}-{stale}-{width}.txt"
                            )),
                            frame_rows(&frame).join("\n"),
                        )
                        .unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn mobile_panes_expansion_labels_match_desktop_and_workspace_remains_selected() {
    let mut state = state();
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!()
    };
    let desktop = super::super::aggregate_navigation::navigator_rows(
        &state.endpoints,
        &state.active_endpoint_id,
        navigator,
    );
    let desktop_labels = desktop
        .iter()
        .filter_map(|row| {
            matches!(row.target, ClientNavigatorTarget::Pane { .. }).then_some(row.label.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(desktop_labels, labels());
    state.overlay = None;
    open(&mut state);
    expand(&mut state);
    assert_eq!(leaves(&mut state, "panes-expanded"), desktop_labels);
    assert!(
        matches!(state.mobile_drag.as_ref(), Some(ClientMobileTarget::Workspace { workspace_id, .. }) if workspace_id == "ws_1")
    );
    let result = event(&mut state, MouseEventKind::Up(MouseButton::Left), 9, 20);
    assert!(result.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. } if matches!(&request.method,
            crate::api::schema::Method::WorkspaceFocus(params) if params.workspace_id == "ws_1")
    )));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn mobile_panes_single_and_empty_workspace_have_no_intermediate_tab() {
    for count in [0, 1] {
        let mut state = state();
        let mut projected = snapshot();
        projected.panes.truncate(count);
        if count == 0 {
            projected.focused_pane_id = None;
        }
        refresh(&mut state, projected);
        let mut presented = surface();
        presented.projection_revision = state.snapshot.as_ref().unwrap().revision;
        presented.panes.truncate(count);
        state.set_pane_surface(presented);
        open(&mut state);
        expand(&mut state);
        assert_eq!(
            leaves(&mut state, &format!("panes-count-{count}")),
            if count == 0 {
                Vec::<String>::new()
            } else {
                vec!["client-shell".into()]
            }
        );
    }
}

#[test]
fn mobile_panes_two_row_steps_horizontal_threshold_and_leaf_release() {
    let mut state = state();
    open(&mut state);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 8, 20);
    assert!(leaves(&mut state, "panes-threshold").is_empty());
    expand(&mut state);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 21);
    assert!(matches!(
        state.mobile_drag.as_ref(),
        Some(ClientMobileTarget::Workspace { .. })
    ));
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 26);
    assert_eq!(leaves(&mut state, "panes-selected-cross-tab"), labels());
    // A right swipe on a leaf resets both baselines; it cannot add a level.
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 13, 26);
    assert_eq!(leaves(&mut state, "panes-leaf-right"), labels());
    let result = event(&mut state, MouseEventKind::Up(MouseButton::Left), 13, 26);
    assert_focus(&result, &ClientEndpointId::Local, "pane_3");
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn mobile_panes_left_collapse_selects_parent_and_fold_keeps_order() {
    let mut state = state();
    open(&mut state);
    expand(&mut state);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 24);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 24);
    assert!(leaves(&mut state, "panes-folded").is_empty());
    assert!(matches!(
        state.mobile_drag.as_ref(),
        Some(ClientMobileTarget::Workspace { .. })
    ));
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 24);
    assert_eq!(leaves(&mut state, "panes-unfolded"), labels());
    let result = event(&mut state, MouseEventKind::Up(MouseButton::Left), 9, 24);
    assert!(result.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. } if matches!(&request.method,
            crate::api::schema::Method::WorkspaceFocus(params) if params.workspace_id == "ws_1")
    )));
}

#[test]
fn mobile_panes_recency_is_presented_focus_only_and_order_is_frozen_until_reopen() {
    let mut state = state();
    present(&mut state, "pane_3");
    present(&mut state, "pane_2");
    // A raw projection from another client is not presentation.
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.focused_pane_id = Some("pane_4".into());
    refresh(&mut state, projected);
    open(&mut state);
    expand(&mut state);
    let expected = vec![
        labels()[1].clone(),
        labels()[2].clone(),
        labels()[0].clone(),
        labels()[3].clone(),
    ];
    assert_eq!(leaves(&mut state, "panes-mru"), expected);
    // A coherent surface must contain the focused projected pane.
    let mut wrong = surface();
    wrong.projection_revision = state.snapshot.as_ref().unwrap().revision;
    wrong.panes[0].pane_id = "pane_3".into();
    state.set_pane_surface(wrong);
    // Preview and a failed request must not record pane use.
    state.open_navigator_overlay();
    state.overlay = None;
    state.set_endpoint_methods(Some(Vec::new()));
    let mut failure = ClientShellInput::default();
    state.focus_or_activate(
        ClientEndpointId::Local,
        ClientEndpointFocusTarget::Pane("pane_4".into()),
        &mut failure,
    );
    assert!(failure.actions.is_empty());
    state.set_endpoint_methods(None);
    present(&mut state, "pane_4");
    assert_eq!(leaves(&mut state, "panes-frozen"), expected);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 20);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 20);
    assert_eq!(leaves(&mut state, "panes-frozen-refold"), expected);
    close(&mut state);
    open(&mut state);
    assert_eq!(
        leaves(&mut state, "panes-reopened"),
        vec![
            labels()[3].clone(),
            labels()[1].clone(),
            labels()[2].clone(),
            labels()[0].clone()
        ]
    );
}

#[test]
fn mobile_panes_append_delete_refresh_and_reused_identity_do_not_reorder_survivors() {
    let mut state = state();
    open(&mut state);
    expand(&mut state);
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.panes.remove(1);
    let mut added = projected.panes[0].clone();
    added.pane_id = "pane_new".into();
    added.label = Some("new-shell".into());
    projected.panes.insert(0, added);
    refresh(&mut state, projected);
    // Labels change their snapshot ordinal, but retained pane identities keep their order.
    assert_eq!(
        leaves(&mut state, "panes-append-delete"),
        vec![
            "build · named-shell · 2",
            "named-agent · 1",
            "terminal · 2",
            "build · new-shell · 1"
        ]
    );
    close(&mut state);
    let mut projected = projection();
    projected.boot_id = "replacement-boot".into();
    refresh(&mut state, projected);
    let mut presented = surface();
    presented.boot_id = "replacement-boot".into();
    presented.projection_revision = state.snapshot.as_ref().unwrap().revision;
    presented.panes.clear();
    state.set_pane_surface(presented);
    open(&mut state);
    assert_eq!(leaves(&mut state, "panes-reused-boot"), labels());
}

#[test]
fn mobile_panes_deleted_selected_or_anchor_and_endpoint_identity_changes_cancel_release() {
    for invalidation in [
        "selected",
        "anchor",
        "workspace",
        "offline",
        "boot",
        "generation",
        "membership",
    ] {
        let mut state = state();
        open(&mut state);
        expand(&mut state);
        event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 26);
        if invalidation == "anchor" {
            // Leaf right no-op makes that leaf the new vertical anchor.
            event(&mut state, MouseEventKind::Drag(MouseButton::Left), 13, 26);
            event(&mut state, MouseEventKind::Drag(MouseButton::Left), 13, 28);
        }
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        match invalidation {
            "selected" | "anchor" => projected.panes.retain(|pane| pane.pane_id != "pane_3"),
            "workspace" => projected.workspaces.clear(),
            "offline" => state
                .set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting),
            "boot" => projected.boot_id = "new-boot".into(),
            "generation" => {
                projected.revision += 1;
                state.set_endpoint_snapshot_for_generation(
                    &ClientEndpointId::Local,
                    77,
                    Box::new(projected.clone()),
                );
            }
            "membership" => projected.panes[2].workspace_id = "different-workspace".into(),
            _ => unreachable!(),
        }
        if invalidation != "generation" {
            refresh(&mut state, projected);
        }
        state.compose(45, 60);
        let x = if invalidation == "anchor" { 13 } else { 9 };
        let y = if invalidation == "anchor" { 28 } else { 26 };
        let released = event(&mut state, MouseEventKind::Up(MouseButton::Left), x, y);
        assert!(
            released.actions.is_empty(),
            "{invalidation}: {:?}",
            released.actions
        );
        assert_eq!(state.mode, ClientShellMode::Navigate, "{invalidation}");
    }
}

#[test]
fn mobile_panes_colliding_endpoint_ids_use_independent_history_and_exact_focus() {
    let (mut state, remote) = super::fork_navigator::two_endpoints();
    state.set_snapshot(Box::new(projection()));
    state.set_pane_surface(surface());
    present(&mut state, "pane_2");
    let mut remote_projection = projection();
    remote_projection.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(remote_projection));
    open(&mut state);
    // Local children are collapsed, so one vertical step selects Remote.
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 22);
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 22);
    assert_eq!(leaves(&mut state, "panes-remote-collisions"), labels());
    event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 28);
    let result = event(&mut state, MouseEventKind::Up(MouseButton::Left), 9, 28);
    assert_focus(&result, &remote, "pane_3");
}

#[test]
fn mobile_panes_pending_future_nonfocused_surfaces_and_generation_do_not_inherit_recency() {
    let mut state = state();
    present(&mut state, "pane_2");
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.revision += 1;
    projected.focused_pane_id = Some("pane_4".into());
    let mut future = surface();
    future.projection_revision = projected.revision;
    future.panes[0].pane_id = "pane_4".into();
    future.panes[0].focused = false;
    state.set_pane_surface(future);
    state.set_snapshot(Box::new(projected));
    open(&mut state);
    expand(&mut state);
    assert_eq!(
        leaves(&mut state, "panes-nonfocused-surface"),
        vec![
            labels()[1].clone(),
            labels()[0].clone(),
            labels()[2].clone(),
            labels()[3].clone()
        ]
    );
    close(&mut state);
    let mut projected = projection();
    projected.revision = state.snapshot.as_ref().unwrap().revision + 1;
    state.set_endpoint_snapshot_for_generation(
        &ClientEndpointId::Local,
        78,
        Box::new(projected.clone()),
    );
    let mut presented = surface();
    presented.projection_revision = projected.revision;
    presented.panes.clear();
    state.set_pane_surface(presented);
    open(&mut state);
    assert_eq!(leaves(&mut state, "panes-new-generation"), labels());
}

#[test]
fn mobile_panes_width_exit_starts_new_order() {
    let mut state = state();
    open(&mut state);
    expand(&mut state);
    event(&mut state, MouseEventKind::Up(MouseButton::Left), 9, 20);
    // Reopen with expansion memory and keep this opening on a simple tap.
    open(&mut state);
    event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 20);
    present(&mut state, "pane_4");
    assert_eq!(leaves(&mut state, "panes-before-width-exit"), labels());
    state.compose(120, 60).unwrap();
    state.compose(45, 60).unwrap();
    assert_eq!(
        leaves(&mut state, "panes-width-reopened"),
        vec![
            labels()[3].clone(),
            labels()[0].clone(),
            labels()[1].clone(),
            labels()[2].clone()
        ]
    );
}

#[test]
fn mobile_panes_suspension_keeps_open_order_when_cancel_returns_to_switcher() {
    let mut state = state();
    state.config.prompt_new_workspace_name = true;
    open(&mut state);
    expand(&mut state);
    // End the hold without activating; keep the switcher open.
    event(&mut state, MouseEventKind::Down(MouseButton::Left), 1, 1);
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.focused_workspace_id = None;
    projected.focused_tab_id = None;
    projected.focused_pane_id = None;
    refresh(&mut state, projected);
    state.compose(45, 60).unwrap();
    let rect = state
        .hits
        .mobile_targets
        .iter()
        .find_map(|(rect, target)| {
            matches!(target, ClientMobileTarget::NewWorkspace).then_some(*rect)
        })
        .unwrap();
    event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        rect.x,
        rect.y,
    );
    assert!(state.mobile_switcher_suspended);
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.panes.swap(2, 3);
    refresh(&mut state, projected);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_eq!(
        leaves(&mut state, "panes-suspended"),
        vec![
            "build · named-shell · 1",
            "build · terminal · 2",
            "named-agent · 2",
            "terminal · 1"
        ]
    );
}

#[test]
#[ignore = "manual expanded mobile pane composition scaling profile"]
fn mobile_panes_expanded_render_scale_profile() {
    for count in [1, 15] {
        let mut state = state();
        let mut projected = snapshot();
        let pane = projected.panes[0].clone();
        projected.panes = (1..=count)
            .map(|number| ClientShellPane {
                pane_id: format!("pane_{number}"),
                label: Some(format!("shell-{number}")),
                ..pane.clone()
            })
            .collect();
        let agent = projection().agents[0].clone();
        projected.agents = projected
            .panes
            .iter()
            .map(|pane| ClientShellAgent {
                pane_id: pane.pane_id.clone(),
                tab_id: pane.tab_id.clone(),
                ..agent.clone()
            })
            .collect();
        refresh(&mut state, projected);
        let mut presented = surface();
        presented.projection_revision = state.snapshot.as_ref().unwrap().revision;
        state.set_pane_surface(presented);
        open(&mut state);
        expand(&mut state);
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
        eprintln!(
            "expanded mobile switcher 45x60: {count} populated panes median_us={:.1} p95_us={:.1}",
            samples[500].as_secs_f64() * 1_000_000.0,
            samples[950].as_secs_f64() * 1_000_000.0
        );
    }
}
