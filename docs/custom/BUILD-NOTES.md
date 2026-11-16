# 构建 / 测试环境备忘（macOS）

本文件记录这台 Mac 上构建、测试时踩过的坑，避免重复。**先看这一页再跑 `cargo`。**

## 一句话口径

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test --tests --no-fail-fast
```

只跑某个 crate / 某个测试时，同样带上这三个环境变量，并复用同一个 `target/`（不要另开 `CARGO_TARGET_DIR`，
否则等于把所有依赖重编一遍，又多占 6G+）。

## 磁盘预算

| 做法 | `target/` 体积 |
|---|---|
| 默认 `cargo test`（含 examples、增量、debuginfo） | **37G**（本机 30G 可用盘直接被写满） |
| 上面那条口径（不编 examples + 关增量 + 关 debuginfo） | **6.6G** |

写满盘的后果不只是"编不完"：暂存类测试会失败，报
`scratch disk … low on free space; spilling is paused` 或 `assertion failed: spill::enabled()`。
这类是**环境失败，不是代码 bug**，先清空间再判断。

## 三个坑（按它们吃掉的体积排序）

1. `target/debug/examples`（约 15G）：`cargo test` 默认把所有 examples 也编一遍。用 `cargo test --tests`。
2. `target/debug/incremental`（约 17G）：`CARGO_INCREMENTAL=0`。
3. debuginfo：仓库把 debug 设成 `line-tables-only` 且 unpacked，deps 体积翻倍。`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`。

`cargo clean` 是在空间已被吃满时的唯一干净出路（删 `target/debug/incremental` 单独救不回来，
且中途 `rm` 有进程占用会失败）。清理后重编一次约 10 分钟。

## macOS 专属坑（Windows/Linux 上不会暴露）

- `rustix::fs::mkfifoat` 在 Apple 平台不存在（rustix 用 `cfg(not(any(..., apple, ...)))` 排除）。
  `apps/photocraft/src/external_svg.rs` 的 FIFO 测试已按 `#[cfg(not(target_vendor = "apple"))]` gate；
  `apps/photocraft/src/external_svg_portal.rs` 里的同名调用属于 Linux 专属 xdg portal 模块，macOS 不编译。
- 菜单相关测试（`mac_menu_appkit`）断言英文菜单名，会被系统语言影响；测试里固定
  `photocraft_ui_egui::i18n::set_current(Lang::EN)`，不要改成依赖系统语言。
- `heif` 特性即使 `--no-default-features` 也可能经 `photocraft-codecs` 的默认特性进来，
  `heif_unsupported` 测试按"错误文案不是'不在本构建中'就跳过"处理。
- 没有 `timeout` 命令（GNU coreutils 未安装），脚本里别用。
- `docs/scorecard.md` 由 `cargo xtask scorecard` 生成，改了 settings 相关代码要重新生成，否则
  `xtask scorecard::tests::staleness_check_and_determinism` 会失败。

### 磁盘不是"够用就行"，spill 测试有硬门槛

`crates/raster/src/spill.rs` 的保留线是 `max(1 GiB, 5 % 卷容量)`：460 G 的盘约 **23 G 必须留空**，
否则所有 scratch/spill 测试统一报 `low on free space; spilling is paused`，表现为一片红，
和代码无关。判据：`df -h /System/Volumes/Data` 的 Avail 是否 ≥ 5 %。
注意 `--tests` 编出来的测试二进制本身会吃 8 G 左右，编译完再看剩余空间，别用编译前的数字判断。

### ui-egui 的 lib 测试要更大的栈

`-p photocraft-ui-egui --lib` 默认 2 MB 测试栈会在
`canvas::background_redraw::tests::a_canvas_over_the_texture_budget_draws_its_viewport_pages_on_the_gpu`
处 `stack overflow`，整轮直接 abort（后面的用例一个都不跑，看起来像"大量失败"）。
跑法：`RUST_MIN_STACK=33554432 cargo test -p photocraft-ui-egui --lib`。
吃完栈之后剩下的是字体/egui-kittest 相关用例（epaint `fonts.rs` 里报错），属 macOS 字体环境差异。

### 字体是可选构建输入，缺了会让 ui 测试大面积报 missing glyph

`CRAFT_FONTS_DIR` 指向 `storytold/craft-fonts` 的 checkout（本机已放在
`~/Documents/PS_photocraft/craft-fonts`，131 M）。不设置时也不会编译失败，但：
- `photocraft-ui-egui --lib` 会出现 122 个 `No glyph for …`（中日韩、阿拉伯文）；
- 打出来的 app 中文/日文/韩文是豆腐块。

