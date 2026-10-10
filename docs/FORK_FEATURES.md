# Herdr Fork (z233) — 功能、修复与合并记录

本文是 fork 差异的维护清单，供后续合并上游时逐项核对。使用说明见根目录 [`README.md`](../README.md)；上游产品文档保持上游版本。

- 最近合入的上游稳定版：v0.9.3（`7b116c05bfda646af39d2524c54e70c751f57ee8`），合并提交 `1d58f1aa`。
- 本次盘点截至 fork 提交：`a9948c18`。这不是“远端上游最新版本”的声明。
- 状态：**保留**表示当前仍需保护；**已替代**表示只保留行为；**已撤销**表示不恢复该实现。
- 提交用于追溯意图，不是待逐个 cherry-pick 的补丁清单。下方历史合并记录不约束当前实现。

## 功能清单（当前均保留）

MRU（Most Recently Used）指最近使用顺序；endpoint 指本地或 SSH 服务器端点。通用 Navigator 是上游能力，下面记录 fork 对它的扩展。

| ID / 能力 | 所属层 | 合并后必须保留的行为 | 关键提交 |
|---|---|---|---|
| F01 多步 prefix chord | input primitive + client controller | prefix 后最多三步，较长的重叠绑定优先；超时与完整 chord 使用同一 action gateway，兼容上游多 prefix。 | `088de543`、`201c8cd7`、`1d58f1aa` |
| F02 四向创建 pane | runtime/API | 支持 left/down/up/right；left/up 插在目标前，ratio 是第一个 child 的比例；保留显式 target 和 caller target。旧 endpoint 未广告 `pane.split.directional` 时不发送 left/up。 | `088de543`、`5dd9a2a7`、`1d58f1aa` |
| F03 MRU hold-to-switch | client shell | 按住可配置修饰键循环，Shift 反向，释放修饰键确认；j/k 导航、h/l 展开/折叠。workspace/tab/pane 目标带 endpoint 身份；MRU 每客户端独立，仅在激活实际呈现后更新。 | `693bca67`、`3f87cb27`、`b8d02dc2`、`5dd9a2a7` |
| F04 switcher 内独立搜索 | client shell | 搜索留在 switcher，按匹配质量、MRU、snapshot 顺序排名，目录候选置后。进入搜索后，本次打开不再释放确认；第一次 Esc 恢复列表、选择、展开和滚动，第二次关闭。直接 Go To 不变。 | `8d79fde7` |
| F05 Zoxide 目录搜索与创建 | endpoint API/runtime | `workspace.search`、`workspace.directory_preview`、`workspace.create` 在所选 endpoint 执行；客户端不读取远端路径。错误和超时可见，响应校验请求、boot 与连接 generation。 | `71833771`、`bc52f25d`、`8688d43d`、`5dd9a2a7` |
| F06 完整 tab 预览与结构化标签 | client shell + 中立读取 API | 宽屏预览整个选中 tab，保留 ANSI 样式；读取使用明确目标且不改变其他客户端焦点或尺寸。worktree 标签显示仓库与分支；render 只绘制解码后的缓存。 | `6a6b7c81`、`9fe1d558`、`f449792e`、`5dd9a2a7` |
| F07 移动端 switcher | client shell | 全屏列表支持输入/粘贴搜索；`s` 是查询文本。tap 保持打开，hold/竖拖选择，右划展开 workspace 的全部 pane，左划折叠，释放确认。workspace 使用跨 endpoint MRU；pane 使用本客户端已呈现的焦点历史，打开期间顺序固定，新 pane 追加、删除项移除。 | `682fac82`、`7ccec3cf`、`ad0069d7`、`55e333c6`、`8d79fde7` |
| F08 移动端 zoom map | client shell | 展示完整 tab 布局、隐藏邻居及其 agent 状态；点击可换焦点。`pane.layout` 使用明确 pane ID；状态按 endpoint + pane ID 关联，不增加 mobile 专属 wire 字段。 | `0bfe5a04`、`a91b936e`、`0b6309e7`、`5dd9a2a7` |
| F09 EasyMotion 与 copy initial action | normal client copy mode | 两字符搜索后按标签跳转，支持 smart case、宽字符及 selection anchor；保留进入 copy mode 后启动 EasyMotion 或上滚半页的入口。 | `e97a050a`、`2b7484ec`、`5dd9a2a7` |
| F10 IME associated text | input primitive | 解析并转发 Kitty keyboard associated text，保留 modifier press/release 语义；共享 TextEditor 在光标处插入，跨读取分片只展开一次。 | `0df99265`、`7a1b143d`、`1d58f1aa` |
| F11 Spaces-only sidebar | client configuration/presentation | `ui.sidebar.agents.visible = false` 隐藏所有 machine group 的 Agents 区并回收几何和命中区域；检测、通知与 runtime agent API 不受影响。 | `2e92522f`、`5dd9a2a7` |
| F12 Kitty RGB/RGBA zlib 输出 | client-to-terminal output | inline 原始像素压缩后小于原始字节数的 90% 才使用标准 `o=z`；否则回退原始输出。PNG、local file 路径、像素和 placement 不变；不承诺上传期间输入响应。 | `ff704290`、`80e3639c` |

