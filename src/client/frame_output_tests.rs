use super::frame_output::*;
use super::image_files::FileTransport;
use crate::kitty_graphics::{GraphicsOperation, GraphicsOutput};
use std::sync::Arc;

fn output(format: u32) -> GraphicsOutput {
    GraphicsOutput {
        operations: vec![
            GraphicsOperation::Bytes(b"before".to_vec()),
            GraphicsOperation::Upload {
                control: format!("a=t,t=d,f={format},s=1,v=1,i=42,q=2"),
                data: Arc::from([1, 2, 3, 4]),
            },
            GraphicsOperation::Bytes(b"after".to_vec()),
        ],
    }
}

#[test]
fn deferred_inline_output_matches_existing_frame_envelope() {
    let graphics = output(32);
    let encoded = b"\x1b[?2026htext\x1b[?2026l";
    let mut expected = Vec::new();
    write_encoded_frame_with_graphics(
        &mut expected,
        encoded,
        &graphics.clone().into_inline_bytes(),
    )
    .unwrap();
    let mut actual = Vec::new();
    write_composed_frame(
        &mut actual,
        encoded,
        &graphics,
        &mut FileTransport::default(),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn native_swap_cleanup_and_placements_share_the_frame_sync_envelope() {
    let graphics = GraphicsOutput::from_bytes(
        concat!(
            "\x1b_Ga=d,d=I,i=41,q=2;\x1b\\",
            "\x1b_Ga=p,i=42,p=1,q=2;\x1b\\",
            "\x1b_Ga=p,i=42,p=2,q=2;\x1b\\",
        )
        .as_bytes()
        .to_vec(),
    );
    let mut actual = Vec::new();
    write_composed_frame(
        &mut actual,
        b"\x1b[?2026hframe\x1b[?2026l",
        &graphics,
        &mut FileTransport::default(),
    )
    .unwrap();
    let text = std::str::from_utf8(&actual).unwrap();
    let sync_begin = text.find("\x1b[?2026h").unwrap();
    let sync_end = text.rfind("\x1b[?2026l").unwrap();
    let deletion = text.find("a=d,d=I,i=41").unwrap();
    let placements = text
        .match_indices("a=p,i=42")
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    assert!(deletion > sync_begin && deletion < sync_end);
    assert!(placements.len() >= 2);
    assert!(placements
        .iter()
        .all(|placement| *placement > sync_begin && *placement < sync_end));
}

#[test]
fn deferred_output_propagates_write_failure() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        write_composed_frame(Broken, b"text", &output(32), &mut FileTransport::default())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[cfg(unix)]
mod local_files {
    use super::*;
    use base64::Engine as _;
    use std::path::PathBuf;
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = PathBuf::from(format!(
                "/var/tmp/herdr-output-test-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn command_path(bytes: &[u8], header: &str) -> PathBuf {
        let text = std::str::from_utf8(bytes).unwrap();
        let command = text
            .split("\x1b_G")
            .find(|part| part.starts_with(header))
            .unwrap();
        let payload = command
            .split_once(';')
            .unwrap()
            .1
            .split("\x1b\\")
            .next()
            .unwrap();
        PathBuf::from(
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .unwrap(),
            )
            .unwrap(),
        )
    }
    #[test]
    fn files_begin_only_after_probe_consumption_and_keep_order() {
        for format in [24, 32, 100] {
            let root = Scratch::new();
            let mut files = FileTransport::for_test(root.0.clone());
            let graphics = output(format);
            let mut first = Vec::new();
            write_composed_frame(&mut first, b"text", &graphics, &mut files).unwrap();
            let first_text = std::str::from_utf8(&first).unwrap();
            assert!(first_text.contains("a=q,t=t"));
            assert!(first_text.contains("a=t,t=d"));
            let probe = command_path(&first, "a=q,");
            assert_eq!(std::fs::read(&probe).unwrap().len(), 4);
            let mut waiting = Vec::new();
            write_composed_frame(&mut waiting, b"text", &graphics, &mut files).unwrap();
            assert!(!std::str::from_utf8(&waiting).unwrap().contains("a=q,"));
            assert!(std::str::from_utf8(&waiting).unwrap().contains("a=t,t=d"));
            std::fs::remove_file(&probe).unwrap();
            let mut ready = Vec::new();
            write_composed_frame(&mut ready, b"text", &graphics, &mut files).unwrap();
            let text = std::str::from_utf8(&ready).unwrap();
            assert!(text.contains(&format!("before\x1b_Ga=t,t=t,f={format}")));
            assert!(text.contains("\x1b\\after"));
            assert!(!text.contains("AQIDBA=="));
            let image = command_path(&ready, "a=t,");
            assert_eq!(std::fs::read(&image).unwrap(), [1, 2, 3, 4]);
            drop(files);
            assert!(!image.exists());
        }
    }
    #[test]
    fn unsupported_format_and_file_creation_failure_remain_inline() {
        let root = Scratch::new();
        let mut files = FileTransport::for_test(root.0.clone());
        let mut unsupported = Vec::new();
        write_composed_frame(&mut unsupported, b"", &output(999), &mut files).unwrap();
        assert!(std::str::from_utf8(&unsupported)
            .unwrap()
            .contains("a=t,t=d,f=999"));
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
        let mut broken = FileTransport::for_test(root.0.join("absent"));
        let mut actual = Vec::new();
        write_composed_frame(&mut actual, b"", &output(32), &mut broken).unwrap();
        let mut expected = Vec::new();
        write_composed_frame(
            &mut expected,
            b"",
            &output(32),
            &mut FileTransport::default(),
        )
        .unwrap();
        assert_eq!(actual, expected);
    }
}

mod compression {
    use super::*;

    fn inline_upload(control: &str, data: &[u8]) -> Vec<u8> {
        let graphics = GraphicsOutput {
            operations: vec![GraphicsOperation::Upload {
                control: control.to_owned(),
                data: std::sync::Arc::from(data),
            }],
        };
        let mut output = Vec::new();
        write_composed_frame(
            &mut output,
            b"text before image",
            &graphics,
            &mut FileTransport::default(),
        )
        .unwrap();
        assert!(output.starts_with(b"text before image\x1b7"));
        assert!(output.ends_with(b"\x1b8"));
        output
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 42_u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    #[test]
    fn inline_rgb_and_rgba_zlib_round_trip_through_ghostty() {
        for (format, channels) in [(24, 3), (32, 4)] {
            // Repeated noise stays large enough to exercise Kitty continuation records.
            let data = noise(256 * 16 * channels).repeat(8);
            let control = format!("a=T,t=d,f={format},s=256,v=128,i=7,q=2");
            let output = inline_upload(&control, &data);
            assert!(output.windows(4).any(|w| w == b",o=z"));
            assert!(output.windows(3).any(|w| w == b"m=1"));
            assert!(output.len() < data.len());

            let mut terminal = ghostty_vt::Terminal::new(80, 24, 0).unwrap();
            terminal.enable_kitty_graphics().unwrap();
            terminal.resize(80, 24, 8, 16).unwrap();
            terminal.write(&output);
            let placements = terminal.kitty_image_placements().unwrap();
            assert_eq!(placements.len(), 1);
            assert_eq!(placements[0].data, data);

            let artifacts = std::path::Path::new("target/kitty-zlib");
            std::fs::create_dir_all(artifacts).unwrap();
            std::fs::write(artifacts.join(format!("rgb{format}.kitty")), &output).unwrap();
        }
    }

    #[test]
    fn inline_incompressible_tiny_png_and_encoded_payloads_stay_unchanged() {
        for (control, data) in [
            ("a=t,t=d,f=32", noise(32 * 1024)),
            ("a=t,t=d,f=24", vec![42; 3]),
            ("a=t,t=d,f=100", vec![42; 32 * 1024]),
            ("a=t,t=d,f=32,o=z", vec![42; 32 * 1024]),
            ("a=t,t=d,f=320", vec![42; 32 * 1024]),
            ("a=t,t=f,f=32", vec![42; 32 * 1024]),
            ("a=t,t=d,f=32", Vec::new()),
        ] {
            let output = inline_upload(control, &data);
            let mut expected = b"text before image\x1b7".to_vec();
            crate::kitty_graphics::write_kitty_data(&mut expected, control, &data).unwrap();
            expected.extend_from_slice(b"\x1b8");
            assert_eq!(output, expected, "{control}");
        }
    }
}