口径：`CRAFT_FONTS_DIR="$HOME/Documents/PS_photocraft/craft-fonts" cargo build -p photocraft`。
带上字体后二进制从 127 M 变成 175 M，这是字体数据，不是编译膨胀。
带字体后 ui lib 仍有约 107 个用例失败，都是按文本行高/字宽断言的布局类用例
（如 `blend_dropdown_groups_tests` 的分组行距），属 macOS 字体度量差异，不是功能问题。

## 参考树（查根因时用，不要在仓库里改它们）

| 用途 | 路径 |
|---|---|
| 0.23.6 自研源码 | `~/Documents/PS_photocraft/archive/code-liquify-023` |
| 0.23.5 源码备份包（含液化纯缓存实现） | `~/Documents/PS_photocraft/archive/backup/PhotoCraft-source-20261030.zip` |
| 画笔混合（C3，Swift/Metal） | `~/Documents/sssp_c3` |
| 上游 PhotoCraft | 本仓库 `origin/main` / `upstream-20261110` 分支 |

口径：**画笔混合看 C3；液化（自研，上游没有）看 0.23.5 备份包；其它源码问题看 0.23 源码 / 上游分支。**

## 本轮又踩到的两个坑（2026-11-15）

### 只建 app，别随手 `--all-targets`

`cargo build --workspace --all-targets` 会把每个 crate 的 test harness 也链一遍，
debug 版链一次就要 15 G 的 `target/debug/deps`，加上 incremental 直接顶穿磁盘：

```
ld: write() failed, errno=28 (No space left on device)
Error writing pre-lto-bitcode file .../incremental/.../x.pre-lto.bc: No space left on device
error: the compiler unexpectedly panicked. this is a bug   ← 磁盘满伪装成编译器 ICE
```

实测满盘那一刻 `target` 44 G（`incremental` 27 G、`deps` 15 G），系统只剩 380 Mi。
口径：
- 日常只 `cargo build -p photocraft`（app）；
- 要跑测试就 `CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p <crate>`，
  绝不加 `--all-targets`；
- 见到 `rustc ... pre-lto-bitcode: No space left` 先 `df -h`，不要去查编译器版本。

### `target/` 会被别的东西整目录删掉 —— 不是 cargo 的锅

盘满那次之后 `target/debug` 整个消失（`ls: target: No such file or directory`），
`df` 同时从 26 G 空闲跳到 48 G。原因不是 cargo，是**另一个会话的后台任务**
在跑 `rm -rf .../photocraft-custom-0.24/target`，然后去 `~/Documents/sssp_c3` 做 `swift build`
（那个 `.build` 目录 4.9 G，5 个 swift-frontend 同时跑）。
教训：多个会话共用同一个仓库时，**先 `ps aux | grep -E "rm -rf|swift-frontend|cargo"`** 再动手，
看到别人正在删/正在编就别抢，否则刚编好的产物会凭空蒸发、时间全浪费。
被删之后只能从零编：`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo build -p photocraft`
（约 4 G，几分钟；不再需要 37 G）。

## 加一个新的 Adjustment 变体要同时改这几处（少一处就 E0004）

按顺序，全部 `cargo build --workspace` 报一次 `non-exhaustive patterns` 才补一处会来回编译五轮：

1. `crates/doc/src/adjust.rs` —— 变体本身 + `label()`
2. `crates/compose/src/adjust.rs` —— `apply` 分发（CPU 实现放 `adjust/<name>.rs`）
3. `crates/gpu/src/plan/lower.rs` —— `adjustment_on_gpu`（纯 CPU 的返回 false）与 `adjustment_program`
   （kind 号 + 参数打包）；需要读邻域的走 `plan/types.rs` 的 `Kernel::AdjustNear`（`entry()` 写
   `"fs_adjust_near"`，`DRAWN` 数组要加一项，否则管线建不出来）
4. `crates/io/src/adjust_map.rs` —— PSD 块写什么（表达不了的写空块）
5. `crates/engine/src/adjust_params.rs` —— `from_params` 的 kind 分支 + `to_params` 的臂
6. `crates/engine/src/cmd/adjust_params.rs` —— `kind()` 的 `&str`（有 `_` 兜底，漏了会静默变 `"invert"`）
7. `crates/io/src/sssp.rs` —— 导入映射 / 导出回填
8. `crates/io/tests/it/adjust_blend_roundtrip.rs` —— 编译期守卫那串 `match`
9. 要能建图层就再加：`crates/ui-egui/src/dialogs/adjust_editors.rs` 的 `KINDS` + 分发、
   `menu_catalog.rs` 的 `Image › Adjustments` 与 `Layer › New Adjustment Layer` 两条
10. PSD 存不下的（网点 / CMYK / 线条强化）还要在 `menu_catalog.rs` 的 `is_sssp_only` 与
    `crates/ui-egui/src/panels/layer_row_ui.rs` 的 `sssp_only()` 各加一臂，否则图层行与菜单不出
    `SSSP` 徽标，用户看不出「只有 `.sssp` 才存得住」。