## Bug 修复清单（当前均保留）

这里记录问题和回归约束，不把历史实现当成必须保留的代码。测试名是可运行的 filter；fixture 与真实终端测试的证据不同。

| ID / 范围 | 原问题 → 必须保留的结果 | 关键提交 | 回归入口 |
|---|---|---|---|
| B01 F03 输入释放 | Tab release 或单独修饰键可能误确认/干扰 prefix → 只在 hold 修饰键 release 确认，重复 ID 按 endpoint 区分。 | `71a8d58f`、`7fa9c983` | `fork_merge_hold_switcher_qualifies_duplicate_ids_and_selects_only_on_modifier_release` |
| B02 F05 过期响应 | 关闭重开或改 query 后，旧搜索/创建响应可能影响新界面 → generation 不复用；switcher 查询编辑使旧 create 失效，响应不得关闭界面或激活目标。 | `bc52f25d`、`5dd9a2a7`、`2590b96f` | `fork_merge_reopened_navigator_rejects_a_previous_creation_result`；`switcher_search_live_acceptance_corrections` |
| B03 F06 预览刷新 | metadata/surface 刷新暂时清空预览 → 身份未变时保留内容；endpoint、boot、generation 或目标变化时丢弃。 | `72e1b3d3` | `fork_merge_navigator_preview_keeps_remote_content_during_metadata_refresh`；`fork_merge_navigator_preview_discards_retained_content_when_identity_changes` |
| B04 F03/F06 桌面外观 | quick-switch popup 几何退化、预览默认背景与 panel 不一致 → hold 保留 inset 几何，Go To 保留自己的几何，预览保留程序背景并隐藏 pane ID。 | `1357e77f`、`4433352f`、`ec0f6d28` | `fork_merge_quick_switch_restores_desktop_inset_geometry`；`fork_merge_navigator_remote_preview_uses_first_row_and_panel_background` |
| B05 F07 列表顺序 | 移动端顺序退化为 endpoint 分组 → 恢复全局 workspace MRU，未知项稳定回退，pane 顺序在本次打开期间不重排。 | `ad0069d7`、`55e333c6` | `mobile_order_is_flat_global_mru_with_stable_unknown_fallback`；`mobile_panes_recency_is_presented_focus_only_and_order_is_frozen_until_reopen` |
| B06 F07 边缘滚动 | 同一次 hold 中不能反向滚动 → 下滚后可上滚，中部停止，释放仍选择当前项。 | `34e30872` | `mobile_edge_scroll_down_then_up_same_hold_and_exact_release` |
| B07 F04/F07 标签与状态 | 展开 pane 缺少状态，窄屏搜索丢失标签/状态图标 → 状态遵循配置，保留 workspace/tab/pane 标签及已有状态图标。 | `02532b92`、`2f8e75e9` | `mobile_panes_status_icons_follow_config_and_survive_narrow_stale_rows`；`switcher_search_live_acceptance_corrections` |
| B08 F07 手势与失效目标 | 背景客户端 render 取消手势，删除目标后可能误激活 → 无关刷新保留锚点；目标移除或 endpoint 身份变化后取消释放确认。 | `b84fc097`、`0a912163`、`b56fcf2b` | `fork_merge_mobile_hold_and_drag_survives_an_unrelated_snapshot_refresh`；`mobile_panes_deleted_selected_or_anchor_and_endpoint_identity_changes_cancel_release` |
| B09 F07 自动打开 | 正常移动端状态也强制打开列表 → 仅空状态自动打开；按钮默认选择当前 workspace。 | `13de15c5`、`7abf3e73` | `fork_merge_live_navigator_two_clients_preview_release_search_copy_and_mobile` |
| B10 F08 旧命中目标 | zoom map 保留已删除邻居或错误后不刷新 → 移除 pane 立即清除旧 hit，最多每秒刷新一次，错误可重试。 | `0b6309e7`、`5dd9a2a7` | `fork_merge_zoom_map_retires_removed_neighbors_before_refresh`；`fork_merge_zoom_map_retries_and_refreshes_without_a_projection_change` |
| B11 F03/F09 copy mode | copy mode 无法打开 switcher，布局/content 变化留下旧 EasyMotion 标签 → switcher 可打开，copy bounds 稳定；resize/content/session 变化取消旧匹配。 | `78735ff7`、`1aed0024`、`5dd9a2a7` | `fork_merge_easymotion_discards_labels_after_resize_or_content_change`；`fork_merge_live_navigator_two_clients_preview_release_search_copy_and_mobile` |
| B12 多客户端 geometry | 无有效 controller 的 tab 取首个 viewer 尺寸，而不是 foreground viewer → 优先当前仍查看该 tab 的 foreground；保留现有有效 controller；foreground 离开时回退其他 viewer。 | `a9948c18` | `public_editor_tab_focus_uses_foreground_viewer`；`public_focused_tab_create_uses_foreground_viewer`；`geometry_reapply_uses_a_viewer_when_foreground_leaves` |

