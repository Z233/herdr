# Herdr Fork (z233) — 新增功能与冲突解决记录

z233 fork 在上游 [ogulcancelik/herdr](https://github.com/ogulcancelik/herdr) 基础上新增 6 项自定义功能。本文既记录功能，也定义 fork 与上游的架构边界；后续实现不得再次把 feature-specific 状态或 mutation 直接铺进上游 hot files。

## v0.7.3 后的架构边界

| 能力 | 所属层 | 当前边界 |
|------|--------|----------|
| Prefix chord | 中立 input primitive | Prefix target 统一解析/执行；短绑定与长 chord 重叠时，超时执行短绑定，完成 chord 与超时走同一 gateway。 |
| 四向 split | runtime/API | TUI 只发 `pane.split`；公开请求使用四向 `PaneDirection`，结构树的 `SplitDirection` 仍只表示 canonical right/down。 |
| Workspace Picker / Quick Switch | TUI fork overlay | 不再新增 `Mode` 变体；overlay 先于 core mode 接收 key/paste/mouse，最后单独渲染。选择目标保存 workspace/tab public ID，row index 只用于一次 projection。 |
| MRU | TUI presentation state | 观察既有 `workspace.focused` / `workspace.closed` 事件更新，不修改 `switch_workspace()`。 |
| EasyMotion | fork feature adapter | EasyMotion 与 Frozen Copy View 会话状态位于 `ForkFeatureState`，controller 位于 `src/fork_features/easymotion.rs`；`CopyModeState` 不再携带 fork 字段。终端层只提供中立的只读可见 cell 捕获。 |
| IME associated text | raw input primitive | host 侧 associated text 在 raw framer 中展开为普通字符 key press，绝不转成 Paste，fork framer 不产生 `Text`。v0.8.0 起上游为 client wire 路径引入 `RawInputEvent::Text`/`TextCommit`（Windows VTI IME 等），与 fork 的 host 展开并存；Text commit 不经 picker overlay，而 fork 展开的 key event 会自然流入 picker 搜索。 |

稳定 hook 只有 overlay input/paste/mouse/render、prefix timer expiry 和 runtime mutation adapter。新增 fork 行为时应扩展这些边界，而不是新增 core `Mode`、修改 copy-mode struct，或从 TUI 直接 spawn/focus runtime。

v0.8.0 后，键路由基于上游 `InputLeaseTable`（`suppressed_repeat_keys` 已删除）：picker 激活时 `terminal_input_context()` 不返回 Pane context，headless 键因此流入 picker pre-dispatch；quick-switch 的 release-accept 挂在 monolithic 与 headless 两条 Release 分支的 lease 转发之前。host 键盘增强标志保持 fork superset（`REPORT_ALL_KEYS_AS_ESCAPE_CODES` + associated-text bit，`from_bits_retain`），上游 terminal_modes 的动态 report-all 开关在此 superset 下退化为无害的重复 push。

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

上游仅支持单步 prefix 键（如 `prefix+h`）。Fork 扩展为最多 3 步的 chord 序列，支持重叠绑定的更长匹配优先。若短绑定同时是长绑定的前缀，等待 `chord_timeout_ms` 后执行短绑定；`0` 禁用多步等待并立即执行短绑定。

引入的类型（后续合并冲突高频点）：
- `BindingTrigger::PrefixSequence` — 新增枚举变体
- `BindingRegistry.prefix_sequences` — 新增 HashMap，值类型经历 `String` → `RegisteredBinding` 升级
- `AppState.pending_chord` / `chord_deadline` — 跟踪进行中的序列

### 方向性面板打开

利用 chord 序列实现 `prefix+w+h/j/k/l` 一步打开新面板到指定方向。

placement 现在由 `pane.split` 请求统一承载：Left/Up 映射为新 child 在前，Right/Down 映射为新 child 在后；ratio 始终描述 left/top（first child）的份额。

- `SplitPlacement` 枚举（`After` / `Before`）— 控制分割位置
- `TileLayout::split_focused_with_placement_and_ratio()` — 布局层唯一的组合 primitive
- `Tab` / `Workspace` 通过 `WithPlacementAndRatio` 传递同一组 placement + ratio 语义

> 历史上的 parallel `SplitMode`/placement 方法是高频冲突根因。v0.7.3 后，方向性 TUI action 不再直接调用 `AppState` split helper，而统一经 runtime API；结构层仅保留一个可组合的 placement+ratio 路径。

### Workspace Picker 工作区选择器

全屏 overlay 工作区选择器，替代上游 `prefix+shift+1..9` 的数字切换：

- 模糊搜索 + 焦点面板预览
- 键盘（`j/k`、`Enter`、`Esc`）和鼠标（悬停、点击、滚轮）完整支持
- Agent 状态点：工作区名称旁显示 agent 运行状态
- 名称显示与侧边栏对齐：分组子工作树使用 git 分支名，共享 `grouped_child_display_label()` 逻辑（fork 将其改为 `pub(crate)` 供 picker 模块调用）

实现仍集中在 `src/ui/workspace_picker.rs`，但它是独立 overlay session，不再是 core `Mode`。目标使用稳定 public ID，避免 workspace/tab reorder 后误选。

### Quick Switch 快速工作区切换

类似 `cmd+tab` 的 MRU 工作区切换器：

- 默认 `ctrl+tab`，可配置为任意修饰键（`cmd`、`alt`、`super`）
- **Release-to-select**：修饰键释放时确认选择
- Shift 反向循环；hold 期间支持 `j/k` 导航、`s` 搜索、`l/h` 展开/折叠
- `quick_switch_workspace_backward` 未设置时从 `quick_switch_workspace` 自动推导

### EasyMotion 复制模式跳转

Vim EasyMotion 风格：复制模式中按 `s`，输入两个字符，按标签跳转。小写查询不区分大小写，大写区分。首次启动会深拷贝当前可见字符 cell 网格为 Frozen Copy View；同一复制模式会话中的匹配、移动、搜索、选择、复制和渲染都复用该视图，不暂停 PTY，也不捕获 scrollback 或 Kitty 图片。跳转保持位于视图内的选择锚点；面板尺寸变化会释放视图并退回普通复制模式。

新增 `CopyModeInitialAction` 枚举（`EasyMotion` / `ScrollUp`），支持进入复制模式后立即执行初始动作，通过可选键绑定 `copy_mode_easymotion` / `copy_mode_scroll_up` 触发。EasyMotion controller/state 已从 `copy_mode.rs` / `CopyModeState` 移到 fork feature adapter，只通过窄 copy-mode helper 读取可见行和同步 cursor/selection。

### IME 输入法支持增强

解析 kitty keyboard protocol 的 associated text 字段，并在 raw framing 边界展开为无修饰符的 Unicode character key press，使 monolithic、headless 和 Windows client 共用同一语义。Release associated text 被忽略；控制码/非法 codepoint 被丢弃；IME 文本不走 bracketed-paste 路径。

扩展 `KeyboardEnhancementFlags`：使用 `from_bits_retain()` 保留 crossterm 0.29 未暴露的 associated-text 位（`0b0001_0000`），同时启用 `REPORT_EVENT_TYPES`、`REPORT_ALTERNATE_KEYS`、`REPORT_ALL_KEYS_AS_ESCAPE_CODES`，在启用 IME 文本报告的同时保留仅修饰键的 press/release 事件。

## 合并冲突记录

### Merge v0.8.0 (346411f, 2026-08-03)

冲突文件 (11)：`src/app/api/panes.rs`、`src/app/input/copy_mode.rs`、`src/app/input/mod.rs`、`src/app/input/navigate.rs`、`src/app/mod.rs`、`src/app/runtime.rs`、`src/config/keybinds.rs`、`src/input/model.rs`、`src/input/parse.rs`、`src/raw_input.rs`、`src/workspace.rs`

非平凡解决：
- **input/model.rs** — 两侧同名函数 `ime_compatible_keyboard_enhancement_flags`：上游保持标志子集并新增 `KITTY_FLAG_REPORT_ALL_KEYS` 常量；fork 保留 superset（`REPORT_ALL_KEYS_AS_ESCAPE_CODES` + associated-text bit）与 fork 侧测试，同时采纳上游常量。`TerminalKey` 因上游新增 `generated_text`/`source` 字段不再 `Copy`。
- **input/parse.rs** — 按上游将 0x1f 解码为 Ctrl+_，fork legacy 矩阵断言同步为 `'_'`；fork associated-text 测试与上游 non-US shifted 测试并集保留。
- **raw_input.rs** — 上游 `RawInputEvent::Text(TextCommit)`（仅 wire TextCommit 构造，framer 不产生）与 fork framer associated-text 展开并存；`events_from_chunks` 保留 fork 的 flat_map 多事件结构，Esc 与常规键按上游携带 `vt_bytes`/`text_commit`。
- **config/keybinds.rs** — `combo()` → `single_combo()` 适配（`matched_index` 采用上游 normalized-expected 实现 #1876，PrefixSequence 触发经 `single_combo()` 自然失配）；`matches_terminal_key`/`matches_prefix_key`/`matched_index` 统一为上游 by-ref 签名。
- **app/input/navigate.rs** — fork 保留 `pending_chord.is_none()` 门控（chord 未决时 prefix 键不直通）；modifier-only guard 采用上游 `matches!(key.code, KeyCode::Modifier(_))` 形式；`TerminalKey` 非 Copy 后 chord 调用点改为 clone。
- **app/input/mod.rs / app/runtime.rs / app/mod.rs** — 上游 `InputLeaseTable` 键生命周期路由取代 `suppressed_repeat_keys`；fork 删除 `handle_raw_key_event`，picker key/paste pre-dispatch 重置在上游 `handle_key -> Option<TerminalInputTarget>` 之上，release-accept 挂在两条 Release 分支的 lease 转发之前；`terminal_input_context()` 增加 picker 感知；fork deadline 测试适配删除的 `next_animation_tick`。
- **app/api/panes.rs** — 保留四向 `PaneDirection`→`(Direction, SplitPlacement)` 映射与 `split_pane_with_placement_and_ratio` 单一调用；采纳上游 `launch_cwd_*` 重命名并接入 `host_terminal_appearance`。
- **workspace.rs / workspace/tab.rs** — placement 适配（第四次）：fork 的 placement split 方法全部接入上游 `host_terminal_appearance` 参数；上游保留的无调用方 `Workspace::split_focused`（test-only）与纯 `split_pane` 按 fork 设计移除。
- **terminal_modes.rs** — 上游 `host_keyboard_report_all_only_changes_the_current_herdr_stack_entry` 期望子集标志（`\x1b[=15u\x1b[=7u`）；fork superset 下两次 push 均为 `\x1b[=31u`（动态开关幂等），按 fork 语义适配期望。
- **新增测试** — `src/app/api.rs::workspace_reordered_event_does_not_churn_client_mru`：上游新增 `workspace.reordered` 事件不改变 MRU recency，close 仍移除条目。

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
| `src/fork_features.rs` | fork aggregate state 与边界说明 |
| `src/fork_features/easymotion.rs` | EasyMotion controller/matching adapter |
| `src/ui/workspace_picker.rs` | Picker/Quick Switch overlay model、input 与 render；使用 stable IDs |
| `src/app/input/copy_mode.rs` | 仅保留启动/转发 EasyMotion 的窄 hook 与中立 copy helpers |
| `src/app/input/navigate.rs` | Prefix target gateway 与 runtime mutation dispatch |
| `src/app/input/overlays.rs` | Picker mouse pre-dispatch hook |
| `src/app/input/mod.rs` | Picker key/paste pre-dispatch hook |
| `src/app/api.rs` | 观察 workspace focus/close event 更新 TUI MRU（忽略 reordered） |
| `src/app/mod.rs` | `chord_deadline` 字段与初始化、headless picker-aware 键/粘贴路由、quick-switch release 路由 |
| `src/app/api/panes.rs` | `pane.split` handler 四向 `PaneDirection`→placement 映射 |
| `src/app/state.rs` | `CopyModeInitialAction`、`pending_chord` 与 `ForkFeatureState` field |
| `src/app/runtime.rs` | Prefix chord deadline 触发统一 expiry gateway |
| `src/config/keybinds.rs` | `BindingTrigger::PrefixSequence`、`BindingRegistry` 扩展 |
| `src/config/model.rs` | 新增配置字段 |
| `src/config.rs` | 新增导出 |
| `src/api/schema/panes.rs` | `pane.split` 使用四向 `PaneDirection` |
| `src/layout.rs` | placement+ratio 单一 split primitive |
| `src/terminal_modes.rs` | host 键盘 report-all 动态开关在 fork superset 下幂等（测试期望按 fork 语义适配） |
| `src/workspace.rs` | split entry points（`split_focused_command`、`split_pane_with_placement_and_ratio`、`split_pane_with_runtime` placement+appearance） |
| `src/workspace/tab.rs` | 统一 placement+ratio runtime path |
| `src/input/parse.rs` | `parse_kitty_associated_text()` |
| `src/input/model.rs` | `KeyboardEnhancementFlags` 扩展 |
| `src/raw_input.rs` | Associated text 在 raw boundary 展开；无 downstream Text variant |
| `src/ui/panes.rs` | EasyMotion 标签渲染 |
| `src/ui/menus.rs` | EasyMotion 视觉区分 |
| `src/ui/sidebar.rs` | `grouped_child_display_label` 共享 |
