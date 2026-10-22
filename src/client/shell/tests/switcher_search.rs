//! Black-box contract tests: Workspace Switcher search stays independent of Go To.
use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crate::client::shell::ClientEndpointFocusTarget;
use crossterm::event::{KeyEventKind, ModifierKeyCode, MouseEvent};

fn search_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "herdr".into(),
        is_linked_worktree: true,
    });
    projected.tabs[0].label = "api".into();
    projected.tabs[0].custom_label = true;
    let mut docs = projected.tabs[0].clone();
    docs.tab_id = "tab_2".into();
    docs.label = "docs".into();
    docs.focused = false;
    projected.tabs.push(docs);
    projected.panes[0].label = Some("editor".into());
    projected.panes[0].cwd = Some("/repo".into());
    projected.panes[0].foreground_cwd = Some("/repo".into());
    let mut logs = projected.panes[0].clone();
    logs.pane_id = "pane_2".into();
    logs.label = Some("logs".into());
    logs.cwd = Some("/var/log".into());
    logs.foreground_cwd = None;
    logs.focused = false;
    projected.panes.push(logs);
    let mut plain = projected.panes[0].clone();
    plain.pane_id = "pane_3".into();
    plain.tab_id = "tab_2".into();
    plain.label = None;
    plain.cwd = None;
    plain.foreground_cwd = None;
    plain.focused = false;
    projected.panes.push(plain);
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("pi".into()),
        display_agent: Some("pi".into()),
        agent: Some("pi".into()),
        title: Some("fix bug".into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    projected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        active_tab_id: "tab_9".into(),
        new_workspace_cwd: "/misc".into(),
        number: 2,
        label: "empty-space".into(),
        custom_label: false,
        branch: None,
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: None,
        focused: false,
        agent_status: AgentStatus::Unknown,
    });
    projected.tabs.push(ClientShellTab {
        tab_id: "tab_9".into(),
        workspace_id: "ws_2".into(),
        number: 1,
        label: "1".into(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Unknown,
    });
    projected.panes.push(ClientShellPane {
        pane_id: "pane_9".into(),
        workspace_id: "ws_2".into(),
        tab_id: "tab_9".into(),
        label: Some("watcher".into()),
        cwd: None,
        foreground_cwd: None,
        focused: false,
        right_click_passthrough: false,
    });
    projected
}
fn search_endpoints() -> (ClientShellState, ClientEndpointId) {
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
    state.set_snapshot(Box::new(search_snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let mut projection = search_snapshot();
    projection.boot_id = "remote-boot".into();
    projection.workspaces[0].label = "Remote directory".into();
    projection.workspaces[1].label = "remote-extra".into();
    projection.panes[0].label = Some("remote-editor".into());
    projection.agents[0].title = Some("deploy 界".into());
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    state.compose(120, 36).unwrap();
    (state, remote)
}

fn key(state: &mut ClientShellState, code: KeyCode, modifiers: KeyModifiers) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        code, modifiers,
    ))])
}

fn open_switcher(state: &mut ClientShellState) {
    let outcome = key(state, KeyCode::Tab, KeyModifiers::ALT);
    assert!(outcome.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}

fn alt_release(state: &mut ClientShellState) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            KeyModifiers::empty(),
        )
        .with_kind(KeyEventKind::Release),
    )])
}

fn targets(state: &mut ClientShellState) -> Vec<ClientNavigatorTarget> {
    state
        .hits
        .navigator_rows
        .iter()
        .map(|(_, target)| target.clone())
        .collect()
}

fn pane_target(pane_id: &str) -> ClientNavigatorTarget {
    ClientNavigatorTarget::Pane {
        endpoint_id: ClientEndpointId::Local,
        pane_id: pane_id.into(),
    }
}

fn remote_pane_target(remote: &ClientEndpointId, pane_id: &str) -> ClientNavigatorTarget {
    ClientNavigatorTarget::Pane {
        endpoint_id: remote.clone(),
        pane_id: pane_id.into(),
    }
}

fn selected(state: &ClientShellState) -> Option<ClientNavigatorTarget> {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    navigator.selected.clone()
}

fn search_focused(state: &ClientShellState) -> bool {
    matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(ClientNavigatorOverlay {
            search_focused: true,
            ..
        }))
    )
}

