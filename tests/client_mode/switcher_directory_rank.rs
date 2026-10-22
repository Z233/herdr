use super::*;

#[test]
fn switcher_directory_rank_live_alpha_search_and_default_acceptance() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_DIRECTORY_RANK_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/switcher-directory-rank")
        });
    fs::create_dir_all(&evidence).unwrap();
    let _ = fs::remove_file(evidence.join("result.json"));
    let directories = [
        "projects/alpha-tools",
        "notes/alpha/personal",
        "worktrees/plugin-alpha/feature",
        "worktrees/plugin-alpha/feature/package",
    ];
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let mut script = String::from("#!/bin/sh\n");
    for (score, directory) in [1.5, 0.5, 0.2, 0.2].into_iter().zip(directories) {
        let path = base.join(directory);
        fs::create_dir_all(&path).unwrap();
        script.push_str(&format!("printf '{score} {}\\n'\n", path.display()));
    }
    fs::write(bin.join("zoxide"), script).unwrap();
    fs::set_permissions(bin.join("zoxide"), fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let source = base.join("existing-session");
    fs::create_dir_all(&source).unwrap();
    let mut results = Vec::new();
    for (mode, cols) in [("desktop", 160), ("mobile", 45)] {
        let config = base.join(mode).join("config");
        let runtime = base.join(mode).join("runtime");
        let api = runtime.join("herdr.sock");
        let server = spawn_server_with_config_and_env(
            &config,
            &runtime,
            &api,
            "onboarding = false\n[ui.sidebar.agents]\nvisible = false\n[keys]\nworkspace_switcher = 'alt+tab'\n",
            &[("PATH", &path), ("HOME", base.to_str().unwrap())],
        );
        wait_for_socket(&api, Duration::from_secs(10));
        let created = send_json_request(
            &api,
            &serde_json::json!({
                "id":"rank-create", "method":"workspace.create",
                "params":{"cwd":source,"label":"existing-session","focus":true}
            })
            .to_string(),
        );
        let pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
        let renamed = send_json_request(
            &api,
            &serde_json::json!({
                "id":"rank-pane", "method":"pane.rename",
                "params":{"pane_id":pane,"label":"working on alpha"}
            })
            .to_string(),
        );
        assert!(renamed.get("error").is_none(), "{renamed}");
        send_pane_shell_command(&api, pane, "PS1='$ '; printf 'DIRECTORY_RANK_%s\\n' READY");
        let client = spawn_client_shell_process(&config, &runtime, &api);
        let master = client._master.as_ref().unwrap();
        master
            .resize(PtySize {
                cols,
                rows: 60,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let output = spawn_pty_drain(master.try_clone_reader().unwrap());
        let mut input = master.take_writer().unwrap();
        wait_screen(&output, cols, 60, "DIRECTORY_RANK_READY");
        let open: &[u8] = if mode == "desktop" {
            b"\x1b[9;3u\x1b[115;3u"
        } else {
            b"\x1b[<0;37;1M\x1b[<0;37;1m"
        };
        input.write_all(open).unwrap();
        input.write_all(b"\x1b[200~alpha\x1b[201~").unwrap();
        input.flush().unwrap();
        wait_screen(&output, cols, 60, "~/projects/alpha-tools");
        save_frame(&evidence, &format!("{mode}-ranked"), &output, cols, 60);
        let frame = screen(&output, cols, 60);
        let list = frame
            .lines()
            .skip_while(|line| !line.contains("/ alpha"))
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n");
        let directory_position = list.find("~/projects/alpha-tools").expect(&frame);
        let workspace_position = list.find("existing-session").expect(&frame);
        assert!(directory_position < workspace_position, "{mode}: {frame}");
        input.write_all(b"\r").unwrap();
        input.flush().unwrap();
        let mut workspaces = Value::Null;
        assert!(
            wait_until(Duration::from_secs(12), Duration::from_millis(40), || {
                workspaces = send_json_request(
                    &api,
                    r#"{"id":"rank-result","method":"workspace.list","params":{}}"#,
                );
                workspaces["result"]["workspaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|workspace| {
                        workspace["label"] == "alpha-tools" && workspace["focused"] == true
                    })
            }),
            "Enter did not create and focus the best directory: {workspaces}"
        );
        let workspace = workspaces["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|workspace| workspace["label"] == "alpha-tools")
            .unwrap();
        let target_pane =
            first_pane_id_in_workspace(&api, workspace["workspace_id"].as_str().unwrap());
        send_pane_shell_command(&api, &target_pane, "printf 'DIRECTORY_RANK_%s\\n' ACCEPTED");
        wait_screen(&output, cols, 60, "DIRECTORY_RANK_ACCEPTED");
        save_frame(&evidence, &format!("{mode}-accepted"), &output, cols, 60);
        results.push(
            serde_json::json!({"mode":mode,"columns":cols,"directory_position":directory_position,
            "workspace_position":workspace_position,"workspaces":workspaces}),
        );
        drop(input);
        drop(client);
        drop(server);
    }
    fs::write(evidence.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "binary":env!("CARGO_BIN_EXE_herdr"),"query":"alpha","directories":directories,
        "fixture":"synthetic directories and pane labels under an isolated HOME",
        "outcome":"directory ranks ahead of the existing workspace group and Enter creates and focuses it on desktop and mobile",
        "results":results
    })).unwrap()).unwrap();
    cleanup_test_base(&base);
}
