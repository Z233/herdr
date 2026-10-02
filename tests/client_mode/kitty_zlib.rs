use super::*;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

struct Relay {
    child: std::process::Child,
    received: Receiver<Vec<u8>>,
}

impl Relay {
    fn start(host: &str) -> Self {
        let mut child = Command::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "Compression=no",
                "-o",
                "ControlPath=none",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=2",
                "--",
                host,
                "printf 'HERDR_RELAY_READY\\n'; exec cat",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start SSH relay");
        let mut stdout = child.stdout.take().unwrap();
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0; 16 * 1024];
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 || sent.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let relay = Self { child, received };
        let mut ready = Vec::new();
        let started = Instant::now();
        while ready.len() < b"HERDR_RELAY_READY\n".len() {
            let remaining = Duration::from_secs(15)
                .checked_sub(started.elapsed())
                .expect("SSH relay startup timeout");
            ready.extend(
                relay
                    .received
                    .recv_timeout(remaining)
                    .expect("SSH host must start the relay within 15 seconds"),
            );
        }
        assert_eq!(ready, b"HERDR_RELAY_READY\n");
        relay
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn terminal() -> ghostty_vt::Terminal {
    let mut terminal = ghostty_vt::Terminal::new(80, 24, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(80, 24, 8, 16).unwrap();
    terminal
}

fn upload(control: &str, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let chunks = data.chunks(3072).collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        let header = if index == 0 { control } else { "" };
        let separator = if index == 0 { "," } else { "" };
        let more = u8::from(index + 1 < chunks.len());
        let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
        write!(bytes, "\x1b_G{header}{separator}m={more};{encoded}\x1b\\").unwrap();
    }
    bytes
}

// Parse the wire records, not Herdr's private output structures.
fn image_upload(wire: &[u8]) -> (String, Vec<u8>, usize) {
    let mut rest = wire;
    let mut control = None;
    let mut data = Vec::new();
    let mut encoded_bytes = 0;
    while let Some(start) = rest.windows(3).position(|w| w == b"\x1b_G") {
        rest = &rest[start + 3..];
        let Some(end) = rest.windows(2).position(|w| w == b"\x1b\\") else {
            break;
        };
        let record = &rest[..end];
        rest = &rest[end + 2..];
        let separator = record.iter().position(|b| *b == b';').unwrap();
        let header = std::str::from_utf8(&record[..separator]).unwrap();
        if control.is_none() {
            if !header.split(',').any(|p| p == "t=d") {
                continue;
            }
            control = Some(header.to_owned());
        }
        let payload = &record[separator + 1..];
        data.extend(
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .unwrap(),
        );
        encoded_bytes += payload.len();
        if !header.split(',').any(|p| p == "m=1") {
            return (control.unwrap(), data, encoded_bytes);
        }
    }
    panic!("no complete inline image upload in the SSH output");
}

fn replay(host: &str, wire: Vec<u8>, pixels: &[u8]) -> u128 {
    let mut relay = Relay::start(host);
    let mut stdin = relay.child.stdin.take().unwrap();
    let expected_len = wire.len();
    let expected_digest = Sha256::digest(&wire);
    let started = Instant::now();
    let writer = thread::spawn(move || stdin.write_all(&wire).expect("write replay to SSH"));
    let mut output = Vec::new();
    let mut terminal = terminal();
    while output.len() < expected_len {
        let remaining = Duration::from_secs(60)
            .checked_sub(started.elapsed())
            .expect("replay timeout");
        let bytes = relay
            .received
            .recv_timeout(remaining)
            .expect("receive SSH replay");
        terminal.write(&bytes);
        output.extend(bytes);
    }
    let elapsed = started.elapsed().as_millis();
    writer.join().expect("SSH replay writer");
    assert_eq!(output.len(), expected_len);
    assert_eq!(Sha256::digest(&output), expected_digest);
    assert!(terminal
        .kitty_image_placements()
        .unwrap()
        .iter()
        .any(|p| p.data == pixels));
    elapsed
}

#[test]
#[ignore = "requires an SSH host; run just test-kitty-zlib-ssh gz"]
fn ssh_relay_preserves_pixels_and_compression_fallback() {
    let _lock = test_lock();
    let host = std::env::var("HERDR_KITTY_SSH_HOST").unwrap_or_else(|_| "gz".into());
    let base = unique_test_dir();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            cleanup_test_base(&self.0);
        }
    }
    let _cleanup = Cleanup(base.clone());
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let config_path = config_home.join(app_dir_name()).join("config.toml");
    let state_home = base.join("state");
    let env = [
        ("HERDR_CONFIG_PATH", config_path.to_str().unwrap()),
        ("XDG_STATE_HOME", state_home.to_str().unwrap()),
        ("TERM", "xterm-kitty"),
        ("SSH_CONNECTION", "test relay"),
    ];
    let server = spawn_server_with_config_and_env(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\n[terminal]\nkitty_graphics = true\n",
        &env,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));
    let mut relay = Relay::start(&host);
    let client = spawn_client_process_with_args_and_env(
        &config_home,
        &runtime_dir,
        &api_socket,
        &["client"],
        &env,
    );
    let mut reader = client._master.as_ref().unwrap().try_clone_reader().unwrap();
    let mut relay_input = relay.child.stdin.take().unwrap();
    thread::spawn(move || {
        let _ = std::io::copy(&mut reader, &mut relay_input);
    });
    let mut terminal = terminal();
    let mut startup = Vec::new();
    let started = Instant::now();
    while !String::from_utf8_lossy(&startup).contains('─') {
        let remaining = Duration::from_secs(15)
            .checked_sub(started.elapsed())
            .expect("attach timeout");
        let bytes = relay
            .received
            .recv_timeout(remaining)
            .expect("client must render via SSH");
        terminal.write(&bytes);
        startup.extend(bytes);
    }
    let workspaces = send_json_request(
        &api_socket,
        r#"{"id":"list","method":"workspace.list","params":{}}"#,
    );
    let workspace = workspaces["result"]["workspaces"][0]["workspace_id"]
        .as_str()
        .unwrap();
    let pane = first_pane_id_in_workspace(&api_socket, workspace);
    let artifacts = PathBuf::from("/var/tmp").join(format!(
        "herdr-kitty-zlib-ssh-{}",
        base.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(&artifacts).unwrap();
    let mut metrics = Vec::new();
    for (name, format, channels, width, height, compressed) in [
        ("rgb", 24, 3, 1920, 1440, true),
        ("rgba", 32, 4, 1920, 1440, true),
        ("noise", 32, 4, 256, 128, false),
    ] {
        let mut random = 42_u32;
        let pixels: Vec<u8> = (0..width * height * channels)
            .map(|index| {
                if compressed {
                    ((index / channels / 240) % 16) as u8 * 16
                } else {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    random as u8
                }
            })
            .collect();
        let mut fixture = b"\x1b_Ga=d,d=A,q=2;\x1b\\\x1b[2J\x1b[H".to_vec();
        fixture.extend(upload(
            &format!("a=t,t=d,f={format},s={width},v={height},i=7,q=2"),
            &pixels,
        ));
        fixture.extend_from_slice(b"\x1b_Ga=p,i=7,c=20,r=10,q=2;\x1b\\");
        let path = base.join(format!("{name}.kitty"));
        fs::write(&path, &fixture).unwrap();
        let mut wire = Vec::new();
        let started = Instant::now();
        send_pane_shell_command(&api_socket, &pane, &format!("cat {}", path.display()));
        loop {
            let remaining = Duration::from_secs(60)
                .checked_sub(started.elapsed())
                .expect("image timeout");
            let bytes = relay
                .received
                .recv_timeout(remaining)
                .expect("image must arrive over SSH");
            terminal.write(&bytes);
            wire.extend(bytes);
            if terminal
                .kitty_image_placements()
                .unwrap()
                .iter()
                .any(|p| p.data == pixels)
            {
                break;
            }
        }
        let end_to_end_ms = started.elapsed().as_millis();
        let (header, payload, encoded_bytes) = image_upload(&wire);
        assert_eq!(
            header.split(',').any(|p| p == "o=z"),
            compressed,
            "{header}"
        );
        let decoded = if compressed {
            miniz_oxide::inflate::decompress_to_vec_zlib(&payload).unwrap()
        } else {
            payload.clone()
        };
        assert_eq!(decoded, pixels);
        if compressed {
            assert!(payload.len() < pixels.len() * 9 / 10);
        }
        let controls = header
            .split(',')
            .filter(|p| !p.starts_with("m=") && *p != "o=z" && !p.starts_with("a="))
            .collect::<Vec<_>>()
            .join(",");
        let placement = b"\x1b_Ga=p,i=7,c=20,r=10,q=2;\x1b\\";
        // Same output controls and pixels for both replays; only compression differs.
        let controls = controls
            .split(',')
            .filter(|p| !p.starts_with("i="))
            .collect::<Vec<_>>()
            .join(",");
        let mut raw = upload(&format!("a=t,{controls},i=7"), &pixels);
        raw.extend_from_slice(placement);
        let mut optimized = upload(
            &format!("a=t,{controls},i=7{}", if compressed { ",o=z" } else { "" }),
            &payload,
        );
        optimized.extend_from_slice(placement);
        let raw_ms = replay(&host, raw.clone(), &pixels);
        let optimized_ms = replay(&host, optimized.clone(), &pixels);
        fs::write(artifacts.join(format!("{name}-live.kitty")), &wire).unwrap();
        fs::write(artifacts.join(format!("{name}-raw.kitty")), &raw).unwrap();
        fs::write(
            artifacts.join(format!("{name}-optimized.kitty")),
            &optimized,
        )
        .unwrap();
        metrics.push(serde_json::json!({
            "fixture": name, "format": format, "width": width, "height": height,
            "raw_bytes": pixels.len(), "transmitted_bytes": payload.len(),
            "raw_base64_bytes": pixels.len().div_ceil(3) * 4, "base64_bytes": encoded_bytes,
            "raw_wire_bytes": raw.len(), "optimized_wire_bytes": optimized.len(),
            "end_to_end_ms": end_to_end_ms, "raw_replay_ms": raw_ms, "optimized_replay_ms": optimized_ms,
            "pixel_sha256": format!("{:x}", Sha256::digest(&pixels)), "compressed": compressed,
        }));
    }
    let report = serde_json::json!({
        "ssh_host": host, "ssh_compression": false,
        "route": "local Herdr client PTY -> SSH host cat -> local libghostty",
        "timing_note": "replay timings exclude SSH startup; not RootShell display latency",
        "fixtures": metrics,
    });
    fs::write(
        artifacts.join("metrics.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "{}\nArtifacts: {}",
        serde_json::to_string_pretty(&report).unwrap(),
        artifacts.display()
    );
    drop(client);
    drop(relay);
    drop(server);
}