fn directory_requests(outcome: &ClientShellInput) -> Vec<ClientEndpointId> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint {
                endpoint_id,
                request,
                ..
            } if matches!(
                request.method,
                crate::api::schema::Method::WorkspaceSearch(_)
            ) =>
            {
                Some(endpoint_id.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn switcher_labels_match_sidebar_and_keep_original_label_as_secondary_text() {
    for (case, custom_label, branch, grouped, expected) in [
        ("grouped", false, Some("worktree/feature"), true, "feature"),
        (
            "branch",
            false,
            Some("feature/topic"),
            true,
            "feature/topic",
        ),
        (
            "custom",
            true,
            Some("worktree/feature"),
            true,
            "repo-feature",
        ),
        ("missing-branch", false, None, true, "repo-feature"),
        (
            "ungrouped",
            false,
            Some("worktree/feature"),
            false,
            "repo-feature",
        ),
    ] {
        for remote_endpoint in [false, true] {
            let (mut state, remote) = search_endpoints();
            let endpoint = if remote_endpoint {
                remote
            } else {
                ClientEndpointId::Local
            };
            let mut projected = search_snapshot();
            projected.workspaces[0]
                .worktree
                .as_mut()
                .unwrap()
                .is_linked_worktree = false;
            let child = &mut projected.workspaces[1];
            child.label = "repo-feature".into();
            child.custom_label = custom_label;
            child.branch = branch.map(str::to_owned);
            child.worktree = Some(ClientShellWorktree {
                key: if grouped { "repo" } else { "other-repo" }.into(),
                label: "repo".into(),
                is_linked_worktree: true,
            });
            if remote_endpoint {
                projected.boot_id = "remote-boot".into();
            }
            state.set_endpoint_snapshot(&endpoint, Box::new(projected.clone()));
            let sidebar = state.compose(160, 44).unwrap();
            let sidebar_rect = state
                .hits
                .workspaces
                .iter()
                .find(|hit| hit.endpoint_id == endpoint && hit.workspace_id == "ws_2")
                .unwrap()
                .rect;
            let target = ClientNavigatorTarget::Workspace {
                endpoint_id: endpoint.clone(),
                workspace_id: "ws_2".into(),
            };
            open_switcher(&mut state);
            let quick_switch = state.compose(160, 44).unwrap();
            let quick_rect = state
                .hits
                .navigator_rows
                .iter()
                .find(|(_, row)| *row == target)
                .unwrap()
                .0;
            key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
            state.handle_input_bytes(b"feature");
            let search = state.compose(160, 44).unwrap();
            let search_rect = state
                .hits
                .navigator_rows
                .iter()
                .find(|(_, row)| *row == target)
                .unwrap()
                .0;
            let evidence = format!(
                "SIDEBAR\n{}\nQUICK SWITCH\n{}\nSEARCH\n{}\n",
                frame_rows(&sidebar).join("\n"),
                frame_rows(&quick_switch).join("\n"),
                frame_rows(&search).join("\n"),
            );
            if let Ok(directory) = std::env::var("HERDR_SWITCHER_LABEL_ARTIFACT_DIR") {
                let directory = std::path::Path::new(&directory);
                std::fs::create_dir_all(directory).unwrap();
                std::fs::write(
                    directory.join(format!("{case}-{remote_endpoint}.txt")),
                    &evidence,
                )
                .unwrap();
            }
            cell_symbol_position(&sidebar, sidebar_rect, expected);
            for (frame, rect, title_word) in
                [(&quick_switch, quick_rect, 1), (&search, search_rect, 0)]
            {
                let row = frame_rows(frame)[usize::from(rect.y)]
                    .chars()
                    .skip(usize::from(rect.x))
                    .take(usize::from(rect.width))
                    .collect::<String>();
                assert_eq!(
                    row.split_whitespace().nth(title_word),
                    Some(expected),
                    "{case}: {evidence}"
                );
                assert_eq!(
                    row.matches("repo-feature").count(),
                    1 + usize::from(expected == "repo-feature"),
                    "{case}: {evidence}"
                );
                if remote_endpoint {
                    assert!(row.contains("Remote"), "{case}: {evidence}");
                }
            }
            if case == "grouped" {
                projected.revision += 1;
                projected.workspaces[1].branch = Some("worktree/renamed".into());
                state.set_endpoint_snapshot(&endpoint, Box::new(projected));
                if endpoint.is_local() {
                    let mut refreshed_surface = surface();
                    refreshed_surface.projection_revision = 2;
                    state.set_pane_surface(refreshed_surface);
                }
                let refreshed = state.compose(160, 44).unwrap();
                let rect = state
                    .hits
                    .navigator_rows
                    .iter()
                    .find(|(_, row)| *row == target)
                    .unwrap()
                    .0;
                let row = frame_rows(&refreshed)[usize::from(rect.y)]
                    .chars()
                    .skip(usize::from(rect.x))
                    .take(usize::from(rect.width))
                    .collect::<String>();
                assert_eq!(row.split_whitespace().next(), Some("renamed"), "{row}");
                assert!(row.contains("repo-feature"), "{row}");
                key(&mut state, KeyCode::Esc, KeyModifiers::empty());
                let restored = state.compose(160, 44).unwrap();
                let rect = state
                    .hits
                    .navigator_rows
                    .iter()
                    .find(|(_, row)| *row == target)
                    .unwrap()
                    .0;
                let row = frame_rows(&restored)[usize::from(rect.y)]
                    .chars()
                    .skip(usize::from(rect.x))
                    .take(usize::from(rect.width))
                    .collect::<String>();
                assert_eq!(row.split_whitespace().nth(1), Some("renamed"), "{row}");
                assert!(row.contains("repo-feature"), "{row}");
            }
        }
    }
}

#[test]
fn switcher_labels_leave_direct_go_to_workspace_names_unchanged() {
    let (mut state, _remote) = search_endpoints();
    let mut projected = search_snapshot();
    projected.workspaces[0]
        .worktree
        .as_mut()
        .unwrap()
        .is_linked_worktree = false;
    projected.workspaces[1].label = "repo-feature".into();
    projected.workspaces[1].branch = Some("worktree/feature".into());
    projected.workspaces[1].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    state.set_snapshot(Box::new(projected));
    state.open_navigator_overlay();
    let frame = state.compose(160, 44).unwrap();
    let rect = state.hits.navigator_rows.iter().find(|(_, target)| matches!(target,
        ClientNavigatorTarget::Workspace { endpoint_id: ClientEndpointId::Local, workspace_id }
            if workspace_id == "ws_2"
    )).unwrap().0;
    let row = frame_rows(&frame)[usize::from(rect.y)]
        .chars()
        .skip(usize::from(rect.x))
        .take(usize::from(rect.width))
        .collect::<String>();
    assert_eq!(row.split_whitespace().next(), Some("repo-feature"), "{row}");
    assert!(row.contains("worktree/feature"), "{row}");
}

#[test]
fn switcher_search_slash_s_and_click_entries_latch_off_release_acceptance() {
    for entry in ["slash", "s", "shift-s", "click"] {
        let (mut state, _remote) = search_endpoints();
        open_switcher(&mut state);
        match entry {
            "slash" => {
                key(&mut state, KeyCode::Char('/'), KeyModifiers::ALT);
            }
            "s" => {
                key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
            }
            "shift-s" => {
                key(
                    &mut state,
                    KeyCode::Char('S'),
                    KeyModifiers::ALT | KeyModifiers::SHIFT,
                );
            }
            "click" => {
                state.compose(120, 36).unwrap();
                let search = state.hits.navigator_search;
                state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: search.x,
                    row: search.y,
                    modifiers: KeyModifiers::empty(),
                })]);
            }
            _ => unreachable!(),
        }
        assert!(search_focused(&state), "{entry}");
        let released = alt_release(&mut state);
        assert!(
            released.actions.is_empty(),
            "{entry}: release after search entry must not accept"
        );
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Navigator(_))),
            "{entry}: release must keep the switcher open"
        );
        assert!(search_focused(&state), "{entry}");
        let restored = key(&mut state, KeyCode::Esc, KeyModifiers::empty());
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Navigator(_))),
            "{entry}: first Esc restores the list"
        );
        assert!(!search_focused(&state), "{entry}");
        assert!(restored.actions.is_empty(), "{entry}");
        let released = alt_release(&mut state);
        assert!(
            released.actions.is_empty(),
            "{entry}: release after Esc restore still must not accept"
        );
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Navigator(_))),
            "{entry}: latched list survives release"
        );
        key(&mut state, KeyCode::Esc, KeyModifiers::empty());
        assert!(state.overlay.is_none(), "{entry}: second Esc closes");
    }
}

