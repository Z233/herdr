# Herdr Fork (z233) — 功能与合并边界

本次合并固定采用上游 v0.9.1（`065ef9d6a531c49fb8bee7e818ef837065b21ee9`）。用户说明集中在根目录 `README.md`；本文记录维护边界、实现入口和验证路线。下方历史记录只说明当时的实现，不是当前架构约束。

## v0.9.1 架构边界

| 能力 | 所属层 | 当前实现与约束 |
|---|---|---|
| Navigator 与 MRU | client shell | 一个 Navigator 同时支持搜索与按住修饰键切换；目标包含 endpoint 与明确的 workspace/tab/pane 类型。MRU 每客户端独立，仅在激活结果实际呈现后更新。 |
| 普通入口与 hold 入口 | client configuration/input | 保留 `workspace_picker`、`workspace_switcher` 和 `workspace_switcher_backward`。保留显式禁用、快捷键冲突诊断与派生反向键。 |
| 目录搜索与创建 | endpoint API/runtime | `workspace.search`、`workspace.directory_preview` 与 `workspace.create` 在所选 endpoint 执行；客户端不读取远端路径。响应绑定请求、endpoint、boot 与连接 generation。 |
| 终端预览 | client shell + 中立读取 API | 宽屏预览整个选中 tab；已有本地 surface 可直接复用，其他目标通过显式 `pane.layout`、`pane.read` 读取。ANSI 解码在响应处理阶段完成，render 只绘制缓存。 |
| Mobile gesture | client shell | tap、hold/drag、展开/折叠均为客户端状态；背景刷新不改变手势锚点，目标移除或 endpoint 更换时取消。 |
| Zoom map | client shell | `pane.layout` 返回完整布局及独立 zoom 标记；状态按 endpoint + pane ID 关联。不新增 `ZoomMapSlot` wire 字段。 |
| EasyMotion | normal client copy mode | 使用 `ClientCopyModeState` 的 selection、surface、content revision 和 geometry 检查，保留两个 initial action；不再使用服务端 FrozenCopyView/ForkFeatureState。 |
| Prefix chord/IME | 中立 input primitive + client controller | 最多三步 prefix chord；超时短绑定和完整 chord 走同一 action gateway。modifier release 与 kitty associated text 保持原语义，共享 TextEditor 在光标处插入。 |
| 四向 split | runtime/API | `pane.split` 使用四向 `PaneDirection`；结构树仍用 canonical right/down。left/up 插在前，ratio 表示第一个 child 的比例；显式 target 与 caller target 均保留。 |
| Sidebar visibility | client configuration/presentation | `ui.sidebar.agents.visible = false` 对全部 machine group 生效，并回收几何与命中区域；不影响检测、通知与 runtime agent API。 |

### Production

```ts
Host input
  → Client shell: Navigator / Mobile / Copy mode
    → Endpoint-qualified target and revision validation
      → focus_or_activate / advertised endpoint API
        → Endpoint runtime
          → Snapshots / pane.layout / pane.read / surface updates
            → Client projection and cached rendering
```

终端 wire protocol 随上游从 20 升至 22；endpoint generation 1 envelope 保持不变。新增读取能力通过 advertised method list 发现。`pane.read.expected_size`、`pane.layout.panes[].terminal_size` 为可选 JSON 字段；读取返回实际 content revision，尺寸或内容在捕获期间变化时返回明确错误。被动预览不得抢占 foreground、focus 或其他客户端的 geometry。客户端仅在 endpoint 广告 `pane.split.directional` 后发送 left/up；该中立 API 与 `pane.split` 共用四向实现。旧客户端的 right/down 请求形状保持不变，公开 `pane.split` API 仍支持四向。

## 配置与行为

| 配置项 | 默认值 | 行为 |
|---|---|---|
| `keys.workspace_picker` | `"prefix+w"` | 普通 Navigator 搜索；与更长 chord 重叠时等待 timeout。 |
| `keys.workspace_switcher` | `"ctrl+tab"` | MRU hold-to-switch，释放打开时使用的修饰键后确认。 |
| `keys.workspace_switcher_backward` | 未设置时自动推导 | 显式空字符串或空列表禁用反向键，不再自动补回。 |
| `keys.chord_timeout_ms` | `500` | 最多三步 prefix chord；`0` 禁用等待。 |
| `keys.open_pane_left/down/up/right` | `prefix+w+h/j/k/l` | 指定方向创建 pane。 |
| `keys.copy_mode_easymotion` | 未设置 | 进入 copy mode 后启动两字符匹配。 |
| `keys.copy_mode_scroll_up` | 未设置 | 进入 copy mode 后上滚半页。 |
| `ui.sidebar.agents.visible` | `true` | 显示所有机器的 agent 区域。 |

