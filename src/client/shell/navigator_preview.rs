use super::*;
use crate::api::schema::{
    Method, PaneLayoutSnapshot, PaneReadParams, ReadFormat, ReadSource, ResponseResult,
};
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_PREVIEW_CELLS: u64 = 256_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreviewScope {
    target: ClientNavigatorTarget,
    boot_id: String,
    connection_generation: Option<u64>,
    projection_revision: u64,
    surface_revision: Option<u64>,
    size: (u16, u16),
    workspace_id: String,
    tab_id: String,
    pane_id: String,
}

#[derive(Debug)]
pub(super) struct PreviewRequest {
    pub(super) endpoint_id: ClientEndpointId,
    scope: PreviewScope,
    pane_id: Option<String>,
}

#[derive(Debug)]
struct PreviewCapture {
    layout: PaneLayoutSnapshot,
    panes: Vec<(String, u64, Buffer)>,
}

#[derive(Debug, Default)]
pub(super) struct NavigatorPreview {
    scope: Option<PreviewScope>,
    requested_at: Option<Instant>,
    in_flight: Option<String>,
    capture: Option<PreviewCapture>,
    ready: Option<PreviewCapture>,
    local: Option<Buffer>,
    error: Option<String>,
}

pub(super) fn split_body(body: Rect, screen_width: u16) -> (Rect, Option<Rect>) {
    if screen_width < 90 || body.height < 4 || body.width < 60 {
        return (body, None);
    }
    let list_width = (body.width * 2 / 5).max(28);
    (
        Rect::new(body.x, body.y, list_width, body.height),
        Some(Rect::new(
            body.x + list_width + 1,
            body.y,
            body.width - list_width - 1,
            body.height,
        )),
    )
}

