# 三方差分清单：上游新版 vs 你的 0.23.6

本文件记录合并前做的完整对比，供后续每次跟随上游时复核。

## 0. 三个基线

| 代号 | 内容 | 来源 |
|---|---|---|
| `base` | 上游原始提交 `2d73f62`（2026-10-19），workspace 0.10.0 | storytold/photocraft |
| `ours` | 你的版本 `deoR/liquify-on-023`（0.23.6，HEAD `d0deb14`） | 本地 `code/` |
| `new` | 上游当前 `main`（2026-11-09 快照，workspace 0.20.1） | 本次提供的源码 |

统计：

| 比较 | 新增文件 | 删除文件 | 修改文件 |
|---|---|---|---|
| base → ours | 45 | 4 | 166 |
| base → new | 1955 | 685 | 614 |
| new → ours | 728 | 1957 | 622 |

三方直接 `git merge` 会产生 **126 个冲突文件、250 个冲突块**。原因不是你的改动多，而是上游对 `paint` 渲染、`ui-egui` 目录结构、引擎命令模块做了大规模重构（`ui-egui` 被拆成 `panels/`、`dialogs/`、`tools/`、`input/`、`canvas/`）。

## 1. 上游新版做了什么（跳过合并会丢掉的）

### 1.1 目录重构（影响所有移植落点）

| 旧路径（base/ours） | 新路径（new） |
|---|---|
| `ui-egui/src/panels.rs`（6695 行） | `ui-egui/src/panels.rs` + `panels/*.rs`（2187 行） |
| `ui-egui/src/liquify_ui.rs` | `ui-egui/src/dialogs/liquify_ui.rs` |
| `ui-egui/src/stylus.rs` | `ui-egui/src/input/stylus.rs` |
| `ui-egui/src/prefs_ui.rs` | `ui-egui/src/prefs/prefs_ui/` |
| `ui-egui/src/brush_panel.rs` | `ui-egui/src/panels/brush_panel.rs` |
| `engine/src/distort_cmds.rs` | `engine/src/cmd/distort_cmds.rs` |
| `engine/src/brush_cmds.rs` | `engine/src/cmd/brush_cmds.rs` |

### 1.2 上游新功能（不能丢）

- 笔刷：`behind` 绘制模式、并行光栅化（rayon）、`TipProfile` 逐行笔尖、mip/pattern 缓存、`composite_atop`、Mixer Brush 事务式实时涂抹、图案库 `patterns.json`。
- 液化：多通道（color set）液化、Smart Filter 重开/编辑、Show Backdrop 的 Use/Mode、Preview 开关、`TwirlCcw` 从工具栏移除。
- 压感：Windows WinTab 后端（`wintab` crate，默认 Windows Ink）、笔杆键/橡皮端指针钩子、`Pose` 分轴覆盖（`overrideTiltX/Y`，带旧 JSON 迁移）、`pressureSize` 默认改为 false。
- 工具：新增 `ArtHistoryBrush`、`Star` 两个工具；工具栏支持 Alt-click 循环、按住拖出 flyout、`Edit Toolbar…` 右键菜单、`groupTools` 持久化。
- 其它：取消未保存文档提示、外部 SVG、启动报告、批量 CLI、纹理/图案库、多语言扩到 34 种。

### 1.3 上游新功能里与你直接冲突的点

