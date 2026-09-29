use super::*;
use crate::api::schema::{Method, PaneReadResult, ReadFormat, ReadSource, ResponseResult};
use ratatui::style::Color;
use std::time::{Duration, Instant};

fn remote_preview() -> (ClientShellState, ClientEndpointId) {
    let (mut state, remote) = super::fork_navigator::two_endpoints();
    let mut projected = snapshot();
    projected.boot_id = "remote-boot".into();
    let mut pane = projected.panes[0].clone();
    pane.pane_id = "pane_2".into();
    pane.focused = false;
    projected.panes.push(pane);
    state.set_endpoint_snapshot(&remote, Box::new(projected));
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Tab {
        endpoint_id: remote.clone(),
        tab_id: "tab_1".into(),
    });
    state.compose(120, 36).unwrap();
    (state, remote)
}

fn layout() -> ResponseResult {
    ResponseResult::PaneLayout {
        layout: serde_json::from_value(serde_json::json!({
            "workspace_id": "ws_1", "tab_id": "tab_1", "area": {"x":0,"y":0,"width":80,"height":20},
            "zoomed": false, "focused_pane_id": "pane_1", "splits": [], "panes": [
                {"pane_id":"pane_1", "focused":true, "rect":{"x":0,"y":0,"width":40,"height":20}, "terminal_size":{"cols":40,"rows":20}},
                {"pane_id":"pane_2", "focused":false, "rect":{"x":40,"y":0,"width":40,"height":20}, "terminal_size":{"cols":40,"rows":20}}
            ]
        })).unwrap(),
    }
}

fn one_row_layout() -> ResponseResult {
    ResponseResult::PaneLayout {
        layout: serde_json::from_value(serde_json::json!({
            "workspace_id": "ws_1", "tab_id": "tab_1", "area": {"x":0,"y":0,"width":80,"height":20},
            "zoomed": false, "focused_pane_id": "pane_1", "splits": [], "panes": [
                {"pane_id":"pane_1", "focused":true, "rect":{"x":0,"y":0,"width":40,"height":1}, "terminal_size":{"cols":40,"rows":1}},
                {"pane_id":"pane_2", "focused":false, "rect":{"x":40,"y":1,"width":40,"height":1}, "terminal_size":{"cols":40,"rows":1}}
            ]
        }))
        .unwrap(),
    }
}

fn read_result(pane_id: &str, text: &str, revision: u64) -> ResponseResult {
    ResponseResult::PaneRead {
        read: PaneReadResult {
            pane_id: pane_id.into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            source: ReadSource::Visible,
            format: ReadFormat::Ansi,
            text: text.into(),
            revision,
            truncated: false,
        },
    }
}