11. `crates/engine/src/cmd.rs` 的 `ADJ` 表：漏了菜单能点但 `image.adjustments.*` 命令不存在，
    `ui adjust_preview` 会报 command / preview 合法性不一致。

## 着色器与 CPU 必须同一个 `fract`

CPU 侧照 C3 着色器复刻网点 / CMYK 时，**不能直接用 Rust 的 `f32::fract`**：它是
`self - self.trunc()`，负数保留符号；WGSL/Metal 的 `fract` 是 `x - floor(x)`，落在 `[0,1)`。
两者在原点左侧（或角度旋转后的负数坐标）差一个整格，网点相位整整反过来，GPU 与 CPU 颜色能差
200/255（`cmyk_screen` 第一次进 `parity::c3_printing_adjustments_matches_cpu` 就是这么炸的）。
写法：`fn fract01(v: f32) -> f32 { v - v.floor() }`。

改 `compose.wgsl` 后先跑 `cargo test -p photocraft-gpu --test it compose_shader`（naga 校验，
不需要 GPU，Apple/Windows/CI 都能过），再去跑真的 parity（要真适配器）。

## 线条强化：C3 的 field 拆分与实测

C3 把 `maxDiff`（边缘量）预先算成一张 field 纹理，之后强度/锐度/三档明暗/线色/混合都只是点查
（`C2AdjustmentShaders.metal:337` 的 `c2LineArtFieldKernel`、`:1802` 的 `webApplyLineArtField`）。
本地 `crates/compose/src/adjust/line_art.rs` 已按这个结构拆开：`field(buf, thickness) -> Field`
（只依赖像素与厚度）+ `apply_field(buf, &field, params)`（点查混合），`apply` = 两者串联，
导出/CPU 成图逐位不变（`gpu --test it c3_printing_adjustments_match_cpu` 仍全过）。

实测（M5，1,296,000 px = 1440×900，8 层合成）：
- `compose::render(below)` 约 13 ms（field 需要的底图）；
- `line_art::field(thickness 3, 59 taps)` 约 86 ms 全分辨率；隔行半分辨率约 1/4 ≈ 22 ms。
- C3 的环上限 `sampleRadius = 10`（最多 441 taps），本地也按 10 截断；每个 tap 都是整像素偏移，
  所以双线性退化成直接读，CPU 侧已改成直接读 + 预计算 `Ring { offsets, weight_sum }`。

## 线条强化的 field 真的接上了（GPU + CPU）

field 落地后，两条路径都只按「整段绘制区域」算一次场：

- **CPU**：`Ctx::set_line_field` 在 `render_tiled_with` 开头把场算一次给所有 tile/band 共用
  （`line_art::apply_shared`）。在这之前 `line_art::apply` 是**逐 tile** 算场，tile 边界上的
  ring 只能读到被钳制的邻居 —— 128/256/512 这些 tile 边界上就少墨，这正是「CPU 有接缝、GPU 也有」
  的真正原因。现在 tile 内每一像素的邻居都来自整段缓冲区。
- **GPU**：planner 对**根层**的线条强化生成 `FieldSpec`（图层 id、根层序号、厚度、`region`
  外扩半径后再裁到画布），`encode` 的 `Compositor::sync_fields` 用 `compose::render` 只合成
  「该层以下」并 `line_art::field` 一次，上传成 R32F 绑到 `group(1) binding 8`；`case 29` 做点查，
  `p3` = field 矩形。分组内的线条强化仍走 `case 28` 的逐块 ring（它的底图是组内合成，截断整个
  `doc.layers` 不成立）。
- **失效**：`prefix::prepare` 记下「最下面被改的根层号」（`changed()`），场只依赖该层**以下**的层，
  所以 `changed >= 该层序号` 时场仍旧有效，`changed < 该层序号` 才重建（上一版把这条写反了，见下节）。
  拖强度/锐度/明暗/线色/混合滑杆、改上层或改自身蒙版都不重算（C3 的 key 里也没有这些外观参数）。
- **算力门槛**：一段区域超过 4 Mpx（16 MiB R32F）就用回逐块 ring，不把场纹理撑爆。

测试：`gpu --test it line_art_field_has_no_seam_across_chunks`（画布 1200×300，跨 1024 的
分块接缝 + 单条窄条的三种取景，GPU 与 CPU 逐像素同值）。

## 线条强化：场的失效条件写反了（2026-11-16 修）

`Compositor::sync_fields`（`crates/gpu/src/fields.rs`）原来把**复用**条件写成
`changed < spec.layer_index`。`changed` 是最下面被改的根层号，而场的输入只有该层**以下**的层：
拖强度/锐度/三档/线色/混合时 `changed == layer_index`，旧条件不成立 → **滑杆每一个 tick 都重建
一次场**，1440×900、默认厚度 13（440 taps）就是每 tick 0.5 s 量级的 CPU 计算，界面直接卡死。
这就是「线条强化特别差」的根因，也正是上一节那句 `changed < 该层序号 才重建` 和代码对不上的
地方：文档写对了，代码取了反。