#[test]
fn switcher_search_reopen_restores_hold_release_acceptance() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    alt_release(&mut state);
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert!(state.overlay.is_none());
    open_switcher(&mut state);
    key(&mut state, KeyCode::Tab, KeyModifiers::ALT);
    let released = alt_release(&mut state);
    assert!(
        matches!(released.actions.as_slice(),
            [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(id)) }]
                if *endpoint_id == remote && id == "ws_1"),
        "reopened switcher accepts on release: {:?}",
        released.actions
    );
    assert!(state.overlay.is_none());
}
#[test]
fn switcher_search_first_escape_restores_list_selection_expansion_and_scroll() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    key(&mut state, KeyCode::Char('l'), KeyModifiers::ALT);
    let saved = selected(&state);
    state.compose(120, 36).unwrap();
    let before = targets(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"remote-editor");
    state.compose(120, 36).unwrap();
    let filtered = targets(&mut state);
    assert!(filtered.len() < before.len());
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(selected(&state), saved);
    state.compose(120, 36).unwrap();
    assert_eq!(targets(&mut state), before);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    assert!(navigator
        .expanded_workspaces
        .contains(&(remote.clone(), "ws_1".into())));
}

#[test]
fn switcher_search_escape_resolves_vanished_saved_tab_to_workspace_then_first_item() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    key(&mut state, KeyCode::Char('l'), KeyModifiers::ALT);
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    assert_eq!(
        selected(&state),
        Some(ClientNavigatorTarget::Tab {
            endpoint_id: remote.clone(),
            tab_id: "tab_2".into()
        })
    );
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let mut projection = search_snapshot();
    projection.boot_id = "remote-boot".into();
    projection.workspaces[0].label = "Remote directory".into();
    projection.workspaces[1].label = "remote-extra".into();
    projection.tabs.retain(|tab| tab.tab_id != "tab_2");
    projection.panes.retain(|pane| pane.pane_id != "pane_3");
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(
        selected(&state),
        Some(ClientNavigatorTarget::Workspace {
            endpoint_id: remote.clone(),
            workspace_id: "ws_1".into()
        })
    );
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert!(state.overlay.is_none());
}

#[test]
fn switcher_search_escape_resolves_vanished_saved_workspace_to_first_item() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    // Hold rows keep snapshot order without MRU: ws_1, ws_2, remote ws_1, ...
    let first = Some(ClientNavigatorTarget::Workspace {
        endpoint_id: ClientEndpointId::Local,
        workspace_id: "ws_1".into(),
    });
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    key(&mut state, KeyCode::Char('j'), KeyModifiers::ALT);
    let saved = selected(&state);
    assert!(matches!(
        saved,
        Some(ClientNavigatorTarget::Workspace { .. })
    ));
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let mut projection = search_snapshot();
    projection.boot_id = "remote-boot".into();
    projection.workspaces[0].label = "Remote directory".into();
    projection.workspaces[1].label = "remote-extra".into();
    projection.workspaces.retain(|workspace| {
        Some(ClientNavigatorTarget::Workspace {
            endpoint_id: remote.clone(),
            workspace_id: workspace.workspace_id.clone(),
        }) != saved
    });
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(selected(&state), first);
}
#[test]
fn switcher_search_parent_match_includes_whole_subtree() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"client-shell");
    state.compose(120, 36).unwrap();
    let local = ClientEndpointId::Local;
    assert_eq!(
        targets(&mut state),
        vec![
            ClientNavigatorTarget::Workspace {
                endpoint_id: local.clone(),
                workspace_id: "ws_1".into(),
            },
            ClientNavigatorTarget::Tab {
                endpoint_id: local.clone(),
                tab_id: "tab_1".into(),
            },
            pane_target("pane_1"),
            pane_target("pane_2"),
            ClientNavigatorTarget::Tab {
                endpoint_id: local.clone(),
                tab_id: "tab_2".into(),
            },
            pane_target("pane_3"),
        ]
    );
}

#[test]
fn switcher_search_child_match_includes_necessary_ancestors_only() {
    let (mut state, remote) = search_endpoints();
    // Keep the child-match check single-endpoint: the shared fixture labels
    // pane_2 "logs" on both endpoints.
    let mut remote_projection = search_snapshot();
    remote_projection.boot_id = "remote-boot".into();
    remote_projection.panes[1].label = Some("remote-tail".into());
    state.set_endpoint_snapshot(&remote, Box::new(remote_projection));
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"logs");
    state.compose(120, 36).unwrap();
    assert_eq!(
        targets(&mut state),
        vec![
            ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id: "ws_1".into(),
            },
            ClientNavigatorTarget::Tab {
                endpoint_id: ClientEndpointId::Local,
                tab_id: "tab_1".into(),
            },
            pane_target("pane_2"),
        ]
    );
}

