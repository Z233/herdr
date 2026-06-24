---
date: 2026-06-24T15:00:24+0800
author: z233
commit: b65d572
branch: z233
repository: herdr
topic: "Extract workspace picker into consolidated src/ui module"
tags: [intent, frd, workspace-picker, refactor, module-extraction]
status: ready
last_updated: 2026-06-24T15:00:24+0800
last_updated_by: z233
---

# FRD: Extract workspace picker into consolidated src/ui module

## Summary
Consolidate all workspace picker (switcher) code — types, operations, input handlers, rendering, and tests — from scattered locations across `src/app/state.rs`, `src/app/actions.rs`, `src/app/input/modal.rs`, and `src/app/input/overlays.rs` into the existing `src/ui/workspace_picker.rs` module. The extraction preserves the `impl AppState` method pattern (Rust allows `impl` blocks in any file) to avoid introducing traits or interfaces, keeping the diff mechanical and low-risk.

## Problem & Intent
The workspace picker logic is scattered across four files in `src/app/` and `src/ui/`. When merging upstream changes to `actions.rs` and `state.rs` — core app files that upstream frequently modifies — merge conflicts are frequent and painful. Extracting all picker code into a single module under `src/ui/` isolates it from the core app files, reducing the conflict surface for future upstream merges.

## Goals
- All workspace picker code (types, operations, key handlers, mouse handling, rect/geometry methods, free functions, rendering, tests) lives in `src/ui/workspace_picker.rs`.
- `src/app/actions.rs` no longer contains picker-specific functions or tests.
- `src/app/state.rs` no longer contains picker type definitions (only the `workspace_picker: WorkspacePickerState` field + import).
- `src/app/input/modal.rs` no longer contains picker key handlers or their tests.
- `src/app/input/overlays.rs` no longer contains picker rect methods, mouse handling, or `workspace_picker_list_width`.
- Zero behavioral change — the picker works identically before and after extraction.

## Non-Goals
- Introducing a trait or interface to decouple the picker from `AppState` — the `impl AppState` pattern is preserved.
- Moving `Mode::WorkspacePicker` out of `state.rs` — it's part of the app's mode dispatch system referenced by 10+ files.
- Moving the `workspace_picker: WorkspacePickerState` field off `AppState` — the state container stays on `AppState`.
- Moving `record_workspace_focus()` — shared with `switch_workspace()`/`switch_workspace_tab()`.
- Moving `workspace_activity_summary()`/`tab_aggregate_state()` — shared with sidebar/navigator.
- Moving `NavigateAction::WorkspacePicker`/`QuickSwitchWorkspace` dispatch entries — they're part of the keybind action system.
- Mobile switcher (`src/ui/mobile.rs`) alignment — separate concern.