| 冲突 | 你的做法 | 上游做法 | 结论 |
|---|---|---|---|
| 笔刷渲染 | 重写 `render.rs`（C3 高斯尖/KM 颜料/湿边/transport），+1341 行 | 并行化 + TipProfile + behind | 以你的为准 |
| 笔刷参数模型 | `BrushSettings` 36→56 字段、`Dab` 15→22、`Control` 8→11 | 只加 `behind` | 以你的为准 |
| 笔刷面板 | 13 节 → 8 节（Tip/Basic/Motion/Texture/Color Mode/Wet Edge/Mix/Pigment），去锁，双 lane | 仍是 13 节 + 锁 | 以你的为准 |
| 液化 | 画布内工具 + GPU 常驻多层预览 + CPU 回退 + layer/all/group 提交 | 全窗口对话框 + 单图层 + 多通道 | 以你的为准 |
| 压感 | macOS 原生低延迟回放 + C3 式滤波/校准（`tablet_pressure.rs`） | 全局压力曲线 + WinTab 后端 | 以你的为准，但保留上游 WinTab 后端 |
| 工具分组 | 置顶 4 槽（笔刷+铅笔 / 橡皮 / 涂抹 / 液化+模糊+锐化），选项栏平铺同组工具 | 无置顶段，一槽一按钮 + flyout | 以你的为准 |
| 平滑 | 指数平滑作废，改为细绳（pulled string） | 保留指数平滑 | 以你的为准 |
| Build-up | 强制关闭（`disable_build_up`） | 保留可用 | 以你的为准 |
| Mixer Brush | 从工具栏和 `Tool::ALL` 移除 | 保留并增强 | 以你的为准（工具栏不显示） |

## 2. 你的版本做了什么（必须保住）

### 2.1 多层液化（第 1 项）

新增文件：

- `ui-egui/src/liquify_cpu.rs`（288 行）：代理文档 + 多目标 warp 的 CPU 预览。
- `ui-egui/src/liquify_gpu.rs`（1358 行）：GPU 常驻预览，分页上传、compute 位移、tile 判定、`SourceOverride`。
- `ui-egui/src/liquify_{brush,preview,tiles}.wgsl`：三段 shader。
- `engine/src/edit_scope.rs`（94 行）：`layer` / `group` / `all` 三种范围的根解析。
- `engine/src/cached_transform.rs`（73 行）：提交前暂存 GPU 像素。
- `gpu/src/cached_*.rs` + `native_format.*`：GPU 常驻替换像素的基础设施。

改写：

- `ui-egui/src/liquify_ui.rs` 1437 → 1880 行：从对话框改为画布内工具。
- `algo/src/liquify.rs`：新增 `LiquifyGpuOp`、`gpu_only`、`gpu_ops`。
- `engine/src/distort_cmds.rs`：新增 `liquify_targets`，`liquify()` 改为多目标原子提交。
- `ui-egui/src/state.rs`：新增 `Tool::Liquify`。

核心机制：**scope 解析 → 每个目标收集 source/mask → GPU 常驻预览（失败则 CPU 代理）→ 确认时一次性回读并作为单个 history 步提交**。

### 2.2 压感优化（第 2 项）

- `paint/src/tablet_pressure.rs`（338 行，新增）：`TabletPressureSettings` + `TabletPressureFilter`，16 位整数域滑动平均、死区、满压校准、弹性曲线、Catmull-Rom 坐标重采样。
- `paint/src/lib.rs`：`StrokePoint` 增加 `is_pen`。
- `engine/src/prefs.rs`：新增 `tools.tabletPressure` 与 `tools.tabletBackend`（`winTab` / `ink`，默认 WinTab）。
- `ui-egui/src/stylus.rs`：`NativePenEvent` 队列、`external_pen`、`native_frame`、`calibration_peak`。
- `ui-egui/src/canvas.rs`：`native_paint_input`（帧内先处理输入再出纹理）。
- `tablet/src/wintab.rs` + `windows.rs`：纯 Rust WinTab 解析 + 动态加载 `wintab32.dll`。
- `tablet/src/macos.rs` + `appkit.rs`：AppKit 原生事件、关闭鼠标合并。
- `ui-egui/src/prefs_ui.rs`：新增 Tablet 设置页（死区/弹性/稳定度/压力校准）。
- `apps/photocraft/src/tablet.rs`：WinTab 与 macOS 原生输入安装。

### 2.3 笔刷设置（第 3 项）

新增引擎模块（`paint/src/`）：`material.rs`、`pigment.rs`、`pigment_material.rs`、`attributes.rs`、`canvas_material.rs`、`motion.rs`、`transport.rs`、`tablet_pressure.rs`。

