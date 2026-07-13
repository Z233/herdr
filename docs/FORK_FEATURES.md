# Herdr Fork (z233) — 新增功能与冲突解决记录

z233 fork 在上游 [ogulcancelik/herdr](https://github.com/ogulcancelik/herdr) 基础上新增 6 项自定义功能。本文既记录功能，也定义 fork 与上游的架构边界；后续实现不得再次把 feature-specific 状态或 mutation 直接铺进上游 hot files。

## v0.7.3 后的架构边界

| 能力 | 所属层 | 当前边界 |
|------|--------|----------|
| Prefix chord | 中立 input primitive | Prefix target 统一解析/执行；短绑定与长 chord 重叠时，超时执行短绑定，完成 chord 与超时走同一 gateway。 |
| 四向 split | runtime/API | TUI 只发 `pane.split`；公开请求使用四向 `PaneDirection`，结构树的 `SplitDirection` 仍只表示 canonical right/down。 |
| Workspace Picker / Quick Switch | TUI fork overlay | 不再新增 `Mode` 变体；overlay 先于 core mode 接收 key/paste/mouse，最后单独渲染。选择目标保存 workspace/tab public ID，row index 只用于一次 projection。 |
| MRU | TUI presentation state | 观察既有 `workspace.focused` / `workspace.closed` 事件更新，不修改 `switch_workspace()`。 |
| EasyMotion | fork feature adapter | 状态位于 `ForkFeatureState`，controller 位于 `src/fork_features/easymotion.rs`；`CopyModeState` 不再携带 fork 字段。 |
| IME associated text | raw input primitive | 在 raw framer 中展开为普通字符 key press；下游无 `RawInputEvent::Text`，也绝不转成 Paste。 |

稳定 hook 只有 overlay input/paste/mouse/render、prefix timer expiry 和 runtime mutation adapter。新增 fork 行为时应扩展这些边界，而不是新增 core `Mode`、修改 copy-mode struct，或从 TUI 直接 spawn/focus runtime。

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

Vim EasyMotion 风格：复制模式中按 `s`，输入两个字符，按标签跳转。小写查询不区分大小写，大写区分。跳转保持选择锚点。

新增 `CopyModeInitialAction` 枚举（`EasyMotion` / `ScrollUp`），支持进入复制模式后立即执行初始动作，通过可选键绑定 `copy_mode_easymotion` / `copy_mode_scroll_up` 触发。EasyMotion controller/state 已从 `copy_mode.rs` / `CopyModeState` 移到 fork feature adapter，只通过窄 copy-mode helper 读取可见行和同步 cursor/selection。

### IME 输入法支持增强

解析 kitty keyboard protocol 的 associated text 字段，并在 raw framing 边界展开为无修饰符的 Unicode character key press，使 monolithic、headless 和 Windows client 共用同一语义。Release associated text 被忽略；控制码/非法 codepoint 被丢弃；IME 文本不走 bracketed-paste 路径。

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
| `src/fork_features.rs` | fork aggregate state 与边界说明 |
| `src/fork_features/easymotion.rs` | EasyMotion controller/matching adapter |
| `src/ui/workspace_picker.rs` | Picker/Quick Switch overlay model、input 与 render；使用 stable IDs |
| `src/app/input/copy_mode.rs` | 仅保留启动/转发 EasyMotion 的窄 hook 与中立 copy helpers |
| `src/app/input/navigate.rs` | Prefix target gateway 与 runtime mutation dispatch |
| `src/app/input/overlays.rs` | Picker mouse pre-dispatch hook |
| `src/app/input/mod.rs` | Picker key/paste pre-dispatch hook |
| `src/app/api.rs` | 观察 workspace focus/close event 更新 TUI MRU |
| `src/app/state.rs` | `CopyModeInitialAction`、`pending_chord` 与 `ForkFeatureState` field |
| `src/app/runtime.rs` | Prefix chord deadline 触发统一 expiry gateway |
| `src/config/keybinds.rs` | `BindingTrigger::PrefixSequence`、`BindingRegistry` 扩展 |
| `src/config/model.rs` | 新增配置字段 |
| `src/config.rs` | 新增导出 |
| `src/api/schema/panes.rs` | `pane.split` 使用四向 `PaneDirection` |
| `src/layout.rs` | placement+ratio 单一 split primitive |
| `src/workspace/tab.rs` | 统一 placement+ratio runtime path |
| `src/input/parse.rs` | `parse_kitty_associated_text()` |
| `src/input/model.rs` | `KeyboardEnhancementFlags` 扩展 |
| `src/raw_input.rs` | Associated text 在 raw boundary 展开；无 downstream Text variant |
| `src/ui/panes.rs` | EasyMotion 标签渲染 |
| `src/ui/menus.rs` | EasyMotion 视觉区分 |
| `src/ui/sidebar.rs` | `grouped_child_display_label` 共享 |
