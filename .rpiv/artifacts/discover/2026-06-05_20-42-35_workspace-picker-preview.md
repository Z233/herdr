---
date: 2026-06-05T20:42:35+0800
author: Z233
commit: e568ac9
branch: feature/workspace-picker-preview
repository: workspace-picker-preview
topic: "workspace-picker-preview"
tags: [intent, frd, workspace-picker, overlay, quick-switch]
status: complete
last_updated: 2026-06-05T20:42:35+0800
last_updated_by: Z233
---

# FRD: workspace-picker-preview

## Summary

Extend the existing workspace picker overlay with a "quick-switch mode" activated by a direct keybind (`ctrl+tab` by default, configurable). In quick-switch mode, the picker shows workspaces in most-recently-used (MRU) order, hides the search bar, and supports Tab/Shift+Tab cycling. Users can expand a workspace to see its tabs with h/l, navigate tabs with j/k, and select with Enter. The existing searchable `prefix+w` picker remains unchanged. This gives end users with many panes a fast, visual Alt+Tab-style workspace switching experience.

## Problem & Intent

> "End user switching panes" — End users who have many panes open find the current workspace switching (sidebar clicks or `prefix+w` searchable picker) too slow when they just want to quickly bounce between recently-used workspaces. They want an OS-style Alt+Tab experience: hold a key combo, see a visual preview of workspaces, cycle through them, and release to switch.

The current picker (`prefix+w`) is great for finding a workspace by name, but it's not optimized for rapid cycling through a small set of recently-used workspaces. The existing `next_workspace`/`previous_workspace` cycles by index but provides no visual feedback.

## Goals

- Provide a fast, visual workspace switcher that feels like OS Alt+Tab
- Show live previews of the selected workspace's focused pane content
- Support MRU ordering so the most recently used workspaces appear first
- Allow expanding a workspace to see and select individual tabs
- Maintain the existing searchable `prefix+w` picker as-is

## Non-Goals

- Replace the existing searchable workspace picker (`prefix+w`)
- Add mouse-only interaction for the quick-switcher
- Persist MRU history across herdr sessions
- Support non-English IME input in the quick-switch mode
- Change the existing `last_pane` or `next_workspace`/`previous_workspace` behavior

## Functional Requirements

1. **Quick-Switch Keybind**: The system SHALL support a configurable keybind `quick_switch_workspace` (default: `ctrl+tab`) that opens the workspace picker in quick-switch mode.
2. **MRU Ordering & Pre-selection**: In quick-switch mode, the system SHALL list workspaces in most-recently-used order, with the **previously active workspace pre-selected** (not the currently active workspace).
3. **Tab Cycling**: The system SHALL support `Tab` to cycle forward and `Shift+Tab` to cycle backward through the MRU-ordered workspace list.
4. **Tab Expansion**: The system SHALL support `h` to collapse and `l` to expand the currently selected workspace's tabs within the picker list.
5. **Tab Navigation**: When a workspace's tabs are expanded, the system SHALL allow `j`/`k` (or Down/Up) to navigate between tabs, and `Enter` to switch to the selected tab.
6. **Search Toggle**: In quick-switch mode, the system SHALL allow pressing `s` to activate the search bar (switching to search mode within the same overlay), and `Esc` to return to quick-switch mode.
7. **Commit on Enter/Release**: The system SHALL switch to the selected workspace (or tab, if expanded) when the user presses `Enter` or releases the quick-switch key combo.
8. **Cancel**: The system SHALL close the picker without switching when the user presses `Esc`.
9. **Visual Preview**: The system SHALL display the focused pane's content preview for the currently selected workspace, using the existing ANSI-to-text rendering.

## Non-Functional Requirements

