use super::*;
use crossterm::event::{MouseButton, MouseEventKind};

#[test]
fn fork_merge_mobile_hold_and_drag_survives_an_unrelated_snapshot_refresh() {
    let (mut state, remote) = super::fork_navigator::two_endpoints();
    state.compose(45, 28).unwrap();
    let button = state.hits.mobile_switch;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: button.x,
        row: button.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(45, 28).unwrap();
    let (rect, target) = state
        .hits
        .mobile_targets
        .iter()
        .find(|(_, target)| {
            matches!(target,
                ClientMobileTarget::Workspace { endpoint_id, .. } if *endpoint_id == remote
            )
        })
        .cloned()
        .expect("remote workspace in mobile list");
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);
    let mut refresh = snapshot();
    refresh.revision += 1;
    refresh.workspaces[0].branch = Some("background-refresh".into());
    let mut refreshed_surface = surface();
    refreshed_surface.projection_revision = refresh.revision;
    refreshed_surface.surface_revision += 1;
    state.set_snapshot(Box::new(refresh));
    state.set_pane_surface(refreshed_surface);
    state.compose(45, 28).unwrap();
    assert_eq!(state.mobile_drag.as_ref(), Some(&target));
    let result = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(result.actions.iter().any(|action| matches!(action,
        ClientShellAction::ActivateEndpoint { endpoint_id, .. } if *endpoint_id == remote
    )));
}

fn mobile_event(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn fork_merge_mobile_horizontal_expansion_and_collapse_preserve_the_parent_target() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    let mut projection = snapshot();
    let mut second = projection.tabs[0].clone();
    second.tab_id = "tab_2".into();
    second.label = "second".into();
    second.focused = false;
    projection.tabs.push(second);
    state.set_snapshot(Box::new(projection));
    state.compose(45, 35).unwrap();
    let switch = state.hits.mobile_switch;
    mobile_event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        switch.x,
        switch.y,
    );
    state.compose(45, 35).unwrap();
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 15);
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 8, 15);
    assert!(state.mobile_expanded_groups.is_empty());
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 15);
    let frame = state.compose(45, 35).unwrap();
    assert!(frame_rows(&frame).join("\n").contains("second"));
    assert!(state
        .mobile_expanded_groups
        .contains(&(ClientEndpointId::Local, "ws_1".into())));
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 9, 19);
    assert!(
        matches!(state.mobile_drag.as_ref(), Some(ClientMobileTarget::Tab { tab_id, .. }) if tab_id == "tab_2")
    );
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 19);
    state.compose(45, 35).unwrap();
    assert!(state.mobile_expanded_groups.is_empty());
    assert!(
        matches!(state.mobile_drag.as_ref(), Some(ClientMobileTarget::Workspace { workspace_id, .. }) if workspace_id == "ws_1")
    );
    let result = mobile_event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 19);
    assert!(result.actions.iter().any(|action| matches!(action,
        ClientShellAction::ActivateEndpoint { endpoint_id: ClientEndpointId::Local, target: Some(ClientEndpointFocusTarget::Workspace(id)) } if id == "ws_1"
    )));
}

#[test]
fn fork_merge_mobile_tap_and_dead_zone_stay_open_and_removed_anchor_cannot_activate() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    state.compose(45, 35).unwrap();
    let switch = state.hits.mobile_switch;
    mobile_event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        switch.x,
        switch.y,
    );
    let released = mobile_event(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        switch.x,
        switch.y,
    );
    assert!(released.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.mode = ClientShellMode::Terminal;
    state.compose(45, 35).unwrap();
    mobile_event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        switch.x,
        switch.y,
    );
    state.compose(45, 35).unwrap();
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 15);
    let released = mobile_event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 16);
    assert!(released.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.mode = ClientShellMode::Terminal;
    state.compose(45, 35).unwrap();
    mobile_event(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        switch.x,
        switch.y,
    );
    state.compose(45, 35).unwrap();
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 15);
    mobile_event(&mut state, MouseEventKind::Drag(MouseButton::Left), 5, 17);
    let mut projection = snapshot();
    projection.revision += 1;
    projection.workspaces.clear();
    projection.tabs.clear();
    projection.panes.clear();
    projection.focused_workspace_id = None;
    projection.focused_tab_id = None;
    projection.focused_pane_id = None;
    let mut empty_surface = surface();
    empty_surface.projection_revision = projection.revision;
    empty_surface.surface_revision += 1;
    empty_surface.panes.clear();
    empty_surface.splits.clear();
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(empty_surface);
    state.compose(45, 35).unwrap();
    let released = mobile_event(&mut state, MouseEventKind::Up(MouseButton::Left), 5, 17);
    assert!(released.actions.is_empty());
}

#[test]
fn fork_merge_agent_sidebar_visibility_hides_all_machine_groups() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    state.config.agents.visible = false;
    state.compose(106, 24).unwrap();
    assert!(state.hits.endpoint_agents.is_empty());
    assert!(state.hits.agent_body.is_empty());
    assert!(!state.hits.workspaces.is_empty());
}