fn request_id(
    actions: &[ClientShellAction],
    endpoint: &ClientEndpointId,
    pane_id: Option<&str>,
) -> String {
    actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint {
                endpoint_id,
                request,
                ..
            } if endpoint_id == endpoint => match &request.method {
                Method::PaneLayout(params) if pane_id.is_none() => {
                    assert!(params.pane_id.is_some());
                    Some(request.id.clone())
                }
                Method::PaneRead(params) if Some(params.pane_id.as_str()) == pane_id => {
                    assert_eq!(params.source, ReadSource::Visible);
                    assert_eq!(params.format, ReadFormat::Ansi);
                    Some(request.id.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("preview request uses explicit endpoint and pane")
}

fn preview_area(state: &ClientShellState, width: u16) -> Rect {
    let popup = state.hits.navigator_popup;
    let body = Rect::new(
        popup.x + 1,
        popup.y + 3,
        popup.width.saturating_sub(2),
        popup.height.saturating_sub(6),
    );
    super::super::navigator_preview::split_body(body, width)
        .1
        .expect("wide terminal preview")
}

#[test]
#[ignore = "manual cached Navigator preview scaling profile"]
fn navigator_cached_preview_render_scale_profile() {
    for count in [1, 15] {
        let (mut state, remote) = super::fork_navigator::two_endpoints();
        let mut projected = snapshot();
        projected.boot_id = "remote-boot".into();
        let template = projected.panes[0].clone();
        projected.panes = (0..count)
            .map(|index| {
                let mut pane = template.clone();
                pane.pane_id = format!("pane_{}", index + 1);
                pane.focused = index == 0;
                pane
            })
            .collect();
        let columns = if count == 1 { 1 } else { 3 };
        let rows = count / columns;
        let (width, height) = (120 / columns, 60 / rows);
        let panes: Vec<_> = projected
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| serde_json::json!({
                "pane_id":pane.pane_id, "focused":pane.focused,
                "rect":{"x":index % columns * width,"y":index / columns * height,"width":width,"height":height},
                "terminal_size":{"cols":width,"rows":height}
            }))
            .collect();
        state.set_endpoint_snapshot(&remote, Box::new(projected));
        state.open_navigator_overlay();
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("navigator");
        };
        navigator.selected = Some(ClientNavigatorTarget::Tab {
            endpoint_id: remote.clone(),
            tab_id: "tab_1".into(),
        });
        state.compose(120, 36).unwrap();
        let mut outcome = ClientShellInput::default();
        state.refresh_navigator_preview(Instant::now(), &mut outcome);
        let request = request_id(&outcome.actions, &remote, None);
        let (_, mut actions) = state.handle_endpoint_result("remote-boot", &request, Ok(ResponseResult::PaneLayout {
            layout: serde_json::from_value(serde_json::json!({
                "workspace_id":"ws_1", "tab_id":"tab_1", "focused_pane_id":"pane_1", "zoomed":false,
                "area":{"x":0,"y":0,"width":120,"height":60}, "panes":panes, "splits":[]
            })).unwrap(),
        }));
        let text = format!("{}\r\n", "\x1b[31mCACHE界 \x1b[0m".repeat(width / 8)).repeat(height);
        for index in 0..count {
            let pane = format!("pane_{}", index + 1);
            let request = request_id(&actions, &remote, Some(&pane));
            (_, actions) = state.handle_endpoint_result(
                "remote-boot",
                &request,
                Ok(read_result(&pane, &text, 2)),
            );
        }
        assert!(actions.is_empty());
        assert!(frame_rows(&state.compose(120, 36).unwrap())
            .join("\n")
            .contains("CACHE"));
        for _ in 0..20 {
            std::hint::black_box(state.compose(120, 36).unwrap());
        }
        let start = Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(state.compose(120, 36).unwrap());
        }
        eprintln!(
            "navigator cached preview: {count} populated panes, {:.1} us/frame",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

#[test]
fn fork_merge_navigator_preview_reads_all_selected_tab_panes_without_focus_and_keeps_ansi() {
    let (mut state, remote) = remote_preview();
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    let layout_request = request_id(&poll.actions, &remote, None);
    let (_, reads) = state.handle_endpoint_result("remote-boot", &layout_request, Ok(layout()));
    let first = request_id(&reads, &remote, Some("pane_1"));
    let (_, reads) = state.handle_endpoint_result(
        "remote-boot",
        &first,
        Ok(read_result("pane_1", "\x1b[31mREMOTE界\x1b[0m", 2)),
    );
    let second = request_id(&reads, &remote, Some("pane_2"));
    let (_, actions) = state.handle_endpoint_result(
        "remote-boot",
        &second,
        Ok(read_result("pane_2", "NEIGHBOR", 8)),
    );
    assert!(actions.is_empty());
    assert_eq!(state.active_endpoint_id, ClientEndpointId::Local);
    let frame = state.compose(120, 36).unwrap();
    let rendered = frame_rows(&frame).join("\n");
    assert!(rendered.contains("REMOTE界"), "{rendered}");
    assert!(rendered.contains("NEIGHBOR"), "{rendered}");
    let buffer = frame.to_ratatui_buffer().unwrap();
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "R"
        && matches!(
            cell.fg,
            Color::Indexed(1) | Color::Rgb(_, _, _) | Color::Red
        )));
    let mut immediate = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut immediate);
    assert!(
        immediate.actions.is_empty(),
        "bounded refresh, no per-frame requests"
    );
    let mut refresh = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now() + Duration::from_secs(2), &mut refresh);
    request_id(&refresh.actions, &remote, None);
}