- **Performance**: Opening the quick-switch overlay SHALL take <50ms from keypress to first rendered frame. Preview refresh SHALL be async and non-blocking.
- **Security**: No new data exposure — preview content is already visible to the user via the existing picker.
- **UX / Accessibility**: The overlay SHALL follow the existing modal/overlay patterns (centered popup, consistent styling, keyboard-only operation). Quick-switch mode SHALL have clear visual distinction from search mode (e.g., search bar visibility).
- **Reliability**: If the quick-switch keybind conflicts with an existing binding, the system SHALL warn at config load time (following existing keybind conflict handling).

## Constraints & Assumptions

- Crossterm does not expose key-release events; the quick-switch "release to select" semantics will be approximated by Enter or by a discrete key combo pattern (e.g., Tab to cycle, Enter to commit).
- The existing `WorkspacePickerState` struct (`src/app/state.rs:836`) will gain a `mode` field to distinguish quick-switch vs. search mode.
- The existing `render_workspace_picker_overlay` (`src/ui/workspace_picker.rs:18`) will conditionally render the search bar based on mode.
- `handle_workspace_picker_key` (`src/app/input/modal.rs:286`) will branch on mode for input semantics.
- The existing `switch_workspace` (`src/app/actions.rs:1030`) and `switch_workspace_tab` (`src/app/actions.rs:1059`) will be reused for final selection.
- A new `quick_switch_workspace` field will be added to `KeysConfig` (`src/config/model.rs`) and `Keybinds` (`src/config/keybinds.rs:270`).
- A new `NavigateAction::QuickSwitchWorkspace` variant will be added (`src/app/input/navigate.rs:477`).

## Acceptance Criteria

- [ ] Pressing `ctrl+tab` (default) opens the workspace picker in quick-switch mode with no search bar visible, workspaces in MRU order, and the **previously active workspace focused** (not the current one).
- [ ] Pressing `Tab` cycles forward through workspaces; `Shift+Tab` cycles backward.
- [ ] Pressing `l` on a selected workspace expands its tabs; `h` collapses them.
- [ ] When tabs are expanded, `j`/`k` navigates tabs and `Enter` switches to the selected tab.
- [ ] Pressing `s` in quick-switch mode shows the search bar and enables search mode; `Esc` returns to quick-switch mode.
- [ ] Pressing `Esc` at any time closes the picker without switching workspaces.
- [ ] Pressing `prefix+w` still opens the existing searchable picker unchanged.
- [ ] `just check` passes with no new warnings.
- [ ] The new `quick_switch_workspace` keybind appears in keybinding help and is configurable in `config.toml`.

## Recommended Approach

Extend the existing workspace picker infrastructure: add a `mode` field to `WorkspacePickerState` (quick-switch vs. search), wire a new `quick_switch_workspace` keybind through `KeysConfig` → `Keybinds` → `NavigateAction` → `open_workspace_picker_from`, and branch rendering/input logic in `render_workspace_picker_overlay` and `handle_workspace_picker_key` based on mode. Reuse `switch_workspace`/`switch_workspace_tab` for final selection and the existing ANSI preview pipeline for live previews.

## Decisions

### Keep existing picker and navigation primitives
**Question**: From the probe I inferred — (1) The current picker opens with prefix+w, (2) last_pane exists as cross-workspace toggle, (3) next_workspace/previous_workspace cycle by index. Keep these as-is?
**Recommended**: Keep all as-is, build quick-switch alongside.
**Chosen**: Keep all as-is.
**Rationale**: `evidence: src/config/model.rs:259` (workspace_picker default), `src/config/model.rs:323` (last_pane), `src/app/actions.rs:1258-1269` (next/previous_workspace) + confirmed.

### Extend existing picker rather than new mode
**Question**: How should the Alt+Tab-style switcher be architecturally integrated?
**Recommended**: New Mode variant (Mode::WorkspaceSwitcher) with separate render/input paths.
**Chosen**: Extend existing picker with a mode sub-state.
**Rationale**: Reuses existing overlay rendering, preview, and mouse handling. Adds complexity to existing picker but avoids duplicating ~300 lines of overlay infrastructure. The two UX patterns (search vs. quick-switch) share enough visual and state structure that a sub-state is appropriate.

