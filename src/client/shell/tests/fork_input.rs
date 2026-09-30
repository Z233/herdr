use super::*;

fn chord_shell(timeout_ms: u64) -> ClientShellState {
    let config: Config = toml::from_str(&format!(
        "[keys]\nchord_timeout_ms = {timeout_ms}\nworkspace_picker = 'prefix+w'\nopen_pane_left = 'prefix+w+h'\n"
    )).unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "pane.split".into(),
        "pane.split.directional".into(),
    ]));
    state.compose(106, 20).unwrap();
    state
}

#[test]
fn fork_merge_default_workspace_chord_timeout_does_not_open_navigator() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).unwrap();
    state.handle_input_bytes(b"\x02w");
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Prefix);
    let mut expired = ClientShellInput::default();
    state.expire_prefix_chord(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        &mut expired,
    );
    assert!(state.overlay.is_none());
    assert!(expired.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn fork_merge_workspace_chord_timeout_and_escape_do_not_open_navigator() {
    let mut state = chord_shell(500);
    state.handle_input_bytes(b"\x02w");
    assert!(state.overlay.is_none());
    let mut expired = ClientShellInput::default();
    state.expire_prefix_chord(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        &mut expired,
    );
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(expired.actions.is_empty());

    let mut state = chord_shell(500);
    state.handle_input_bytes(b"\x02w\x1b");
    let mut canceled = ClientShellInput::default();
    state.expire_prefix_chord(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        &mut canceled,
    );
    assert!(state.overlay.is_none());
    assert!(canceled.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn fork_merge_zero_chord_timeout_does_not_restore_removed_picker() {
    let mut state = chord_shell(0);
    let outcome = state.handle_input_bytes(b"\x02w");
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(outcome.actions.is_empty());
}

#[test]
fn fork_merge_config_coexists_and_three_step_chord_opens_left_on_the_selected_pane() {
    let config: Config = toml::from_str(
        "[keys]\nworkspace_picker = 'prefix+w'\nworkspace_switcher = 'alt+tab'\nopen_pane_left = 'prefix+w+h'\n",
    ).expect("upstream Navigator and fork switcher bindings coexist");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "pane.split".into(),
        "pane.split.directional".into(),
    ]));
    state.compose(106, 20).expect("composed frame");
    let prefix = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('b'),
        KeyModifiers::CONTROL,
    ))]);
    assert!(prefix.actions.is_empty());
    for ch in ['w', 'h'] {
        let outcome = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(KeyCode::Char(ch), KeyModifiers::empty()),
        )]);
        if ch == 'w' {
            assert!(outcome.actions.is_empty(), "ambiguous short binding waits");
        } else {
            let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
                panic!("completed chord must issue an endpoint request");
            };
            let request = serde_json::to_value(request).unwrap();
            assert_eq!(request["method"], "pane.split.directional");
            assert_eq!(request["params"]["direction"], "left");
            assert_eq!(request["params"]["target_pane_id"], "pane_1");
        }
    }
}

#[test]
fn fork_merge_four_way_split_requires_an_advertised_directional_method() {
    for advertised in [
        None,
        Some(vec!["pane.split".into()]),
        Some(vec!["pane.split".into(), "pane.split.directional".into()]),
    ] {
        for (key, direction) in [('h', "left"), ('j', "down"), ('k', "up"), ('l', "right")] {
            let mut state = chord_shell(500);
            state.set_endpoint_methods(advertised.clone());
            state.handle_input_bytes(b"\x02w");
            let result = state.handle_input_bytes(&[key as u8]);
            let new_direction = matches!(key, 'h' | 'k');
            let supports = advertised.as_ref().is_some_and(|methods| {
                methods
                    .iter()
                    .any(|method| method == "pane.split.directional")
            });
            if new_direction && !supports {
                assert!(
                    result.actions.is_empty(),
                    "old endpoint must not receive {direction}"
                );
                assert!(frame_rows(&state.compose(106, 20).unwrap())
                    .join("\n")
                    .contains("Action unavailable"));
            } else {
                let [ClientShellAction::Endpoint { request, .. }] = result.actions.as_slice()
                else {
                    panic!("split request");
                };
                let request = serde_json::to_value(request).unwrap();
                assert_eq!(
                    request["method"],
                    if new_direction {
                        "pane.split.directional"
                    } else {
                        "pane.split"
                    }
                );
                assert_eq!(request["params"]["direction"], direction);
                assert_eq!(request["params"]["target_pane_id"], "pane_1");
            }
        }
    }
}
