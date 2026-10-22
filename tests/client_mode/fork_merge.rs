use super::*;
use std::os::unix::fs::PermissionsExt;

#[path = "workspace_recency.rs"]
mod workspace_recency;

#[path = "switcher_directory_rank.rs"]
mod switcher_directory_rank;

#[test]
fn fork_merge_mobile_panes_live_hold_focus_fold_and_reopen_recency() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_MOBILE_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/mobile-pane-evidence")
        });
    fs::create_dir_all(&evidence).unwrap();
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let mut server = spawn_server_with_config(
        &config,
        &runtime,
        &api,
        &runtime.join("client.sock"),
        "onboarding = false\n[ui.sidebar.agents]\nvisible = false\n",
    );
    wait_for_socket(&api, Duration::from_secs(10));
    let mut records = Vec::new();
    let mut request = |method: &str, params: Value| {
        let message = serde_json::json!({"id":format!("mobile-{}", records.len()),"method":method,"params":params});
        let response = send_json_request(&api, &message.to_string());
        records.push(serde_json::json!({"request":message,"response":response}));
        fs::write(
            evidence.join("mobile-live-api.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response
    };
    let workspace = request(
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"SwipeWorkspace"}),
    );
    let workspace_id = workspace["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = workspace["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let tab = request(
        "tab.create",
        serde_json::json!({"workspace_id":workspace_id,"focus":false}),
    );
    let second = tab["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let split = request(
        "pane.split",
        serde_json::json!({"target_pane_id":second,"direction":"right","focus":false}),
    );
    let third = split["result"]["pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (pane, label) in [(&first, "Alpha"), (&second, "Beta"), (&third, "Gamma")] {
        request(
            "pane.rename",
            serde_json::json!({"pane_id":pane,"label":label}),
        );
    }
    request("pane.focus", serde_json::json!({"pane_id":first}));
    let client = spawn_client_process_with_args(&config, &runtime, &api, &["client"]);
    client
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 45,
            rows: 36,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output = spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
    wait_screen(&output, 45, 36, "switch");
    let mut sequences = Vec::new();
    let mut mouse = |button: u8, x: u16, y: u16, release: bool| {
        let sequence = format!(
            "\x1b[<{button};{};{}{}",
            x + 1,
            y + 1,
            if release { 'm' } else { 'M' }
        );
        input.write_all(sequence.as_bytes()).unwrap();
        input.flush().unwrap();
        sequences.push(sequence);
        fs::write(
            evidence.join("mobile-live-input.json"),
            serde_json::to_vec_pretty(&sequences).unwrap(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(150));
    };
    mouse(0, 36, 0, false);
    wait_screen(&output, 45, 36, "close");
    mouse(32, 5, 12, false);
    mouse(32, 9, 12, false);
    wait_screen(&output, 45, 36, "└─ Gamma · 2");
    let expanded = screen(&output, 45, 36);
    save_frame(&evidence, "mobile-live-expanded", &output, 45, 36);
    assert!(
        expanded.contains("└─ Alpha") && expanded.contains("└─ Beta · 1"),
        "{expanded}"
    );
    assert!(expanded.find("└─ Alpha").unwrap() < expanded.find("└─ Beta · 1").unwrap());
    assert!(expanded.find("└─ Beta · 1").unwrap() < expanded.find("└─ Gamma · 2").unwrap());
    mouse(32, 9, 16, false);
    mouse(32, 5, 16, false);
    assert!(!screen(&output, 45, 36).contains("└─"));
    save_frame(&evidence, "mobile-live-folded", &output, 45, 36);
    mouse(32, 9, 16, false);
    wait_screen(&output, 45, 36, "└─ Gamma · 2");
    // Clamp at the last pane, then append a pane without moving the pointer.
    mouse(32, 9, 28, false);
    save_frame(
        &evidence,
        "mobile-live-selected-before-append",
        &output,
        45,
        36,
    );
    let appended = request(
        "tab.create",
        serde_json::json!({"workspace_id":workspace_id,"focus":false}),
    );
    let fourth = appended["result"]["root_pane"]["pane_id"].as_str().unwrap();
    request(
        "pane.rename",
        serde_json::json!({"pane_id":fourth,"label":"Delta"}),
    );
    wait_screen(&output, 45, 36, "└─ Delta");
    save_frame(
        &evidence,
        "mobile-live-appended-during-hold",
        &output,
        45,
        36,
    );
    mouse(0, 9, 28, true);
    request(
        "pane.list",
        serde_json::json!({"workspace_id":workspace_id}),
    );
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(40), || {
            let response = send_json_request(&api, &serde_json::json!({
            "id":"mobile-focus-proof","method":"pane.list","params":{"workspace_id":workspace_id}
        }).to_string());
            response["result"]["panes"].as_array().is_some_and(|panes| {
                panes
                    .iter()
                    .any(|pane| pane["pane_id"] == third && pane["focused"] == true)
            })
        }),
        "hold release did not focus exact cross-tab pane"
    );
    request(
        "pane.list",
        serde_json::json!({"workspace_id":workspace_id}),
    );
    wait_screen(&output, 45, 36, "switch");
    assert!(!screen(&output, 45, 36).contains("close"));
    save_frame(&evidence, "mobile-live-focused", &output, 45, 36);
    mouse(0, 36, 0, false);
    wait_screen(&output, 45, 36, "close");
    mouse(0, 36, 0, true);
    wait_screen(&output, 45, 36, "└─ Gamma · 2");
    let reopened = screen(&output, 45, 36);
    assert!(reopened.find("└─ Gamma · 2").unwrap() < reopened.find("└─ Alpha").unwrap());
    assert!(reopened.find("└─ Alpha").unwrap() < reopened.find("└─ Beta · 1").unwrap());
    save_frame(&evidence, "mobile-live-reopened-mru", &output, 45, 36);
    // Another client's focus changes the snapshot, not this opening's order.
    request("pane.focus", serde_json::json!({"pane_id":second}));
    thread::sleep(Duration::from_millis(500));
    let frozen = screen(&output, 45, 36);
    assert!(frozen.find("└─ Gamma · 2").unwrap() < frozen.find("└─ Alpha").unwrap());
    save_frame(&evidence, "mobile-live-frozen-refresh", &output, 45, 36);
    request(
        "pane.list",
        serde_json::json!({"workspace_id":workspace_id}),
    );
    drop(input);
    drop(client);
    if let Some(pid) = server.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(20), || {
            server.child.try_wait().unwrap().is_some()
        }),
        "isolated server did not shut down"
    );
    drop(server);
    cleanup_test_base(&base);
}

