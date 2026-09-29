# herdr (z233 fork)

<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  Fork of <a href="https://github.com/herdrdev/herdr">herdr</a> — agent multiplexer that lives in your terminal.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/z233/herdr/releases"><img src="https://img.shields.io/github/downloads/z233/herdr/total?labelColor=333333&color=666666" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/z233/herdr/stargazers"><img src="https://img.shields.io/github/stars/z233/herdr?labelColor=333333&color=666666&logo=github" alt="GitHub stars" /></a>
</p>

---

> **Looking for the original README?** Herdr is developed upstream at **[herdrdev/herdr](https://github.com/herdrdev/herdr)** — install instructions, full documentation, sponsors, and contribution guide all live there.
>
> This README covers **only what this fork adds**.

## What this fork adds

This fork adds Navigator, mobile navigation, copy-mode, and input features to the v0.9.1 client-owned shell. Runtime operations use endpoint-qualified API requests.

### Prefix Chord Sequences

Upstream herdr supports single-step prefix keys (`prefix+h`). This fork extends the prefix system to **multi-step chord sequences** — up to 3 keys after the prefix, with longer overlapping matches taking priority.

This is the foundation for directional pane opening and other multi-key bindings.

```toml
[keys]
chord_timeout_ms = 500   # 0 disables chords entirely
```

### Directional Pane Opening

Open a new pane in a specific direction in a single chord, instead of split-then-navigate:

| Key | Action |
|-----|--------|
| `prefix+w+h` | open pane to the left |
| `prefix+w+j` | open pane below |
| `prefix+w+k` | open pane above |
| `prefix+w+l` | open pane to the right |

The public `pane.split` API supports all four directions. Clients use the advertised `pane.split.directional` endpoint method for left/up, so older endpoints receive only their supported right/down requests. The split ratio is the first child's share, including when the new pane is placed before the target.

`herdr pane split --current` requires `HERDR_PANE_ID`; a missing caller is an error. Omit the target to use the server's normal focused-pane fallback, or pass a pane ID to target it explicitly.

### Workspace Switcher

One client-owned Navigator provides ordinary search and release-to-select switching across Local and saved SSH machines. Existing direct workspace navigation remains available.

- **Fuzzy search** with a live preview of every pane in the selected tab on wide terminals
- **MRU quick-switch** — hold `ctrl+tab` to cycle through recent workspaces, release to select (like `cmd+tab`)
- **Shift reverses direction**; during hold, `j/k` navigates, `s` searches, `l/h` expands/collapses
- **Agent state dots** next to workspace names
- **Repository names** shown for worktree switcher items, with structured labels (git branch names for grouped child worktrees)
- **Zoxide integration** — search any directory by path and open it as a workspace (see below)
- **Mobile support** — switcher auto-opens in empty state on narrow terminals
- **Mobile Quick Switch gesture** — hold `switch`, drag vertically to highlight an item, swipe right/left to expand/collapse a workspace's tabs, and release to accept; a tap leaves the switcher open
- Full keyboard **and** mouse support (hover, click, scroll)

Default bindings: `prefix+w` opens Navigator search through `workspace_picker`; `ctrl+tab` opens hold-to-switch through `workspace_switcher`. When a longer `prefix+w+…` chord is configured, `prefix+w` waits for the chord timeout. Explicit disabled bindings and binding-conflict diagnostics remain effective.

### Zoxide Workspace Search

The Navigator integrates [zoxide](https://github.com/ajeetdsouza/zoxide) as a search provider. Search, path resolution, directory previews, and workspace creation run on the selected machine. The overlay reports offline machines, missing zoxide, timeouts, and creation errors. Search keeps the shared cursor-aware text editor.

Closing and reopening the Navigator invalidates earlier search and creation responses. Directory and terminal previews use passive, explicit-target reads and do not change another client's focus or terminal size.

### EasyMotion Copy Mode Jumps

Vim EasyMotion-style cursor jumping inside copy mode:

1. Press `s` in copy mode
2. Type two target characters
3. Press the visible label key to jump the cursor to that match

Smart case: lowercase queries are case-insensitive, uppercase makes them case-sensitive. The selection anchor is preserved across jumps. `Esc` or `q` cancels at any point. EasyMotion uses normal client copy mode and cancels stale matches when content, geometry, or the copy session changes.

Two opt-in keybindings trigger copy mode with an initial action:

```toml
[keys]
copy_mode_easymotion = "prefix+space"   # enter copy mode and immediately start EasyMotion
copy_mode_scroll_up  = "prefix+u"       # enter copy mode and immediately scroll half a page up
```

### IME Input Enhancement

Parses and forwards the kitty keyboard protocol's **associated text** field, so IME composition text (CJK input methods, etc.) reaches panes correctly. Extends `KeyboardEnhancementFlags` to preserve associated-text bits that newer crossterm versions don't expose, while keeping modifier-only press/release events working.

No configuration needed — active whenever the host terminal supports kitty keyboard protocol.

### Mobile Zoom Map

The narrow-screen header shows the full tab layout while a pane is zoomed, including hidden neighbors and their agent status. The client requests `pane.layout` with an explicit pane ID and joins status on the same machine. Tap a neighbor to move focus; the runtime protocol has no mobile-only layout fields.

Layout refreshes are bounded to one per second and retry after errors. Removing a pane invalidates its cached hit target before the next layout response arrives.

### Spaces-only Sidebar

Hide the desktop Agents section across all machine groups while keeping the Spaces section and the existing sidebar expand/collapse behavior:

```toml
[ui.sidebar.agents]
visible = false
```

The Spaces section uses the reclaimed height in both expanded and compact collapsed modes. Agent detection, notifications, keyboard navigation, sorting configuration, and the agent-view API continue to operate. The default is `true`, so existing configurations keep the standard Spaces and Agents sections.

---

## Configuration

Most fork-specific options live under the `[keys]` section of `config.toml`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `chord_timeout_ms` | `u64` | `500` | Chord sequence timeout in ms; `0` disables chords |
| `workspace_picker` | `BindingConfig` | `"prefix+w"` | Open ordinary Navigator search |
| `workspace_switcher` | `BindingConfig` | `"ctrl+tab"` | Open workspace switcher / MRU quick-switch |
| `workspace_switcher_backward` | `BindingConfig` | *(auto-derived)* | Reverse cycle key; derived from `workspace_switcher` when unset |
| `open_pane_left` | `BindingConfig` | `"prefix+w+h"` | Open pane to the left |
| `open_pane_down` | `BindingConfig` | `"prefix+w+j"` | Open pane below |
| `open_pane_up` | `BindingConfig` | `"prefix+w+k"` | Open pane above |
| `open_pane_right` | `BindingConfig` | `"prefix+w+l"` | Open pane to the right |
| `copy_mode_easymotion` | `BindingConfig` | *(empty)* | Enter copy mode and immediately start EasyMotion |
| `copy_mode_scroll_up` | `BindingConfig` | *(empty)* | Enter copy mode and immediately scroll half a page up |

`workspace_switcher` supports modifiers beyond `ctrl` — `cmd`, `alt`, and `super` all work as the hold modifier for quick-switch.

Agents section visibility is a desktop presentation option:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `ui.sidebar.agents.visible` | `bool` | `true` | Show the Agents section; set to `false` for a Spaces-only sidebar |

---

## Install & Build

Build from source:

Use rustup-managed Cargo and Rust so `rust-toolchain.toml` selects Rust 1.96.1. The vendored terminal library requires Zig 0.16.0. Set `ZIG` to that executable when another Zig version is on `PATH`.

```bash
git clone https://github.com/z233/herdr
cd herdr
cargo build --release
```

For prebuilt binaries, install scripts, Homebrew, and all other installation methods, see the [upstream README](https://github.com/herdrdev/herdr#install).

## Upstream Sync

This fork tracks upstream releases and merges each new tag (`vX.Y.Z`). Fork-specific code is isolated to minimize merge conflicts. See [`docs/FORK_FEATURES.md`](./docs/FORK_FEATURES.md) for a detailed feature analysis and merge conflict history.

## License

[Apache License 2.0](LICENSE) — same as upstream.
