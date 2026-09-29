use super::*;

impl ClientShellState {
    #[cfg(unix)]
    pub(crate) fn graphics_scope(&self) -> &str {
        self.graphics.scope()
    }

    #[cfg(unix)]
    pub(crate) fn trust_direct_graphics_asset(
        &mut self,
        key: &crate::protocol::SurfaceGraphicsAssetKey,
        image_id: u32,
    ) -> bool {
        self.graphics.trust_direct_asset(key, image_id)
    }

    #[cfg(unix)]
    pub(crate) fn retire_direct_graphics_image(&mut self, image_id: u32) {
        self.graphics.retire_direct_image(image_id);
    }

    pub(crate) fn take_pending_graphics_cleanup(&mut self) -> Vec<u8> {
        // The output owner drains cleanup in bounded acknowledged transactions.
        Vec::new()
    }

    pub(crate) fn set_graphics_cell_size(&mut self, width_px: u32, height_px: u32) {
        self.graphics_cell_size = crate::kitty_graphics::HostCellSize {
            width_px: width_px.max(1),
            height_px: height_px.max(1),
        };
    }

    pub(super) fn compose_graphics(
        &mut self,
        _frame: &mut FrameData,
        layout: ClientShellLayout,
        occlusion: &crate::kitty_graphics::surface::Occlusion,
    ) {
        let visibility = if self.endpoint_error.is_some() {
            crate::kitty_graphics::surface::Visibility::Hidden
        } else if self.hits.popup.is_some() {
            crate::kitty_graphics::surface::Visibility::Popup
        } else {
            crate::kitty_graphics::surface::Visibility::Main
        };
        let popup_origin = self
            .hits
            .popup
            .as_ref()
            .map(|popup| (popup.inner_rect.x, popup.inner_rect.y));
        self.graphics.prepare(
            visibility,
            (layout.pane_surface.x, layout.pane_surface.y),
            popup_origin,
            self.graphics_cell_size,
            occlusion,
        );
    }

    pub(crate) fn next_graphics_unit(
        &mut self,
    ) -> Option<crate::kitty_graphics::surface::GraphicsUnit> {
        self.graphics.next_unit()
    }

    pub(crate) fn graphics_upload_in_progress(&self) -> bool {
        self.graphics.upload_in_progress()
    }

    pub(crate) fn interrupt_graphics_upload(&mut self) {
        self.graphics.interrupt_upload();
    }

    pub(crate) fn acknowledge_graphics(
        &mut self,
        unit: crate::kitty_graphics::surface::GraphicsUnit,
    ) {
        self.graphics.acknowledge(unit);
    }
}