改写：`render.rs`（+1341）、`dynamics.rs`（+478）、`brush.rs`（+386）。

UI：`brush_sections.rs` 804 → 1438、`brush_panel.rs` 559 → 596、`brush_preview.rs`、`brushes_tab.rs`、`brush_preset_dialog.rs`。

存储：`preset_store.rs` 位图槽 3 → 7（副笔刷/画布/副画布）、新增 `canvas-presets.json`；`io/src/c3.rs`（1545 行）C3 导入。

关键设计：`bounded_for_render` 在渲染前主动关闭 Pose、画布材质、翻转、投影、轮廓、双向散布、Wetness/Mix Jitter、Build-up，强制 Normal 混合，并按参考应用的控制表降级非法 Control。

### 2.4 工具分组集合显示（第 4 项）

- `panels.rs`：`TOOL_SECTIONS` 新增置顶第 0 段 `[Brush,Pencil] / [Eraser] / [Smudge] / [Liquify,Blur,Sharpen]`；选择类 10 工具合并成一槽；`ColorReplacement` 独立；`visible_sections` 排除单行/单列选区并让第 0 段免隐藏。
- `panels.rs::options_bar`：`direct_group` 分支——当前工具所在组在选项栏**平铺显示全部同组工具**，不再只显示一个图标。注意工具栏本身仍是 flyout，平铺只发生在选项栏。
- `state.rs`：新增 `Tool::Liquify`，`MixerBrush` 移出 `Tool::ALL`。
- `icons.rs`：`Liquify => droplet`，`Blur => scan`。
- `prefs_ui.rs`：工具栏设置新增 Selection 总开关，置顶工具不可单独隐藏。

## 3. 合并策略

以 `new` 为骨架，按功能域注入你的实现；在某功能域内以你的版本为准。

| 序号 | 功能域 | 注入方式 |
|---|---|---|
| 1 | 工具分组 | 改 `panels/toolbar.rs` 的 `TOOL_SECTIONS` 与 `visible_sections`；改 `panels/options_bar.rs` 的平铺分支；保留上游 `MixerBrush`/`ArtHistoryBrush`/`Star` |
| 2 | 压感 | 新增 `paint/tablet_pressure.rs` 与 `is_pen`；改 `input/stylus.rs`、`canvas.rs`、`prefs`；保留上游 WinTab 后端，WinTab 与 Ink 两种模式并存 |
| 3 | 笔刷设置 | 新增 8 个引擎模块；`brush.rs`/`dynamics.rs`/`render.rs` 以你的实现为准并补齐上游 `behind`；面板改 8 节 + 双 lane；预设槽 7 位 + C3 导入 |
| 4 | 多层液化 | 新增 `edit_scope`/`cached_transform`/`liquify_cpu`/`liquify_gpu`；改 `cmd/distort_cmds.rs`；`dialogs/liquify_ui.rs` 以你的画布内工具为准；保留上游多通道与 Smart Filter |

### 3.1 明确要放弃的上游部分（因为与你的实现不兼容）

- 上游的 13 节笔刷面板、笔刷锁、`checkbox_only`。
- 上游的液化全窗口对话框（改为你的画布内工具）。
- 上游的指数平滑与 Build-up 行为（你的 `bounded_for_render` 会关闭它们）。
- 上游 `Tool::MixerBrush` 的工具栏入口（枚举保留，`Tool::ALL` 不含）。
- 上游液化 `TwirlCcw` 工具入口（你的工具条不暴露）。

### 3.2 明确要保留的上游部分（即使你的实现覆盖了同领域）

- 上游 `behind` 混合与 `composite_atop`。
- 上游 WinTab 后端（`tablet/src/wintab.rs` 的 crate 版本）+ 笔杆键/橡皮端钩子。
- 上游多通道液化（color set）与 Smart Filter 编辑。
- 上游 `ArtHistoryBrush`、`Star` 工具。
- 上游并行 rasterize 与 mip/pattern 缓存（性能不能回退）。
- 上游 `Pose` 分轴覆盖 + 旧 JSON 迁移。