#[test]
fn switcher_search_cross_field_and_ancestor_context_words_match() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"herdr docs");
    state.compose(120, 36).unwrap();
    let local: Vec<_> = targets(&mut state)
        .into_iter()
        .filter(|target| target.endpoint_id() == &ClientEndpointId::Local)
        .collect();
    assert_eq!(
        local,
        vec![
            ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id: "ws_1".into(),
            },
            ClientNavigatorTarget::Tab {
                endpoint_id: ClientEndpointId::Local,
                tab_id: "tab_2".into(),
            },
            pane_target("pane_3"),
        ]
    );
    state.handle_input_bytes(b"\x7f\x7f\x7f\x7fapi");
    state.compose(120, 36).unwrap();
    let local: Vec<_> = targets(&mut state)
        .into_iter()
        .filter(|target| target.endpoint_id() == &ClientEndpointId::Local)
        .collect();
    assert_eq!(
        local,
        vec![
            ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id: "ws_1".into(),
            },
            ClientNavigatorTarget::Tab {
                endpoint_id: ClientEndpointId::Local,
                tab_id: "tab_1".into(),
            },
            pane_target("pane_1"),
            pane_target("pane_2"),
        ]
    );
    state.handle_input_bytes(b"\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7fhrd rmt-ed");
    state.compose(120, 36).unwrap();
    let remote_rows: Vec<_> = targets(&mut state)
        .into_iter()
        .filter(|target| target.endpoint_id() == &remote)
        .collect();
    assert!(remote_rows.contains(&remote_pane_target(&remote, "pane_1")));
}
fn workspace_groups(state: &mut ClientShellState) -> Vec<String> {
    targets(state)
        .into_iter()
        .filter_map(|target| match target {
            ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id,
            } => Some(workspace_id),
            _ => None,
        })
        .collect()
}

#[test]
fn switcher_search_group_order_uses_rank_then_mru_then_snapshot() {
    let (mut state, _remote) = search_endpoints();
    let mut projected = search_snapshot();
    projected.workspaces[0].label = "terminal".into();
    projected.workspaces[1].label = "term-late".into();
    let mut fuzzy = projected.workspaces[0].clone();
    fuzzy.workspace_id = "ws_3".into();
    fuzzy.label = "totally remote match".into();
    fuzzy.number = 3;
    fuzzy.focused = false;
    projected.workspaces.push(fuzzy);
    projected.focused_workspace_id = Some("ws_1".into());
    state.retire_endpoint(&ClientEndpointId::Local);
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    state.set_snapshot(Box::new(projected));
    state.record_presented_workspace();
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.focused_workspace_id = Some("ws_2".into());
    state.set_snapshot(Box::new(projected));
    state.record_presented_workspace();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"term");
    state.compose(120, 36).unwrap();
    assert_eq!(workspace_groups(&mut state), vec!["ws_2", "ws_1", "ws_3"]);
    state.handle_input_bytes(b" trm");
    state.compose(120, 36).unwrap();
    assert_eq!(workspace_groups(&mut state), vec!["ws_2", "ws_1", "ws_3"]);
}

#[test]
fn switcher_search_machine_match_stays_activatable_and_groups_are_labeled() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"remote");
    let frame = state.compose(120, 36).unwrap();
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("Remote"), "{text}");
    let rows = targets(&mut state);
    assert!(
        rows.contains(&ClientNavigatorTarget::Machine {
            endpoint_id: remote.clone()
        }),
        "machine target remains: {rows:?}"
    );
    assert!(rows.contains(&ClientNavigatorTarget::Tab {
        endpoint_id: remote.clone(),
        tab_id: "tab_1".into()
    }));
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    assert!(
        accepted
            .actions
            .iter()
            .any(|action| matches!(action, ClientShellAction::ActivateEndpoint { .. })),
        "ranked remote target activates: {:?}",
        accepted.actions
    );
}

#[test]
fn switcher_search_missing_optional_metadata_still_matches() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"empty-space watcher");
    state.compose(120, 36).unwrap();
    assert_eq!(
        targets(&mut state),
        vec![
            ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id: "ws_2".into(),
            },
            ClientNavigatorTarget::Tab {
                endpoint_id: ClientEndpointId::Local,
                tab_id: "tab_9".into(),
            },
            pane_target("pane_9"),
        ]
    );
}

#[test]
fn switcher_search_ignores_transient_status_text() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"working");
    let frame = state.compose(120, 36).unwrap();
    let text = frame_rows(&frame).join("\n");
    assert!(
        text.contains("No matching workspaces, tabs, or panes"),
        "{text}"
    );
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    assert!(accepted.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}
#[test]
fn switcher_search_blank_and_whitespace_query_restore_list_without_directory_request() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"x");
    assert_eq!(directory_requests(&typed).len(), 1);
    let blank = state.handle_input_bytes(b"\x7f");
    assert!(directory_requests(&blank).is_empty());
    let frame = state.compose(120, 36).unwrap();
    assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));
    state.compose(120, 36).unwrap();
    let rows = targets(&mut state);
    assert!(
        rows.iter().all(|target| matches!(
            target,
            ClientNavigatorTarget::Workspace { .. } | ClientNavigatorTarget::Tab { .. }
        )),
        "blank query restores the original switcher list: {rows:?}"
    );
    let whitespace = state.handle_input_bytes(b"  ");
    assert!(directory_requests(&whitespace).is_empty());
    let cursor = key(&mut state, KeyCode::Left, KeyModifiers::empty());
    assert!(directory_requests(&cursor).is_empty());
}

#[test]
fn switcher_search_auto_selection_prefers_best_match_and_skips_context_ancestors() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"herdr logs");
    assert_eq!(selected(&state), Some(pane_target("pane_2")));
    state.handle_input_bytes(b"\x7f\x7f\x7f\x7f\x7f");
    assert_eq!(
        selected(&state),
        Some(ClientNavigatorTarget::Workspace {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into()
        })
    );
    state.handle_input_bytes(b"\x7f\x7f\x7f\x7f\x7f");
    assert_eq!(selected(&state), None);
    state.handle_input_bytes(b"docs");
    assert_eq!(
        selected(&state),
        Some(ClientNavigatorTarget::Tab {
            endpoint_id: ClientEndpointId::Local,
            tab_id: "tab_2".into()
        })
    );
}