## Functional Requirements
1. The module `src/ui/workspace_picker.rs` SHALL contain all picker type definitions: `WorkspacePickerTarget`, `WorkspacePickerMode` (+ impl), `WorkspacePickerRow`, `WorkspacePickerPreview` (+ Default impl), `WorkspacePickerState`.
2. The module SHALL contain all picker operations as `impl AppState` methods: `open_workspace_picker`, `open_workspace_picker_from`, `open_quick_switch_workspace_from`, `workspace_picker_rows`, `workspace_picker_rows_from`, `workspace_picker_workspace_row`, `workspace_picker_tab_rows`, `workspace_picker_max_scroll_from`, `ensure_workspace_picker_selection_visible_from`, `clamp_workspace_picker_selection_from`, `move_workspace_picker_selection_from`, `cycle_quick_switch_workspace_from`, `expand_selected_workspace_picker_workspace_from`, `collapse_selected_workspace_picker_workspace_from`, `enter_quick_switch_search_from`, `leave_quick_switch_search_from`, `accept_workspace_picker_selection_from`, `focus_workspace_picker_target`, `refresh_workspace_picker_preview_from`, `selected_workspace_picker_ws_idx_from`, `workspace_picker_preview_for_target_from`.
3. The module SHALL contain the picker-specific free functions: `workspace_mru_indices`, `workspace_picker_match_rank`, `workspace_picker_match_position_sum`, `workspace_picker_fuzzy_match`.
4. The module SHALL contain the picker key handlers: `handle_workspace_picker_key`, `handle_quick_switch_workspace_picker_key`, `quick_switch_command_modifiers`.
5. The module SHALL contain the picker mouse handler, extracted from the inline block in `overlays.rs:202-256` into a standalone function.
6. The module SHALL contain the picker rect/geometry methods (as `impl AppState`): `workspace_picker_list_width`, `workspace_picker_popup_rect`, `workspace_picker_inner_rect`, `workspace_picker_search_rect`, `workspace_picker_content_rect`, `workspace_picker_body_rect`, `workspace_picker_divider_rect`, `workspace_picker_preview_rect`, `workspace_picker_footer_rect`, `workspace_picker_popup_contains`, `workspace_picker_row_index_at_from`.
7. The module SHALL contain all picker-related tests, moved from `actions.rs` (11 tests) and `modal.rs` (15 tests), alongside the existing rendering tests.
8. The module visibility in `src/ui.rs` SHALL change from `mod workspace_picker;` to `pub(crate) mod workspace_picker;` to allow cross-module access from `src/app/`.
9. `src/app/state.rs` SHALL import `WorkspacePickerState` from the picker module and retain only the `pub workspace_picker: WorkspacePickerState` field on `AppState`.
10. The picker-specific key release logic in `src/app/input/mod.rs:handle_key_release` SHALL be extracted into a function in the picker module; `handle_key_release` SHALL delegate to it when mode is `WorkspacePicker`.

## Non-Functional Requirements
- **Performance**: No runtime change — pure code movement, no new abstractions or indirection.
- **Security**: N/A — display-only refactor.
- **UX / Accessibility**: Zero behavioral change — the picker looks and behaves identically.
- **Reliability**: No new error paths; the extraction is mechanical code movement preserving all existing logic.

## Constraints & Assumptions
- The `impl AppState` pattern is preserved — picker operations stay as methods on `AppState`, just defined in a different file. Rust allows `impl` blocks in any file within the same crate.
- `Mode::WorkspacePicker` stays in `state.rs` — it's referenced by `src/app/input/mouse.rs:96`, `src/app/input/sidebar.rs:344`, `src/ui/sidebar.rs:83`, `src/ui.rs:417`, `src/app/mod.rs:1534`, and other dispatch points.
- `record_workspace_focus()` (`actions.rs:1228`) stays in `actions.rs` — called by `switch_workspace()` (`actions.rs:1509`) and `switch_workspace_tab()` (`actions.rs:1551`).
- `workspace_activity_summary()` (`actions.rs:1383`) and `tab_aggregate_state()` (`actions.rs:1342`) stay in `actions.rs` — shared with sidebar and navigator.
- `leave_modal()` (`modal.rs:521`) stays in `modal.rs` — generic overlay closer, not picker-specific.
- Test helpers (`app_with_workspaces`, `mark_linked_worktree`, `mark_parent_worktree` from `actions.rs`; `config_with_quick_switch`, `state_with_quick_switch_binding` from `modal.rs`) move with their tests or become `pub(crate)` shared helpers.
- Assumption: no upstream code references the picker types by their current module path (`crate::app::state::WorkspacePickerState`) — if any does, the import paths need updating.