## 4. 后续跟随上游的固定流程

1. 记录当前合并版本对应的上游 commit（写入 `UPSTREAM.lock`）。
2. 上游有新提交时，用 `vendor-deps` 里的本地镜像更新 `Cargo.toml` 的 rev。
3. 以 `docs/custom/MERGE-DIFF.md` 的「功能域」为边界做三方合并，不整体覆盖 `paint`、`ui-egui`。
4. 每次合并后跑：
   - `cargo check -p photocraft-paint -p photocraft-engine -p photocraft-ui-egui`
   - `cargo test -p photocraft-paint`（笔刷像素回归）
   - `cargo test -p photocraft-engine`（液化 scope/压感 prefs）
5. 冲突一律以你的功能域为准；上游新功能按本文第 1.2 节逐条确认仍在。

## 5. 用户明确指定的例外规则

### 5.1 液化：不跟随上游，屏蔽原版入口

用户 2026-11-09 明确指示：

> 液化的部分和原版是完全不一样的，这里面不用跟随，并且可以屏蔽原版的液化入口。
> 现在的液化属于一个单独的工具类。

因此：

- **液化以用户实现为唯一来源。** 上游 `dialogs/liquify_ui.rs` 的全窗口对话框、`filter.liquify` 入口、`Cmd+Shift+X` 菜单项、`distort_ui` 的 liquify 分支一律**不合并**。
- 液化是**独立工具类** `Tool::Liquify`，走画布内工具 + 选项栏，不走 Filter 对话框。
- `Filter › Liquify…` 菜单项保留为**跳转到该工具**（或直接移除），不能打开上游对话框。
- 以后跟随上游时，`crates/ui-egui/src/dialogs/liquify_ui.rs`、`crates/engine/src/cmd/distort_cmds.rs` 的 liquify 分支属于**已冻结区域**：上游的改动不自动接受，只在确认不影响自定义液化时手工挑拣。
- 上游的**多通道（color set）能力**仍可保留，但必须作用在自定义液化上；如果做不到，宁可不保留，也不能退回上游对话框。

### 5.2 其它三项的优先级

- 笔刷设置、压感、工具分组：以用户实现为准，但**保留上游同领域的新能力**（见 3.2）。
- 与用户实现直接冲突时，一律以用户为准。

## 6. 移植后已知的预期测试差异（不是 bug）

`crates/paint/src/brush_tests.rs` 里有 17 个上游断言失败，全部因为**你的 `bounded_for_render`
主动禁用了上游保留的旧特性**。这是你的设计意图（见 2.3），不是移植错误：

| 失败测试 | 原因 |
|---|---|
| `pose_overrides_stylus_values` | `bounded_for_render` 强制 `pose.enabled = false` |
| `tilt_scale_squashes_the_tip_height_with_pen_tilt` | 强制 `shape_dynamics.tilt_scale = 0.0` |
| `brush_projection_foreshortens_...` | 强制 `shape_dynamics.brush_projection = false` |
| `ordinary_scatter_is_unchanged_by_the_cap` | 强制 `scattering.both_axes = false` |
| `transfer_wetness_and_mix_vary_per_dab` / `mixer::transfer_wetness...` | 强制 `transfer.wetness/mix` 归零 |
| `wet_edges_and_noise` | `noise` 跟随 `grain` 开关 |
| `fade_ramps_over_steps` / `jitter_is_deterministic_per_seed` / `spacing_off_...` | 强制 `spacing_enabled = true`，步长用可见半径重算 |
| `blend_mode_and_dissolve` | 强制 `mode = Normal`（非橡皮、非铅笔） |
| `color_dynamics_ranges` / `dual_brush_intersects` / `c3_sensor_controls...` / `material_grain...` / `scattering_stays_in_bounds` / `render::material_regression::dual_brush_opacity...` | 新材料/双笔刷管线改变了像素与字段语义 |