## 已替代与已撤销

| 状态 | 历史能力/实现 | 当前处理 |
|---|---|---|
| 已替代 | 独立 `workspace_picker` 入口 | `44a95b03` 移除。旧 key 静默忽略；`prefix+w` 仅作为方向 chord leader，timeout/Esc 不打开界面。普通搜索使用 `keys.goto`，hold 使用 `workspace_switcher`。 |
| 已替代 | 服务端 FrozenCopyView / ForkFeatureState、旧 picker/switcher overlay | v0.9.1 合并 `5dd9a2a7` 后由 client shell 和正常 copy mode 实现；保护 F03/F09 行为，不恢复旧模块。 |
| 已撤销 | ACK 驱动的异步图像上传工作线程 | `761058bc` 等实现曾随 v0.9.3 合入，但 `21c44c43` 已撤销。当前仅保留 F12 压缩；不得把“上传不阻塞输入”当成现有能力。 |

## 当前架构边界

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

截至本次盘点，终端 wire protocol 为 22，endpoint generation 1 envelope 保持不变。新增读取能力通过 advertised method list 发现。`pane.read.expected_size`、`pane.layout.panes[].terminal_size` 为可选 JSON 字段；读取返回实际 content revision，尺寸或内容在捕获期间变化时返回明确错误。被动预览不得抢占 foreground、focus 或其他客户端的 geometry。旧客户端的 right/down 请求形状保持不变，公开 `pane.split` API 仍支持四向。上游 v0.9.2 已移除自定义 `pane.graphics` API，图像经标准 Kitty 协议传输。