#[test]
fn fork_merge_zoom_map_renders_hidden_neighbors_with_endpoint_qualified_status() {
    let (mut state, remote) = super::fork_navigator::two_endpoints();
    let mut projection = snapshot();
    projection.tabs[0].zoomed = true;
    let mut neighbor = projection.panes[0].clone();
    neighbor.pane_id = "pane_2".into();
    neighbor.focused = false;
    projection.panes.push(neighbor);
    projection.agents = vec![ClientShellAgent {
        pane_id: "pane_2".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("Local agent".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Blocked,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }];
    state.set_snapshot(Box::new(projection.clone()));
    projection.boot_id = "remote-boot".into();
    projection.agents[0].agent_status = AgentStatus::Working;
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    let layout = serde_json::from_value(serde_json::json!({
        "workspace_id":"ws_1", "tab_id":"tab_1", "focused_pane_id":"pane_1", "zoomed":true,
        "area":{"x":0,"y":0,"width":80,"height":20},
        "panes":[
            {"pane_id":"pane_1","focused":true,"rect":{"x":0,"y":0,"width":40,"height":20}},
            {"pane_id":"pane_2","focused":false,"rect":{"x":40,"y":0,"width":40,"height":20}}
        ],
        "splits":[]
    }))
    .unwrap();
    state.zoom_map.set_layout(layout);
    state.compose(45, 28).unwrap();
    assert!(state
        .hits
        .zoom_map_panes
        .iter()
        .any(|(_, pane_id)| pane_id == "pane_2"));
    assert_eq!(
        super::super::render::endpoint_pane_agent_status(
            &state.endpoints,
            &ClientEndpointId::Local,
            "pane_2"
        ),
        Some(AgentStatus::Blocked)
    );
    let (rect, _) = state
        .hits
        .zoom_map_panes
        .iter()
        .find(|(_, pane)| pane == "pane_2")
        .unwrap();
    let result = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(result.actions.iter().any(|action| matches!(action,
        ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Pane(pane_id)) }
            if *endpoint_id == ClientEndpointId::Local && pane_id == "pane_2"
    )));
}

#[test]
fn fork_merge_zoom_map_retires_removed_neighbors_before_refresh() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    let mut projection = snapshot();
    projection.tabs[0].zoomed = true;
    let mut neighbor = projection.panes[0].clone();
    neighbor.pane_id = "pane_2".into();
    neighbor.focused = false;
    projection.panes.push(neighbor);
    state.set_snapshot(Box::new(projection.clone()));
    state.zoom_map.set_layout(
        serde_json::from_value(serde_json::json!({
            "workspace_id":"ws_1", "tab_id":"tab_1", "focused_pane_id":"pane_1", "zoomed":true,
            "area":{"x":0,"y":0,"width":80,"height":20},
            "panes":[
                {"pane_id":"pane_1","focused":true,"rect":{"x":0,"y":0,"width":40,"height":20}},
                {"pane_id":"pane_2","focused":false,"rect":{"x":40,"y":0,"width":40,"height":20}}
            ],
            "splits":[]
        }))
        .unwrap(),
    );
    state.compose(45, 28).unwrap();
    assert!(state
        .hits
        .zoom_map_panes
        .iter()
        .any(|(_, pane)| pane == "pane_2"));
    projection.revision += 1;
    projection.panes.retain(|pane| pane.pane_id != "pane_2");
    let mut updated_surface = surface();
    updated_surface.projection_revision = projection.revision;
    updated_surface.surface_revision += 1;
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(updated_surface);
    state.compose(45, 28).unwrap();
    assert!(
        state.hits.zoom_map_panes.is_empty(),
        "removed panes must not remain clickable while the next layout is pending"
    );
}

#[test]
fn fork_merge_zoom_map_retries_and_refreshes_without_a_projection_change() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    let mut projection = snapshot();
    projection.tabs[0].zoomed = true;
    state.set_snapshot(Box::new(projection));
    state.compose(45, 28).unwrap();
    for succeed in [false, true, true] {
        state.zoom_map.requested_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        let mut outcome = ClientShellInput::default();
        state.request_zoom_map_layout(&mut outcome);
        let (boot_id, request_id) = outcome
            .actions
            .iter()
            .find_map(|action| match action {
                ClientShellAction::Endpoint {
                    boot_id, request, ..
                } if matches!(request.method, crate::api::schema::Method::PaneLayout(_)) => {
                    Some((boot_id.clone(), request.id.clone()))
                }
                _ => None,
            })
            .expect("zoom map retries failures and refreshes successful layouts");
        let mut duplicate = ClientShellInput::default();
        state.request_zoom_map_layout(&mut duplicate);
        assert!(duplicate.actions.is_empty(), "one layout request at a time");
        let result = if succeed {
            Ok(crate::api::schema::ResponseResult::PaneLayout {
                layout: serde_json::from_value(serde_json::json!({
                    "workspace_id":"ws_1", "tab_id":"tab_1", "focused_pane_id":"pane_1", "zoomed":true,
                    "area":{"x":0,"y":0,"width":80,"height":20},
                    "panes":[{"pane_id":"pane_1","focused":true,"rect":{"x":0,"y":0,"width":80,"height":20}}],
                    "splits":[]
                })).unwrap(),
            })
        } else {
            Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "Timed out".into(),
            })
        };
        state.handle_endpoint_result(&boot_id, &request_id, result);
        let mut throttled = ClientShellInput::default();
        state.zoom_map.requested_at = Some(std::time::Instant::now());
        state.request_zoom_map_layout(&mut throttled);
        assert!(throttled.actions.is_empty(), "refresh remains throttled");
    }
}