- 现在：`changed.is_none_or(|ch| ch >= spec.layer_index)` —— 改在层自身或其上方则继续用缓存；
  层以下变动（含从该处起新增/删层）才重建。`sync_fields` 顺带返回本次重建个数，累进
  `Stats::fields_built`（`crates/gpu/src/textures.rs`、`encode.rs`）。
- 回归测试 `gpu --test it line_art_field_is_reused_for_its_own_appearance_sliders`：首帧 1 次；
  强度 / 锐度 / 三档不透明 / 线色 / 混合 各改一次都 `fields_built == 0`；改厚度、再用同一厚度
  渲染、改下方图层则各 1 次。
- CPU 侧 `Ctx::set_line_field` 仍是每次 `render` 现算（`Ctx` 只活一次 render），导出一帧，
  不受影响；无 GPU 时的画布回退没有跨帧缓存，属已知限制。

**厚度不再实时预览**：厚度是唯一进 `FieldSpec` 的滑块，一次拖拽要重建整个取景的场。现在
`crates/ui-egui/src/dialogs/adjust_editors/line_art.rs` 的 `thickness_row` 把拖动中的值只放进
egui temp（`line-thickness-drag`）让行控件跟手，**不写 `v`、不报 changed**；松手
（`drag_stopped`）才写值并 `commit`，即一次 `layer.setAdjustment` + 一次场重建。双击复位与
滚轮/箭头步进仍走 `Edit::of`。外观滑杆保持 C3 的实时点查预览。UI 回归：
`ui-egui --lib line_art_previews_appearance_and_commits_thickness_on_release`（新层厚度 13、
拖强度中途 `live_adjust.is_some()`、拖厚度中途 `live_adjust.is_none()` 且层内值不变、松手各 +1 步）。

**默认值对齐 C3**（`crates/io/src/sssp.rs` 的 `line_art()` payload 是本地唯一权威记录）：强度 100、
锐度 100、厚度 13、暗部 24 / 中间调 32 / 亮部 90、线色黑、混合 Multiply。为此厚度量程从
`0.1..=10` 放宽到 `0.1..=30`（`adjust::THICKNESS_MIN/MAX`），否则 13 会被两处静默钳制吃掉：
`engine` 的 `num_in`（超范围静默 clamp）钳到 10、GPU `plan/lower.rs` 的 `p[0]` 钳到 10。
**环的读取半径仍按 C3 的 `sampleRadius = 10` 截断**（`line_art::radius`），所以 13 与 10 的 taps
数一致、只是权重不同：默认值不再被吃掉，成本不变。`compose.wgsl` 的 `case 28` 也补了
`min(i32(ceil(outer)), 10)`——之前 GPU 不截断，厚度 ≥ 7 时 CPU/GPU 会不一致。

**场算法本身**（`line_art::field`）：每像素 luma 预算一次、环内走无钳制快路径（只有贴边才 clamp）、
`tap_value` 用平方比较跳过 sqrt（`chroma²·2.1025 ≤ luma项²·0.9999` 时不必算距离，`luma项 ≥ 1`
直接返回权重），与旧实现逐像素在 1e-6 内一致（`optimised_field_matches_the_plain_ring`，
thickness 0.5/3/8/13/30）。

实测（本机 Windows，1440×900 = 1,296,000 px，底图合成 13 ms 另算）：

| 厚度 | taps | `field()` |
|---|---|---|
| 3 | 60 | 125 ms |
| 13（默认） | 440 | 550 ms |
| 30 | 440 | 487 ms |

结论：**场不能进滑杆循环**。缓存修好后一次拖拽最多一次重建（松手那下），但缩放/平移、改厚度、
改下方图层仍各重建一次，默认厚度约 0.5 s。要消掉这一段得把 ring 搬到 GPU 专用 pass（C3 在
Metal 里跑同一圈是毫秒级），那是下一步，不在这次改动里。

## GPU 读邻域的调整层：分块接缝

`CHUNK = 1024`，`tex_a` 是**分块尺寸**的纹理，所以读邻域只能在块内读。`fs_adjust_near` 用
`op.p3` 带上作品矩形（`planner::adjust` 从 `cx.canvas` 填），块内与 CPU 逐像素一致
（`parity::c3_printing_adjustments_matches_cpu` 200×160 全过）；但画布大于 1024 时，块边界最外
`thickness + soft_edge` 像素按块边钳制，会与 CPU 差一点——屏幕预览里的极细接缝，**导出/预览走
CPU 所以成图是准的**。要彻底消掉得让这类 pass 不分块（目前 planner 没有这开关）。