fn screen(output: &SharedOutput, cols: u16, rows: u16) -> String {
    terminal_screen::text(&output.lock().unwrap().bytes, cols, rows)
}

fn save_frame(
    directory: &std::path::Path,
    name: &str,
    output: &SharedOutput,
    cols: u16,
    rows: u16,
) {
    let bytes = output.lock().unwrap().bytes.clone();
    fs::write(directory.join(format!("{name}.ansi")), &bytes).unwrap();
    fs::write(
        directory.join(format!("{name}.txt")),
        terminal_screen::text(&bytes, cols, rows),
    )
    .unwrap();
}

fn wait_screen(output: &SharedOutput, cols: u16, rows: u16, needle: &str) {
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(40), || {
            screen(output, cols, rows).contains(needle)
        }),
        "missing {needle:?}: {}",
        screen(output, cols, rows)
    );
}

#[test]
fn fork_merge_live_navigator_two_clients_preview_release_search_copy_and_mobile() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_MERGE_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fork-merge-evidence")
        });
    fs::create_dir_all(&evidence).unwrap();
    let config = base.join("local-config");
    let runtime = base.join("local-runtime");
    let api = runtime.join("herdr.sock");
    let remote_config = base.join("remote-config");
    let remote_runtime = base.join("remote-runtime");
    let remote_api = remote_runtime.join("herdr.sock");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("zoxide"),
        "#!/bin/sh\nprintf 'remote zoxide failure\\n' >&2\nexit 17\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("zoxide"), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_herdr"), bin.join("herdr")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let settings = "onboarding = false\n[keys]\nworkspace_picker = 'ctrl+g'\nworkspace_switcher = 'alt+tab'\ncopy_mode_easymotion = 'ctrl+e'\ncopy_mode_scroll_up = 'ctrl+u'\n";
    let local =
        spawn_server_with_config_and_env(&config, &runtime, &api, settings, &[("PATH", &path)]);
    let remote = spawn_server_with_config_and_env(
        &remote_config,
        &remote_runtime,
        &remote_api,
        settings,
        &[("PATH", &path)],
    );
    wait_for_socket(&api, Duration::from_secs(10));
    wait_for_socket(&remote_api, Duration::from_secs(10));
    let checked = std::process::Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["config", "check"])
        .env(
            "HERDR_CONFIG_PATH",
            config.join(app_dir_name()).join("config.toml"),
        )
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout).trim(),
        "config: ok"
    );
    assert!(checked.stderr.is_empty(), "{checked:?}");
    let mut records = Vec::new();
    let mut request = |endpoint: &str, socket: &PathBuf, method: &str, params: Value| {
        let message = serde_json::json!({"id":format!("e2e-{}", records.len()),"method":method,"params":params});
        let response = send_json_request(socket, &message.to_string());
        records
            .push(serde_json::json!({"endpoint":endpoint,"request":message,"response":response}));
        fs::write(
            evidence.join("live-api.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response
    };
    let created = request(
        "local",
        &api,
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"LocalBase"}),
    );
    let local_pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = request(
        "remote",
        &remote_api,
        "workspace.create",
        serde_json::json!({"cwd":base,"label":"RemoteBase"}),
    );
    let remote_pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        local_pane, remote_pane,
        "the two servers must exercise colliding public IDs"
    );
    let split = request(
        "remote",
        &remote_api,
        "pane.split",
        serde_json::json!({"pane_id":remote_pane,"direction":"right","focus":false}),
    );
    let neighbor = split["result"]["pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let catalog = runtime.join("state").join(app_dir_name()).join("client");
    fs::create_dir_all(&catalog).unwrap();
    fs::write(catalog.join("endpoints.json"), serde_json::json!({
        "version":1,"selected_profile":null,
        "ssh":[{"id":"0123456789abcdef0123456789abcdef","label":"Merge remote","target":"test-only","session":"default","enabled":true}]
    }).to_string()).unwrap();
    let quote = |value: &std::path::Path| {
        format!("'{}'", value.display().to_string().replace('\'', "'\\''"))
    };
    let ssh_log = evidence.join("ssh-commands.txt");
    fs::write(&ssh_log, "").unwrap();
    fs::write(bin.join("ssh"), format!(
        "#!/bin/sh\nexport XDG_CONFIG_HOME={} XDG_RUNTIME_DIR={} HERDR_SOCKET_PATH={}\nunset HERDR_CLIENT_SOCKET_PATH HERDR_SESSION\nfor arg do last=\"$arg\"; done\nprintf '%s\\n' \"$last\" >> {}\nexec /bin/sh -c \"$last\"\n",
        quote(&remote_config), quote(&remote_runtime), quote(&remote_api), quote(&ssh_log)
    )).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
    let first = spawn_client_process_with_args_and_env(
        &config,
        &runtime,
        &api,
        &["client"],
        &[("PATH", &path)],
    );
    first
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 120,
            rows: 36,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let first_output = spawn_pty_drain(first._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut first_input = first._master.as_ref().unwrap().take_writer().unwrap();
    let second = spawn_client_process_with_args_and_env(
        &config,
        &runtime,
        &api,
        &["client"],
        &[("PATH", &path)],
    );
    second
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let second_output =
        spawn_pty_drain(second._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut second_input = second._master.as_ref().unwrap().take_writer().unwrap();
    wait_screen(&first_output, 120, 36, "RemoteBase");
    wait_screen(&second_output, 100, 30, "LocalBase");
    send_pane_shell_command(&api, &local_pane, "PS1='$ '; printf 'LOCAL_%s\\n' READY");
    send_pane_shell_command(
        &remote_api,
        &remote_pane,
        "PS1='$ '; printf '\\033[31mREMOTE_%s界Q\\033[0m\\n' LEFT",
    );
    send_pane_shell_command(
        &remote_api,
        &neighbor,
        "PS1='$ '; printf 'REMOTE_%s\\n' RIGHT",
    );
    wait_screen(&second_output, 100, 30, "LOCAL_READY");
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(200), || {
            if screen(&second_output, 100, 30).contains("LOCAL_CONTROL") {
                return true;
            }
            second_input
                .write_all(b"printf 'LOCAL_%s\\n' CONTROL\r")
                .unwrap();
            false
        }),
        "local input remained gated: {}",
        screen(&second_output, 100, 30)
    );
    second_input.write_all(b"\x02w").unwrap();
    thread::sleep(Duration::from_millis(800));
    assert!(!screen(&second_output, 100, 30).contains("PREFIX"));
    assert!(!screen(&second_output, 100, 30).contains("release to open"));
    second_input
        .write_all(b"\x07printf 'PICKER_%s\\n' IGNORED\r")
        .unwrap();
    wait_screen(&second_output, 100, 30, "PICKER_IGNORED");
    save_frame(&evidence, "removed-picker-ignored", &second_output, 100, 30);
    second_input.write_all(b"echo AB CD").unwrap();
    second_input.write_all(b"\x1bb").unwrap();
    second_input.write_all(b"Z_").unwrap();
    second_input.write_all(b"\r").unwrap();
    wait_screen(&second_output, 100, 30, "AB Z_CD");
    second_input.write_all(b"echo ").unwrap();
    second_input.write_all(b"\x1b[0;;20320:22909u").unwrap();
    second_input.write_all(b"\r").unwrap();
    wait_screen(&second_output, 100, 30, "你好");
    save_frame(
        &evidence,
        "escape-hotfix-and-associated-text",
        &second_output,
        100,
        30,
    );
    second_input.write_all(b"\x02wh").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(40), || {
            let layout = send_json_request(
                &api,
                &serde_json::json!({
                    "id":"split-proof", "method":"pane.layout", "params":{"pane_id":local_pane}
                })
                .to_string(),
            );
            layout["result"]["layout"]["panes"]
                .as_array()
                .is_some_and(|panes| panes.len() == 2)
        }),
        "prefix+w+h did not split the local pane"
    );
    let before = request(
        "local",
        &api,
        "pane.layout",
        serde_json::json!({"pane_id":local_pane}),
    );

    first_input.write_all(b"\x1b[9;3u").unwrap();
    wait_screen(&first_output, 120, 36, "REMOTE_LEFT界Q");
    wait_screen(&first_output, 120, 36, "REMOTE_RIGHT");
    assert!(screen(&first_output, 120, 36).contains("release to open"));
    save_frame(&evidence, "navigator-preview", &first_output, 120, 36);
    let after = request(
        "local",
        &api,
        "pane.layout",
        serde_json::json!({"pane_id":local_pane}),
    );
    assert_eq!(
        before["result"]["layout"]["panes"][0]["terminal_size"],
        after["result"]["layout"]["panes"][0]["terminal_size"]
    );
    assert!(screen(&second_output, 100, 30).contains("LOCAL_CONTROL"));

    first_input.write_all(b"\x1b[57443;1:3u").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || { !screen(&first_output, 120, 36).contains("release to open") }
    ));
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(200), || {
            if screen(&first_output, 120, 36).contains("REMOTE_INPUT") {
                return true;
            }
            first_input
                .write_all(b"printf 'REMOTE_%s\\n' INPUT\r")
                .unwrap();
            false
        }),
        "remote input remained gated: {}",
        screen(&first_output, 120, 36)
    );
    save_frame(&evidence, "remote-activated", &first_output, 120, 36);
    let local_read = request(
        "local",
        &api,
        "pane.read",
        serde_json::json!({"pane_id":local_pane,"source":"visible"}),
    );
    assert!(!local_read["result"]["read"]["text"]
        .as_str()
        .unwrap()
        .contains("REMOTE_INPUT"));
    first_input
        .write_all(b"\x1b[9;3u\x1b[115;3umissing-directory")
        .unwrap();
    wait_screen(&first_output, 120, 36, "remote zoxide failure");
    save_frame(&evidence, "remote-search-error", &first_output, 120, 36);
    first_input.write_all(b"\x1b[27u\x1b[27u").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(40), || {
            !screen(&first_output, 120, 36).contains("back esc")
        }),
        "Navigator did not close: {}",
        screen(&first_output, 120, 36)
    );
    first_input.write_all(b"\x05").unwrap();
    wait_screen(&first_output, 120, 36, "EasyMotion");
    first_input.write_all("界Q".as_bytes()).unwrap();
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(40), || {
            let frame = screen(&first_output, 120, 36);
            frame.contains("EasyMotion 界Q") && !frame.contains("0 matches")
        }),
        "wide-character match was not found: {}",
        screen(&first_output, 120, 36)
    );
    save_frame(&evidence, "easymotion-wide", &first_output, 120, 36);
    first_input.write_all(b"f").unwrap();
    assert!(wait_until(
        Duration::from_secs(5),
        Duration::from_millis(40),
        || {
            let frame = screen(&first_output, 120, 36);
            frame.contains("COPY") && !frame.contains("EasyMotion")
        }
    ));
    first_input.write_all(b"\x1b[27u").unwrap();
    first
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 45,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    wait_screen(&first_output, 45, 30, "switch");
    request(
        "remote",
        &remote_api,
        "pane.zoom",
        serde_json::json!({"pane_id":remote_pane}),
    );
    wait_screen(&first_output, 45, 30, "[@]");
    save_frame(&evidence, "mobile-zoom", &first_output, 45, 30);
    let frame = screen(&first_output, 45, 30);
    let (row, column) = frame
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("switch").map(|index| {
                (
                    row + 1,
                    unicode_width::UnicodeWidthStr::width(&line[..index]) + 2,
                )
            })
        })
        .unwrap();
    first_input
        .write_all(format!("\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m").as_bytes())
        .unwrap();
    wait_screen(&first_output, 45, 30, "close");
    save_frame(&evidence, "mobile-tap-switcher", &first_output, 45, 30);
    save_frame(
        &evidence,
        "second-client-preserved",
        &second_output,
        100,
        30,
    );
    assert!(screen(&second_output, 100, 30).contains("LOCAL_CONTROL"));
    fs::write(
        evidence.join("live-api.json"),
        serde_json::to_vec_pretty(&records).unwrap(),
    )
    .unwrap();
    fs::write(evidence.join("input-sequence.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "binary":env!("CARGO_BIN_EXE_herdr"),"transport":"private SSH command adapter to real stdio endpoint bridge",
        "first_client":["alt+tab hold","left-alt release","remote shell input","alt+tab then alt+s directory search","esc twice","ctrl+e","界Q","f","esc","resize 45x30","tap switch"],
        "second_client":"100x30 Local; prefix+w idle; legacy ctrl+g forwarded to shell; recorded geometry before and after remote preview"
    })).unwrap()).unwrap();
    drop(first_input);
    drop(second_input);
    drop(first);
    drop(second);
    drop(local);
    drop(remote);
    cleanup_test_base(&base);
}
#[test]
fn mobile_switcher_search_live_input_cancel_and_accept() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_MOBILE_SEARCH_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/mobile-switcher-search/live")
        });
    fs::create_dir_all(&evidence).unwrap();
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let directory = base.join("mobile-dir42");
    let bin = base.join("bin");
    fs::create_dir_all(&directory).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("zoxide"),
        format!("#!/bin/sh\nprintf '10 {}\\n'\n", directory.display()),
    )
    .unwrap();
    fs::set_permissions(bin.join("zoxide"), fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let server = spawn_server_with_config_and_env(
        &config,
        &runtime,
        &api,
        "onboarding = false\n",
        &[("PATH", &path)],
    );
    wait_for_socket(&api, Duration::from_secs(10));
    let created = send_json_request(&api, &serde_json::json!({
        "id":"mobile-create", "method":"workspace.create", "params":{"cwd":base,"label":"SearchDest42"}
    }).to_string());
    assert!(created.get("error").is_none(), "{created}");
    let pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    let client = spawn_client_shell_process(&config, &runtime, &api);
    client
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 44,
            rows: 20,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output = spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
    wait_screen(&output, 44, 20, "switch");
    send_pane_shell_command(&api, pane, "PS1='$ '; printf 'MOBILE_%s\\n' READY");
    wait_screen(&output, 44, 20, "MOBILE_READY");
    let open = |input: &mut Box<dyn Write + Send>| {
        input.write_all(b"\x1b[<0;36;1M\x1b[<0;36;1m").unwrap();
        wait_screen(&output, 44, 20, "+ new workspace");
    };
    open(&mut input);
    save_frame(&evidence, "01-switcher", &output, 44, 20);
    input.write_all(b"s").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            let frame = screen(&output, 44, 20);
            frame.contains("/ s") && !frame.contains("Go to")
        }
    ));
    save_frame(&evidence, "02-first-s", &output, 44, 20);
    input.write_all(b"earchDest42").unwrap();
    wait_screen(&output, 44, 20, "/ searchDest42");
    save_frame(&evidence, "03-complete-query", &output, 44, 20);
    input.write_all(b"\x1b[27u").unwrap();
    wait_screen(&output, 44, 20, "+ new workspace");
    save_frame(&evidence, "04-restored", &output, 44, 20);
    input.write_all(b"\x1b[27u").unwrap();
    wait_screen(&output, 44, 20, "MOBILE_READY");
    assert!(!screen(&output, 44, 20).contains("+ new workspace"));
    save_frame(&evidence, "05-terminal", &output, 44, 20);
    open(&mut input);
    input.write_all(b"\x1b[0;;20320:22909u").unwrap();
    wait_screen(&output, 44, 20, "你好");
    input.write_all(b"\x1b[200~ world 42\x1b[201~").unwrap();
    wait_screen(&output, 44, 20, "你好 world 42");
    save_frame(&evidence, "06-unicode-paste", &output, 44, 20);
    input.write_all(b"\x1b[27u").unwrap();
    wait_screen(&output, 44, 20, "+ new workspace");
    input.write_all(b"\x1b[27u").unwrap();
    wait_screen(&output, 44, 20, "MOBILE_READY");
    open(&mut input);
    input.write_all(b"\x1b[200~SearchDest42\x1b[201~").unwrap();
    wait_screen(&output, 44, 20, "/ SearchDest42");
    input.write_all(b"\r").unwrap();
    wait_screen(&output, 44, 20, "MOBILE_READY");
    assert!(!screen(&output, 44, 20).contains("+ new workspace"));
    save_frame(&evidence, "07-accepted", &output, 44, 20);
    input.write_all(b"printf 'MOBILE_%s\\n' CLEAN\r").unwrap();
    wait_screen(&output, 44, 20, "MOBILE_CLEAN");
    save_frame(&evidence, "08-no-pane-forwarding", &output, 44, 20);
    open(&mut input);
    input.write_all(b"\x1b[200~mobile-dir42\x1b[201~").unwrap();
    wait_screen(&output, 44, 20, "create workspace");
    save_frame(&evidence, "09-directory-search", &output, 44, 20);
    input.write_all(b"\r").unwrap();
    // Creation is asynchronous: poll the authoritative workspace list until
    // the new directory workspace exists instead of reading an early frame.
    let mut directory_workspace = String::new();
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(80), || {
            let workspaces = send_json_request(
                &api,
                r#"{"id":"directory-workspaces","method":"workspace.list","params":{}}"#,
            );
            let Some(found) =
                workspaces["result"]["workspaces"]
                    .as_array()
                    .and_then(|workspaces| {
                        workspaces
                            .iter()
                            .find(|workspace| workspace["label"] == "mobile-dir42")
                    })
            else {
                return false;
            };
            directory_workspace = found["workspace_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            !directory_workspace.is_empty()
        }),
        "created directory workspace: {}",
        screen(&output, 44, 20)
    );
    let directory_workspace = directory_workspace.as_str();
    let directory_pane = first_pane_id_in_workspace(&api, directory_workspace);
    assert_ne!(directory_pane, pane);
    send_pane_shell_command(&api, &directory_pane, "printf 'DIRECTORY_%s\\n' READY");
    wait_screen(&output, 44, 20, "DIRECTORY_READY");
    save_frame(&evidence, "10-directory-ready", &output, 44, 20);
    input
        .write_all(b"printf 'DIRECTORY_%s\\n' ACCEPTED\r")
        .unwrap();
    wait_screen(&output, 44, 20, "DIRECTORY_ACCEPTED");
    save_frame(&evidence, "11-directory-accepted", &output, 44, 20);
    fs::write(evidence.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "binary":env!("CARGO_BIN_EXE_herdr"), "geometry":"44x20", "created":created,
        "inputs":["tap switch", "s", "earchDest42", "Esc restore", "Esc terminal", "tap switch", "kitty associated text 你好", "bracketed paste ' world 42'", "Esc restore", "Esc terminal", "tap switch", "bracketed paste SearchDest42", "Enter", "printf MOBILE_CLEAN", "tap switch", "bracketed paste mobile-dir42", "Enter", "targeted API printf DIRECTORY_READY", "observe DIRECTORY_READY in client frame", "printf DIRECTORY_ACCEPTED"],
        "directory_readiness":{"workspace_id":directory_workspace,"pane_id":directory_pane,"marker":"DIRECTORY_READY"},
        "outcome":"inline switcher search, two-stage Esc, Unicode, paste, pane and directory acceptance and no pane forwarding passed"
    })).unwrap()).unwrap();
    drop(input);
    drop(client);
    drop(server);
    cleanup_test_base(&base);
}