## Acceptance Criteria
- [ ] `src/ui/workspace_picker.rs` contains all 5 picker types, all ~21 picker operations, all 4 free functions, all 3 key handler functions, the extracted mouse handler function, all 11 rect/geometry methods, and all 26 picker tests.
- [ ] `grep -n "WorkspacePickerTarget\|WorkspacePickerMode\|WorkspacePickerRow\|WorkspacePickerPreview\|WorkspacePickerState" src/app/state.rs` returns only the `workspace_picker: WorkspacePickerState` field declaration and its import.
- [ ] `grep -n "fn workspace_picker\|fn open_workspace_picker\|fn open_quick_switch\|fn accept_workspace_picker\|fn handle_workspace_picker_key\|fn handle_quick_switch\|fn workspace_picker_match_rank\|fn workspace_picker_fuzzy_match\|fn workspace_mru_indices" src/app/actions.rs` returns no results.
- [ ] `grep -n "fn handle_workspace_picker_key\|fn handle_quick_switch_workspace_picker_key\|fn quick_switch_command_modifiers" src/app/input/modal.rs` returns no results.
- [ ] `grep -n "fn workspace_picker_popup_rect\|fn workspace_picker_body_rect\|fn workspace_picker_list_width\|fn workspace_picker_row_index_at" src/app/input/overlays.rs` returns no results.
- [ ] The picker mouse handling block is no longer inline in `overlays.rs` — it calls a function from the picker module.
- [ ] `src/ui.rs` declares `pub(crate) mod workspace_picker;`.
- [ ] `cargo test --bin herdr` exits 0.
- [ ] `cargo clippy --bin herdr` introduces no new warnings.

## Recommended Approach
Expand `src/ui/workspace_picker.rs` to become the single home for all workspace picker code. Move types from `state.rs:805-883`, operations (as `impl AppState` blocks) from `actions.rs:731-1315`, free functions from `actions.rs:1275-1315`, key handlers from `modal.rs:286-428`, rect methods from `overlays.rs:21,420-523`, and the extracted mouse handler from `overlays.rs:202-256`. Extract the picker-specific key release logic from `mod.rs:handle_key_release` into a picker module function. Change module visibility to `pub(crate)`. Move 26 picker tests from `actions.rs` and `modal.rs` into the module's test block. `AppState` retains the `workspace_picker` field; `Mode::WorkspacePicker` stays in `state.rs`.

## Decisions

### Expand existing workspace_picker.rs as consolidation target
**Question**: From the probe: the rendering code already lives in `src/ui/workspace_picker.rs:19-565`. Keep this as the consolidation target (expand it with types + operations moved from `state.rs`/`actions.rs`), or create a new sibling module under `src/ui/`?
**Recommended**: Expand `src/ui/workspace_picker.rs` — one file, one module, minimal new module wiring.
**Chosen**: Expand `src/ui/workspace_picker.rs`.
**Rationale**: The rendering code is already there; consolidating into one file gives the simplest module structure and maximizes conflict reduction.

### Keep impl AppState pattern
**Question**: From the probe: picker operations are `impl AppState` methods (`src/app/actions.rs:731-1315`) that directly access `self.workspaces`, `self.terminals`, `self.active`, etc. Keep this `impl AppState` pattern (Rust allows `impl` blocks in any file — just move the block), or refactor to a trait/interface to decouple the picker from AppState?
**Recommended**: Keep `impl AppState` — move the impl block to the new file. Zero behavioral change, minimal diff, preserves all existing access. Optimizes for low-risk extraction, loses true decoupling.
**Chosen**: Keep `impl AppState`.
**Rationale**: The goal is conflict reduction, not architectural decoupling. A trait would add complexity and its own merge-conflict surface in the trait definition.

### Move picker types to new module
**Question**: From the probe: picker state types are defined in `src/app/state.rs:805-883`. Move them to the new `src/ui/` module, or keep them in `state.rs`?
**Recommended**: Move types to the new module — one place for all picker code. `state.rs` just has `pub workspace_picker: WorkspacePickerState` field + import.
**Chosen**: Move types to new module.
**Rationale**: Consolidating types with their logic is the core value of the extraction; leaving types in `state.rs` would split the picker across two modules.

### Move key handlers + rect methods to picker module
**Question**: The picker's input handlers live in `src/app/input/modal.rs:286-428` (key handlers) and `src/app/input/overlays.rs:21,420-523` (rect methods + mouse handling). Should they move to the picker module too, or stay in `src/app/input/`?
**Recommended**: Move key handlers too — maximizes conflict reduction, the module mixes rendering + input logic.
**Chosen**: Move key handlers too.
**Rationale**: Key handlers and rect methods are entirely picker-specific; moving them further reduces the picker's footprint in core app files that upstream changes.