#[test]
fn switcher_search_query_edits_reset_selection_on_desktop_and_mobile() {
    for mobile in [false, true] {
        for edit in [
            RawInputEvent::Text(crate::input::TextCommit::new(" ")),
            RawInputEvent::Paste(" ".into()),
            RawInputEvent::Key(crate::input::TerminalKey::new(
                KeyCode::Backspace,
                KeyModifiers::empty(),
            )),
        ] {
            let mut state = if mobile {
                mobile_shell()
            } else {
                let (mut state, _) = search_endpoints();
                open_switcher(&mut state);
                key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
                state
            };
            state.handle_input_bytes(b"client-shell ");
            let first = selected(&state);
            assert_eq!(
                first,
                Some(ClientNavigatorTarget::Workspace {
                    endpoint_id: ClientEndpointId::Local,
                    workspace_id: "ws_1".into()
                })
            );
            key(&mut state, KeyCode::Down, KeyModifiers::empty());
            key(&mut state, KeyCode::Down, KeyModifiers::empty());
            assert_eq!(selected(&state), Some(pane_target("pane_1")));
            let description = format!("mobile={mobile}, edit={edit:?}");
            state.handle_raw_events(vec![edit]);
            assert_eq!(selected(&state), first, "{description}");
        }
    }
}

#[test]
fn switcher_search_refresh_and_cursor_motion_preserve_manual_selection() {
    let (mut state, _) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"client-shell");
    let query = directory_query_id(&typed);
    key(&mut state, KeyCode::Down, KeyModifiers::empty());
    key(&mut state, KeyCode::Down, KeyModifiers::empty());
    let manual = Some(pane_target("pane_1"));
    assert_eq!(selected(&state), manual);
    for code in [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::Backspace,
    ] {
        key(&mut state, code, KeyModifiers::empty());
        assert_eq!(selected(&state), manual, "{code:?}");
    }
    state.handle_endpoint_result(
        "boot-1",
        &query,
        Ok(directory_candidates(&["/home/u/client-shell"])),
    );
    assert_eq!(selected(&state), manual);
    let mut refreshed = search_snapshot();
    refreshed.revision += 1;
    refreshed.agents[0].state_change_seq += 1;
    state.set_snapshot(Box::new(refreshed));
    assert_eq!(selected(&state), manual);
    let mut refreshed_surface = surface();
    refreshed_surface.projection_revision += 1;
    refreshed_surface.surface_revision += 1;
    state.set_pane_surface(refreshed_surface);
    state.compose(120, 36).unwrap();
    assert!(targets(&mut state).contains(manual.as_ref().unwrap()));
}

#[test]
fn switcher_search_selection_movement_and_enter_accepts_ranked_targets() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"client-shell");
    assert_eq!(
        selected(&state),
        Some(ClientNavigatorTarget::Workspace {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into()
        })
    );
    key(&mut state, KeyCode::Down, KeyModifiers::empty());
    key(&mut state, KeyCode::Down, KeyModifiers::empty());
    assert_eq!(selected(&state), Some(pane_target("pane_1")));
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    // pane_1 lives outside the active workspace: acceptance activates it.
    assert!(accepted.actions.iter().any(|action| matches!(action,
        ClientShellAction::ActivateEndpoint { endpoint_id, target }
            if *endpoint_id == ClientEndpointId::Local
                && *target == Some(ClientEndpointFocusTarget::Pane("pane_1".into()))
    )));
    assert!(state.overlay.is_none());
}

#[test]
fn switcher_search_click_row_accepts() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"logs");
    state.compose(120, 36).unwrap();
    let (rect, _) = state
        .hits
        .navigator_rows
        .iter()
        .find(|(_, target)| matches!(target, ClientNavigatorTarget::Pane { pane_id, .. } if pane_id == "pane_2"))
        .cloned()
        .expect("pane row hit");
    let accepted = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        accepted.actions.iter().any(|action| matches!(action,
            ClientShellAction::ActivateEndpoint { endpoint_id, target }
                if *endpoint_id == ClientEndpointId::Local
                    && *target == Some(ClientEndpointFocusTarget::Pane("pane_2".into()))
        )),
        "accepted actions: {:?}",
        accepted.actions
    );
    assert!(state.overlay.is_none());
}
fn directory_query_id(outcome: &ClientShellInput) -> String {
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

fn directory_candidates(paths: &[&str]) -> crate::api::schema::ResponseResult {
    crate::api::schema::ResponseResult::WorkspaceSearch {
        candidates: paths
            .iter()
            .map(|path| crate::api::schema::WorkspaceSearchCandidate {
                shown_path: (*path).into(),
                canonical_path: (*path).into(),
                score: 1.0,
            })
            .collect(),
    }
}

fn created_result(workspace_id: &str) -> crate::api::schema::ResponseResult {
    crate::api::schema::ResponseResult::WorkspaceCreated {
        workspace: crate::api::schema::WorkspaceInfo {
            workspace_id: workspace_id.into(),
            number: 9,
            label: "client".into(),
            focused: false,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "tab_created".into(),
            agent_status: AgentStatus::Unknown,
            tokens: HashMap::new(),
            worktree: None,
        },
        tab: crate::api::schema::TabInfo {
            tab_id: "tab_created".into(),
            workspace_id: workspace_id.into(),
            number: 1,
            label: "1".into(),
            focused: false,
            pane_count: 1,
            agent_status: AgentStatus::Unknown,
        },
        root_pane: match pane_scroll_result(0, 0, 1) {
            crate::api::schema::ResponseResult::PaneInfo { pane } => pane,
            _ => unreachable!(),
        },
    }
}

#[test]
fn switcher_search_directories_stay_a_separate_final_group() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"client");
    let query = directory_query_id(&typed);
    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &query,
        Ok(directory_candidates(&[
            "/home/u/client-tools",
            "/home/u/client",
        ])),
    );
    assert!(repaint && actions.is_empty());
    state.compose(120, 36).unwrap();
    let rows = targets(&mut state);
    let last_open = rows
        .iter()
        .rposition(|target| !matches!(target, ClientNavigatorTarget::Directory { .. }))
        .expect("open rows");
    let first_directory = rows
        .iter()
        .position(|target| matches!(target, ClientNavigatorTarget::Directory { .. }))
        .expect("directory rows");
    assert!(first_directory > last_open);
    assert_eq!(
        rows.iter()
            .filter(|target| matches!(target, ClientNavigatorTarget::Directory { .. }))
            .count(),
        2
    );
    let frame = frame_rows(&state.compose(120, 36).unwrap()).join("\n");
    assert!(frame.contains("create workspace"), "{frame}");
}

