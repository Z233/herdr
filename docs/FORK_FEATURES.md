# Herdr Fork (z233) — 新增功能与冲突解决记录

z233 fork 在上游 [ogulcancelik/herdr](https://github.com/ogulcancelik/herdr) 基础上新增 6 项自定义功能。上游 remote 为 `origin`，fork remote 为 `z233`，工作分支 `z233`。每次上游发布新 tag 时执行 `git merge tag 'vX.Y.Z'`，手动解决冲突。

## 功能一览

| 功能 | 默认键绑定 | 配置项 | 说明 |
|------|-----------|--------|------|
| Prefix Chord | — | `chord_timeout_ms` | 多步 prefix 键序列（如 `prefix+w+h`） |
| 方向性面板打开 | `prefix+w+h/j/k/l` | `open_pane_*` | 一步指定方向打开新面板 |
| Workspace Picker | `prefix+w` / `prefix+w+w` | `workspace_picker` | 模糊搜索 + 预览的工作区选择器 |
| Quick Switch | `ctrl+tab` | `quick_switch_workspace[_backward]` | MRU 快速工作区切换 |
| EasyMotion 跳转 | 复制模式 `s` | `copy_mode_easymotion/scroll_up` | Vim 风格光标跳转 |
| IME 增强 | — | — | kitty associated text 解析转发 |

## 功能详解

### Prefix Chord 序列键绑定

上游仅支持单步 prefix 键（如 `prefix+h`）。Fork 扩展为最多 3 步的 chord 序列，支持重叠绑定的更长匹配优先。

引入的类型（后续合并冲突高频点）：
- `BindingTrigger::PrefixSequence` — 新增枚举变体
- `BindingRegistry.prefix_sequences` — 新增 HashMap，值类型经历 `String` → `RegisteredBinding` 升级
- `AppState.pending_chord` / `chord_deadline` — 跟踪进行中的序列

### 方向性面板打开

利用 chord 序列实现 `prefix+w+h/j/k/l` 一步打开新面板到指定方向。

引入 **placement** 概念，贯穿 layout 和 workspace 两层：

- `SplitPlacement` 枚举（`After` / `Before`）— 控制分割位置
- `TileLayout::split_focused_with_placement()` — 布局层
- `Tab` 的 `split_focused` 重构为 `SplitMode` 枚举（`Default` / `WithRatio` / `WithPlacement`）— 面板层

> **placement** 是后续合并冲突的高频根因：上游每次修改 `split_at()` 或 `split_focused()` 签名时，fork 的 `SplitPlacement` 参数和 `SplitMode` 枚举都需要同步适配。四次合并中有三次涉及此模式。

### Workspace Picker 工作区选择器

全屏 overlay 工作区选择器，替代上游 `prefix+shift+1..9` 的数字切换：

- 模糊搜索 + 焦点面板预览
- 键盘（`j/k`、`Enter`、`Esc`）和鼠标（悬停、点击、滚轮）完整支持
- Agent 状态点：工作区名称旁显示 agent 运行状态
- 名称显示与侧边栏对齐：分组子工作树使用 git 分支名，共享 `grouped_child_display_label()` 逻辑（fork 将其改为 `pub(crate)` 供 picker 模块调用）

独立模块 `src/ui/workspace_picker.rs`（~1900 行）。

### Quick Switch 快速工作区切换

类似 `cmd+tab` 的 MRU 工作区切换器：

- 默认 `ctrl+tab`，可配置为任意修饰键（`cmd`、`alt`、`super`）
- **Release-to-select**：修饰键释放时确认选择
- Shift 反向循环；hold 期间支持 `j/k` 导航、`s` 搜索、`l/h` 展开/折叠
- `quick_switch_workspace_backward` 未设置时从 `quick_switch_workspace` 自动推导

### EasyMotion 复制模式跳转

Vim EasyMotion 风格：复制模式中按 `s`，输入两个字符，按标签跳转。小写查询不区分大小写，大写区分。跳转保持选择锚点。

新增 `CopyModeInitialAction` 枚举（`EasyMotion` / `ScrollUp`），支持进入复制模式后立即执行初始动作，通过可选键绑定 `copy_mode_easymotion` / `copy_mode_scroll_up` 触发。EasyMotion 模式有独立视觉样式。

### IME 输入法支持增强

解析和转发 kitty keyboard protocol 的 associated text 字段，使 IME 组合文本正确输入到面板。

扩展 `KeyboardEnhancementFlags`：使用 `from_bits_retain()` 保留 crossterm 0.29 未暴露的 associated-text 位（`0b0001_0000`），同时启用 `REPORT_EVENT_TYPES`、`REPORT_ALTERNATE_KEYS`、`REPORT_ALL_KEYS_AS_ESCAPE_CODES`，在启用 IME 文本报告的同时保留仅修饰键的 press/release 事件。

## 合并冲突记录

### Merge master (e8c23ef, 2026-06-13)

冲突文件 (11)：`docs/next/CHANGELOG.md`、`src/app/actions.rs`、`src/app/input/mod.rs`、`src/app/input/modal.rs`、`src/app/input/navigate.rs`、`src/config/keybinds.rs`、`src/config/model.rs`、`src/input/mod.rs`、`src/input/model.rs`、`src/layout.rs`、`src/workspace/tab.rs`

非平凡解决：
- **keybinds.rs** — fork 的 `prefix_sequences` HashMap 值类型从 `String` 升级为上游新增的 `RegisteredBinding`，与 `BindingSource` 类型系统合并
- **layout.rs / tab.rs / input/mod.rs** — 上游为 `split_focused()` 新增 `Vec::new()` 参数，fork 需在 `SplitPlacement::After` 和 `Before` 两个分支中分别添加（**placement** 适配）

### Merge v0.6.10 (7afa5f3, 2026-06-13)

无冲突。

### Merge v0.7.0 (32cc447, 2026-06-16)

冲突文件 (5)：`docs/next/website/src/content/docs/concepts.mdx`、`src/app/input/mod.rs`、`src/layout.rs`、`src/workspace.rs`、`src/workspace/tab.rs`

非平凡解决：
- **input/mod.rs / layout.rs / tab.rs** — 与 master merge 相同的 **placement** 适配模式：上游再次修改 split 签名，fork 的 `SplitPlacement` 参数和 `SplitMode` 枚举需同步适配

### Merge v0.7.1 (1709705, 2026-06-25)

冲突文件 (13)：`AGENTS.md`、`src/app/input/mod.rs`、`src/app/mod.rs`、`src/app/state.rs`、`src/client/input.rs`、`src/config.rs`、`src/config/keybinds.rs`、`src/config/model.rs`、`src/layout.rs`、`src/raw_input.rs`、`src/ui/panes.rs`、`src/ui/sidebar.rs`、`src/workspace/tab.rs`

非平凡解决：
- **keybinds.rs** — fork 的 `combo()` 调用适配为上游新增的 `single_combo()` 方法（`binding.trigger.combo().0` → `binding.trigger.single_combo().is_some_and(|combo| ...)`）；`prefix_sequences` 值类型再次升级为 `RegisteredBinding`
- **state.rs** — 上游将 `AgentPanelScope` 重命名为 `AgentPanelSort`（`CurrentWorkspace`/`AllWorkspaces` → `Spaces`/`Priority`），fork 保留 `CopyModeInitialAction` 并采纳重命名
- **config.rs / model.rs** — 同步 `AgentPanelScopeConfig` → `AgentPanelSortConfig` 重命名
- **sidebar.rs** — fork 将 `grouped_child_display_label` 改为 `pub(crate)`，上游修改了同名函数签名；保留 fork 可见性，采纳上游签名
- **layout.rs / tab.rs** — **placement** 适配（第三次）
- **input/mod.rs** — fork 和上游都定义了 `#[cfg(test)] fn mouse()` helper，保留 fork 位置（文件顶部），删除上游末尾重复定义

## 配置项

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `chord_timeout_ms` | `u64` | `500` | Chord 序列超时（ms），`0` 禁用 |
| `workspace_picker` | `BindingConfig` | `["prefix+w", "prefix+w+w"]` | 打开工作区选择器 |
| `quick_switch_workspace` | `BindingConfig` | `"ctrl+tab"` | MRU 快速切换 |
| `quick_switch_workspace_backward` | `BindingConfig` | （自动推导） | 反向循环键 |
| `copy_mode_easymotion` | `BindingConfig` | （未设置） | 进入复制模式即启动 EasyMotion |
| `copy_mode_scroll_up` | `BindingConfig` | （未设置） | 进入复制模式即滚动半页 |
| `open_pane_left` | `ActionKeybinds` | `"prefix+w+h"` | 向左打开面板 |
| `open_pane_down` | `ActionKeybinds` | `"prefix+w+j"` | 向下打开面板 |
| `open_pane_up` | `ActionKeybinds` | `"prefix+w+k"` | 向上打开面板 |
| `open_pane_right` | `ActionKeybinds` | `"prefix+w+l"` | 向右打开面板 |

## 文件清单

| 文件 | Fork 变更 |
|------|----------|
| `src/ui/workspace_picker.rs` | Workspace picker 完整模块（~1900 行） |
| `src/app/input/copy_mode.rs` | EasyMotion 子模式 |
| `src/app/input/modal.rs` | Picker 键盘/鼠标输入 |
| `src/app/input/navigate.rs` | 方向性面板打开导航 |
| `src/app/input/overlays.rs` | Picker overlay 渲染 |
| `src/app/input/mod.rs` | Chord 处理、模式分发 |
| `src/app/actions.rs` | Picker action 分发 |
| `src/app/state.rs` | `WorkspacePickerState`、`CopyModeInitialAction`、`pending_chord` |
| `src/app/mod.rs` | Chord 清理、runtime 集成 |
| `src/config/keybinds.rs` | `BindingTrigger::PrefixSequence`、`BindingRegistry` 扩展 |
| `src/config/model.rs` | 新增配置字段 |
| `src/config.rs` | 新增导出 |
| `src/layout.rs` | `SplitPlacement`、`split_focused_with_placement()` |
| `src/workspace/tab.rs` | `SplitMode` 枚举 |
| `src/input/parse.rs` | `parse_kitty_associated_text()` |
| `src/input/model.rs` | `KeyboardEnhancementFlags` 扩展 |
| `src/raw_input.rs` | Associated text 集成 |
| `src/ui/panes.rs` | EasyMotion 标签渲染 |
| `src/ui/menus.rs` | EasyMotion 视觉区分 |
| `src/ui/sidebar.rs` | `grouped_child_display_label` 共享 |
