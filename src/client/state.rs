use super::*;
use std::collections::VecDeque;

pub(super) enum OutputCompletion {
    Frame(FrameData, render_ansi::EncodedBlit),
    Patch(
        Vec<crate::protocol::PaneSurfacePatchRow>,
        Option<crate::protocol::CursorState>,
        render_ansi::EncodedBlit,
    ),
    Graphics(Box<crate::kitty_graphics::surface::GraphicsUnit>),
    Control,
    #[cfg(unix)]
    DirectFile(
        endpoint::ClientEndpointId,
        u64,
        u32,
        Option<crate::protocol::SurfaceGraphicsAssetKey>,
    ),
}

#[cfg(unix)]
pub(super) struct DirectFileWritten {
    pub(super) owner: endpoint::ClientEndpointId,
    pub(super) transfer_id: u64,
    pub(super) image_id: u32,
}

/// State tracking for the thin client.
pub(super) struct ClientState {
    pub(super) output: Option<output::TerminalOutput>,
    pub(super) output_error: Option<io::Error>,
    pub(super) pending_frame: Option<FrameData>,
    pub(super) pending_controls: VecDeque<(Vec<u8>, OutputCompletion)>,
    pub(super) queued_control_bytes: usize,
    pub(super) output_completion: Option<OutputCompletion>,
    #[cfg(unix)]
    pub(super) direct_file_written: Option<DirectFileWritten>,
    /// Stateful semantic-frame encoder used when the server sends FrameData.
    pub(super) blit_encoder: render_ansi::BlitEncoder,
    pub(super) mouse_capture_active: bool,
    pub(super) endpoint_mouse_capture_requested: bool,
    pub(super) endpoint_sgr_pixels_requested: bool,
    /// Latest physical host theme observations, retained so an endpoint selected after the
    /// observation receives the same client-owned baseline.
    pub(super) host_theme_updates: Vec<crate::protocol::ClientHostThemeUpdate>,
    pub(super) direct_mouse_capture_preference: bool,
    pub(super) shell_mouse_capture_preference: bool,
    pub(super) direct_keyboard_protocol: crate::terminal_modes::DirectHostKeyboardState,
    pub(super) pane_keyboard_report_all: bool,
    pub(super) keyboard_report_all_active: bool,
    pub(super) reported_size: (u16, u16),
    pub(super) reported_cell_size: (u32, u32),
    pub(super) sound_config: crate::config::SoundConfig,
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_enabled: bool,
    pub(super) pixel_geometry_exact: bool,
    #[cfg(unix)]
    pub(super) direct_graphics_response: Arc<Mutex<direct_graphics::ResponseMatcher>>,
    #[cfg(unix)]
    pub(super) retired_direct_graphics: Option<(endpoint::ClientEndpointId, u64, u32)>,
    #[cfg(unix)]
    pub(super) pending_surface_graphics:
        HashMap<(endpoint::ClientEndpointId, u64, u32), crate::protocol::SurfaceGraphicsAssetKey>,
    pub(super) attach_escape: Option<AttachEscapeState>,
    #[cfg(unix)]
    pub(super) mouse_scroll_lines: usize,
    pub(super) remote_image_paste_key:
        Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
    pub(super) redraw_on_focus_gained: bool,
    pub(super) repaint_pending: bool,
    /// During a source-off-first handoff the currently blitted frame remains authoritative until
    /// an acknowledged target snapshot/surface pair commits.
    pub(super) presentation_frozen: bool,
    /// Latest explicit Local selection awaiting this client's replacement Local connection.
    pub(super) deferred_local_activation: Option<endpoint::EndpointActivationIntent>,
    pub(super) draw_host_cursor: bool,
    pub(super) detached_process_children: Vec<std::process::Child>,
    pub(super) shell: Option<shell::ClientShellState>,
}