#[test]
fn switcher_search_directory_endpoint_precedence_no_fanout_and_no_cursor_requests() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"remote");
    assert_eq!(directory_requests(&typed).len(), 1);
    loop {
        if matches!(
            selected(&state),
            Some(target) if target.endpoint_id() == &remote
        ) {
            break;
        }
        let moved = key(&mut state, KeyCode::Down, KeyModifiers::empty());
        assert!(directory_requests(&moved).len() <= 1);
    }
    let cursor = key(&mut state, KeyCode::Left, KeyModifiers::empty());
    assert!(directory_requests(&cursor).is_empty());
    let cursor = key(&mut state, KeyCode::Right, KeyModifiers::empty());
    assert!(directory_requests(&cursor).is_empty());
    let home = key(&mut state, KeyCode::Home, KeyModifiers::empty());
    assert!(directory_requests(&home).is_empty());
    let typed = state.handle_input_bytes(b"2");
    assert_eq!(directory_requests(&typed), vec![remote.clone()]);
}
#[test]
fn switcher_search_directory_error_and_stale_query_stay_scoped() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"client");
    let query = directory_query_id(&typed);
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &query,
        Err(ClientShellEndpointError {
            code: Some("workspace_search_unavailable".into()),
            message: "zoxide missing".into(),
        }),
    );
    assert!(repaint);
    let frame = frame_rows(&state.compose(120, 36).unwrap()).join("\n");
    assert!(frame.contains("zoxide missing"), "{frame}");
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    let (repaint, actions) =
        state.handle_endpoint_result("boot-1", &query, Ok(directory_candidates(&["/stale"])));
    assert!(!repaint && actions.is_empty());
    state.compose(120, 36).unwrap();
    assert!(!targets(&mut state)
        .iter()
        .any(|target| matches!(target, ClientNavigatorTarget::Directory { .. })));
}

#[test]
fn switcher_search_create_response_after_escape_cannot_close_or_activate() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    let typed = state.handle_input_bytes(b"client");
    let query = directory_query_id(&typed);
    state.handle_endpoint_result("boot-1", &query, Ok(directory_candidates(&["/client"])));
    loop {
        if matches!(
            selected(&state),
            Some(ClientNavigatorTarget::Directory { .. })
        ) {
            break;
        }
        key(&mut state, KeyCode::Down, KeyModifiers::empty());
    }
    let created = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    let create = created
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
        .expect("create request");
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    let (repaint, actions) =
        state.handle_endpoint_result("boot-1", &create, Ok(created_result("ws_created")));
    assert!(!repaint && actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}

#[test]
fn switcher_search_unavailable_method_stays_in_switcher() {
    let (mut state, remote) = search_endpoints();
    state.set_endpoint_methods_for(&remote, Some(vec!["pane.read".into()]));
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    loop {
        if matches!(
            selected(&state),
            Some(target) if target.endpoint_id() == &remote
        ) {
            break;
        }
        key(&mut state, KeyCode::Down, KeyModifiers::empty());
    }
    let typed = state.handle_input_bytes(b"remote");
    assert!(directory_requests(&typed).is_empty());
    let frame = frame_rows(&state.compose(120, 36).unwrap()).join("\n");
    assert!(frame.contains("does not support"), "{frame}");
    assert!(frame.contains("Remote directory"), "{frame}");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}
#[test]
fn switcher_search_keeps_inset_geometry_switcher_title_and_target_count() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    let frame = state.compose(200, 50).unwrap();
    assert_eq!(state.hits.navigator_popup, Rect::new(12, 5, 176, 40));
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("Workspace Switcher"), "{text}");
    assert!(!text.contains("Go to"), "{text}");
    assert!(text.contains("items"), "{text}");
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"client-shell");
    let frame = state.compose(200, 50).unwrap();
    assert_eq!(
        state.hits.navigator_popup,
        Rect::new(12, 5, 176, 40),
        "search keeps the inset switcher geometry"
    );
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("Workspace Switcher"), "{text}");
    assert!(!text.contains("terminals"), "{text}");
}

#[test]
fn switcher_search_restarted_endpoint_and_removed_targets_cannot_activate() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"remote-editor");
    assert_eq!(
        selected(&state),
        Some(remote_pane_target(&remote, "pane_1"))
    );
    let mut restarted = search_snapshot();
    restarted.boot_id = "replacement-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(restarted));
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    assert!(accepted.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"remote-editor");
    assert_eq!(
        selected(&state),
        Some(remote_pane_target(&remote, "pane_1"))
    );
    let mut projection = search_snapshot();
    projection.boot_id = "remote-boot".into();
    projection.panes.retain(|pane| pane.pane_id != "pane_1");
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    assert_ne!(
        selected(&state),
        Some(remote_pane_target(&remote, "pane_1"))
    );
}

#[test]
fn switcher_search_preview_follows_ranked_selection() {
    let (mut state, remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_input_bytes(b"remote-editor");
    assert_eq!(
        selected(&state),
        Some(remote_pane_target(&remote, "pane_1"))
    );
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(std::time::Instant::now(), &mut poll);
    assert!(poll.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { endpoint_id, request, .. }
            if endpoint_id == &remote
                && matches!(&request.method, crate::api::schema::Method::PaneLayout(params)
                    if params.pane_id.as_deref() == Some("pane_1"))
    )));
}

#[test]
fn switcher_search_outer_focus_loss_keeps_latched_search_and_list() {
    let (mut state, _remote) = search_endpoints();
    open_switcher(&mut state);
    key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
    state.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
    assert!(search_focused(&state));
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    state.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}

