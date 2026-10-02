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
            row.split_once("└─ ")
                .map(|(_, label)| label.trim().to_owned())
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