旧的 `quick_switch_workspace` / `quick_switch_workspace_backward` 名称继续作为过时配置诊断，不是当前配置字段。`workspace_picker` 已恢复，旧测试中拒绝该字段的断言已明确替换为与 switcher 共存的验证。

Hold 模式支持修饰键+Tab 循环、Shift 反向、j/k 导航、h/l 折叠/展开、s 或 / 转为搜索，Esc 取消。切换尚未完成时显式本地选择可取消远端目标。目标的 boot 或连接 generation 变化后不得接受旧选择。

Zoxide 子进程输出有上限，查询与路径规范化共享截止时间；目录预览最多保留 200 条条目，文件系统后台任务并发有界。进程不可用、非零退出、超时、目录错误和创建错误都保留在客户端状态中。

Navigator 的查询 generation 在重开后不复用；旧创建响应不得改变新 overlay，已替换的目录预览及时移除 pending scope。Zoom map 每秒最多刷新一次，错误后可重试；pane 集合、zoom 或焦点变化会清除旧布局和命中目标，agent 状态仍从当前 endpoint snapshot 读取。

## 当前文件入口

| 范围 | 入口 |
|---|---|
| Navigator/MRU/search | `src/client/shell/navigator.rs`、`aggregate_navigation.rs`、`overlay_input.rs` |
| Cached terminal preview | `src/client/shell/navigator_preview.rs`、`src/pane/terminal/ansi_snapshot.rs` |
| Endpoint search provider | `src/app/workspace_search_provider.rs`、`src/app/api/workspace_search.rs` |
| Passive endpoint commands | `src/client/endpoint_commands.rs`、`src/server/headless/endpoint_requests.rs`、`client_views.rs` |
| Mobile/zoom | `src/client/shell/mobile.rs`、`mobile_gesture.rs`、`zoom_map.rs` |
| Copy/EasyMotion | `src/client/shell/copy_mode.rs`、`easymotion.rs`、`surface_patch.rs` |
| Chords/raw input | `src/client/shell/prefix_chord.rs`、`src/input/keybindings.rs`、`src/raw_input.rs` |
| Split semantics | `src/app/api/panes.rs`、`src/layout.rs`、`src/workspace.rs`、`src/workspace/tab.rs` |
| Config/sidebar | `src/config/{model,keybinds,io}.rs`、`src/client/shell/sidebar.rs`、`endpoint_sidebar.rs` |

旧的 `src/ui/workspace_picker.rs`、`src/ui/workspace_switcher.rs`、`src/fork_features.rs` 和服务端 fork overlay adapter 已不作为实现入口；不得恢复它们来绕过 client shell。

## 验证路线

### Tests

```ts
Fork characterization + upstream regression cases
  → Isolated clients and local/controlled SSH endpoints
    → Recorded terminal input and API requests
      → JSON assertions and captured terminal frames
```

先运行 `just test-one fork_merge` 与相应 feature filter，再运行 `just check`。`tests/fork_merge.rs` 为 Unix 上可实际执行的 CLI 黑盒入口，不将 macOS 上零匹配的 CLI target 视为通过。client shell 的 fixture tests 与真实终端 E2E、跨版本 handoff 是不同证据，不得互相替代。

必须保留的 upstream regression：`foreign_preview_survives_local_updates_and_rejects_stale_enter`、`clicking_local_can_cancel_a_remote_switch_while_local_is_still_displayed`、`navigator_foreign_tab_selection_keeps_the_tab_target`、`api_pane_layout_returns_public_ids_rects_and_splits`、`same_tab_geometry_follows_meaningful_client_activity`、`text_delivery_paths_insert_at_the_cursor`、`keyboard_copy_mode_content_motion_is_endpoint_backed_and_stale_safe`。

渲染相关修改需执行 `just bench-render-scale` 并记录 1/15 pane 对比。protocol 20→22 迁移和 live handoff 要分别验证。最终验证结果记录在合并证据目录中；本节仅列验证要求，不代替执行结果。

## 历史合并记录（非当前架构）


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
