use super::*;

pub(super) struct ClientPrefixChord {
    keys: Vec<crate::input::TerminalKey>,
    fallback: Option<crate::input::KeybindMatch>,
    return_mode: ClientShellMode,
    pub(super) deadline: std::time::Instant,
}

impl ClientShellState {
    pub(super) fn route_prefix_chord(
        &mut self,
        key: &crate::input::TerminalKey,
        return_mode: ClientShellMode,
        outcome: &mut ClientShellInput,
    ) {
        let pending = self.pending_chord.take();
        let (mut keys, fallback) = pending
            .map(|chord| (chord.keys, chord.fallback))
            .unwrap_or_default();
        keys.push(key.clone());
        let (binding, longer) =
            crate::input::resolve_prefix_chord(&self.config.keybinds.keybinds, &keys);
        let timeout = self.config.keybinds.keybinds.chord_timeout;
        if longer && !timeout.is_zero() {
            self.pending_chord = Some(ClientPrefixChord {
                keys,
                fallback: binding.or(fallback),
                return_mode,
                deadline: std::time::Instant::now() + timeout,
            });
        } else {
            self.mode = return_mode;
            if let Some(binding) = binding {
                self.record_binding(binding, outcome);
            }
        }
        outcome.repaint = true;
    }

    pub(crate) fn expire_prefix_chord(
        &mut self,
        now: std::time::Instant,
        outcome: &mut ClientShellInput,
    ) {
        if self
            .pending_chord
            .as_ref()
            .is_none_or(|chord| now < chord.deadline)
        {
            return;
        }
        let Some(chord) = self.pending_chord.take() else {
            return;
        };
        if self.mode != ClientShellMode::Prefix || self.overlay.is_some() {
            return;
        }
        self.mode = chord.return_mode;
        if let Some(binding) = chord.fallback {
            self.record_binding(binding, outcome);
        }
        outcome.repaint = true;
    }
}
