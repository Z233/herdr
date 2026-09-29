use super::*;

fn copy_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut visible = surface();
    let buffer = Buffer::with_lines(["界ab  ab", "        "]);
    visible.frame = FrameData::from_ratatui_buffer(&buffer, None);
    visible.panes[0].inner_rect.width = buffer.area.width;
    visible.panes[0].rect.width = buffer.area.width;
    visible.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        viewport_rows: 2,
    });
    state.set_pane_surface(visible);
    state.compose(106, 24).unwrap();
    state
}

#[test]
fn fork_merge_easymotion_uses_wide_cell_positions_and_preserves_the_selection_anchor() {
    let mut state = copy_state();
    state.handle_input_bytes(b"\x02[");
    assert_eq!(state.mode, ClientShellMode::Copy);
    state.handle_input_bytes(b"vsab");
    let anchor = state.copy_mode.as_ref().unwrap().selection;
    let frame = state.compose(106, 24).unwrap();
    assert!(frame_rows(&frame).join("\n").contains("EasyMotion"));
    state.handle_input_bytes(b"f");
    let copy = state.copy_mode.as_ref().unwrap();
    assert_eq!(
        copy.cursor,
        crate::api::schema::PaneTextPoint { row: 0, col: 2 }
    );
    assert_eq!(copy.selection, anchor);
    assert!(state.selection.as_ref().unwrap().is_visible());
}

#[test]
fn fork_merge_easymotion_discards_labels_after_resize_or_content_change() {
    for resize in [false, true] {
        let mut state = copy_state();
        state.handle_input_bytes(b"\x02[sab");
        let before = state.copy_mode.as_ref().unwrap().cursor;
        let mut updated = state.pane_surface.clone().unwrap();
        updated.surface_revision += 1;
        if resize {
            updated.panes[0].inner_rect.width -= 1;
        } else {
            updated.panes[0].content_revision += 1;
        }
        state.set_pane_surface(updated);
        state.compose(106, 24).unwrap();
        let outcome = state.handle_input_bytes(b"f");
        assert!(outcome.actions.is_empty());
        assert_eq!(state.copy_mode.as_ref().unwrap().cursor, before);
    }
}

#[test]
fn fork_merge_copy_initial_actions_enter_the_normal_client_copy_session() {
    use super::super::easymotion::CopyModeInitialAction;
    let mut state = copy_state();
    let mut outcome = ClientShellInput::default();
    assert!(
        state.enter_copy_mode_with_initial_action(CopyModeInitialAction::EasyMotion, &mut outcome)
    );
    state.handle_input_bytes(b"abf");
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.col, 2);
    assert_eq!(state.mode, ClientShellMode::Copy);
    state.exit_copy_mode(false, &mut outcome);
    assert!(
        state.enter_copy_mode_with_initial_action(CopyModeInitialAction::ScrollUp, &mut outcome)
    );
    assert_eq!(state.mode, ClientShellMode::Copy);
}

#[test]
fn fork_merge_copy_scroll_up_initial_action_preserves_half_page_distance() {
    let mut state = copy_state();
    let mut visible = state.pane_surface.clone().unwrap();
    visible.surface_revision += 1;
    visible.frame = FrameData::from_ratatui_buffer(&Buffer::with_lines(["row"; 10]), None);
    visible.panes[0].inner_rect.height = 10;
    visible.panes[0].rect.height = 10;
    visible.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 100,
        viewport_rows: 10,
    });
    state.set_pane_surface(visible);
    state.compose(106, 24).unwrap();
    let mut outcome = ClientShellInput::default();
    assert!(state.enter_copy_mode_with_initial_action(
        super::super::easymotion::CopyModeInitialAction::ScrollUp,
        &mut outcome
    ));
    assert_eq!(state.copy_mode.as_ref().unwrap().offset_from_bottom, 5);
}