/// Last absolute cursor position (0-based) written to the host terminal.
fn last_cursor_position(output: &SharedOutput) -> Option<(u16, u16)> {
    let bytes = &output.lock().unwrap().bytes;
    let mut found = None;
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == 0x1b && bytes[index + 1] == b'[' {
            let mut cursor = index + 2;
            let mut row = 0u32;
            let mut row_digits = 0;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                row = row * 10 + u32::from(bytes[cursor] - b'0');
                cursor += 1;
                row_digits += 1;
            }
            if row_digits > 0 && cursor < bytes.len() && bytes[cursor] == b';' {
                cursor += 1;
                let mut column = 0u32;
                let mut column_digits = 0;
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    column = column * 10 + u32::from(bytes[cursor] - b'0');
                    cursor += 1;
                    column_digits += 1;
                }
                if column_digits > 0
                    && cursor < bytes.len()
                    && matches!(bytes[cursor], b'H' | b'f')
                    && row > 0
                    && column > 0
                {
                    found = Some(((row - 1) as u16, (column - 1) as u16));
                }
            }
            index = cursor;
        } else {
            index += 1;
        }
    }
    found
}

fn switcher_acceptance_cli(env_pairs: &[(&str, &str)], args: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(args)
        .env_clear()
        .envs(env_pairs.iter().copied())
        .output()
        .expect("herdr cli");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("herdr cli json")
}

