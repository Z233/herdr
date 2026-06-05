---
date: 2026-06-05T21:41:13+0800
author: Z233
commit: 3cfcabe
branch: feature/workspace-picker-preview
repository: workspace-picker-preview
topic: "workspace-picker-preview Bug Fix"
tags: [bug-fix, quick-switch, key-release, workspace-picker]
status: in_progress
last_updated: 2026-06-05T21:41:13+0800
last_updated_by: Z233
type: bug_fix
---

# Handoff: Quick-Switch Release-to-Select Not Working

## Task(s)
- **COMPLETED**: Fix Ghostty `ctrl+tab` not triggering quick-switch — resolved by configuring Ghostty to send Kitty CSI sequences (`keybind = ctrl+tab=csi:9;5u`).
- **IN PROGRESS**: Fix quick-switch "release to select" behavior. Currently releasing `ctrl+tab` does NOT switch to the selected workspace. Only `Enter` works.

## Critical References
- FRD: `.rpiv/artifacts/discover/2026-06-05_20-42-35_workspace-picker-preview.md` — see "Commit on Enter/Release" requirement
- `src/app/mod.rs:1317-1341` — key event routing, Release branch is empty
- `src/app/input/modal.rs:366-406` — quick-switch key handler (no release handling)

## Recent changes
- `src/input/parse.rs:470-482` — added tests for `ctrl+tab` parsing (both modifyOtherKeys and Kitty protocols)
- `~/Library/Application Support/com.mitchellh.ghostty/config` — configured Ghostty to send Kitty CSI for `ctrl+tab`

## Learnings
- Ghostty intercepts `ctrl+tab` by default for tab switching. Must explicitly configure it to send escape sequences to applications.
- `unbind` in Ghostty does NOT automatically send escape sequences — must use `csi:` or `text:` to forward.
- Crossterm **does** receive release events when Kitty keyboard protocol is enabled (`PushKeyboardEnhancementFlags` with `REPORT_EVENT_TYPES`).
- Current code in `src/app/mod.rs:1337-1339` discards Release events:
  ```rust
  crossterm::event::KeyEventKind::Release => {
      self.suppressed_repeat_keys.remove(&key_id);
  }
  ```
- The FRD acknowledges crossterm limitations but the actual implementation has Kitty protocol support.

## Artifacts
- `.rpiv/artifacts/discover/2026-06-05_20-42-35_workspace-picker-preview.md` — FRD with quick-switch requirements
- `src/input/parse.rs:470-482` — new tests for ctrl+tab parsing

## Action Items & Next Steps
1. **Implement release-to-select for quick-switch mode**:
   - In `src/app/mod.rs:1337-1339`, add logic to handle Release events when in `Mode::WorkspacePicker` with `WorkspacePickerMode::QuickSwitch`
   - On release of the quick-switch key (`ctrl+tab`), call `accept_workspace_picker_selection_from()` to switch to selected workspace
   - Need to track which key opened the quick-switcher to know which key release should commit
   - Consider: should any key release in quick-switch mode commit, or only the specific quick-switch key?

2. **Test the fix**:
   - Run `cargo test --bin herdr quick_switch`
   - Test manually in Ghostty: `ctrl+tab` to open, release to select

3. **Consider edge cases**:
   - What if user presses other keys while holding `ctrl+tab`?
   - What if user cycles with `Tab` then releases `ctrl+tab`?
   - Should `Esc` still cancel even after release logic is added?

## Other Notes
- The quick-switch keybind is configurable (`quick_switch_workspace` in config), so hardcoding `ctrl+tab` in release handling won't work. Need to check against the configured binding.
- `src/app/input/navigate.rs:569-631` — `action_for_key` can be used to check if a key matches the quick-switch binding
- `src/app/state.rs:863-870` — `WorkspacePickerState` struct may need a new field to track the "opening key" or "commit on release" flag