### Extract and move mouse handling
**Question**: The picker mouse handling is an inline `if mode == WorkspacePicker { ... }` block in `overlays.rs:202-256`. Extract it into a standalone function and move to the picker module, or leave it inline?
**Recommended**: Extract + move — cleanest extraction, small refactor in `overlays.rs`.
**Chosen**: Extract + move.
**Rationale**: The inline block is self-contained and picker-specific; extracting it into a function allows the picker module to own all picker input handling uniformly.

### Move workspace_mru_indices, leave shared functions
**Question**: `workspace_mru_indices()` (`actions.rs:1199`) is only called by the picker, but `record_workspace_focus()` (`actions.rs:1228`) is shared, and `workspace_activity_summary`/`tab_aggregate_state` (`actions.rs:1342,1383`) are shared with sidebar/navigator. Move the picker-only function and leave shared ones?
**Recommended**: Move `workspace_mru_indices`, leave shared functions in `actions.rs`.
**Chosen**: Move mru_indices, leave shared.
**Rationale**: `workspace_mru_indices()` is picker-only (`actions.rs:792`); `record_workspace_focus()` is called by `switch_workspace()` (`actions.rs:1509`) and `switch_workspace_tab()` (`actions.rs:1551`); activity/tab state functions are used by sidebar and navigator.

### Move picker tests to the module
**Question**: Picker tests currently live in `actions.rs:4000-4198` (11 tests) and `modal.rs:1089-2255` (15 tests). Move them to the picker module's test block, or leave them in place?
**Recommended**: Move tests too — maximizes consolidation, larger diff.
**Chosen**: Move tests too.
**Rationale**: Tests are picker-specific; moving them completes the single-module consolidation and removes picker test code from core app files.

## Open Questions
None — all decisions resolved during the interview.

## Suggested Follow-ups
- The `handle_key_release` method (`src/app/input/mod.rs:72`) is `impl App`, not `impl AppState` — the picker-specific logic should be extracted into a function in the picker module, with `handle_key_release` becoming a thin delegate. The exact function signature is an implementation detail for the `implement` phase.
- Test helpers (`app_with_workspaces`, `mark_linked_worktree`, `mark_parent_worktree` from `actions.rs`; `config_with_quick_switch`, `state_with_quick_switch_binding` from `modal.rs`) are used by both picker and non-picker tests. They may need to become `pub(crate)` shared helpers or be duplicated — left for `implement` to decide.
- Duplicate `truncate_text`/`truncate` functions exist in `src/ui/workspace_picker.rs:374`, `src/ui/mobile.rs:997`, and `src/ui/sidebar.rs:206` — code duplication candidate for consolidation, separate from this FRD's scope.

## References
- Input: "给予这个实现，我想更进一步，把 workspace switcher 提取到 @src/ui/ 下一个独立的模块，降低未来合并 upstream 时的冲突概率"
- `src/ui/workspace_picker.rs:19-565` — existing rendering code (consolidation target)
- `src/app/state.rs:805-883` — picker type definitions to move
- `src/app/actions.rs:731-1315` — picker operations (impl AppState) to move
- `src/app/actions.rs:1275-1315` — picker free functions to move
- `src/app/actions.rs:1199` — `workspace_mru_indices()` to move
- `src/app/actions.rs:4000-4198` — picker tests to move
- `src/app/input/modal.rs:286-428` — picker key handlers to move
- `src/app/input/modal.rs:1089-2255` — picker key handler tests to move
- `src/app/input/overlays.rs:21,420-523` — picker rect methods + list_width to move
- `src/app/input/overlays.rs:202-256` — picker mouse handling to extract + move
- `src/app/input/mod.rs:72-101` — picker-specific key release logic to extract
