---
date: 2026-06-05T22:39:45+0800
author: Z233
commit: 088de54
branch: feature/workspace-picker-preview
repository: workspace-picker-preview
topic: "Configurable quick-switch modifier and cycle keys"
tags: [intent, frd, keybinds, quick-switch, workspace-picker]
status: complete
last_updated: 2026-06-05T22:39:45+0800
last_updated_by: Z233
---

# FRD: Configurable quick-switch modifier and cycle keys

## Summary
Make the quick-switch workspace picker's cycle keys and follow-up command modifiers derive from the configured `quick_switch_workspace` keybinding instead of being hardcoded to CONTROL+Tab. Add an optional `quick_switch_workspace_backward` config key for explicit backward cycle customization. This enables macOS users to use `cmd+tab` / `cmd+shift+tab` matching the system app switcher behavior, and supports any parseable key (including function keys like F13) as the cycle key.

## Problem & Intent
As a macOS user, I want `cmd+tab` to trigger the quick-switch workspace picker and `cmd+shift+tab` to cycle backward, matching the system app switcher muscle memory. I also want to use non-standard keys like F13 as the cycle key (e.g., `cmd+f13` / `cmd+shift+f13`).

Currently, the cycle keys are hardcoded to Tab/BackTab (`src/app/input/modal.rs:377-378`) and the follow-up command modifier acceptance is hardcoded to CONTROL (`src/app/input/modal.rs:411-413`). This makes any non-ctrl+tab configuration broken — follow-up commands are rejected and the modifier release detection logic mismatches.

## Goals
- Cycle keys (forward/backward) derive from the configured `quick_switch_workspace` binding keycode and modifier
- Follow-up commands (j/k/h/l/s) accept the same modifier as the `quick_switch_workspace` binding
- Default behavior remains unchanged for existing `ctrl+tab` users
- Support any parseable key as the cycle key (Tab, F13, etc.)
- Add optional `quick_switch_workspace_backward` config for explicit backward cycle override

## Non-Goals
- Support prefix-mode triggered quick-switch (only direct bindings)
- Change modifier release detection logic (already correctly reads from configured binding)
- Support per-binding follow-up modifier sets (multi-binding configs use the first direct binding)
- Add forward cycle customization (forward cycle keycode is always the same as the `quick_switch_workspace` binding keycode)

## Functional Requirements
1. The system SHALL derive the accepted follow-up command modifier from the first direct binding of `quick_switch_workspace`, instead of hardcoding CONTROL.
2. The system SHALL derive the forward cycle keycode from the first direct binding of `quick_switch_workspace` (e.g., Tab → Tab, F13 → F13). The backward cycle keycode defaults to the same keycode with SHIFT, unless overridden by `quick_switch_workspace_backward`.
3. The system SHALL support a new `keys.quick_switch_workspace_backward` config of type `BindingConfig` for explicit backward cycle customization.
4. If `quick_switch_workspace_backward` is unset, the system SHALL derive backward cycle as the forward keycode with SHIFT modifier added (e.g., Tab → BackTab, F13 → Shift+F13).
5. If `quick_switch_workspace_backward` is set, the system SHALL use that binding's keycode and modifiers as the backward cycle key.

## Non-Functional Requirements
- **Performance**: No specific constraint — key handling is synchronous and the modifier extraction is O(1) over the binding list.
- **Security**: No impact — this is purely input handling.
- **UX / Accessibility**: Default behavior must be unchanged for existing `ctrl+tab` users. macOS users opting into `cmd+tab` should get system-native switcher behavior.
- **Reliability**: Must handle multi-binding configs gracefully by using the first direct binding. Must not panic if no direct binding is configured.

## Constraints & Assumptions
- Quick-switch only supports direct bindings (`BindingTrigger::Direct`). Prefix or prefix-sequence bindings are not supported for quick-switch modifier derivation.
- When multiple direct bindings are configured for `quick_switch_workspace`, the first one's modifier is canonical for follow-up behavior.
- Crossterm's `KeyEventKind::Release` events are available (already enabled via Kitty keyboard protocol).
- The `quick_switch_modifier_release_matches` function at `src/app/input/mod.rs:413-424` already correctly handles configurable modifiers and does not need changes.
- Any key that parses through `parse_key_combo` (including F13-F35) can be used as the quick-switch trigger and cycle key. The parser uses `KeyCode::F(n)` for function keys (`keybinds.rs:1206`).
- Kitty keyboard protocol supports F13+modifier via codepoint 57376+ (`parse.rs:57376..=57398`).
- The terminal emulator must support transmitting the chosen key (e.g., F13) with modifiers via Kitty protocol or legacy sequences.