impl Drop for ClientState {
    fn drop(&mut self) {
        if let Some(mut output) = self.output.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            let written = output
                .finish_pending(deadline.saturating_duration_since(std::time::Instant::now()));
            if written.as_ref().is_some_and(Result::is_ok) {
                if let Some(OutputCompletion::Graphics(unit)) = self.output_completion.take() {
                    if let Some(shell) = self.shell.as_mut() {
                        shell.acknowledge_graphics(*unit);
                    }
                }
                if let Some(shell) = self
                    .shell
                    .as_mut()
                    .filter(|shell| shell.graphics_upload_in_progress())
                {
                    shell.interrupt_graphics_upload();
                    if let Some(unit) = shell.next_graphics_unit() {
                        let mut bytes = Vec::new();
                        bytes.extend_from_slice(b"\x1b7");
                        bytes.extend_from_slice(&unit.bytes);
                        bytes.extend_from_slice(b"\x1b8");
                        if output.send(bytes).is_ok() {
                            let _ = output.finish_pending(
                                deadline.saturating_duration_since(std::time::Instant::now()),
                            );
                        }
                    }
                }
            }
            output.stop();
        }
        if self.attach_escape.is_some() {
            let _ = crate::terminal_modes::set_direct_host_keyboard_protocol(
                &mut io::stdout(),
                &mut self.direct_keyboard_protocol,
                0,
                0,
            );
        }
    }
}

impl ClientState {
    #[cfg(test)]
    pub(super) fn test_new() -> Self {
        Self {
            output: None,
            output_error: None,
            pending_frame: None,
            pending_controls: VecDeque::new(),
            queued_control_bytes: 0,
            output_completion: None,
            #[cfg(unix)]
            direct_file_written: None,
            blit_encoder: render_ansi::BlitEncoder::new(),
            mouse_capture_active: false,
            endpoint_mouse_capture_requested: false,
            endpoint_sgr_pixels_requested: false,
            host_theme_updates: Vec::new(),
            direct_mouse_capture_preference: false,
            shell_mouse_capture_preference: false,
            direct_keyboard_protocol: Default::default(),
            pane_keyboard_report_all: false,
            keyboard_report_all_active: false,
            reported_size: (100, 30),
            reported_cell_size: (0, 0),
            sound_config: Default::default(),
            kitty_graphics_enabled: false,
            pixel_geometry_enabled: false,
            pixel_geometry_exact: false,
            #[cfg(unix)]
            direct_graphics_response: Default::default(),
            #[cfg(unix)]
            retired_direct_graphics: None,
            #[cfg(unix)]
            pending_surface_graphics: HashMap::new(),
            attach_escape: None,
            #[cfg(unix)]
            mouse_scroll_lines: 3,
            remote_image_paste_key: None,
            redraw_on_focus_gained: false,
            repaint_pending: false,
            presentation_frozen: false,
            deferred_local_activation: None,
            draw_host_cursor: false,
            detached_process_children: Vec::new(),
            shell: Some(shell::ClientShellState::new(
                shell::ClientShellConfig::from_config(&crate::config::Config::default()),
            )),
        }
    }

    pub(super) fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    pub(super) fn freeze_presentation(&mut self) {
        self.presentation_frozen = true;
        self.pending_frame = None;
    }

    pub(super) fn record_host_theme_update(
        &mut self,
        update: &crate::protocol::ClientHostThemeUpdate,
    ) {
        use crate::protocol::ClientHostThemeUpdate;

        match update {
            ClientHostThemeUpdate::DefaultColor { kind, .. } => {
                self.host_theme_updates.retain(|current| {
                    !matches!(
                        current,
                        ClientHostThemeUpdate::DefaultColor {
                            kind: current_kind,
                            ..
                        } if current_kind == kind
                    )
                });
            }
            ClientHostThemeUpdate::PaletteColors(_) => self
                .host_theme_updates
                .retain(|current| !matches!(current, ClientHostThemeUpdate::PaletteColors(_))),
            ClientHostThemeUpdate::Appearance(_) => self
                .host_theme_updates
                .retain(|current| !matches!(current, ClientHostThemeUpdate::Appearance(_))),
        }
        self.host_theme_updates.push(update.clone());
    }