#[test]
fn switcher_search_direct_go_to_is_unchanged() {
    let (mut state, _remote) = search_endpoints();
    state.open_navigator_overlay();
    let frame = state.compose(200, 50).unwrap();
    assert_eq!(state.hits.navigator_popup, Rect::new(42, 4, 116, 42));
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("Go to"), "{text}");
    state.handle_input_bytes(b"/clsh");
    let frame = frame_rows(&state.compose(200, 50).unwrap()).join("\n");
    assert!(frame.contains("No matching agents or terminals"), "{frame}");
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    assert_eq!(navigator.query.as_str(), "clsh");
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert!(state.overlay.is_none());
}
#[test]
#[ignore = "manual switcher search composition scaling profile"]
fn switcher_search_render_scale_profile() {
    for panes in [1, 15] {
        let mut projected = snapshot();
        projected.workspaces[0].worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "herdr".into(),
            is_linked_worktree: true,
        });
        let pane = projected.panes[0].clone();
        projected.panes = (0..panes)
            .map(|number| ClientShellPane {
                pane_id: format!("pane_{number}"),
                label: Some(format!("terminal {number}")),
                focused: number == 0,
                ..pane.clone()
            })
            .collect();
        let config: Config = toml::from_str("[keys]\nworkspace_switcher = 'alt+tab'\n").unwrap();
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_snapshot(Box::new(projected.clone()));
        let mut presented = surface();
        presented.panes[0].pane_id = "pane_0".into();
        state.set_pane_surface(presented);
        key(&mut state, KeyCode::Tab, KeyModifiers::ALT);
        key(&mut state, KeyCode::Char('s'), KeyModifiers::ALT);
        state.handle_input_bytes(b"terminal 0");
        for _ in 0..20 {
            std::hint::black_box(state.compose(106, 30).expect("switcher search frame"));
        }
        let mut samples = Vec::new();
        for _ in 0..1000 {
            let start = std::time::Instant::now();
            std::hint::black_box(state.compose(106, 30).expect("switcher search frame"));
            samples.push(start.elapsed());
        }
        samples.sort();
        eprintln!(
            "switcher search 106x30: {panes} panes median_us={:.1} p95_us={:.1}",
            samples[500].as_secs_f64() * 1_000_000.0,
            samples[950].as_secs_f64() * 1_000_000.0
        );
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(projected));
        state.mode = ClientShellMode::Navigate;
        state
            .compose(45, 60)
            .expect("mobile layout before first text");
        state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
            "terminal 0",
        ))]);
        assert!(
            matches!(
                state.overlay,
                Some(ClientShellOverlay::Navigator(ClientNavigatorOverlay {
                    search_focused: true,
                    ..
                }))
            ),
            "mobile first text must enter a focused search before timing"
        );
        for _ in 0..20 {
            std::hint::black_box(state.compose(45, 60).expect("mobile search frame"));
        }
        let mut samples = Vec::new();
        for _ in 0..1000 {
            let start = std::time::Instant::now();
            std::hint::black_box(state.compose(45, 60).expect("mobile search frame"));
            samples.push(start.elapsed());
        }
        samples.sort();
        eprintln!(
            "mobile switcher search 45x60: {panes} panes median_us={:.1} p95_us={:.1}",
            samples[500].as_secs_f64() * 1_000_000.0,
            samples[950].as_secs_f64() * 1_000_000.0
        );
    }
}
fn mobile_projection() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.tabs[0].label = "api".into();
    projected.tabs[0].custom_label = true;
    let mut docs = projected.tabs[0].clone();
    docs.tab_id = "tab_2".into();
    docs.label = "docs".into();
    docs.focused = false;
    projected.tabs.push(docs);
    projected.panes[0].label = Some("editor".into());
    let mut logs = projected.panes[0].clone();
    logs.pane_id = "pane_2".into();
    logs.label = Some("logs".into());
    logs.focused = false;
    projected.panes.push(logs);
    let mut plain = projected.panes[0].clone();
    plain.pane_id = "pane_3".into();
    plain.tab_id = "tab_2".into();
    plain.label = None;
    plain.focused = false;
    projected.panes.push(plain);
    projected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        active_tab_id: "tab_9".into(),
        new_workspace_cwd: "/misc".into(),
        number: 2,
        label: "empty-space".into(),
        custom_label: false,
        branch: None,
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: None,
        focused: false,
        agent_status: AgentStatus::Unknown,
    });
    projected
}

fn mobile_shell() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(mobile_projection()));
    state.set_pane_surface(surface());
    state.compose(45, 60).expect("mobile header");
    let switch = state.hits.mobile_switch;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: switch.x,
        row: switch.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(45, 60).expect("mobile switcher");
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state
}

fn mobile_frame(state: &mut ClientShellState) -> String {
    let frame = state.compose(45, 60).expect("mobile frame");
    frame_rows(&frame).join("\n")
}

#[test]
fn mobile_switcher_search_first_text_stays_in_full_screen_switcher() {
    for ch in ['s', 'a', '/', '界'] {
        let mut state = mobile_shell();
        let result = key(&mut state, KeyCode::Char(ch), KeyModifiers::NONE);
        assert!(mobile_switcher_search_open(&state), "{ch}: no Go To popup");
        assert_eq!(state.mode, ClientShellMode::Navigate, "{ch}");
        assert!(result.requests.is_empty(), "{ch}: no terminal input");
        let frame = mobile_frame(&mut state);
        assert!(
            frame.contains("switch"),
            "{ch}: full-screen switcher: {frame}"
        );
        assert!(frame.contains(&ch.to_string()), "{ch}: query text: {frame}");
        assert!(
            result.actions.iter().all(
                |action| matches!(action, ClientShellAction::Endpoint { request, .. }
                if matches!(&request.method, crate::api::schema::Method::WorkspaceSearch(_)))
            ),
            "{ch}: only directory requests: {:?}",
            result.actions
        );
    }
}