## Acceptance Criteria
- [ ] Setting `quick_switch_workspace = "cmd+tab"` makes `cmd+tab` trigger quick-switch, `cmd+shift+tab` cycle backward, and `cmd+j/k/h/l/s` work as follow-up commands.
- [ ] Setting `quick_switch_workspace = "cmd+f13"` makes `cmd+f13` trigger quick-switch, `cmd+shift+f13` cycle backward, and `cmd+j/k/h/l/s` work as follow-up commands.
- [ ] Releasing the modifier key (cmd, ctrl, alt, or super) accepts the quick-switch selection.
- [ ] Default config (`ctrl+tab`) behavior is unchanged — no regression for existing users.
- [ ] New `quick_switch_workspace_backward` config can override the derived backward cycle key.
- [ ] Tests cover `ctrl+tab`, `cmd+tab`, `alt+tab`, `super+tab`, `cmd+f13`, and `ctrl+f13` as `quick_switch_workspace` bindings, verifying cycle keys, follow-up commands, and modifier release.

## Recommended Approach
Add helper methods on `Keybinds` that extract the `KeyModifiers` and `KeyCode` from the first `Direct` binding of `quick_switch_workspace`. Modify `handle_quick_switch_workspace_picker_key` in `src/app/input/modal.rs` to use the extracted keycode for matching cycle keys (instead of hardcoded Tab/BackTab) and the extracted modifier for follow-up commands. Add `quick_switch_workspace_backward: BindingConfig` to `KeysConfig` in `src/config/model.rs`, parse it in `validated_keybinds`, and use it when set. Update tests in `modal.rs` to parameterize over different modifiers and keycodes.

## Decisions

### Cycle key derivation from binding modifier
**Question**: Pre-resolved from codebase evidence — confirmed in Step 4
**Recommended**: Cycle keys should derive from the configured `quick_switch_workspace` binding modifier
**Chosen**: Keep this behavior
**Rationale**: `evidence: src/app/input/modal.rs:377-378 + confirmed` — hardcoded Tab/BackTab should use the binding's modifier

### Follow-up command modifier from binding
**Question**: Pre-resolved from codebase evidence — confirmed in Step 4
**Recommended**: Follow-up commands should accept the same modifier as the `quick_switch_workspace` binding
**Chosen**: Keep this behavior
**Rationale**: `evidence: src/app/input/modal.rs:411-413 + confirmed` — hardcoded CONTROL should be replaced with binding-derived modifier

### Modifier release detection stays as-is
**Question**: Pre-resolved from codebase evidence — confirmed in Step 4
**Recommended**: `quick_switch_modifier_release_matches` already reads modifiers from the configured binding and works correctly
**Chosen**: Keep this behavior
**Rationale**: `evidence: src/app/input/mod.rs:413-424 + confirmed` — no changes needed

### Multi-binding handling
**Question**: The quick_switch_workspace config can have multiple bindings. How should follow-up commands and cycle keys behave?
**Recommended**: Use the first direct binding's modifier
**Chosen**: Use the first direct binding's modifier
**Rationale**: Simple and predictable. The first binding is the canonical one for follow-up behavior.

### Backward cycle customization
**Question**: Should backward cycle be always modifier+shift+tab, or configurable separately?
**Recommended**: Always modifier+shift+tab
**Chosen**: Configurable separately via `quick_switch_workspace_backward`
**Rationale**: User wants explicit control over backward cycle for flexibility.

### Backward cycle config name
**Question**: What should the new config key be called?
**Recommended**: `quick_switch_workspace_backward`
**Chosen**: `quick_switch_workspace_backward`
**Rationale**: Mirrors existing `quick_switch_workspace` name with `_backward` suffix. Clear and discoverable.

### Backward cycle default
**Question**: What should the default for `quick_switch_workspace_backward` be?
**Recommended**: Unset / derive from forward binding
**Chosen**: Unset / derive from forward binding
**Rationale**: No extra config needed for the common case. If `quick_switch_workspace` is `ctrl+tab`, backward is implicitly `ctrl+shift+tab`.

### Test coverage
**Question**: Which scenarios must be covered by tests?
**Recommended**: All modifier combinations (ctrl+tab, cmd+tab, alt+tab, super+tab)
**Chosen**: All modifier combinations
**Rationale**: Ensure correctness across platforms and modifier choices.

## Open Questions
- None — all questions resolved during the interview.

## Suggested Follow-ups
- Consider whether `switch_tab` indexed bindings should also support custom modifiers beyond the current `prefix+1..9` default. Currently `switch_tab` uses `Vec<IndexedKeybind>` with range parsing (`keybinds.rs:740-757`). This is out of scope for the quick-switch modifier work but was surfaced during codebase probe.

## References
- `src/app/input/modal.rs` — quick-switch key handling
- `src/app/input/mod.rs` — modifier release detection
- `src/config/keybinds.rs` — binding parsing and modifier extraction
- `src/config/model.rs` — keybind configuration defaults
