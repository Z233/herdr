# herdr (z233 fork)

<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  Fork of <a href="https://github.com/ogulcancelik/herdr">herdr</a> — agent multiplexer that lives in your terminal.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/z233/herdr/releases"><img src="https://img.shields.io/github/downloads/z233/herdr/total?labelColor=333333&color=666666" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/z233/herdr/stargazers"><img src="https://img.shields.io/github/stars/z233/herdr?labelColor=333333&color=666666&logo=github" alt="GitHub stars" /></a>
</p>

---

> **Looking for the original README?** Herdr is developed upstream at **[ogulcancelik/herdr](https://github.com/ogulcancelik/herdr)** — install instructions, full documentation, sponsors, and contribution guide all live there.
>
> This README covers **only what this fork adds**.

## What this fork adds

Six features on top of upstream herdr, all configurable and off-by-default where they change existing behavior.

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

### Workspace Switcher

A full-screen overlay workspace switcher that replaces upstream's `prefix+shift+1..9` number-based switching.

- **Fuzzy search** with live preview of the selected workspace's focused pane
- **MRU quick-switch** — hold `ctrl+tab` to cycle through recent workspaces, release to select (like `cmd+tab`)
- **Shift reverses direction**; during hold, `j/k` navigates, `s` searches, `l/h` expands/collapses
- **Agent state dots** next to workspace names
- **Repository names** shown for worktree switcher items, with structured labels (git branch names for grouped child worktrees)
- **Zoxide integration** — search any directory by path and open it as a workspace (see below)
- **Mobile support** — switcher auto-opens in empty state on narrow terminals
- Full keyboard **and** mouse support (hover, click, scroll)

Default binding: `ctrl+tab`. Bind to `prefix+w` for the overlay variant.

### Zoxide Workspace Search

The workspace switcher integrates [zoxide](https://github.com/ajeetdsouza/zoxide) as a search provider. Type a path in the switcher's search box to find and open any directory as a workspace — no need to pre-register it. Loading and error states are shown inline.

### EasyMotion Copy Mode Jumps

Vim EasyMotion-style cursor jumping inside copy mode:

1. Press `s` in copy mode
2. Type two target characters
3. Press the visible label key to jump the cursor to that match

Smart case: lowercase queries are case-insensitive, uppercase makes them case-sensitive. The selection anchor is preserved across jumps. `Esc` or `q` cancels at any point.

Two opt-in keybindings trigger copy mode with an initial action:

```toml
[keys]
copy_mode_easymotion = "prefix+space"   # enter copy mode and immediately start EasyMotion
copy_mode_scroll_up  = "prefix+u"       # enter copy mode and immediately scroll half a page up
```

### IME Input Enhancement

Parses and forwards the kitty keyboard protocol's **associated text** field, so IME composition text (CJK input methods, etc.) reaches panes correctly. Extends `KeyboardEnhancementFlags` to preserve associated-text bits that newer crossterm versions don't expose, while keeping modifier-only press/release events working.

No configuration needed — active whenever the host terminal supports kitty keyboard protocol.

---

## Configuration

All fork-specific options live under the `[keys]` section of `config.toml`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `chord_timeout_ms` | `u64` | `500` | Chord sequence timeout in ms; `0` disables chords |
| `workspace_switcher` | `BindingConfig` | `"ctrl+tab"` | Open workspace switcher / MRU quick-switch |
| `workspace_switcher_backward` | `BindingConfig` | *(auto-derived)* | Reverse cycle key; derived from `workspace_switcher` when unset |
| `open_pane_left` | `BindingConfig` | `"prefix+w+h"` | Open pane to the left |
| `open_pane_down` | `BindingConfig` | `"prefix+w+j"` | Open pane below |
| `open_pane_up` | `BindingConfig` | `"prefix+w+k"` | Open pane above |
| `open_pane_right` | `BindingConfig` | `"prefix+w+l"` | Open pane to the right |
| `copy_mode_easymotion` | `BindingConfig` | *(empty)* | Enter copy mode and immediately start EasyMotion |
| `copy_mode_scroll_up` | `BindingConfig` | *(empty)* | Enter copy mode and immediately scroll half a page up |

`workspace_switcher` supports modifiers beyond `ctrl` — `cmd`, `alt`, and `super` all work as the hold modifier for quick-switch.

---

## Install & Build

Build from source:

```bash
git clone https://github.com/z233/herdr
cd herdr
cargo build --release
```

For prebuilt binaries, install scripts, Homebrew, and all other installation methods, see the [upstream README](https://github.com/ogulcancelik/herdr#install).

## Upstream Sync

This fork tracks upstream releases and merges each new tag (`vX.Y.Z`). Fork-specific code is isolated to minimize merge conflicts. See [`docs/FORK_FEATURES.md`](./docs/FORK_FEATURES.md) for a detailed feature analysis and merge conflict history.

## License

[Apache License 2.0](LICENSE) — same as upstream.
