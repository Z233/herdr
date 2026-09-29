use super::harness::*;

#[test]
fn fork_merge_explicit_current_without_caller_does_not_split_another_pane() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket = runtime_dir.join("herdr.sock");
    let server = spawn_herdr(&config_home, &runtime_dir, &socket);
    wait_for_socket(&socket, Duration::from_secs(10));
    run_cli_json(
        &socket,
        &[
            "workspace",
            "create",
            "--cwd",
            base.to_str().unwrap(),
            "--focus",
        ],
    );
    let before = send_request(
        &socket,
        r#"{"id":"fork:before","method":"pane.list","params":{}}"#,
    );
    assert!(!before["result"]["panes"].as_array().unwrap().is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["pane", "split", "--current", "--direction", "left"])
        .env("HERDR_SOCKET_PATH", &socket)
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .output()
        .unwrap();
    let after = send_request(
        &socket,
        r#"{"id":"fork:after","method":"pane.list","params":{}}"#,
    );
    cleanup_spawned_herdr(server, base);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--current requires HERDR_PANE_ID"));
    assert_eq!(before["result"]["panes"], after["result"]["panes"]);
}

#[test]
fn fork_merge_directory_preview_is_endpoint_backed_and_reports_failures() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket = runtime_dir.join("herdr.sock");
    let directory = base.join("directory with spaces");
    fs::create_dir_all(directory.join("child")).unwrap();
    fs::write(directory.join(".hidden"), "preview").unwrap();
    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket);
    wait_for_socket(&socket, Duration::from_secs(10));
    let preview = send_request(
        &socket,
        &serde_json::json!({
            "id": "fork:preview", "method": "workspace.directory_preview",
            "params": {"path": directory}
        })
        .to_string(),
    );
    let data = &preview["result"]["preview"];
    assert_eq!(
        data["canonical_path"],
        fs::canonicalize(&directory).unwrap().to_str().unwrap(),
        "{preview}"
    );
    let entries = data["entries"]
        .as_array()
        .expect("preview directory entries");
    assert!(entries
        .iter()
        .any(|entry| entry["name"] == "child" && entry["is_dir"] == true));
    assert!(entries
        .iter()
        .any(|entry| entry["name"] == ".hidden" && entry["is_dir"] == false));
    let missing = send_request(
        &socket,
        &serde_json::json!({
            "id": "fork:missing", "method": "workspace.directory_preview",
            "params": {"path": directory.join("missing")}
        })
        .to_string(),
    );
    assert_eq!(
        missing["error"]["code"], "directory_preview_failed",
        "{missing}"
    );
    assert!(!missing["error"]["message"].as_str().unwrap().is_empty());
    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn fork_merge_four_direction_split_preserves_target_ratio_and_focus() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket = runtime_dir.join("herdr.sock");
    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket);
    wait_for_socket(&socket, Duration::from_secs(10));

    let mut evidence = Vec::new();
    for direction in ["left", "right", "up", "down"] {
        let created = run_cli_json(
            &socket,
            &["workspace", "create", "--cwd", base.to_str().unwrap()],
        );
        let target = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .expect("root pane public ID");
        let split = run_cli_json(
            &socket,
            &[
                "pane",
                "split",
                target,
                "--direction",
                direction,
                "--ratio",
                "0.3",
                "--no-focus",
            ],
        );
        let added = split["result"]["pane"]["pane_id"]
            .as_str()
            .expect("new pane public ID");
        let response = send_request(
            &socket,
            &serde_json::json!({
                "id": "fork:layout", "method": "pane.layout", "params": {"pane_id": target}
            })
            .to_string(),
        );
        let layout = &response["result"]["layout"];
        let panes = layout["panes"].as_array().expect("full layout panes");
        assert_eq!(panes.len(), 2, "{response}");
        assert_eq!(layout["focused_pane_id"], target, "{response}");
        assert!((layout["splits"][0]["ratio"].as_f64().unwrap() - 0.3).abs() < 0.001);
        let original = &panes.iter().find(|pane| pane["pane_id"] == target).unwrap()["rect"];
        let new = &panes.iter().find(|pane| pane["pane_id"] == added).unwrap()["rect"];
        let axis = if matches!(direction, "left" | "right") {
            "x"
        } else {
            "y"
        };
        let first = matches!(direction, "left" | "up");
        assert_eq!(
            new[axis].as_u64().unwrap() < original[axis].as_u64().unwrap(),
            first,
            "{direction}: {response}"
        );
        evidence
            .push(serde_json::json!({"direction": direction, "split": split, "layout": response}));
    }
    if let Some(directory) = std::env::var_os("HERDR_MERGE_EVIDENCE_DIR") {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("four-direction-split.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn fork_merge_directional_endpoint_method_keeps_the_public_split_target() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket = runtime_dir.join("herdr.sock");
    let server = spawn_herdr(&config_home, &runtime_dir, &socket);
    wait_for_socket(&socket, Duration::from_secs(10));
    let created = run_cli_json(
        &socket,
        &["workspace", "create", "--cwd", base.to_str().unwrap()],
    );
    let target = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    for direction in ["left", "up"] {
        let result = send_request(
            &socket,
            &serde_json::json!({
                "id":"fork:directional", "method":"pane.split.directional", "params": {
                    "target_pane_id":target,"direction":direction,"ratio":0.3,"focus":false
                }
            })
            .to_string(),
        );
        assert!(result.get("error").is_none(), "{result}");
        assert_ne!(result["result"]["pane"]["pane_id"], target);
        let layout = send_request(
            &socket,
            &serde_json::json!({
                "id":"fork:layout", "method":"pane.layout", "params":{"pane_id":target}
            })
            .to_string(),
        );
        assert_eq!(layout["result"]["layout"]["focused_pane_id"], target);
    }
    cleanup_spawned_herdr(server, base);
}