#[test]
fn fork_merge_navigator_local_preview_uses_default_and_program_backgrounds() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    let mut pane_surface = surface();
    let mut terminal = Buffer::with_lines(["LIVE", "PANE"]);
    terminal[(0, 0)].set_fg(Color::Green);
    terminal[(1, 0)].set_bg(Color::Indexed(4));
    pane_surface.frame = FrameData::from_ratatui_buffer(&terminal, None);
    state.set_pane_surface(pane_surface);
    state.config.palette.panel_bg = Color::Indexed(238);
    let normal = state.compose(120, 36).unwrap();
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Pane {
        endpoint_id: ClientEndpointId::Local,
        pane_id: "pane_1".into(),
    });
    state.compose(120, 36).unwrap();
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    assert!(poll.actions.is_empty());
    let frame = state.compose(120, 36).unwrap();
    let area = preview_area(&state, 120);
    let buffer = frame.to_ratatui_buffer().unwrap();
    assert_eq!(buffer[(area.x, area.y)].fg, Color::Green);
    assert_eq!(buffer[(area.x, area.y)].bg, Color::Reset);
    assert_eq!(buffer[(area.x + 1, area.y)].bg, Color::Indexed(4));
    assert_eq!(buffer[(area.x + 8, area.y + 4)].bg, Color::Reset);

    state.overlay = None;
    let after_close = state.compose(120, 36).unwrap();
    assert_eq!(after_close.cells, normal.cells);
}

#[test]
fn fork_merge_navigator_remote_preview_uses_first_row_and_default_background() {
    let (mut state, remote) = remote_preview();
    state.config.palette.panel_bg = Color::Indexed(238);
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    let layout_request = request_id(&poll.actions, &remote, None);
    let (_, reads) =
        state.handle_endpoint_result("remote-boot", &layout_request, Ok(one_row_layout()));
    let first = request_id(&reads, &remote, Some("pane_1"));
    let (_, reads) = state.handle_endpoint_result(
        "remote-boot",
        &first,
        Ok(read_result("pane_1", "\x1b[44mFIRSTROW\x1b[0m DEFAULT", 2)),
    );
    let second = request_id(&reads, &remote, Some("pane_2"));
    let (_, actions) = state.handle_endpoint_result(
        "remote-boot",
        &second,
        Ok(read_result("pane_2", "SECONDROW", 8)),
    );
    assert!(actions.is_empty());

    let frame = state.compose(120, 36).unwrap();
    let area = preview_area(&state, 120);
    let buffer = frame.to_ratatui_buffer().unwrap();
    let second_x = area.x + (40 * area.width / 80);
    let second_y = area.y + (area.height / 20);
    assert_eq!(buffer[(area.x, area.y)].symbol(), "F");
    assert_eq!(buffer[(area.x, area.y)].bg, Color::Indexed(4));
    assert_eq!(buffer[(area.x + 9, area.y)].symbol(), "D");
    assert_eq!(buffer[(area.x + 9, area.y)].bg, Color::Reset);
    assert_eq!(buffer[(second_x, second_y)].symbol(), "S");
    assert_eq!(buffer[(second_x, second_y)].bg, Color::Reset);
    assert_eq!(buffer[(area.x + 8, area.y + 4)].bg, Color::Reset);
    let preview_text = frame_rows(&frame)
        .into_iter()
        .skip(area.y as usize)
        .take(area.height as usize)
        .map(|row| {
            row.chars()
                .skip(area.x as usize)
                .take(area.width as usize)
                .collect::<String>()
        })
        .collect::<String>();
    assert!(!preview_text.contains("pane_1"));
    assert!(!preview_text.contains("pane_2"));
}

