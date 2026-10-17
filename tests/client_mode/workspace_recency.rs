use super::*;

#[test]
fn workspace_recency_live_admission_refresh_selection_and_frozen_openings() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_WORKSPACE_RECENCY_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/workspace-recency-evidence")
        });
    fs::create_dir_all(&evidence).unwrap();
    let _ = fs::remove_file(evidence.join("result.json"));
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let server = spawn_server_with_config(
        &config,
        &runtime,
        &api,
        &runtime.join("client.sock"),
        "onboarding = false\n[ui.sidebar.agents]\nvisible = false\n[keys]\nworkspace_switcher = 'alt+tab'\n",
    );
    wait_for_socket(&api, Duration::from_secs(10));
    let mut records = Vec::new();
    let mut request = |method: &str, params: Value| {
        let message = serde_json::json!({"id":format!("recency-{}", records.len()),"method":method,"params":params});
        let response = send_json_request(&api, &message.to_string());
        records.push(serde_json::json!({"request":message,"response":response}));
        fs::write(
            evidence.join("api.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response
    };
    let first = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentA","focus":true}),
    );
    let older = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentC","focus":false}),
    );
    let pane_a = first["result"]["root_pane"]["pane_id"].as_str().unwrap();
    let pane_c = older["result"]["root_pane"]["pane_id"].as_str().unwrap();
    for (pane, marker) in [(pane_a, "A_READY"), (pane_c, "C_READY")] {
        send_pane_shell_command(&api, pane, &format!("printf 'RECENCY_%s\\n' {marker}"));
    }
    let client = spawn_client_shell_process(&config, &runtime, &api);
    let master = client._master.as_ref().unwrap();
    master
        .resize(PtySize {
            cols: 120,
            rows: 40,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output = spawn_pty_drain(master.try_clone_reader().unwrap());
    let mut input = master.take_writer().unwrap();
    let mut sequences = Vec::new();
    let mut send = |bytes: &[u8]| {
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
        sequences.push(String::from_utf8_lossy(bytes).into_owned());
        fs::write(
            evidence.join("input.json"),
            serde_json::to_vec_pretty(&sequences).unwrap(),
        )
        .unwrap();
    };
    let terminal = |cols, marker: &str| {
        assert!(
            wait_until(Duration::from_secs(12), Duration::from_millis(40), || {
                let frame = screen(&output, cols, 40);
                frame.contains(marker)
                    && !frame.contains("Workspace Switcher")
                    && !frame.contains("+ new workspace")
            }),
            "missing terminal {marker}: {}",
            screen(&output, cols, 40)
        );
    };
    let check = |name: &str, cols, expected: &[&str], absent: Option<&str>| {
        save_frame(&evidence, name, &output, cols, 40);
        let frame = screen(&output, cols, 40);
        let list = if cols == 120 {
            frame
                .lines()
                .skip_while(|line| !line.contains("Workspace Switcher"))
                .filter_map(|line| line.split('│').nth(1))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            frame
                .lines()
                .skip_while(|line| line.trim() != "spaces")
                .take_while(|line| line.trim() != "tabs")
                .collect::<Vec<_>>()
                .join("\n")
        };
        let positions = expected
            .iter()
            .map(|label| {
                list.find(label)
                    .unwrap_or_else(|| panic!("missing {label}: {frame}"))
            })
            .collect::<Vec<_>>();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{expected:?}: {frame}"
        );
        if let Some(label) = absent {
            assert!(!list.contains(label), "unexpected {label}: {frame}");
        }
    };
    terminal(120, "RECENCY_A_READY");
    send(b"\x1b[9;3u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    check("01-baseline", 120, &["RecentA", "RecentC"], None);
    send(b"\x1b[27u");
    terminal(120, "RECENCY_A_READY");

    let added = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentB","focus":false}),
    );
    let pane_b = added["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(&api, pane_b, "printf 'RECENCY_%s\\n' B_READY");
    send_pane_shell_command(&api, pane_a, "printf 'RECENCY_%s\\n' A_REFRESH");
    terminal(120, "RECENCY_A_REFRESH");
    send(b"\x1b[9;3u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    check(
        "02-new-before-current-after-output",
        120,
        &["RecentB", "RecentA", "RecentC"],
        None,
    );
    send(b"\x1b[57443;1:3u");
    terminal(120, "RECENCY_B_READY");
    send(b"\x1b[9;3u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    send(b"\x1b[57443;1:3u");
    terminal(120, "RECENCY_A_REFRESH");
    send(b"\x1b[9;4u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    check("03-reverse", 120, &["RecentA", "RecentB", "RecentC"], None);
    send(b"\x1b[57443;1:3u");
    terminal(120, "RECENCY_C_READY");
    send(b"\x1b[9;3u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    send(b"\x1b[57443;1:3u");
    terminal(120, "RECENCY_A_REFRESH");
    send(b"\x1b[9;3u");
    wait_screen(&output, 120, 40, "Workspace Switcher");
    let later = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentD","focus":false}),
    );
    let pane_d = later["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(&api, pane_d, "printf 'RECENCY_%s\\n' D_READY");
    request(
        "workspace.rename",
        serde_json::json!({"workspace_id":first["result"]["workspace"]["workspace_id"],"label":"RecentA2"}),
    );
    wait_screen(&output, 120, 40, "RecentA2");
    check(
        "04-desktop-frozen",
        120,
        &["RecentA2", "RecentC", "RecentB"],
        Some("RecentD"),
    );
    send(b"\x1b[27u\x1b[9;3u");
    wait_screen(&output, 120, 40, "RecentD");
    check(
        "05-desktop-reopened",
        120,
        &["RecentD", "RecentA2", "RecentC", "RecentB"],
        None,
    );
    send(b"\x1b[57443;1:3u");
    terminal(120, "RECENCY_D_READY");

    master
        .resize(PtySize {
            cols: 45,
            rows: 40,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    wait_screen(&output, 45, 40, "switch");
    let mobile = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentE","focus":false}),
    );
    let pane_e = mobile["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(&api, pane_e, "printf 'RECENCY_%s\\n' E_READY");
    send_pane_shell_command(&api, pane_d, "printf 'RECENCY_%s\\n' D_REFRESH");
    terminal(45, "RECENCY_D_REFRESH");
    send(b"\x1b[<0;37;1M\x1b[<0;37;1m");
    wait_screen(&output, 45, 40, "+ new workspace");
    check(
        "06-mobile-admission",
        45,
        &["RecentE", "RecentD", "RecentA2", "RecentC", "RecentB"],
        None,
    );
    send(b"\r");
    terminal(45, "RECENCY_D_REFRESH");
    send(b"\x1b[<0;37;1M\x1b[<0;37;1m");
    wait_screen(&output, 45, 40, "+ new workspace");
    let last = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RecentF","focus":false}),
    );
    let pane_f = last["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(&api, pane_f, "printf 'RECENCY_%s\\n' F_READY");
    request(
        "workspace.rename",
        serde_json::json!({"workspace_id":mobile["result"]["workspace"]["workspace_id"],"label":"RecentE2"}),
    );
    wait_screen(&output, 45, 40, "RecentE2");
    check(
        "07-mobile-frozen",
        45,
        &["RecentE2", "RecentD", "RecentA2", "RecentC", "RecentB"],
        Some("RecentF"),
    );
    send(b"\x1b[27u");
    terminal(45, "RECENCY_D_REFRESH");
    send(b"\x1b[<0;37;1M\x1b[<0;37;1m");
    wait_screen(&output, 45, 40, "+ new workspace");
    check(
        "08-mobile-reopened",
        45,
        &[
            "RecentF", "RecentE2", "RecentD", "RecentA2", "RecentC", "RecentB",
        ],
        None,
    );
    send(b"\x1b[A\x1b[A\r");
    terminal(45, "RECENCY_F_READY");
    save_frame(&evidence, "09-mobile-confirmed", &output, 45, 40);
    fs::write(evidence.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "binary":env!("CARGO_BIN_EXE_herdr"), "geometries":["120x40","45x40"],
        "outcome":"new workspace admission, ordinary output, forward/reverse initial selection, presented activation and frozen desktop/mobile openings passed"
    })).unwrap()).unwrap();
    drop(input);
    drop(client);
    drop(server);
    cleanup_test_base(&base);
}