### 处理原则

1. **不改 `bounded_for_render` 去迁就上游测试。** 它是你的笔刷行为的定义。
2. 以后跟随上游时，这些测试会再次失败。判断标准：看是不是 `bounded_for_render` 关闭的特性；
   是则视为预期，不是则视为真 bug。
3. 若要长期干净，可以把这 17 个上游断言改写为「特性被关闭」的断言（即断言它们无效），
   这样既保住上游测试名，又表达你的设计。

## 7. 用户 2026-11-09 的补充决定

### 7.1 安全钳制：合并，取更强的一个

`BrushSettings::bounded_for_render` 的安全部分两边都保留：

| 检查 | 来源 | 处理 |
|---|---|---|
| `size` clamp(0.5, MAX)，非有限值 → 0.5 | 你的版本 | 保留 |
| `scatter` clamp(0, MAX_SCATTER)，NaN → 0 | 你的版本 | 保留 |
| `opacity` / `flow` / `hardness` / `spacing` NaN → 默认 | 上游 | **合并进来** |
| `dual_brush.hardness` / `dual_brush.spacing` NaN → 默认 | 上游 | **合并进来** |

上游这几个是纯兜底，不与你的设计冲突，所以合并后更安全。

### 7.2 渲染：整套用你的，但并行调度必须保留

用户指示：「画笔的渲染整套要使用我的，但是你看看源码是否有冲突，如果是旧的就分析下哪个性能更好，如果只是屏蔽就忽略。」

**结论：不是“只是屏蔽”，上游的并行是真实性能差异，必须保留。**

实测（`crates/paint/examples/stroke_bench.rs`，3000 dabs，RGBA8 4928×3280，Apple Silicon release）：

| 笔尖 | 你的原渲染（串行） | 上游渲染（并行） | 合并后（你的算法 + 并行调度） |
|---|---|---|---|
| 20 px | 129.1 µs/dab | 45.8 µs/dab | 97.4 µs/dab |
| 60 px | 712.4 µs/dab | 209.6 µs/dab | 529.9 µs/dab |
| 200 px | 6480.7 µs/dab | 990.4 µs/dab | 2306.4 µs/dab |

处理方式：

- **像素算法、笔尖形状、颜料/湿边/双笔刷合成逻辑：100% 用你的实现。**
- 只把上游的三项纯调度基础设施移植进来，它们不改变任何像素：
  1. `for_each_item`：按像素数阈值（`DEFAULT_PARALLEL_MIN_PIXELS = 256×256`）决定是否走 rayon。
  2. `RegionPool`：tile 缓冲复用，避免长笔画逐 tile 分配。
  3. `composite` 改为「逐 tile 独立缓冲 → 分批并行 → 按 key 顺序写回」。
- 合并后测试结果与并行前**完全一致**（189 passed / 17 failed，失败集合相同），证明像素没有改变。
- 小笔刷（20 px）仍低于并行阈值走串行，所以只快了约 25%（来自顺带的 `coverage_at` 热路径 hoist）；
  大笔刷（200 px）提速约 2.8 倍。
- 与上游仍有差距（200 px：2306 vs 990 µs），因为你的渲染每个像素多做颜料/湿边/transport 计算，
  这是你功能的固有成本，不是调度问题。若以后要继续优化，方向是让 `coverage_at` 在颜料未启用时
  走上游的快速路径，而不是改算法。

## 8. 用户 2026-11-09 后续要求（已处理）

### 8.1 选项栏去掉滑条，只留数值框拖拽

用户截图指出 Size、强度、Opacity 三处的**左右滑条**要去掉，统一成「数值框内左右拖拽」。

- 合并版走的是上游的 `options_bar.rs`，其中 Size/Opacity/Flow/Strength 全部是
  `widgets::value_field`（数值框内拖拽），**没有滑条**。