    /// Replay the retained physical-host baseline only after an endpoint owns the committed
    /// presentation. The endpoint transport preserves this order ahead of the resync control.
    pub(super) fn replay_host_theme(
        &self,
        endpoints: &mut endpoint::EndpointRegistry,
        endpoint_id: &endpoint::ClientEndpointId,
    ) {
        for update in &self.host_theme_updates {
            let _ = endpoints.send_to(
                endpoint_id,
                &crate::protocol::ClientMessage::ClientShellHostTheme {
                    update: update.clone(),
                },
            );
        }
    }

    pub(super) fn unfreeze_presentation(&mut self) {
        self.presentation_frozen = false;
        // A resize or metadata event may have happened while frozen. Force a full frame rather
        // than attempting to patch the old source frame.
        self.request_repaint();
    }

    /// Present a composed error/chrome frame while retaining the handoff input freeze. The pane
    /// cells are still the last coherent surface; only client chrome (including the error) moves.
    pub(super) fn present_frozen_chrome(&mut self, frame_data: FrameData) {
        let frozen = self.presentation_frozen;
        self.presentation_frozen = false;
        self.present_frame(frame_data);
        self.presentation_frozen = frozen;
    }

    pub(super) fn present_graphics(&mut self, graphics: &[u8]) {
        if self.presentation_frozen || !self.kitty_graphics_enabled {
            return;
        }
        if let Some(output) = self.output.as_ref() {
            if !graphics.is_empty() {
                if let Err(error) = self.queue_terminal_bytes(graphics.to_vec()) {
                    self.output_error = Some(error);
                }
            } else if !output.is_busy() {
                if let Err(error) = self.pump_output() {
                    self.output_error = Some(error);
                }
            }
            return;
        }
        if graphics.is_empty() {
            return;
        }
        let mut stdout = io::stdout();
        let _ = write_encoded_frame_with_graphics(&mut stdout, &[], graphics);
        let _ = stdout.flush();
    }

    pub(super) fn present_surface_patch(
        &mut self,
        patch: shell::ClientComposedSurfacePatch,
    ) -> io::Result<bool> {
        if self.presentation_frozen || self.repaint_pending {
            crate::render_prof::event("client_surface_patch.fallback.repaint");
            return Ok(false);
        }
        if self
            .output
            .as_ref()
            .is_some_and(output::TerminalOutput::is_busy)
            || self.pending_frame.is_some()
        {
            return Ok(false);
        }
        let rows = if self.draw_host_cursor {
            let Some(rows) = self
                .blit_encoder
                .patch_rows_with_drawn_cursor(&patch.rows, patch.cursor.as_ref())
            else {
                crate::render_prof::event("client_surface_patch.fallback.drawn_cursor");
                return Ok(false);
            };
            rows
        } else {
            patch.rows
        };
        let encode_started = crate::render_prof::timer();
        let Some(encoded) =
            self.blit_encoder
                .encode_patch(&rows, patch.cursor.clone(), self.draw_host_cursor)
        else {
            crate::render_prof::event("client_surface_patch.fallback.encode");
            return Ok(false);
        };
        crate::render_prof::duration_since("client_surface_patch.encode", encode_started);
        if let Some(output) = self.output.as_mut() {
            let bytes = encoded.bytes.clone();
            output.send(bytes)?;
            self.output_completion = Some(OutputCompletion::Patch(rows, patch.cursor, encoded));
            return Ok(true);
        }
        let write_started = crate::render_prof::timer();
        let mut stdout = io::stdout();
        stdout.write_all(&encoded.bytes)?;
        stdout.flush()?;
        crate::render_prof::duration_since("client_surface_patch.write", write_started);
        let committed = self.blit_encoder.commit_patch(&rows, patch.cursor, encoded);
        crate::render_prof::event(if committed {
            "client_surface_patch.success"
        } else {
            "client_surface_patch.fallback.commit"
        });
        Ok(committed)
    }