#[test]
fn switcher_search_live_acceptance_corrections() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_MERGE_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/switcher-search-acceptance")
        });
    fs::create_dir_all(&evidence).unwrap();
    let stub_bin = base.join("stub-bin");
    fs::create_dir_all(&stub_bin).unwrap();
    let zoxide = stub_bin.join("zoxide");
    fs::write(
        &zoxide,
        "#!/bin/sh\necho COORDINATOR_DIRECTORY_ERROR >&2\nexit 2\n",
    )
    .unwrap();
    fs::set_permissions(&zoxide, fs::Permissions::from_mode(0o700)).unwrap();
    let path_env = format!(
        "{}:{}",
        stub_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let server = spawn_server_with_config_and_env(
        &config,
        &runtime,
        &api,
        "onboarding = false\n[ui]\nstatus_indicators = 'symbols'\n[keys]\nworkspace_switcher = 'alt+tab'\n",
        &[("PATH", path_env.as_str())],
    );
    wait_for_socket(&api, Duration::from_secs(10));
    let root = base.join(format!("ws-path-marker-{}", std::process::id() % 1000));
    let other = base.join("other");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&other).unwrap();
    let created = send_json_request(
        &api,
        &serde_json::json!({
            "id":"accept-create", "method":"workspace.create",
            "params":{"cwd":root,"label":"ProofParent"}
        })
        .to_string(),
    );
    assert!(created.get("error").is_none(), "{created}");
    let workspace = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let state_dir = runtime.join("state").to_str().unwrap().to_owned();
    let data_dir = base.join("data").to_str().unwrap().to_owned();
    let cache_dir = base.join("cache").to_str().unwrap().to_owned();
    let cli_env = [
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", config.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ("XDG_STATE_HOME", state_dir.as_str()),
        ("XDG_DATA_HOME", data_dir.as_str()),
        ("XDG_CACHE_HOME", cache_dir.as_str()),
        ("HERDR_SOCKET_PATH", api.to_str().unwrap()),
        ("PATH", path_env.as_str()),
    ];
    for label in ["zzneedleSlow", "needleFast"] {
        switcher_acceptance_cli(
            &cli_env,
            &[
                "tab",
                "create",
                "--workspace",
                &workspace,
                "--cwd",
                other.to_str().unwrap(),
                "--label",
                label,
                "--no-focus",
            ],
        );
    }
    let client = spawn_client_shell_process(&config, &runtime, &api);
    client
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 120,
            rows: 36,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output = spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
    wait_screen(&output, 120, 36, "ProofParent");
    let mut checks = serde_json::Map::new();
    // The workspace's projected path matches and includes its whole subtree,
    // including Tabs located elsewhere. Historical creation paths are not
    // promised once every pane has moved away.
    input.write_all(b"\x1b[9;3u\x1b[115;3u").unwrap();
    let marker = root.file_name().unwrap().to_str().unwrap().to_owned();
    input.write_all(marker.as_bytes()).unwrap();
    wait_screen(&output, 120, 36, &format!("/ {marker}"));
    save_frame(&evidence, "01-workspace-path", &output, 120, 36);
    let path_frame = screen(&output, 120, 36);
    checks.insert(
        "workspace_path".into(),
        (!path_frame.contains("No matching")).into(),
    );
    checks.insert(
        "workspace_path_includes_whole_subtree".into(),
        (path_frame.contains("zzneedleSlow") && path_frame.contains("needleFast")).into(),
    );
    // Tab siblings sort by match quality: prefix needleFast before zzneedleSlow.
    input
        .write_all(b"\x1b[27u\x1b[27u\x1b[9;3u\x1b[115;3u")
        .unwrap();
    input.write_all(b"needle").unwrap();
    wait_screen(&output, 120, 36, "/ needle");
    save_frame(&evidence, "02-tab-order", &output, 120, 36);
    let lines: Vec<String> = screen(&output, 120, 36)
        .lines()
        .map(str::to_owned)
        .collect();
    let title = lines
        .iter()
        .position(|line| line.contains("Workspace Switcher"))
        .unwrap();
    let fast = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > title + 2 && line.contains("needleFast"))
        .map(|(index, _)| index)
        .unwrap();
    let slow = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > title + 2 && line.contains("zzneedleSlow"))
        .map(|(index, _)| index)
        .unwrap();
    checks.insert("tab_prefix_before_contains".into(), (fast < slow).into());
    checks.insert(
        "tab_row_indices".into(),
        serde_json::json!({"prefix":fast,"contains":slow}),
    );
    // Removed selected tab restores its surviving workspace on Esc.
    input.write_all(b"\x1b[27u\x1b[27u").unwrap();
    let second = send_json_request(
        &api,
        &serde_json::json!({
            "id":"accept-second", "method":"workspace.create",
            "params":{"cwd":other,"label":"RestoreParent","focus":false}
        })
        .to_string(),
    );
    assert!(second.get("error").is_none(), "{second}");
    let second_workspace = second["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let extra = switcher_acceptance_cli(
        &cli_env,
        &[
            "tab",
            "create",
            "--workspace",
            &second_workspace,
            "--cwd",
            other.to_str().unwrap(),
            "--label",
            "RestoreTab",
            "--no-focus",
        ],
    );
    let extra_tab = extra["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_screen(&output, 120, 36, "RestoreParent");
    input.write_all(b"\x1b[9;3u\x1b[108;3u").unwrap();
    wait_screen(&output, 120, 36, "RestoreTab");
    input.write_all(b"\x1b[106;3u\x1b[106;3u").unwrap();
    save_frame(&evidence, "03-selected-tab-before-search", &output, 120, 36);
    input.write_all(b"\x1b[115;3umissing-query-accept").unwrap();
    wait_screen(&output, 120, 36, "/ missing-query-accept");
    send_json_request(
        &api,
        &serde_json::json!({
            "id":"accept-close-tab", "method":"tab.close", "params":{"tab_id":extra_tab}
        })
        .to_string(),
    );
    thread::sleep(Duration::from_millis(400));
    input.write_all(b"\x1b[27u").unwrap();
    wait_screen(
        &output,
        120,
        36,
        "search workspaces, tabs, panes, directories",
    );
    save_frame(&evidence, "03-missing-tab-restored", &output, 120, 36);
    input.write_all(b"\r").unwrap();
    let focused_workspace = |request_id: &str| -> String {
        send_json_request(
            &api,
            &serde_json::json!({
                "id":request_id, "method":"workspace.list", "params":{}
            })
            .to_string(),
        )["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["focused"] == true)
            .and_then(|item| item["workspace_id"].as_str())
            .unwrap()
            .to_owned()
    };
    let mut focused = String::new();
    wait_until(Duration::from_secs(10), Duration::from_millis(100), || {
        focused = focused_workspace("accept-focus-1");
        focused == second_workspace
    });
    checks.insert(
        "removed_tab_restores_parent".into(),
        (focused == second_workspace).into(),
    );
    checks.insert(
        "removed_tab_focus".into(),
        serde_json::json!({"actual":focused,"expected":second_workspace}),
    );
    // Clicking the search input while search is active must not recapture the
    // return selection.
    send_json_request(
        &api,
        &serde_json::json!({
            "id":"accept-refocus", "method":"workspace.focus", "params":{"workspace_id":workspace}
        })
        .to_string(),
    );
    thread::sleep(Duration::from_millis(400));
    input.write_all(b"\x1b[9;3u\x1b[115;3uProofParent").unwrap();
    wait_screen(&output, 120, 36, "/ ProofParent");
    let lines: Vec<String> = screen(&output, 120, 36)
        .lines()
        .map(str::to_owned)
        .collect();
    let title = lines
        .iter()
        .position(|line| line.contains("Workspace Switcher"))
        .unwrap();
    let query_row = title + 2;
    input
        .write_all(format!("\x1b[<0;12;{query_row}M\x1b[<0;12;{query_row}m").as_bytes())
        .unwrap();
    input.write_all(b"\x1b[27u").unwrap();
    save_frame(&evidence, "04-input-click-restored", &output, 120, 36);
    input.write_all(b"\r").unwrap();
    let mut focused = String::new();
    wait_until(Duration::from_secs(10), Duration::from_millis(100), || {
        focused = focused_workspace("accept-focus-2");
        focused == second_workspace
    });
    checks.insert(
        "input_click_preserves_return_selection".into(),
        (focused == second_workspace).into(),
    );
    checks.insert(
        "input_click_focus".into(),
        serde_json::json!({"expected":second_workspace,"actual":focused}),
    );
    // Mobile caret follows the real editor cursor and cell widths.
    client
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 45,
            rows: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            screen(&output, 45, 60)
                .lines()
                .take(2)
                .any(|line| line.contains("switch"))
        }
    ));
    let lines: Vec<String> = screen(&output, 45, 60).lines().map(str::to_owned).collect();
    let switch_row = lines
        .iter()
        .take(2)
        .position(|line| line.contains("switch"))
        .unwrap();
    let switch_col = lines[switch_row].find("switch").unwrap();
    input
        .write_all(
            format!(
                "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                switch_col + 1,
                switch_row + 1,
                switch_col + 1,
                switch_row + 1
            )
            .as_bytes(),
        )
        .unwrap();
    wait_screen(&output, 45, 60, "+ new workspace");
    input
        .write_all("\x1b[200~你好AB\x1b[201~".as_bytes())
        .unwrap();
    wait_screen(&output, 45, 60, "你好AB");
    thread::sleep(Duration::from_millis(300));
    let caret_before = last_cursor_position(&output);
    input.write_all(b"\x1b[D").unwrap();
    thread::sleep(Duration::from_millis(300));
    let caret_after = last_cursor_position(&output);
    checks.insert(
        "mobile_unicode_caret_width".into(),
        (caret_before == Some((1, 9))).into(),
    );
    checks.insert(
        "mobile_left_moves_caret".into(),
        (caret_after == Some((1, 8))).into(),
    );
    checks.insert(
        "mobile_caret_positions".into(),
        serde_json::json!({"before":caret_before,"after":caret_after}),
    );
    checks.insert(
        "mobile_directory_error_visible".into(),
        wait_until(Duration::from_secs(12), Duration::from_millis(40), || {
            let frame = screen(&output, 45, 60);
            frame.contains("zoxide") || frame.contains("COORDINATOR_DIRECTORY_ERROR")
        })
        .into(),
    );
    save_frame(&evidence, "05-mobile-caret-and-error", &output, 45, 60);
    // Blank query restores the switcher list; item clicks still accept.
    input.write_all(b"\x1b[97;5u\x1b[107;5u").unwrap();
    wait_screen(&output, 45, 60, "+ new workspace");
    let lines: Vec<String> = screen(&output, 45, 60).lines().map(str::to_owned).collect();
    let item_row = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > 3 && line.contains("ProofParent"))
        .map(|(index, _)| index)
        .unwrap();
    input
        .write_all(format!("\x1b[<0;8;{}M\x1b[<0;8;{}m", item_row + 1, item_row + 1).as_bytes())
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    checks.insert(
        "mobile_blank_list_click_accepts".into(),
        (!screen(&output, 45, 60).contains("+ new workspace")).into(),
    );
    save_frame(&evidence, "06-mobile-blank-click", &output, 45, 60);
    // The search surface keeps the close affordance.
    if !screen(&output, 45, 60)
        .lines()
        .take(2)
        .any(|line| line.contains("switch"))
    {
        input.write_all(b"\x1b[27u").unwrap();
    }
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            screen(&output, 45, 60)
                .lines()
                .take(2)
                .any(|line| line.contains("switch"))
        }
    ));
    let lines: Vec<String> = screen(&output, 45, 60).lines().map(str::to_owned).collect();
    let switch_row = lines
        .iter()
        .take(2)
        .position(|line| line.contains("switch"))
        .unwrap();
    let switch_col = lines[switch_row].find("switch").unwrap();
    input
        .write_all(
            format!(
                "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                switch_col + 1,
                switch_row + 1,
                switch_col + 1,
                switch_row + 1
            )
            .as_bytes(),
        )
        .unwrap();
    wait_screen(&output, 45, 60, "+ new workspace");
    input
        .write_all("\x1b[200~needle\x1b[201~".as_bytes())
        .unwrap();
    wait_screen(&output, 45, 60, "/ needle");
    let close: Vec<(usize, usize)> = screen(&output, 45, 60)
        .lines()
        .take(2)
        .enumerate()
        .filter_map(|(row, line)| line.find("close").map(|col| (row, col)))
        .collect();
    checks.insert(
        "mobile_search_close_button_present".into(),
        (!close.is_empty()).into(),
    );
    if let Some((row, col)) = close.first() {
        input
            .write_all(
                format!(
                    "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                    col + 1,
                    row + 1,
                    col + 1,
                    row + 1
                )
                .as_bytes(),
            )
            .unwrap();
        thread::sleep(Duration::from_millis(300));
    }
    let frame = screen(&output, 45, 60);
    checks.insert(
        "mobile_search_close_returns_terminal".into(),
        (!frame.contains("/ needle") && !frame.contains("+ new workspace")).into(),
    );
    save_frame(&evidence, "07-mobile-close", &output, 45, 60);
    // Agent status icon and pane label stay visible with a long path meta.
    let agent_pane = first_pane_id_in_workspace(&api, &workspace);
    send_json_request(
        &api,
        &serde_json::json!({
            "id":"agent-rename", "method":"pane.rename",
            "params":{"pane_id":agent_pane,"label":"IconProbe"}
        })
        .to_string(),
    );
    send_json_request(
        &api,
        &serde_json::json!({
            "id":"agent-report", "method":"pane.report_agent",
            "params":{"pane_id":agent_pane,"source":"acceptance-proof","agent":"pi","state":"working","seq":1}
        })
        .to_string(),
    );
    let switch: Vec<(usize, usize)> = screen(&output, 45, 60)
        .lines()
        .take(2)
        .enumerate()
        .filter_map(|(row, line)| line.find("switch").map(|col| (row, col)))
        .collect();
    if let Some((row, col)) = switch.first() {
        input
            .write_all(
                format!(
                    "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                    col + 1,
                    row + 1,
                    col + 1,
                    row + 1
                )
                .as_bytes(),
            )
            .unwrap();
    }
    wait_screen(&output, 45, 60, "+ new workspace");
    input.write_all(b"\x1b[200~IconProbe\x1b[201~").unwrap();
    wait_screen(&output, 45, 60, "/ IconProbe");
    thread::sleep(Duration::from_millis(500));
    save_frame(&evidence, "08-mobile-agent-icon-long-path", &output, 45, 60);
    let agent_rows = screen(&output, 45, 60)
        .lines()
        .skip(3)
        .collect::<Vec<_>>()
        .join("\n");
    checks.insert(
        "mobile_long_path_preserves_pane_name".into(),
        agent_rows.contains("IconProbe").into(),
    );
    checks.insert(
        "mobile_configured_agent_icon_visible".into(),
        agent_rows.contains("◐").into(),
    );
    // An accepted query edit while a Directory create is pending rejects the
    // stale create result: the Switcher stays open with the new query.
    struct ContGuard(libc::pid_t);
    impl Drop for ContGuard {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGCONT) };
        }
    }
    let race_directory = base.join(format!("kzxv987q{}", std::process::id() % 1000));
    fs::create_dir_all(&race_directory).unwrap();
    fs::write(
        &zoxide,
        format!("#!/bin/sh\nprintf '10 {}\\n'\n", race_directory.display()),
    )
    .unwrap();
    input.write_all(b"\x1b[27u\x1b[27u").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            let rows: Vec<String> = screen(&output, 45, 60)
                .lines()
                .take(2)
                .map(str::to_owned)
                .collect();
            rows.iter().any(|line| line.contains("switch"))
                && !rows.iter().any(|line| line.contains("close"))
        },
    ));
    let switch: Vec<(usize, usize)> = screen(&output, 45, 60)
        .lines()
        .take(2)
        .enumerate()
        .filter_map(|(row, line)| line.find("switch").map(|col| (row, col)))
        .collect();
    if let Some((row, col)) = switch.first() {
        input
            .write_all(
                format!(
                    "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                    col + 1,
                    row + 1,
                    col + 1,
                    row + 1
                )
                .as_bytes(),
            )
            .unwrap();
    }
    wait_screen(&output, 45, 60, "+ new workspace");
    input.write_all(b"\x1b[200~kzxv987q\x1b[201~").unwrap();
    wait_screen(&output, 45, 60, "create workspace");
    let directory_row = screen(&output, 45, 60)
        .lines()
        .position(|line| line.contains("create workspace"))
        .unwrap()
        + 1;
    let server_pid = server.child.process_id().expect("server pid") as libc::pid_t;
    let guard = ContGuard(server_pid);
    unsafe { libc::kill(server_pid, libc::SIGSTOP) };
    input
        .write_all(format!("\x1b[<0;8;{directory_row}M\x1b[<0;8;{directory_row}m").as_bytes())
        .unwrap();
    input
        .write_all(b"\x1b[200~query-after-create\x1b[201~")
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    checks.insert(
        "query_accepted_before_create_response".into(),
        screen(&output, 45, 60)
            .lines()
            .take(3)
            .any(|line| line.contains("query-after-create"))
            .into(),
    );
    save_frame(&evidence, "09-stale-create-paused", &output, 45, 60);
    unsafe { libc::kill(server_pid, libc::SIGCONT) };
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(100),
        || {
            send_json_request(
                &api,
                r#"{"id":"race-list","method":"workspace.list","params":{}}"#,
            )["result"]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| {
                    item["label"]
                        .as_str()
                        .is_some_and(|label| label.contains("kzxv987q"))
                })
        },
    ));
    thread::sleep(Duration::from_millis(500));
    save_frame(&evidence, "10-stale-create-resumed", &output, 45, 60);
    checks.insert(
        "query_change_during_create_keeps_switcher".into(),
        screen(&output, 45, 60)
            .lines()
            .take(3)
            .any(|line| line.contains("query-after-create"))
            .into(),
    );
    drop(guard);
    fs::write(
        evidence.join("acceptance-result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "binary":env!("CARGO_BIN_EXE_herdr"), "geometry":"120x36 then 45x60", "checks":checks,
            "outcome":"switcher search acceptance corrections live proof"
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        checks
            .values()
            .filter(|value| value.is_boolean())
            .all(|value| value == true),
        "{checks:?}"
    );
    drop(input);
    drop(client);
    drop(server);
    cleanup_test_base(&base);
}
#[test]
fn switcher_search_live_desktop_hold_release_restore_and_accept() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let evidence = std::env::var_os("HERDR_MERGE_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/switcher-search-live")
        });
    fs::create_dir_all(&evidence).unwrap();
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let server = spawn_server_with_config(
        &config,
        &runtime,
        &api,
        &runtime.join("client.sock"),
        "onboarding = false\n[keys]\ngoto = 'ctrl+g'\nworkspace_switcher = 'alt+tab'\n",
    );
    wait_for_socket(&api, Duration::from_secs(10));
    let created = send_json_request(&api, &serde_json::json!({
        "id":"desktop-create", "method":"workspace.create", "params":{"cwd":base,"label":"LocalBase"}
    }).to_string());
    assert!(created.get("error").is_none(), "{created}");
    let pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    let second = send_json_request(&api, &serde_json::json!({
        "id":"desktop-create-2", "method":"workspace.create", "params":{"cwd":base,"label":"SecondBase","focus":false}
    }).to_string());
    assert!(second.get("error").is_none(), "{second}");
    let second_pane = second["result"]["root_pane"]["pane_id"].as_str().unwrap();
    let client = spawn_client_shell_process(&config, &runtime, &api);
    client
        ._master
        .as_ref()
        .unwrap()
        .resize(PtySize {
            cols: 120,
            rows: 36,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output = spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
    let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
    send_pane_shell_command(&api, pane, "PS1='$ '; printf 'LOCAL_%s\\n' READY");
    send_pane_shell_command(&api, second_pane, "PS1='$ '; printf 'SECOND_%s\\n' READY");
    wait_screen(&output, 120, 36, "LOCAL_READY");
    input.write_all(b"\x1b[9;3u").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            let frame = screen(&output, 120, 36);
            frame.contains("Workspace Switcher") && frame.contains("release to open")
        }
    ));
    save_frame(&evidence, "01-hold", &output, 120, 36);
    input.write_all(b"\x1b[115;3u").unwrap();
    input.write_all(b"LocalB").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            let frame = screen(&output, 120, 36);
            frame.contains("/ LocalB") && frame.contains("LocalBase")
        }
    ));
    save_frame(&evidence, "02-query", &output, 120, 36);
    input.write_all(b"\x1b[57443;1:3u").unwrap();
    thread::sleep(Duration::from_millis(300));
    let latched = screen(&output, 120, 36);
    assert!(
        latched.contains("Workspace Switcher") && latched.contains("/ LocalB"),
        "modifier release must not accept during search: {latched}"
    );
    save_frame(&evidence, "03-release-latched", &output, 120, 36);
    input.write_all(b"\x1b[27u").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || {
            let frame = screen(&output, 120, 36);
            frame.contains("LocalBase")
                && frame.contains("SecondBase")
                && !frame.contains("/ LocalB")
        }
    ));
    save_frame(&evidence, "04-restored", &output, 120, 36);
    input.write_all(b"\x1b[57443;1:3u").unwrap();
    thread::sleep(Duration::from_millis(300));
    let latched = screen(&output, 120, 36);
    assert!(
        latched.contains("Workspace Switcher"),
        "release after Esc restore still must not accept: {latched}"
    );
    save_frame(&evidence, "05-restored-release-latched", &output, 120, 36);
    input.write_all(b"\x1b[27u").unwrap();
    assert!(wait_until(
        Duration::from_secs(12),
        Duration::from_millis(40),
        || { !screen(&output, 120, 36).contains("Workspace Switcher") }
    ));
    save_frame(&evidence, "06-closed", &output, 120, 36);
    input.write_all(b"\x1b[9;3u").unwrap();
    wait_screen(&output, 120, 36, "Workspace Switcher");
    input.write_all(b"\x1b[57443;1:3u").unwrap();
    wait_screen(&output, 120, 36, "SECOND_READY");
    save_frame(&evidence, "07-reopen-release-accepts", &output, 120, 36);
    input.write_all(b"\x07").unwrap();
    wait_screen(&output, 120, 36, "Go to");
    save_frame(&evidence, "08-go-to-unchanged", &output, 120, 36);
    input.write_all(b"\x1b[27u").unwrap();
    assert!(wait_until(
        Duration::from_secs(5),
        Duration::from_millis(40),
        || { !screen(&output, 120, 36).contains("Go to") }
    ));
    fs::write(evidence.join("switcher-search-result.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "binary":env!("CARGO_BIN_EXE_herdr"), "geometry":"120x36", "created":[created, second],
        "inputs":["alt+tab hold","alt+s search entry","type LocalB","left-alt release latched","Esc restore","left-alt release latched","Esc close","alt+tab hold","left-alt release accepts SecondBase","ctrl+g Go to","Esc close"],
        "outcome":"switcher search entry, release latch, two-Esc restore/close, reopen hold acceptance and unchanged Go to passed"
    })).unwrap()).unwrap();
    drop(input);
    drop(client);
    drop(server);
    cleanup_test_base(&base);
}
