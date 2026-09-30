use super::*;
use std::os::unix::fs::PermissionsExt;

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