- 你的 0.23.6 选项栏里才有 `brush_size_slider` / `brush_attribute_controls` 的 `widgets::slider`；
  合并版已不使用这两个函数，因此该要求已满足。
- 若以后有人把旧选项栏合并回来，必须同时删掉这两处的 `widgets::slider`。

### 8.2 压感滤波默认值（C3）

用户指定：滤波**默认启动**，弹性默认 **6**，稳定度默认 **2**。

```rust
// crates/paint/src/tablet_pressure.rs
enabled: true,
maximum_pressure: 1.0,
dead_zone: 0.0,
elasticity: 6.0,
coordinate_window: 2,     // 稳定度
pressure_window: 5,       // 压力平滑
```

注意：`pressure_size`（选项栏的「压力控制大小」快捷开关）**保持上游默认 false**。
「每个参数的控制默认 Off」由 `Dynamic::default()` 的 `Control::Off` 保证，两者是不同层级：
- `Control::Off` = 该参数不被任何输入驱动
- `pressure_size` = 压力这个具体驱动是否整体接管尺寸

### 8.3 透明滚动条

`crates/ui-egui/src/theme.rs`：滚动条只画手柄，不画轨道。

```rust
scroll.dormant_background_opacity = 0.0;
scroll.active_background_opacity = 0.0;
scroll.interact_background_opacity = 0.0;
```

上游 hover 时会画 0.4 的轨道，已改回全透明。

### 8.4 浮动文档窗口在前台

用户要求：拖动文档标签撕成浮窗后，浮窗要激活到前台，但**只在应用内部**，不能挡住其他软件。

- 实现：`crates/ui-egui/src/canvas/tabs.rs::extra_windows`，窗口获得焦点或正在被拖动时
  发送 `ViewportCommand::Focus`，并 `session.set_active(w.document)`。
- **明确不使用** `WindowLevel::AlwaysOnTop`：那会让窗口压在其他应用之上。

### 8.5 透明小标题与文档拖拽

合并版已具备，无需改动：
- `integrated_titlebar`（macOS 集成/透明标题栏）
- `custom_titlebar`（Windows/Linux 自绘标题栏，含拖动与边缘缩放）
- `panels/titlebar.rs` 发送 `ViewportCommand::StartDrag` 实现拖动

## 9. 用户 2026-11-09 第二轮要求（已处理）

### 9.1 画布投影对齐 C3

现象：画布旋转时，投影（纸底阴影）跟着一起转，形成交叉；尺寸也不合适。

原因（`crates/ui-egui/src/canvas/gpu_canvas/shaders.rs::fs_shadow`）：

```wgsl
let d = px_to_doc(p);              // 已反旋转回文档空间
let p_align = view.b.xy + d * view.a.z;
box_blur(p_align - vec2(0.0, 7.0*u), ...)   // 在文档空间向下 → 跟着转
```

C3 的做法（`SimpleShaders.metal::canvasShadowVertex`）：几何体跟着画布转，但偏移在
**裁剪/屏幕空间**做（`out.position.y -= shadowInfo.w`），所以光永远从上方来。

修正：

```wgsl
let box = doc_screen_aabb();       // 旋转后的文档屏幕 AABB
let ambient = box_blur(p - vec2(0.0, 10.0 * u), box.xy, box.zw, 8.0 * u) * 0.14;
```

**第二次修正仍不完整**（用户 2026-11-10 又反馈"是最早那个没有旋转的方形"）。

真因有两个：
1. 旋转后页面的屏幕 AABB 比纸张本身大一圈，投影就变成一个轴对齐的大方块贴在外面：
   既没跟着纸张转向，尺寸也过大。
2. 更关键的是：**投影被写成了 CPU / GPU 两套实现**。GPU 着色器里那套改了，CPU 回退路径
   还是 `egui::Shadow` 的轴对齐方块（`offset [0,8] blur 28`），所以用户看到的一直是旧的
   方形——底栏显示 "CPU Rendering" 时走的正是这一条。