#[test]
fn fork_merge_navigator_status_backgrounds_and_directory_style_are_distinct() {
    let (mut state, remote) = remote_preview();
    state.config.palette.panel_bg = Color::Indexed(238);
    state.retire_endpoint(&remote);
    let unavailable = state.compose(120, 36).unwrap();
    let area = preview_area(&state, 120);
    let buffer = unavailable.to_ratatui_buffer().unwrap();
    assert_eq!(buffer[(area.x, area.y)].fg, state.config.palette.text);
    assert_eq!(buffer[(area.x + 30, area.y + 5)].bg, Color::Reset);
    assert!(frame_rows(&unavailable)
        .join("\n")
        .contains("Preview unavailable"));

    let (mut state, remote) = remote_preview();
    state.config.palette.panel_bg = Color::Indexed(238);
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    let loading = state.compose(120, 36).unwrap();
    let area = preview_area(&state, 120);
    let buffer = loading.to_ratatui_buffer().unwrap();
    assert_eq!(buffer[(area.x + 30, area.y + 5)].bg, Color::Reset);
    assert!(frame_rows(&loading).join("\n").contains("Loading preview"));
    let layout_request = request_id(&poll.actions, &remote, None);
    let (_, reads) = state.handle_endpoint_result("remote-boot", &layout_request, Ok(layout()));
    let first = request_id(&reads, &remote, Some("pane_1"));
    state.handle_endpoint_result(
        "remote-boot",
        &first,
        Err(ClientShellEndpointError {
            code: Some("endpoint_timeout".into()),
            message: "Preview timed out".into(),
        }),
    );
    let failed = state.compose(120, 36).unwrap();
    let buffer = failed.to_ratatui_buffer().unwrap();
    assert_eq!(buffer[(area.x + 30, area.y + 5)].bg, Color::Reset);
    assert!(frame_rows(&failed).join("\n").contains("Preview timed out"));

    let (mut state, remote) = remote_preview();
    state.config.palette.panel_bg = Color::Indexed(238);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Directory {
        endpoint_id: remote,
        shown_path: "/repo".into(),
        canonical_path: "/repo".into(),
    });
    navigator.fork.directory.preview = Some(crate::api::schema::WorkspaceDirectoryPreview {
        canonical_path: "/repo".into(),
        entries: vec![crate::api::schema::WorkspaceDirectoryEntry {
            name: "src".into(),
            is_dir: true,
        }],
        truncated: false,
    });
    let directory = state.compose(120, 36).unwrap();
    let area = preview_area(&state, 120);
    let buffer = directory.to_ratatui_buffer().unwrap();
    assert_eq!(buffer[(area.x, area.y)].symbol(), "/");
    assert_eq!(buffer[(area.x, area.y)].bg, Color::Indexed(238));
}

#[test]
fn fork_merge_navigator_preview_rejects_selection_endpoint_projection_and_resize_changes() {
    for change in 0..5 {
        let (mut state, remote) = remote_preview();
        let mut poll = ClientShellInput::default();
        state.refresh_navigator_preview(Instant::now(), &mut poll);
        let layout_request = request_id(&poll.actions, &remote, None);
        let (_, reads) = state.handle_endpoint_result("remote-boot", &layout_request, Ok(layout()));
        let first = request_id(&reads, &remote, Some("pane_1"));
        match change {
            0 => {
                let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
                    panic!("navigator");
                };
                navigator.selected = Some(ClientNavigatorTarget::Tab {
                    endpoint_id: ClientEndpointId::Local,
                    tab_id: "tab_1".into(),
                });
            }
            1 => {
                let mut projection = snapshot();
                projection.boot_id = "remote-boot".into();
                projection.revision += 1;
                state.set_endpoint_snapshot(&remote, Box::new(projection));
            }
            2 => {
                let mut projection = snapshot();
                projection.boot_id = "next-remote-boot".into();
                state.set_endpoint_snapshot(&remote, Box::new(projection));
            }
            3 => {
                state.compose(100, 30).unwrap();
            }
            _ => state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting),
        }
        let (repaint, actions) = state.handle_endpoint_result(
            "remote-boot",
            &first,
            Ok(read_result("pane_1", "STALE_PREVIEW", 2)),
        );
        assert!(!repaint && actions.is_empty(), "change {change}");
        assert!(!frame_rows(&state.compose(100, 30).unwrap())
            .join("\n")
            .contains("STALE_PREVIEW"));
    }
}