### Trigger: configurable ctrl+tab
**Question**: What keybind triggers the quick-switch mode, and what are the input semantics?
**Recommended**: Prefix+Tab with discrete press cycling.
**Chosen**: Direct `ctrl+tab` (configurable via `quick_switch_workspace` in config).
**Rationale**: Direct key combo matches OS Alt+Tab convention. Configurable like all other herdr keybinds. No crossterm key-release support means "release to select" is approximated by Enter or discrete Tab presses.

### Visual: same overlay, search hidden, tabs expandable
**Question**: In quick-switch mode, what should the visual experience look like?
**Recommended**: Same overlay, no search bar, full preview on the right.
**Chosen**: Same overlay layout; search bar hidden in quick-switch mode; `s` shows search bar; `h`/`l` expand/collapse workspace tabs; `j`/`k` navigate.
**Rationale**: Minimal visual divergence from existing picker maintains UI consistency. Tab expansion adds useful context for users with many tabs per workspace.

### Tab navigation when expanded
**Question**: When h/l expands/collapses a workspace's tabs, what can the user do with the expanded tabs?
**Recommended**: j/k navigates tabs when expanded, Enter selects tab.
**Chosen**: j/k navigates tabs when expanded, Enter selects tab.
**Rationale**: Natural extension of existing j/k navigation. When tabs are collapsed, j/k navigates workspaces; when expanded, j/k navigates tabs. Context-aware navigation.

### Pre-select previous workspace, not active
**Question**: When the quick-switcher opens, should the focused item be the currently active workspace or a different one?
**Recommended**: Pre-select the previously active workspace (second in MRU list), so Tab once switches away — matching OS Alt+Tab behavior.
**Chosen**: The focused item must NOT be the active workspace; pre-select the previously active workspace.
**Rationale**: `evidence: src/app/actions.rs:602` (open_workspace_picker_from selects current) + corrected. OS Alt+Tab pre-selects the previous app, not the current one. If the active workspace is pre-selected, the user must press Tab twice to switch — breaks muscle memory.

### MRU ordering, Tab cycles, Enter commits
**Question**: Quick-switch behavior details: ordering, cycling, commit semantics?
**Recommended**: MRU order, Tab forward, timeout auto-close.
**Chosen**: MRU order, Tab forward/Shift+Tab backward, Enter to commit, no timeout.
**Rationale**: MRU matches OS Alt+Tab mental model. No timeout gives user control — they decide when to commit. Timeout adds unpredictability and potential accidental switches.

## Open Questions

- None — all deferred items resolved during interview.

## Suggested Follow-ups

- Consider whether `cycle_pane_next` (`prefix+tab`, `src/config/model.rs:318`) conflicts visually or conceptually with the new `quick_switch_workspace` (`ctrl+tab`). The two bindings serve different scopes (pane vs. workspace) but share the Tab key.
- The existing `last_pane` (`src/config/model.rs:323`) is unset by default and operates at pane level. Consider promoting a workspace-level `last_workspace` as a separate feature if users request it.

## References

- Input description: `feature/workspace-picker-preview` branch, user intent "alttab 风格的 ux"
- Probe artifacts: codebase-locator (c4fbb3ac-cfac-4d2), codebase-analyzer (5cf8f72f-bc68-458)
- Key source files:
  - `src/app/state.rs:813-841` — WorkspacePickerState, WorkspacePickerRow, WorkspacePickerPreview
  - `src/app/actions.rs:596-618` — open_workspace_picker_from
  - `src/app/input/modal.rs:286-365` — handle_workspace_picker_key
  - `src/ui/workspace_picker.rs:16-42` — render_workspace_picker_overlay
  - `src/config/model.rs:239-323` — KeysConfig
  - `src/config/keybinds.rs:127-473` — Keybinds / ActionKeybinds
  - `src/app/input/navigate.rs:477-497` — NavigateAction enum