#[test]
fn mobile_switcher_search_preserves_complete_first_text_unicode_and_paste() {
    for event in [
        RawInputEvent::Text(crate::input::TextCommit::new("你好 world 42")),
        RawInputEvent::Paste("你好 world 42".into()),
        RawInputEvent::Key(
            crate::input::TerminalKey::new(KeyCode::Char('x'), KeyModifiers::NONE)
                .with_generated_text(Some("你好 world 42".into())),
        ),
    ] {
        let mut state = mobile_shell();
        let result = state.handle_raw_events(vec![event]);
        assert!(mobile_switcher_search_open(&state));
        assert!(result.requests.is_empty());
        // Wide characters occupy continuation cells in the composed frame, so
        // the exact query is checked on state and the frame carries it.
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
            panic!("mobile search");
        };
        assert_eq!(navigator.query.as_str(), "你好 world 42");
        let frame = mobile_frame(&mut state);
        assert!(frame.contains("world 42"), "{frame}");
    }
}
#[test]
fn mobile_switcher_search_two_escapes_restore_then_return_to_terminal() {
    let mut state = mobile_shell();
    key(&mut state, KeyCode::Char('s'), KeyModifiers::NONE);
    let first = key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert!(first.actions.is_empty() && first.requests.is_empty());
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    let frame = mobile_frame(&mut state);
    assert!(frame.contains("+ new workspace"), "restored list: {frame}");
    let second = key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert!(second.actions.is_empty() && second.requests.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn mobile_switcher_search_acceptance_focuses_result_and_enters_terminal() {
    let mut state = mobile_shell();
    state.handle_raw_events(vec![RawInputEvent::Paste("client-shell".into())]);
    let frame = mobile_frame(&mut state);
    assert!(frame.contains("switch"), "full-screen switcher: {frame}");
    assert!(frame.contains("client-shell"), "{frame}");
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    assert!(accepted.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(params)
                if params.workspace_id == "ws_1")
    )));
    assert!(accepted.requests.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.navigate_workspace_id.is_none());
}

#[test]
fn mobile_switcher_search_blank_query_restores_list_and_keeps_input_focus() {
    let mut state = mobile_shell();
    let typed = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "x",
    ))]);
    assert_eq!(directory_requests(&typed).len(), 1);
    let blank = key(&mut state, KeyCode::Backspace, KeyModifiers::empty());
    assert!(directory_requests(&blank).is_empty());
    let frame = state.compose(45, 60).expect("blank search frame");
    assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("+ new workspace"), "original list: {text}");
    assert!(text.contains("spaces"), "{text}");
}

#[test]
fn mobile_switcher_search_ranked_hierarchy_and_click_acceptance() {
    let mut state = mobile_shell();
    state.handle_raw_events(vec![RawInputEvent::Paste("logs".into())]);
    state.compose(45, 60).expect("search rows");
    let (rect, _) = state
        .hits
        .navigator_rows
        .iter()
        .find(|(_, target)| matches!(target, ClientNavigatorTarget::Pane { pane_id, .. } if pane_id == "pane_2"))
        .cloned()
        .expect("pane row hit");
    let frame = mobile_frame(&mut state);
    assert!(frame.contains("switch"), "{frame}");
    assert!(frame.contains("client-shell"), "workspace context: {frame}");
    let accepted = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(accepted.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::PaneFocus(params)
                if params.pane_id == "pane_2")
    )));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn mobile_switcher_search_clears_underlying_terminal_and_gesture_hits() {
    let mut state = mobile_shell();
    state.handle_raw_events(vec![RawInputEvent::Paste("logs".into())]);
    state.compose(45, 60).expect("search frame");
    assert!(state.hits.panes.is_empty());
    assert!(state.hits.popup.is_none());
    let tapped = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 30,
        row: 40,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(tapped.requests.is_empty(), "no terminal forwarding");
    assert_eq!(state.mode, ClientShellMode::Navigate);
}
#[test]
fn mobile_switcher_search_restores_expansion_scroll_and_selection() {
    let mut state = mobile_shell();
    for (x, y) in [(5, 20), (9, 20)] {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::empty(),
        })]);
    }
    let expanded = mobile_frame(&mut state);
    assert!(expanded.contains("└─"), "expanded panes: {expanded}");
    key(&mut state, KeyCode::Char('x'), KeyModifiers::NONE);
    let search = mobile_frame(&mut state);
    assert!(!search.contains("+ new workspace"), "searching: {search}");
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    let restored = mobile_frame(&mut state);
    assert!(restored.contains("+ new workspace"), "{restored}");
    assert!(restored.contains("└─"), "expansion restored: {restored}");
}

#[test]
fn mobile_switcher_search_directory_flow_and_create() {
    let mut state = mobile_shell();
    let typed = state.handle_raw_events(vec![RawInputEvent::Paste("dirnine".into())]);
    assert_eq!(directory_requests(&typed).len(), 1);
    let query = directory_query_id(&typed);
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &query,
        Ok(directory_candidates(&["/tmp/dirnine"])),
    );
    assert!(repaint);
    let frame = mobile_frame(&mut state);
    assert!(frame.contains("create workspace"), "{frame}");
    assert!(frame.contains("switch"), "{frame}");
    let accepted = key(&mut state, KeyCode::Enter, KeyModifiers::empty());
    let create = accepted
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
        .expect("create request");
    let (repaint, actions) =
        state.handle_endpoint_result("boot-1", &create, Ok(created_result("ws_created")));
    assert!(repaint);
    assert!(actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(params)
                if params.workspace_id == "ws_created")
    )));
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.navigate_workspace_id.is_none());
}

#[test]
fn mobile_switcher_search_restores_first_valid_item_when_saved_workspace_vanishes() {
    let mut state = mobile_shell();
    key(&mut state, KeyCode::Char('x'), KeyModifiers::NONE);
    let mut projected = mobile_projection();
    projected
        .workspaces
        .retain(|workspace| workspace.workspace_id != "ws_1");
    projected.focused_workspace_id = Some("ws_2".into());
    state.set_snapshot(Box::new(projected));
    key(&mut state, KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    let target = state
        .navigate_workspace_id
        .as_ref()
        .expect("restored selection");
    assert!(target.matches(&ClientEndpointId::Local, "ws_2"));
}
/// Shared mobile Switcher-search controller: the Navigator overlay carries
/// Switcher search state; the frame stays the full Switcher surface.
fn mobile_switcher_search_open(state: &ClientShellState) -> bool {
    matches!(&state.overlay,
    Some(ClientShellOverlay::Navigator(navigator))
        if navigator.search_focused
            && navigator.fork.quick_switch
            && matches!(&navigator.fork.switcher_search,
                Some(super::super::navigator::SwitcherSearch {
                    origin: super::super::navigator::SwitcherOrigin::Mobile(_),
                    ..
                })))
}