## 配置与请求约束

配置示例和默认值见 [`README.md` 的 Configuration](../README.md#configuration)，实际字段见 `src/config/model.rs`。合并时额外核对：

- 保留显式禁用、快捷键冲突诊断与派生反向键。`workspace_switcher_backward` 的空字符串/空列表表示禁用，不能自动补回。
- 上游 `prefix` 可以是数组；每个 prefix 进入同一 chord 控制器，全部 prefix 键均为保留键。`chord_timeout_ms = 0` 禁用等待。
- 旧 `quick_switch_workspace` / `quick_switch_workspace_backward` 保留过时配置诊断，不恢复为当前字段。
- 切换尚未完成时，显式本地选择可取消远端目标。boot 或连接 generation 变化后拒绝旧选择。

Zoxide 子进程输出有上限，查询与路径规范化共享截止时间；目录预览最多保留 200 条条目，文件系统后台任务并发有界。进程不可用、非零退出、超时、目录错误和创建错误都保留在客户端状态中。

已替换的目录预览及时移除 pending scope。switcher 搜索编辑 query 时取消旧 create 的客户端接受权限；这不撤销服务器上可能已经完成的 workspace 创建，也不改变直接 Go To 的 create/cancel 行为。

## 当前文件入口

| 范围 | 入口 |
|---|---|
| Navigator/MRU/search | `src/client/shell/navigator.rs`、`src/client/shell/switcher_search.rs`、`src/client/shell/aggregate_navigation.rs`、`src/client/shell/overlay_input.rs` |
| Cached terminal preview | `src/client/shell/navigator_preview.rs`、`src/pane/terminal/ansi_snapshot.rs` |
| Endpoint search provider | `src/app/workspace_search_provider.rs`、`src/app/api/workspace_search.rs` |
| Passive endpoint commands | `src/client/endpoint_commands.rs`、`src/server/headless/endpoint_requests.rs`、`src/server/headless/client_views.rs` |
| Mobile/zoom | `src/client/shell/mobile.rs`、`src/client/shell/mobile_gesture.rs`、`src/client/shell/mobile_panes.rs`、`src/client/shell/zoom_map.rs` |
| Copy/EasyMotion | `src/client/shell/copy_mode.rs`、`src/client/shell/easymotion.rs`、`src/client/shell/surface_patch.rs` |
| Chords/raw input | `src/client/shell/prefix_chord.rs`、`src/input/keybindings.rs`、`src/raw_input.rs` |
| Split semantics | `src/app/api/panes.rs`、`src/layout.rs`、`src/workspace.rs`、`src/workspace/tab.rs` |
| Config/sidebar | `src/config/model.rs`、`src/config/keybinds.rs`、`src/config/io.rs`、`src/client/shell/sidebar.rs`、`src/client/shell/endpoint_sidebar.rs` |
| Kitty 压缩 | `src/client/frame_output.rs`、`src/client/frame_output_tests.rs` |
| Tab geometry controller | `src/server/headless/client_views.rs`、`tests/client_mode/tab_geometry.rs` |

旧的 `src/ui/workspace_picker.rs`、`src/ui/workspace_switcher.rs`、`src/fork_features.rs` 和服务端 fork overlay adapter 已不作为实现入口；不得恢复它们来绕过 client shell。

## 验证路线

### Tests

```ts
Fork characterization + upstream regression cases
  → Isolated clients and local/controlled SSH endpoints
    → Recorded terminal input and API requests
      → JSON assertions and captured terminal frames
```

先运行 `just test-one fork_merge` 与改动范围的 filter，再运行 `just check`。`tests/fork_merge.rs` 为 Unix 上可实际执行的 CLI 黑盒入口，不将 macOS 上零匹配的 CLI target 视为通过。测试文件按范围查找：

| 范围 | Fixture / 黑盒入口 |
|---|---|
| F01/F02/F10 | `src/client/shell/tests/fork_input.rs`；`tests/cli/fork_merge.rs`；`src/raw_input.rs` |
| F03/F04/F05 | `src/client/shell/tests/fork_navigator.rs`、`src/client/shell/tests/switcher_search.rs`；`tests/client_mode/fork_merge.rs` |
| F06 | `src/client/shell/tests/fork_preview.rs`；`tests/client_mode/fork_merge.rs` |
| F07/F08/F11 | `src/client/shell/tests/fork_mobile.rs`、`src/client/shell/tests/mobile_order.rs`、`src/client/shell/tests/mobile_panes.rs`；`tests/client_mode/fork_merge.rs` |
| F09 | `src/client/shell/tests/fork_easymotion.rs`；`tests/client_mode/fork_merge.rs` |
| F12 | `src/client/frame_output_tests.rs`；`tests/client_mode/kitty_zlib.rs`（SSH 测试为 opt-in） |
| B12 | `tests/client_mode/tab_geometry.rs` |

client shell fixture、真实终端 E2E 和跨版本 handoff 是不同证据，不得互相替代。E2E 产物包括终端帧和 JSON；可用 `HERDR_MERGE_EVIDENCE_DIR` 指定 fork E2E 目录，`HERDR_TAB_GEOMETRY_ARTIFACT_DIR` 指定 geometry 目录。真实终端 E2E 额外记录 escape-prefixed 词移动（`AB Z_CD`）与 kitty associated-text（`你好`）的按键证据帧。

F12 使用 `just test-kitty-zlib-ssh <host>` 验证 RGB/RGBA 像素与不可压缩回退；终端流、像素 hash、字节数和计时保存在命令打印的 `/var/tmp/herdr-kitty-zlib-ssh-*` 目录。它测量 client-to-terminal SSH relay，不证明 server-to-client 流量减少或上传时输入可响应。

必须保留的 upstream regression：`foreign_preview_survives_local_updates_and_rejects_stale_enter`、`clicking_local_can_cancel_a_remote_switch_while_local_is_still_displayed`、`navigator_foreign_tab_selection_keeps_the_tab_target`、`api_pane_layout_returns_public_ids_rects_and_splits`、`same_tab_geometry_follows_meaningful_client_activity`、`text_delivery_paths_insert_at_the_cursor`、`keyboard_copy_mode_content_motion_is_endpoint_backed_and_stale_safe`。

渲染相关修改需执行 `just bench-render-scale` 并记录 1/15 pane 对比。protocol 20→22 迁移和 live handoff 要分别验证。最终验证结果记录在合并证据目录中；本节仅列验证要求，不代替执行结果。

## 维护与合并核对

### 新增功能或修复时

1. 在对应清单新增稳定 ID，或更新已有行；写明用户可见行为、关键提交和回归入口。
2. 若能力被撤销或实现被替代，移到对应状态表；保留原提交及替代关系，不把旧实现继续列为有效能力。
3. 更新受影响的文件入口和盘点截止提交；本文不复制上游 changelog，不记录无行为影响的逐次重构。

### 合并上游时

1. 确认目标稳定 tag。结合本文和 `git diff <tag> HEAD` 核对差异；仅凭提交标题不能判断能力是否仍存在。
2. 为每个 F/B ID 标明本次结果：fork 保留、上游等价覆盖、适配后保留或经用户确认撤销。只有回归行为验证通过，才将补丁记为上游覆盖。
3. 发生冲突时，先保护清单行为和 endpoint 兼容边界，再选择实现；已撤销项保持撤销。具体流程见 [merge-herdr-upstream](../.agents/skills/merge-herdr-upstream/SKILL.md)。
4. 执行受影响的回归入口及完整检查，记录失败、跳过和产物路径。更新基线、截止提交和历史合并结果后，才算完成盘点。

## 历史合并记录（非当前架构）

### Merge v0.9.3 (7b116c05, 2026-09-30)

注意：下方 ACK 输出工作线程及对应测试仅描述合并当时的结果；后续 `21c44c43` 已撤销该方案。

冲突文件 (37)：构建/文档 18（`Cargo.toml`、`Cargo.lock`、`justfile`、`skills/herdr/SKILL.md`、`CHANGELOG.md`、`docs/next` 多语言文档），运行时 19（`src/api/schema/panes.rs`、`src/client/mod.rs`、`src/client/state.rs`、`src/client/terminal_geometry.rs`、`src/client/terminal_setup.rs`、`src/client/shell/{actions,aggregate_navigation,composition,graphics,input,overlays,render}.rs`、`src/client/shell/tests/{endpoints,graphics,input}.rs`、`src/config/{keybinds,model}.rs`、`src/kitty_graphics/surface.rs`、`src/layout.rs`、`src/platform/windows.rs`、`src/server/client_commands.rs`）。

非平凡解决：
- **multi-prefix × prefix chord** — 上游 `prefix: BindingConfig` 解析为 `Vec<KeyCombo>` 并保留全部 prefix 键；fork 保留三步 `PrefixSequence`、chord timeout gateway、派生反向键与显式禁用。任一 prefix 进入同一 chord 控制器；chord 任一步与任一 prefix 冲突均拒绝。`workspace_picker` 移除保持不回退。
- **native Kitty × ACK 输出工作线程** — 采纳上游 native Kitty（自定义 `pane.graphics` API 保持删除）、`ComposedFrame`/deferred `GraphicsOutput`、native tempfile banks/fallback/retirement、cell-grid 裁剪与 offscreen 保留；移植进 fork 的 ACK 驱动有界输出管线（`queue_direct_file`、generation 键控 ack、`DirectFile` 四元组），不在输入循环直接写 stdout。`run_client_loop` 参数收敛为 `ClientTerminalLifecycle`。
- **Navigator 分组 × hold/tab 目标** — 采纳上游 workspace→pane 分组、逐词搜索、滚动条与 ←/→ 跳转；保留 fork `ClientNavigatorTarget::Tab`、hold 展开、MRU 仅在呈现后更新、目录搜索与 endpoint+boot+generation stale 校验。修复自动合并静默丢失的 Tab 变体与 `navigator_foreign_tab_selection_keeps_the_tab_target`。
- **layout targeted split × placement** — 上游 `find_pane_mut`/`split_node` 定向叶修改；fork 新增 `split_node_with_placement`：left/up 插在前、ratio 表示第一个 child，focus history 与 rollback 保留。
- **API 方法表** — `pane.clear` 等上游方法与 fork `pane.split.directional`/预览方法并存于 advertised 摘要；`tests/fixtures/endpoint-method-shapes-v1.json` 保持字节不变。
- **client_mode graphics 测试** — 两个驱动已删除自定义 API 的 fork 测试移植为 native 路径（pane 应用经脚本文件写标准 Kitty chunk）：保留输入响应、文本先于末块、有界上传单元、未变化场景不重传、SIGHUP 关停先终止上传等断言；层 API 专属段落按上游 native retirement 与 frame_output_tests 覆盖删除。
- **raw_input** — 干净合并：上游字节成帧层（escape/mouse/host-color 恢复）与 fork 事件展开层（associated-text flat_map、release 抑制）叠加；新增 `kitty_associated_text_split_across_reads_expands_once`。
- **文档/构建** — 冲突文档按 hunk 审计后与上游 v0.9.3 字节一致（fork 文档改动均被上游包含或为旧 base 文本）；`Cargo.toml` 0.9.3 + `crates/ghostty-vt` workspace；justfile 保留 fork `test_install_and_handoff` 与上游全部新 recipe；vendor patch 0006/0007 随上游索引与应用。
- **环境基线（非合并回归）** — `just windows-lint` 在合并前后同样失败（Zig native helper 在 Windows sysroot 下找不到 libSystem）；`native_file_render_scale_profile` 在本机 pristine v0.9.3 同样失败（CoW 保留不可用）。

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
