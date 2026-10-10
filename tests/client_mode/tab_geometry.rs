use super::*;

struct GeometryClient {
    stream: UnixStream,
    snapshot: Value,
}

impl GeometryClient {
    fn connect(socket: &PathBuf, cols: u16, rows: u16) -> Self {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (generation, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, cols, rows)
            .expect("client shell handshake");
        assert_eq!(generation, CURRENT_PROTOCOL);
        assert!(error.is_none(), "{error:?}");
        loop {
            let (tag, payload) = read_server_message(&mut stream).unwrap();
            if tag == support::SERVER_MESSAGE_ENDPOINT_CONTROL {
                let ((kind, data), _) = bincode::serde::decode_from_slice::<(String, String), _>(
                    &payload,
                    bincode::config::standard(),
                )
                .unwrap();
                if kind == "shell.snapshot.v1" {
                    return Self {
                        stream,
                        snapshot: serde_json::from_str(&data).unwrap(),
                    };
                }
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) {
        let request = serde_json::json!({"id":"geometry", "method":method, "params":params});
        let payload = bincode::serde::encode_to_vec(
            (
                15_u32,
                self.snapshot["boot_id"].as_str().unwrap(),
                request.to_string(),
            ),
            bincode::config::standard(),
        )
        .unwrap();
        self.stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .unwrap();
        self.stream.write_all(&payload).unwrap();
        let mut response = Vec::new();
        loop {
            let (tag, payload) = read_server_message(&mut self.stream).unwrap();
            if tag == 18 {
                let ((_, id, final_chunk, data), _) =
                    bincode::serde::decode_from_slice::<(String, String, bool, Vec<u8>), _>(
                        &payload,
                        bincode::config::standard(),
                    )
                    .unwrap();
                assert_eq!(id, "geometry");
                response.extend(data);
                if final_chunk {
                    let response: Value = serde_json::from_slice(&response).unwrap();
                    assert!(response.get("error").is_none(), "{response}");
                    return;
                }
            }
        }
    }
}

fn public_request(api: &PathBuf, method: &str, params: Value) -> Value {
    let response = send_json_request(
        api,
        &serde_json::json!({"id":"geometry", "method":method, "params":params}).to_string(),
    );
    assert!(response.get("error").is_none(), "{response}");
    response
}

fn pane_size(api: &PathBuf, pane_id: &str) -> Value {
    let response = public_request(api, "pane.layout", serde_json::json!({"pane_id":pane_id}));
    response["result"]["layout"]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["pane_id"] == pane_id)
        .unwrap()["terminal_size"]
        .clone()
}

fn public_tab_creation_geometry(focus_during_create: bool) {
    let _lock = test_lock();
    let evidence = std::env::var_os("HERDR_TAB_GEOMETRY_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tab-geometry-evidence")
        });
    fs::create_dir_all(&evidence).unwrap();
    for (first_size, second_size) in [((51, 26), (274, 77)), ((274, 77), (51, 26))] {
        let base = unique_test_dir();
        let config = base.join("config");
        let runtime = base.join("runtime");
        let api = runtime.join("herdr.sock");
        let socket = runtime.join("herdr-client.sock");
        let server = spawn_server(&config, &runtime, &api, &socket);
        wait_for_socket(&api, Duration::from_secs(10));
        wait_for_socket(&socket, Duration::from_secs(10));
        let workspace = public_request(&api, "workspace.create", serde_json::json!({"cwd":base}));
        let workspace_id = workspace["result"]["workspace"]["workspace_id"]
            .as_str()
            .unwrap();
        let source_tab = workspace["result"]["tab"]["tab_id"].as_str().unwrap();
        let source_pane = workspace["result"]["root_pane"]["pane_id"]
            .as_str()
            .unwrap();
        let mut first = GeometryClient::connect(&socket, first_size.0, first_size.1);
        let mut second = GeometryClient::connect(&socket, second_size.0, second_size.1);
        second.request("pane.focus", serde_json::json!({"pane_id":source_pane}));
        let expected = pane_size(&api, source_pane);
        assert_eq!(expected["rows"], second_size.1);
        let created = public_request(
            &api,
            "tab.create",
            serde_json::json!({
                "workspace_id":workspace_id, "cwd":base, "label":"editor", "focus":focus_during_create
            }),
        );
        let editor_tab = created["result"]["tab"]["tab_id"].as_str().unwrap();
        let editor_pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
        if !focus_during_create {
            let snapshot = public_request(&api, "session.snapshot", serde_json::json!({}));
            assert_eq!(snapshot["result"]["snapshot"]["focused_tab_id"], source_tab);
            public_request(&api, "tab.focus", serde_json::json!({"tab_id":editor_tab}));
        }
        let actual = pane_size(&api, editor_pane);
        let record = serde_json::json!({
            "focus_during_create":focus_during_create, "first_surface":first_size,
            "foreground_surface":second_size, "expected_terminal":expected, "editor_terminal":actual
        });
        let artifact = evidence.join(format!(
            "create-{focus_during_create}-first-{}.json",
            first_size.0
        ));
        fs::write(&artifact, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
        assert_eq!(
            actual, expected,
            "new editor must use the foreground viewer: {record}"
        );

        first.request("pane.focus", serde_json::json!({"pane_id":editor_pane}));
        let existing_controller_size = pane_size(&api, editor_pane);
        second.request("pane.focus", serde_json::json!({"pane_id":source_pane}));
        public_request(&api, "tab.focus", serde_json::json!({"tab_id":editor_tab}));
        let preserved = pane_size(&api, editor_pane);
        assert_eq!(
            preserved, existing_controller_size,
            "public focus must retain a viewing controller"
        );
        fs::write(&artifact, serde_json::to_vec_pretty(&serde_json::json!({
            "creation":record, "existing_controller":existing_controller_size, "after_public_refocus":preserved
        })).unwrap()).unwrap();
        drop(first);
        drop(second);
        cleanup_spawned_herdr(server, base);
    }
}

#[test]
fn public_editor_tab_focus_uses_foreground_viewer() {
    public_tab_creation_geometry(false);
}

#[test]
fn public_focused_tab_create_uses_foreground_viewer() {
    public_tab_creation_geometry(true);
}

#[test]
fn geometry_reapply_uses_a_viewer_when_foreground_leaves() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let socket = runtime.join("herdr-client.sock");
    let server = spawn_server(&config, &runtime, &api, &socket);
    wait_for_socket(&api, Duration::from_secs(10));
    wait_for_socket(&socket, Duration::from_secs(10));
    let workspace = public_request(&api, "workspace.create", serde_json::json!({"cwd":base}));
    let source_pane = workspace["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap();
    let mut first = GeometryClient::connect(&socket, 51, 26);
    let second = GeometryClient::connect(&socket, 274, 77);
    let third = GeometryClient::connect(&socket, 80, 24);
    first.request("tab.create", serde_json::json!({"cwd":base, "focus":true}));
    let size = pane_size(&api, source_pane);
    assert_eq!(
        size,
        serde_json::json!({"cols":273, "rows":77}),
        "the foreground client no longer views this tab"
    );
    drop((first, second, third));
    cleanup_spawned_herdr(server, base);
}