#[test]
fn fork_merge_navigator_preview_uses_current_local_surface_and_does_not_fetch_on_mobile() {
    let (mut state, _) = super::fork_navigator::two_endpoints();
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("navigator");
    };
    navigator.selected = Some(ClientNavigatorTarget::Pane {
        endpoint_id: ClientEndpointId::Local,
        pane_id: "pane_1".into(),
    });
    state.compose(120, 36).unwrap();
    let mut poll = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    assert!(poll.actions.is_empty());
    let frame = state.compose(120, 36).unwrap();
    assert!(frame_rows(&frame).join("\n").contains("LIVE"));
    let mut fresh = surface();
    fresh.surface_revision += 1;
    fresh.frame = FrameData::from_ratatui_buffer(&Buffer::with_lines(["NEW!", "PANE"]), None);
    state.set_pane_surface(fresh);
    state.refresh_navigator_preview(Instant::now(), &mut poll);
    assert!(frame_rows(&state.compose(120, 36).unwrap())
        .join("\n")
        .contains("NEW!"));
    let (mut state, _) = remote_preview();
    state.compose(45, 30).unwrap();
    let mut mobile = ClientShellInput::default();
    state.refresh_navigator_preview(Instant::now(), &mut mobile);
    assert!(mobile.actions.is_empty());
}

#[test]
fn fork_merge_navigator_preview_cancellation_and_reopened_overlay_discard_inflight_reads() {
    for reopen in [false, true] {
        let (mut state, remote) = remote_preview();
        let mut poll = ClientShellInput::default();
        state.refresh_navigator_preview(Instant::now(), &mut poll);
        let request = request_id(&poll.actions, &remote, None);
        if reopen {
            state.open_navigator_overlay();
            let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
                panic!("navigator");
            };
            navigator.selected = Some(ClientNavigatorTarget::Tab {
                endpoint_id: remote,
                tab_id: "tab_1".into(),
            });
            let (repaint, actions) =
                state.handle_endpoint_result("remote-boot", &request, Ok(layout()));
            assert!(!repaint && actions.is_empty());
        } else {
            state.cancel_endpoint_request(&request);
        }
    }
}

#[test]
fn fork_merge_navigator_preview_rejects_wrong_pane_and_displays_endpoint_failures() {
    for wrong_pane in [false, true] {
        let (mut state, remote) = remote_preview();
        let mut poll = ClientShellInput::default();
        state.refresh_navigator_preview(Instant::now(), &mut poll);
        let request = request_id(&poll.actions, &remote, None);
        let (_, reads) = state.handle_endpoint_result("remote-boot", &request, Ok(layout()));
        let request = request_id(&reads, &remote, Some("pane_1"));
        let result = if wrong_pane {
            Ok(read_result("pane_elsewhere", "WRONG_PREVIEW", 2))
        } else {
            Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "Preview timed out".into(),
            })
        };
        let (_, actions) = state.handle_endpoint_result("remote-boot", &request, result);
        assert!(actions.is_empty());
        let rendered = frame_rows(&state.compose(120, 36).unwrap()).join("\n");
        assert!(!rendered.contains("WRONG_PREVIEW"));
        if !wrong_pane {
            assert!(rendered.contains("Preview timed out"), "{rendered}");
        }
    }
}