**现在的实现：只有一套**，放在 egui 绘制层，两条渲染路径共用
（`crates/ui-egui/src/canvas/paint.rs::paint_canvas_shadow`）：

```rust
// 12 层由外向内收缩的页面四边形（每层随视图旋转），屏幕空间统一下移 5 pt。
// 每层只补上更宽那层还没覆盖的 alpha，重叠时是淡出而不是叠加。
for i in (1..=STEPS).rev() {
    let t = i / STEPS;
    let target = 0.14 * (1.0 - smoothstep01(t));   // 由 0 到 0.14
    let a = (target - cum) / (1.0 - cum);          // "over" 合成
    let d = 8.0 * t;                               // 8 pt 淡出带
    mesh.quad(page grown by d, offset (0, 5));
}
```

要点：
- **不再分 CPU / GPU**：`paint_gpu_view` 在画 canvas 之前调用同一个
  `paint_canvas_shadow`；GPU 着色器里的 `vs_shadow`/`fs_shadow`、`shadow_pipeline` 已删除。
- 几何随页面旋转（贴合纸张四条边），偏移在屏幕空间（光从正上方来），与 C3
  `canvasShadowVertex`/`canvasShadowFragment` 一致。
- 偏移 **5 pt**：C3 把 `10 / viewSize.height` 传给裁剪空间 `y -=`，裁剪空间 y 跨 2.0，
  所以屏幕位移是 10/2。
- 淡出 8 pt、峰值不透明度 0.14。
- 用 egui 网格而不是 shader 的理由：投影是纯屏幕空间几何，两条路径都要，写在共用层
  才不会再次分叉。`Shadow` 的模糊是盒式且不可旋转，只能手绘分层。

### 9.2 触摸板/旋转后按 Esc 复位角度

`crates/ui-egui/src/canvas.rs`：Escape 释放时，若当前 `view.rotation != 0` 且没有修饰键，
把角度归零。不复用 `consume_key`，所以工具自己的 Escape 取消逻辑不受影响。

**2026-11-10 修正**：原来这段嵌在 `if nav_hovered` 里，还要求
`!is_shape_tool && !is_selection_tool`。结果是鼠标不在画布上、或正在用选框工具时按 Esc
完全不复位。现在复位逻辑移到 `nav_hovered` 之外，只保留"非文本编辑 + 无修饰键"，
任意工具、任意指针位置都能一键回正。

### 9.3 缩放/旋转手势从 Shift 改为按住 Esc

用户要求：原版按住 Shift 拖动（上下缩放、左右旋转）改为**按住 Esc 拖动**，
且 Esc 原有的单击取消不能被破坏。

- `crates/ui-egui/src/canvas/rotate_view.rs::held_key_navigation`：移植 C3
  `CanvasMacInputView.beginShift/updateShift`，前 10 px 位移决定是缩放还是旋转。
- `canvas.rs` 只在 **Esc 按住 + 主键按下** 时才接管；纯单击 Esc 走 9.2 的复位或工具的取消。
- 拖动过的会话会记一个 `nav-dragged` 标记，释放时只结束手势、不误触发复位。

### 9.4 浮动面板的投影

`craft_ui::docking::FloatingFrame` 只 `rect_filled`，不画阴影，所以页内浮动面板平贴在画布上。
（原生独立窗口才由系统画阴影。）

`crates/ui-egui/src/panels/panel_docking.rs::floating_shadows`：在 dock 绘制前，
按 layout 里的浮动窗口矩形画一层 `egui::Shadow`（offset 3 / blur 9 / 0.30 × `t.shadow`，
取自你旧版 `floating_frame`）。

### 9.5 关于“移植整个浮动面板系统”的结论

检查后发现上游 `panels/panel_docking.rs`（1325 行、27 个 API）**已经比旧版的 1176 行更完整**，
旧版独有的只是浮动面板的位置记忆和它的阴影样式。所以只补了阴影，没有回退整套系统。