impl ClientShellState {
    fn navigator_preview_scope(&self) -> Option<PreviewScope> {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return None;
        };
        let size = self.last_composed_size?;
        if size.0 < 90 || size.1 < 12 {
            return None;
        }
        let target = navigator.selected.as_ref()?;
        if !self.navigator_target_is_current(target) {
            return None;
        }
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == target.endpoint_id())?;
        let snapshot = endpoint.snapshot.as_ref()?;
        let tab_id = match target {
            ClientNavigatorTarget::Workspace { workspace_id, .. } => {
                &snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| &workspace.workspace_id == workspace_id)?
                    .active_tab_id
            }
            ClientNavigatorTarget::Tab { tab_id, .. } => tab_id,
            ClientNavigatorTarget::Pane { pane_id, .. } => {
                &snapshot
                    .panes
                    .iter()
                    .find(|pane| &pane.pane_id == pane_id)?
                    .tab_id
            }
            ClientNavigatorTarget::Machine { .. } => snapshot.focused_tab_id.as_ref()?,
            ClientNavigatorTarget::Directory { .. } => return None,
        };
        let pane = match target {
            ClientNavigatorTarget::Pane { pane_id, .. } => {
                snapshot.panes.iter().find(|pane| &pane.pane_id == pane_id)
            }
            _ => snapshot
                .panes
                .iter()
                .find(|pane| &pane.tab_id == tab_id && pane.focused)
                .or_else(|| snapshot.panes.iter().find(|pane| &pane.tab_id == tab_id)),
        }?;
        let surface_revision = self
            .pane_surface
            .as_ref()
            .filter(|surface| {
                endpoint.endpoint_id == self.active_endpoint_id
                    && self.pane_surface_generation == endpoint.snapshot_generation
                    && surface.boot_id == snapshot.boot_id
                    && surface.projection_revision == snapshot.revision
                    && snapshot.focused_tab_id.as_ref() == Some(tab_id)
                    && surface
                        .panes
                        .iter()
                        .any(|item| item.pane_id == pane.pane_id)
                    && !snapshot
                        .tabs
                        .iter()
                        .any(|tab| &tab.tab_id == tab_id && tab.zoomed)
            })
            .map(|surface| surface.surface_revision);
        Some(PreviewScope {
            target: target.clone(),
            boot_id: snapshot.boot_id.clone(),
            connection_generation: endpoint.snapshot_generation,
            projection_revision: snapshot.revision,
            surface_revision,
            size,
            workspace_id: pane.workspace_id.clone(),
            tab_id: tab_id.clone(),
            pane_id: pane.pane_id.clone(),
        })
    }

    pub(crate) fn refresh_navigator_preview(
        &mut self,
        now: Instant,
        outcome: &mut ClientShellInput,
    ) {
        let scope = self.navigator_preview_scope();
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let preview = &mut navigator.fork.preview;
        if preview.scope != scope {
            *preview = NavigatorPreview {
                scope: scope.clone(),
                ..Default::default()
            };
            self.pending_requests.retain(|_, pending| {
                !matches!(pending.kind, PendingEndpointKind::NavigatorPreview(_))
            });
            outcome.repaint = true;
        }
        let Some(scope) = scope else {
            return;
        };
        if scope.surface_revision.is_some() {
            if preview.local.is_none() {
                preview.local = self
                    .pane_surface
                    .as_ref()
                    .and_then(|surface| surface.frame.to_ratatui_buffer());
                outcome.repaint = true;
            }
            return;
        }
        if preview.in_flight.is_some()
            || preview
                .requested_at
                .is_some_and(|time| now.saturating_duration_since(time) < REFRESH_INTERVAL)
        {
            return;
        }
        preview.requested_at = Some(now);
        self.request_navigator_preview(
            scope.clone(),
            None,
            Method::PaneLayout(crate::api::schema::PaneLayoutParams {
                pane_id: Some(scope.pane_id.clone()),
            }),
            outcome,
        );
    }

    fn request_navigator_preview(
        &mut self,
        scope: PreviewScope,
        pane_id: Option<String>,
        method: Method,
        outcome: &mut ClientShellInput,
    ) {
        let endpoint_id = scope.target.endpoint_id().clone();
        let method_name = crate::api::api_method_name(&method).to_owned();
        let unsupported = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.methods.as_ref())
            .is_some_and(|methods| !methods.contains(&method_name));
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        if unsupported {
            navigator.fork.preview.error = Some(format!("Endpoint does not support {method_name}"));
            navigator.fork.preview.capture = None;
            outcome.repaint = true;
            return;
        }
        let request_id = format!("client-shell:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        navigator.fork.preview.in_flight = Some(request_id.clone());
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: scope.boot_id.clone(),
                method_name,
                confirmation_workspace_id: None,
                kind: PendingEndpointKind::NavigatorPreview(PreviewRequest {
                    endpoint_id: endpoint_id.clone(),
                    scope: scope.clone(),
                    pane_id,
                }),
            },
        );
        outcome.actions.push(ClientShellAction::Endpoint {
            endpoint_id,
            boot_id: scope.boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
    }

    pub(super) fn complete_navigator_preview(
        &mut self,
        request_id: &str,
        request: PreviewRequest,
        boot_id: &str,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let current = self.navigator_preview_scope();
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return (false, Vec::new());
        };
        let preview = &mut navigator.fork.preview;
        if preview.in_flight.as_deref() != Some(request_id) {
            return (false, Vec::new());
        }
        preview.in_flight = None;
        if current.as_ref() != Some(&request.scope) || request.scope.boot_id != boot_id {
            return (false, Vec::new());
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                preview.capture = None;
                preview.error = Some(error.message);
                return (true, Vec::new());
            }
        };
        let valid = match (request.pane_id.as_deref(), result) {
            (None, ResponseResult::PaneLayout { layout }) => {
                let snapshot = self
                    .endpoints
                    .iter()
                    .find(|endpoint| endpoint.endpoint_id == request.endpoint_id)
                    .and_then(|endpoint| endpoint.snapshot.as_ref());
                let cells: u64 = layout
                    .panes
                    .iter()
                    .filter_map(|pane| pane.terminal_size)
                    .map(|size| u64::from(size.cols) * u64::from(size.rows))
                    .sum();
                let valid = layout.workspace_id == request.scope.workspace_id
                    && layout.tab_id == request.scope.tab_id
                    && layout
                        .panes
                        .iter()
                        .any(|pane| pane.pane_id == request.scope.pane_id)
                    && layout.area.width > 0
                    && layout.area.height > 0
                    && cells <= MAX_PREVIEW_CELLS
                    && layout.panes.len() <= 256
                    && layout.panes.iter().all(|pane| {
                        pane.terminal_size
                            .is_some_and(|size| size.cols > 0 && size.rows > 0)
                            && snapshot.is_some_and(|snapshot| {
                                snapshot.panes.iter().any(|known| {
                                    known.pane_id == pane.pane_id
                                        && known.tab_id == layout.tab_id
                                        && known.workspace_id == layout.workspace_id
                                })
                            })
                    });
                if valid {
                    preview.capture = Some(PreviewCapture {
                        layout,
                        panes: Vec::new(),
                    });
                }
                valid
            }
            (Some(pane_id), ResponseResult::PaneRead { read }) => {
                let capture = preview.capture.as_mut();
                if read.pane_id != pane_id
                    || read.workspace_id != request.scope.workspace_id
                    || read.tab_id != request.scope.tab_id
                    || read.source != ReadSource::Visible
                    || read.format != ReadFormat::Ansi
                    || !read.revision.is_multiple_of(2)
                {
                    false
                } else if let Some(capture) = capture {
                    if let Some(pane) = capture
                        .layout
                        .panes
                        .get(capture.panes.len())
                        .filter(|pane| pane.pane_id == pane_id)
                    {
                        let Some(size) = pane.terminal_size else {
                            return (false, Vec::new());
                        };
                        let older = preview
                            .ready
                            .as_ref()
                            .and_then(|ready| ready.panes.iter().find(|(id, _, _)| id == pane_id))
                            .is_some_and(|(_, revision, _)| *revision > read.revision);
                        match crate::pane::render_ansi_snapshot(&read.text, size.cols, size.rows) {
                            Ok(buffer) if !older => {
                                capture
                                    .panes
                                    .push((pane_id.to_owned(), read.revision, buffer));
                                true
                            }
                            _ => false,
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        if !valid {
            preview.capture = None;
            preview.error = Some("Preview changed or returned an invalid result".into());
            return (true, Vec::new());
        }
        let next = preview
            .capture
            .as_ref()
            .and_then(|capture| capture.layout.panes.get(capture.panes.len()))
            .cloned();
        let mut outcome = ClientShellInput::default();
        if let Some(pane) = next {
            let pane_id = pane.pane_id;
            self.request_navigator_preview(
                request.scope,
                Some(pane_id.clone()),
                Method::PaneRead(PaneReadParams {
                    pane_id,
                    source: ReadSource::Visible,
                    format: ReadFormat::Ansi,
                    lines: None,
                    strip_ansi: false,
                    expected_size: pane.terminal_size,
                    intent: crate::api::schema::ReadIntent::Passive,
                }),
                &mut outcome,
            );
        } else {
            preview.ready = preview.capture.take();
            preview.error = None;
        }
        (true, outcome.actions)
    }

    pub(super) fn render_navigator_preview(&self, buffer: &mut Buffer, popup: Rect) {
        let body = Rect::new(
            popup.x + 1,
            popup.y + 3,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(6),
        );
        let (_, Some(area)) = split_body(body, buffer.area.width) else {
            return;
        };
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return;
        };
        let style = Style::default()
            .fg(self.config.palette.subtext0)
            .bg(self.config.palette.panel_bg);
        for y in area.y..area.bottom() {
            buffer[(area.x - 1, y)].set_symbol("│").set_style(style);
        }
        if matches!(
            navigator.selected,
            Some(ClientNavigatorTarget::Directory { .. })
        ) {
            if let Some(directory) = navigator.fork.directory.preview.as_ref() {
                super::render::put_text(
                    buffer,
                    area.x,
                    area.y,
                    area.width,
                    &directory.canonical_path,
                    style,
                );
                for (index, entry) in directory
                    .entries
                    .iter()
                    .take(usize::from(area.height.saturating_sub(1)))
                    .enumerate()
                {
                    let name = format!("{}{}", entry.name, if entry.is_dir { "/" } else { "" });
                    super::render::put_text(
                        buffer,
                        area.x,
                        area.y + 1 + index as u16,
                        area.width,
                        &name,
                        style,
                    );
                }
            }
            return;
        }
        let preview = &navigator.fork.preview;
        if preview.scope.is_none() || preview.scope != self.navigator_preview_scope() {
            return;
        }
        if let Some(error) = &preview.error {
            super::render::put_text(buffer, area.x, area.y, area.width, error, style);
        } else if let Some(local) = &preview.local {
            copy_cells(local, buffer, area);
        } else if let Some(ready) = &preview.ready {
            let layout = &ready.layout;
            for (pane_id, _, frame) in &ready.panes {
                let Some(pane) = layout.panes.iter().find(|pane| &pane.pane_id == pane_id) else {
                    continue;
                };
                let scale = |value: u16, total: u16, size: u16| {
                    (u32::from(value) * u32::from(size) / u32::from(total)) as u16
                };
                let x = scale(
                    pane.rect.x.saturating_sub(layout.area.x),
                    layout.area.width,
                    area.width,
                );
                let y = scale(
                    pane.rect.y.saturating_sub(layout.area.y),
                    layout.area.height,
                    area.height,
                );
                let right = scale(
                    pane.rect
                        .x
                        .saturating_add(pane.rect.width)
                        .saturating_sub(layout.area.x),
                    layout.area.width,
                    area.width,
                )
                .min(area.width);
                let bottom = scale(
                    pane.rect
                        .y
                        .saturating_add(pane.rect.height)
                        .saturating_sub(layout.area.y),
                    layout.area.height,
                    area.height,
                )
                .min(area.height);
                let pane_area = Rect::new(
                    area.x.saturating_add(x),
                    area.y.saturating_add(y),
                    right.saturating_sub(x),
                    bottom.saturating_sub(y),
                );
                if pane_area.height == 0 || pane_area.width == 0 {
                    continue;
                }
                super::render::put_text(
                    buffer,
                    pane_area.x,
                    pane_area.y,
                    pane_area.width,
                    pane_id,
                    style,
                );
                copy_cells(
                    frame,
                    buffer,
                    Rect::new(
                        pane_area.x,
                        pane_area.y + 1,
                        pane_area.width,
                        pane_area.height - 1,
                    ),
                );
            }
        } else {
            super::render::put_text(
                buffer,
                area.x,
                area.y,
                area.width,
                "Loading preview…",
                style,
            );
        }
    }
}

fn copy_cells(source: &Buffer, destination: &mut Buffer, area: Rect) {
    let area = area.intersection(destination.area);
    let width = source.area.width.min(area.width);
    for y in 0..source.area.height.min(area.height) {
        for x in 0..width {
            let cell = &source[(source.area.x + x, source.area.y + y)];
            if cell.symbol().width() <= usize::from(width - x) {
                destination[(area.x + x, area.y + y)] = cell.clone();
            }
        }
    }
}
