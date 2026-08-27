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

**Frozen Copy View**:
An immutable view of a Pane's visible terminal cells captured when EasyMotion first starts within a copy-mode session. It is distinct from the Pane's live content.
_Avoid_: Pane snapshot, frozen Pane

**EasyMotion Target**:
A query match in a Frozen Copy View that is identified by an EasyMotion label. It is distinct from a Selection Anchor.
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

**Workspace Switcher**:
The overlay that presents switcher items for moving among runtime destinations or opening a searched directory as a workspace. It includes Quick Switch and Search behavior.
_Avoid_: Workspace Picker, picker

**Switcher Item**:
A selectable destination in the Workspace Switcher that refers to a workspace, tab, or directory.
_Avoid_: Row, card, search result

**Quick Switch**:
The Workspace Switcher interaction that cycles through recently used workspaces and accepts the selected destination when its hold modifier is released.
_Avoid_: Full list, Workspace Picker
