# UI 面：保留自研 / 暂用上游（2026-11-15 裁决）

处理 ui-egui 那批失败时，按用户口径分两类：**下面的面必须保留本地自研实现**，其余 UI 面暂时回到上游 `origin/main` 的实现。

## 保留自研（不许回退）

| 面 | 实现文件（crates/ui-egui/src） | 对应失败用例 |
|---|---|---|
| 色板 + 色板右键菜单 | `panels/color_panel_ui.rs`、`panels/color_c2.rs` | `color_panel_tests` 4 条 |
| 文档标题 / 文档标签页 | `canvas/tabs.rs` | `canvas::tabs_tests` 2 条、`canvas::tests::spectrum_document_tabs_meet_canvas_without_a_gap` |
| 透明滚动条 | `theme.rs`（`ScrollStyle::thin()` + `floating`，约 11 行） | 无直接失败用例 |
| 画布投影与旋转 | 投影：`canvas/gpu_canvas/shaders.rs`（C3 `canvasShadow*`，5 pt 偏移、随页面旋转）；旋转：`canvas/rotate_view.rs`（新增 57 行） | 无直接失败用例；`display.rs` 与上游零差异 |
| ESC 控制 | `canvas/canvas_tool_menu.rs`、`canvas/paint_mouse.rs`、`input/shortcuts.rs`、`input/shortcut_dispatch.rs` | `canvas::canvas_tool_menu` 3 条、`canvas::paint_mouse` 1 条 |
| C3 画笔管线（笔刷形状/预览/平滑） | `panels/brush_sections.rs`、`panels/brush_panel.rs`、`panels/brushes_tab.rs`、`brush_preview*`、`widgets/fields.rs` | `brush_sections` 6、`brush_panel` 5、`brushes_tab` 4、`brush_preview` 1、`canvas::tests` 3、`options_hit_area` 3、`caps_lock` 3 |
| C3 独立笔刷管线：涂抹 + 颜色双输出 | `crates/paint/src/transport.rs`（`BrushMixSettings{ color_output, outputs_smudge, smudge_enabled }` 两条独立 lane）、`render.rs`、`retouch.rs` | 两条 lane 必须能同时输出，不能退化成单输出 |
| 颜料混合（上游没有） | `crates/paint/src/pigment_material.rs`（新增 509 行） | 上游无此文件，纯自研 |
| 工具预设面板（可并入大面板） | `panels/preset_panels.rs`、`panels/dock.rs` 的 `dock::Group::ToolPresets` | 停靠分组必须保留，回退 dock.rs 会缺 `ToolPresets` 变体 |

C3 相关面**不能直接回退**：上游版本会因 `StepInput` 新字段（`distance`/`speed`/`pressure_released`）、`tip_texture`→`tip_texture_shaped`、`dock::Group::ToolPresets` 等 0.24 API 变化而编译失败，需要按用例逐个补行为。

## 已回退到上游（c05840a1e）

| 文件 | 效果 |
|---|---|
| `panels/options_bar.rs` | brush tooltips 5/5、pencil pressure 5/5、背景橡皮 14/14、字段滚轮 3/3 转绿 |
| `panels/layers.rs` | blend 分组断线、两个滚轮步进用例转绿 |
| `dialogs/brush_preset_dialog.rs` | 回退后仅把 `tip_texture()` 改成本地签名 `tip_texture_shaped()` 一处适配 |

## 数字（本机 macOS + craft-fonts，`RUST_MIN_STACK=33554432`）

| 阶段 | 通过 | 失败 | 其中环境类 |
|---|---|---|---|
| 初始 | 6092 | 107 | 7 |
| 回退 options_bar 后 | 6114 | 88 | 7 |
| 再回退 layers / 预设对话框后 | 6117 | 85 | 7 |

环境类 7 条上游树同样失败，不属分支问题：`layer_filter_ui` 光标、`sampler_menu_tests` 3 条、`font_commit_tests` pattern_0/2/3。
