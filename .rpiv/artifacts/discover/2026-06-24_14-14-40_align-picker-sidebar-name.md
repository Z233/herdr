---
date: 2026-06-24T14:14:40+0800
author: fronzzhang
commit: 32cc447
branch: z233
repository: herdr
topic: "Align workspace picker name display with sidebar"
tags: [intent, frd, workspace-picker, sidebar, grouped-child-display-label]
status: ready
last_updated: 2026-06-24T14:14:40+0800
last_updated_by: fronzzhang
---

# FRD: Align workspace picker name display with sidebar

## Summary
The workspace switcher (picker) and the sidebar display different names for the same workspace when the workspace is a grouped child worktree. The sidebar applies `grouped_child_display_label()` which substitutes the git branch name (stripping the `worktree/` prefix) for indented child workspaces without custom names; the picker does not. This FRD aligns the picker to the sidebar's behavior by replicating the same condition and substitution rule.

## Problem & Intent
User-facing confusion: end users see one name in the workspace switcher and a different name in the sidebar for the same workspace. For example, a worktree child whose CWD-derived name is `herdr-issue` but whose branch is `worktree/issue-137` appears as `issue-137` in the sidebar but `herdr-issue` in the picker. The user cannot tell they are looking at the same workspace.

## Goals
- A grouped child workspace (linked worktree in a group with ≥2 members, no custom name) displays the same name in both the workspace picker and the sidebar.
- Standalone workspaces and workspaces with custom names are unaffected — they already match.
- `grouped_child_display_label()` becomes a single shared function, not duplicated.

## Non-Goals
- Mobile switcher (`src/ui/mobile.rs`) alignment — out of scope per developer decision.
- Truncation behavior alignment — the picker uses `truncate_text()` with `…` ellipsis while the sidebar silently clips; this inconsistency is out of scope.
- Refactoring `display_name_from()` itself — the shared name source stays as-is; alignment happens in the rendering/row-construction layer.

## Functional Requirements
1. The workspace picker SHALL apply `grouped_child_display_label()` to any workspace that is a grouped child — i.e., a linked worktree whose `worktree_space()` group has ≥2 members — substituting the git branch name (stripping the `worktree/` prefix) when the workspace has no custom name.
2. The workspace picker SHALL determine grouped-child status using the same `worktree_space()` grouping logic as the sidebar's `workspace_list_entries()` (`src/ui/sidebar.rs:349-444`).
3. The `grouped_child_display_label()` function SHALL be `pub(crate)` in `src/ui/sidebar.rs` so the picker can import and call it without code duplication.
4. Standalone workspaces (not grouped children) and workspaces with custom names SHALL continue to display the raw `display_name_from()` result in the picker, matching current behavior.

## Non-Functional Requirements
- **Performance**: No new computation per render frame beyond a `worktree_space()` lookup per workspace — negligible.
- **Security**: N/A — display-only change.
- **UX / Accessibility**: A user switching between the sidebar and the picker sees the same workspace name in both views, eliminating disorientation.
- **Reliability**: No new error paths; the substitution is a pure string transformation with existing fallback logic.

## Constraints & Assumptions
- The picker's row construction lives in `src/app/actions.rs:800-840` (`workspace_picker_rows_from` / `workspace_picker_workspace_row`); the label is set at line 804 as `ws.display_name_from(...)`.
- The sidebar's grouping logic in `workspace_list_entries()` (`src/ui/sidebar.rs:349-444`) is the canonical reference for determining grouped-child status.
- `workspace_parent_group_state()` (`src/ui/sidebar.rs:286-300`) already encodes the "is this a linked worktree in a ≥2-member group" check — it may be reusable or its logic may need to be extracted for the picker's use.
- Assumption: the picker does not need to visually indent grouped children (no UI layout change); only the label text changes.

## Acceptance Criteria
- [ ] For a workspace that is a linked worktree child in a group with ≥2 members, no custom name, with branch `worktree/issue-137`: the workspace picker displays `issue-137` (matching the sidebar).
- [ ] For a standalone workspace (not a grouped child) with no custom name and branch `main`: the workspace picker still displays the CWD-derived name (unchanged).
- [ ] For a workspace with a custom name set: the workspace picker displays the custom name in both grouped-child and standalone cases (unchanged).
- [ ] `cargo test` exits 0.
- [ ] `cargo clippy` exits 0.
- [ ] No duplicate copy of `grouped_child_display_label()` logic exists outside `src/ui/sidebar.rs`.

## Recommended Approach
Make `grouped_child_display_label()` in `src/ui/sidebar.rs:308` `pub(crate)`. In `workspace_picker_rows_from()` (`src/app/actions.rs:800`), after computing `display_name_from()` for each workspace, determine whether the workspace is a grouped child using the same `worktree_space()` grouping logic as the sidebar, and if so, apply `grouped_child_display_label()` to the label before passing it to `workspace_picker_workspace_row()`. The picker's rendering code (`src/ui/workspace_picker.rs`) needs no change — it already renders the row label as-is.