    #[cfg(unix)]
    pub(super) fn retire_endpoint_graphics(&mut self, endpoint_id: &endpoint::ClientEndpointId) {
        let transfer_ids = self
            .pending_surface_graphics
            .keys()
            .filter(|(owner, _, _)| owner == endpoint_id)
            .map(|(_, transfer_id, _)| *transfer_id)
            .collect::<Vec<_>>();
        self.pending_surface_graphics
            .retain(|(owner, _, _), _| owner != endpoint_id);
        if self
            .retired_direct_graphics
            .as_ref()
            .is_some_and(|(owner, _, _)| owner == endpoint_id)
        {
            self.retired_direct_graphics = None;
        }
        if let Ok(mut matcher) = self.direct_graphics_response.lock() {
            for transfer_id in transfer_ids {
                matcher.retire(transfer_id);
            }
        }
    }

    pub(super) fn present_frame(&mut self, frame_data: FrameData) {
        if self.presentation_frozen {
            return;
        }
        let frame_data = if self.draw_host_cursor {
            render_ansi::frame_with_drawn_cursor(frame_data)
        } else {
            frame_data
        };
        if self.output.is_some() {
            self.pending_frame = Some(frame_data);
            if let Err(error) = self.pump_output() {
                self.output_error = Some(error);
            }
            return;
        }
        let encoded = if self.draw_host_cursor {
            self.blit_encoder
                .encode_with_suppressed_visible_cursor(&frame_data, self.repaint_pending)
        } else {
            self.blit_encoder.encode(&frame_data, self.repaint_pending)
        };
        let mut stdout = io::stdout();
        let graphics = if self.kitty_graphics_enabled {
            frame_data.graphics.as_slice()
        } else {
            &[]
        };
        let _ = write_encoded_frame_with_graphics(&mut stdout, &encoded.bytes, graphics);
        let _ = stdout.flush();
        self.blit_encoder.commit(frame_data, encoded);
        self.repaint_pending = false;
    }

    pub(super) fn queue_terminal_bytes(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        self.queue_control(bytes, OutputCompletion::Control)
    }

    pub(super) fn queue_host_effect(
        &mut self,
        effect: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut bytes = Vec::new();
        effect(&mut bytes)?;
        self.queue_terminal_bytes(bytes)
    }

