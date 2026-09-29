# Herdr

Herdr coordinates terminal-based agent work through workspaces, tabs, panes, and selectable destinations. This glossary standardizes product language across the runtime and TUI.

## Language

**Workspace**:
A named top-level session container that owns tabs and keeps an identity independent of its display order.
_Avoid_: Project, workspace row

**Tab**:
A workspace-owned container that arranges one or more panes.
_Avoid_: Workspace, pane group

**Pane**:
A terminal session surface within a tab. An agent can run in a pane, but the pane is not the agent.
_Avoid_: Agent, terminal tab

**Copy-mode Surface**:
The terminal cells used by a client's current copy-mode session. EasyMotion results are valid only while the endpoint, content revision, geometry, selection, and copy session still match.
_Avoid_: Frozen Copy View, frozen Pane

**EasyMotion Target**:
A query match in the validated Copy-mode Surface that is identified by an EasyMotion label. It is distinct from a Selection Anchor.
_Avoid_: Anchor, jump anchor

**Selection Anchor**:
The fixed endpoint from which a copy-mode selection extends. It is distinct from an EasyMotion Target.
_Avoid_: EasyMotion anchor, target

**Managed Linked Worktree**:
A workspace checkout that Herdr has explicitly associated with a repository and identified as a Git linked worktree. An arbitrary linked checkout opened outside Herdr management is not a managed linked worktree.
_Avoid_: Managed workspace, any linked checkout

**Repository Name**:
The human-readable name of the shared Git repository that gives a managed linked worktree its repository context. It is distinct from the checkout directory and repository path.
_Avoid_: Repository path, checkout name

**Navigator**:
The client-owned overlay for searching destinations across Local and saved SSH machines, opening a directory as a workspace, and Quick Switch. The `workspace_picker` and `workspace_switcher` bindings open different modes of this same controller.
_Avoid_: Server overlay, separate picker controller

**Switcher Item**:
A selectable destination in the Navigator that includes its endpoint and a workspace, tab, pane, or directory target.
_Avoid_: Row, card, search result

**Quick Switch**:
The Navigator interaction that cycles through a client's recently used workspaces and accepts the selected destination when its hold modifier is released. Recency changes after successful activation is presented.
_Avoid_: Server MRU, attempted activation