## Decisions

### Keep display_name_from() as shared name source
**Question**: Pre-resolved from codebase evidence — all three components already share `display_name_from()` (`src/workspace.rs:1095-1102`) as the name source; the inconsistency is in downstream rendering logic. Keep it and align downstream, or refactor branch-name substitution into `display_name_from()` itself?
**Recommended**: Keep `display_name_from()` as-is; align in the rendering layer.
**Chosen**: Keep, align downstream.
**Rationale**: evidence: `src/workspace.rs:1095-1102` + confirmed. The shared name source is correct; only the downstream rendering diverges.

### Align picker to sidebar direction
**Question**: The sidebar's `grouped_child_display_label()` (`src/ui/sidebar.rs:308-319`) replaces the CWD-derived name with the git branch name for indented child workspaces without custom names. The picker doesn't apply this. Which direction should the alignment go?
**Recommended**: Extend `grouped_child_display_label()` to the picker (align picker → sidebar).
**Chosen**: Extend to picker — make the workspace switcher align to the sidebar.
**Rationale**: The branch name is the more meaningful identifier for worktree children; the sidebar's existing behavior is the desired one. Mobile switcher is explicitly out of scope.

### Truncation inconsistency out of scope
**Question**: The probe found a second difference: the picker uses `truncate_text()` with `…` ellipsis (`src/ui/workspace_picker.rs:374-387`), while the sidebar silently clips without ellipsis (`src/ui/sidebar.rs:948-951`). Is this also in scope?
**Recommended**: Branch-name substitution only — leave truncation as a separate concern.
**Chosen**: Branch-name only.
**Rationale**: Developer scoped the work to the name display rule, not truncation behavior. Truncation is a separate UX concern.

### Match sidebar's exact grouping condition
**Question**: The sidebar applies `grouped_child_display_label()` only for indented (grouped child) workspaces — linked worktrees in groups with ≥2 members. The picker has no grouping concept today. Should the picker replicate the sidebar's exact condition, or apply the substitution unconditionally to any workspace without a custom name that has a branch?
**Recommended**: Match the sidebar's exact condition (determine grouped-child status via `worktree_space()`).
**Chosen**: Match sidebar condition.
**Rationale**: Exact behavioral parity with the sidebar — only linked-worktree children in groups with ≥2 members get the branch-name substitution. Standalone worktrees keep the CWD-derived name in both views.

### Make grouped_child_display_label() pub(crate)
**Question**: `grouped_child_display_label()` is currently a private function in `src/ui/sidebar.rs:308`. The picker needs to call it. How should the code be shared?
**Recommended**: Change visibility to `pub(crate)` in sidebar.rs and import it from the picker's row-construction code.
**Chosen**: Make `pub(crate)`.
**Rationale**: Minimal change, single source of truth — no function duplication, no new module needed for a single function.

## Open Questions
None — all decisions resolved during the interview.

## Suggested Follow-ups
- Mobile switcher (`src/ui/mobile.rs:471`) has the same branch-name substitution inconsistency — does not apply `grouped_child_display_label()`. Out of scope per developer decision; candidate for a future alignment pass.
- Truncation inconsistency: picker uses `truncate_text()` with `…` (`src/ui/workspace_picker.rs:374-387`), sidebar silently clips (`src/ui/sidebar.rs:948-951`). Out of scope per developer decision.
- Duplicate `truncate_text`/`truncate` functions exist in `src/ui/workspace_picker.rs:374`, `src/ui/mobile.rs:997`, and `src/ui/sidebar.rs:206` — code duplication candidate for consolidation.
- Sidebar branch truncation uses byte-slicing (`src/ui/sidebar.rs:970`: `&branch[..max_branch_len.saturating_sub(1)]`) which could panic on multi-byte UTF-8 characters — potential bug distinct from this FRD's scope.

## References
- Input: free-text feature description — "workspace switcher 里 workspace name 的显示规则和 sidebar name 的显示规则不一样，需要对齐"
- `src/ui/sidebar.rs:308-319` — `grouped_child_display_label()` definition
- `src/ui/sidebar.rs:349-444` — `workspace_list_entries()` grouping logic
- `src/ui/sidebar.rs:286-300` — `workspace_parent_group_state()` linked-worktree check
- `src/app/actions.rs:800-840` — picker row construction (`workspace_picker_rows_from` / `workspace_picker_workspace_row`)
- `src/workspace.rs:1095-1102` — `display_name_from()` shared name source
- `src/ui/workspace_picker.rs:174-182` — picker label rendering with truncation
- `src/ui/mobile.rs:469-482` — mobile switcher label rendering (out of scope)