    fn queue_control(&mut self, bytes: Vec<u8>, completion: OutputCompletion) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        const MAX_QUEUED_CONTROL_BYTES: usize = crate::protocol::MAX_GRAPHICS_FRAME_SIZE;
        if self.queued_control_bytes.saturating_add(bytes.len()) > MAX_QUEUED_CONTROL_BYTES {
            return Err(io::Error::other("terminal control output queue is full"));
        }
        self.queued_control_bytes += bytes.len();
        self.pending_controls.push_back((bytes, completion));
        self.pump_output()
    }

    #[cfg(unix)]
    pub(super) fn queue_direct_file(
        &mut self,
        bytes: Vec<u8>,
        owner: endpoint::ClientEndpointId,
        transfer_id: u64,
        image_id: u32,
        asset: Option<crate::protocol::SurfaceGraphicsAssetKey>,
    ) -> io::Result<()> {
        record_received_kitty_graphics(&bytes);
        self.queue_control(
            bytes,
            OutputCompletion::DirectFile(owner, transfer_id, image_id, asset),
        )
    }

    /// Advances a single accepted output unit. An acknowledged frame is the only
    /// frame against which another delta may be encoded.
    pub(super) fn pump_output(&mut self) -> io::Result<()> {
        let Some(output) = self.output.as_mut() else {
            return Ok(());
        };
        if let Some(result) = output.acknowledge() {
            result?;
            match self.output_completion.take() {
                Some(OutputCompletion::Frame(frame, encoded)) => {
                    self.blit_encoder.commit(frame, encoded);
                }
                Some(OutputCompletion::Patch(rows, cursor, encoded)) => {
                    if !self.blit_encoder.commit_patch(&rows, cursor, encoded) {
                        self.repaint_pending = true;
                    }
                }
                Some(OutputCompletion::Graphics(unit)) => {
                    if let Some(shell) = self.shell.as_mut() {
                        shell.acknowledge_graphics(*unit);
                    }
                }
                #[cfg(unix)]
                Some(OutputCompletion::DirectFile(owner, transfer_id, image_id, asset)) => {
                    if let Some(asset) = asset {
                        self.pending_surface_graphics
                            .insert((owner.clone(), transfer_id, image_id), asset);
                    }
                    if let Ok(mut matcher) = self.direct_graphics_response.lock() {
                        matcher.start(transfer_id);
                    }
                    self.direct_file_written = Some(DirectFileWritten {
                        owner,
                        transfer_id,
                        image_id,
                    });
                }
                Some(OutputCompletion::Control) | None => {}
            }
        }
        if output.is_busy() {
            return Ok(());
        }
        if let Some(frame) = self.pending_frame.take() {
            let encoded = if self.draw_host_cursor {
                self.blit_encoder
                    .encode_with_suppressed_visible_cursor(&frame, self.repaint_pending)
            } else {
                self.blit_encoder.encode(&frame, self.repaint_pending)
            };
            let mut bytes = Vec::new();
            let graphics = if self.kitty_graphics_enabled {
                frame.graphics.as_slice()
            } else {
                &[]
            };
            write_encoded_frame_with_graphics(&mut bytes, &encoded.bytes, graphics)?;
            output.send(bytes)?;
            self.output_completion = Some(OutputCompletion::Frame(frame, encoded));
            self.repaint_pending = false;
            return Ok(());
        }
        let graphics_control_waiting =
            self.pending_controls
                .front()
                .is_some_and(|(bytes, completion)| {
                    #[cfg(unix)]
                    if matches!(completion, OutputCompletion::DirectFile(..)) {
                        return true;
                    }
                    let _ = completion;
                    contains_kitty_graphics_bytes(bytes)
                });
        if graphics_control_waiting
            && self
                .shell
                .as_ref()
                .is_some_and(shell::ClientShellState::graphics_upload_in_progress)
        {
            let shell = self.shell.as_mut().expect("checked shell");
            shell.interrupt_graphics_upload();
            if let Some(unit) = shell.next_graphics_unit() {
                let mut bytes = Vec::new();
                write_encoded_frame_with_graphics(&mut bytes, &[], &unit.bytes)?;
                output.send(bytes)?;
                self.output_completion = Some(OutputCompletion::Graphics(Box::new(unit)));
                return Ok(());
            }
        }
        if let Some((bytes, completion)) = self.pending_controls.pop_front() {
            self.queued_control_bytes -= bytes.len();
            output.send(bytes)?;
            self.output_completion = Some(completion);
            return Ok(());
        }
        if !self.presentation_frozen && self.kitty_graphics_enabled {
            if let Some(unit) = self
                .shell
                .as_ref()
                .and_then(shell::ClientShellState::next_graphics_unit)
            {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"\x1b7");
                bytes.extend_from_slice(&unit.bytes);
                bytes.extend_from_slice(b"\x1b8");
                record_received_kitty_graphics(&unit.bytes);
                output.send(bytes)?;
                self.output_completion = Some(OutputCompletion::Graphics(Box::new(unit)));
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(super) fn take_direct_file_written(&mut self) -> Option<DirectFileWritten> {
        self.direct_file_written.take()
    }
}
