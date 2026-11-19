# PhotoCraft 自定义版：需求、进度与上游跟随记录

> 这个仓库是 **上游 `storytold/photocraft`** 与 **作者 0.23.6 自研功能** 的合并版。
> 目标：既能持续跟随上游更新，又永久保留自研功能。

最后更新：2026-11-10（已跟随上游第二次更新）

---

## 1. 仓库关系

| | 地址 / 位置 |
|---|---|
| 上游（跟随） | https://github.com/storytold/photocraft |
| 本仓库（自定义合并版） | https://github.com/R19988088/sss.photocraft |
| 推送分支 | `custom-merged`（`main` 保持原样） |
| 合并前的自研分支 | 原 `code/` 目录，分支 `liquify-on-023`，HEAD `d0deb14`，版本 0.23.6 |
| 合并工作副本 | 工作区 `E:\000 app\s.PhotoCraft\` 下的 `source/sss.photocraft/`（即本仓库工作树） |

### 当前基线

```
上游快照   storytold/photocraft main, 2026-11-10（第二次跟随）
           工作区版本 0.24.0（自定义版）
上游祖先   2d73f62b0f83fb2330751906f1f034d95e9ab7a8 (2026-10-19)
自研基线   R19988088/S.photocraft  分支 liquify-on-023, 0.23.6, d0deb14
```

详见 `UPSTREAM.lock`。

---

## 2. 需求清单（作者指定，**优先级最高**）

以下四项一旦与上游冲突，**一律以本仓库实现为准**。

### 2.1 多层液化

- 液化是**独立工具类**（`Tool::Liquify`），不是 Filter 对话框
- 支持 `layer` / `group` / `all` 三种范围
- GPU 常驻预览 + CPU 回退
- 确认时以单个历史步提交所有目标图层

**明确要求**：上游的 `Filter › Liquify…` 全窗口对话框**不跟随、不启用**。
这是冻结区，上游改动只能手工挑拣。

### 2.2 压力优化

- 因果滤波：死区、满压校准、弹性曲线、坐标/压力滑动窗口
- 原生低延迟通道（AppKit `install_input` + WinTab）
- 默认值按 C3：**滤波默认开启，弹性 6，稳定度 2**
- **接收按时间、按样本**（2026-11-19，ProCraft 口径）：`StylusFeed` 每个样本带到达时间，
  `Stylus::select` 在 move 自己的时间上插值；抬笔帧用 `held`（笔离开时的读数）收尾，
  `ToolEvent::Up` 走 `release_pressure()`，不再回落到鼠标的 1.0。滤波按 **每个样本**喂，
  不再一帧一条（那会让窗口滞后约 80ms，抬笔尾巴变粗）。
- **dab 间距跟着实际直径**（同日）：`DabBuilder::step_len` = 实际直径 × roundness × spacing%，
  `PathWalker` 用相邻 gap 的均值推进，笔画变细时间距同步变细（重叠密度恒定）。
  旧规则（size × spacing%）会让快笔/轻压读成「间距变稀」。细则见 `AGENTS.md` §5 第 1 条。
- **新接收管线挂在开关上，默认关**（2026-11-19）：`tools.tabletOptimization` → `Stylus::optimized`，
  UI 在 `Preferences › 数位板` 的「数位板优化」。关时完全走旧链路（index 均分 / 每帧一次滤波 /
  末点压力收尾），开才启用上面两条新口径。
- **`使用数位板压感` 开关已移除**：`tools.useTabletPressure` 的 UI 行隐藏，`set` 与加载都强制
  `true`（压感永远开启）；`压感曲线`（方格 + 线性/软/硬）整块从 `工具` 页搬到 `数位板` 页，
  仍存 `tools.pressureCurve`。
- **压感不做窗口平均，抬笔读真实读数**（2026-11-19，修「抬笔有错误的压力提升」）：
  `pressure_window` 默认 5 → 1，加载时强制 1（ProCraft 的压力是原始读数，只有 3 样本一阶软化）；
  抬笔改走 `mapped_pressure()`（同一映射、无窗口、无延迟线），样本取 `held` 而非 `current`。
  旧实现 `filtered_at(clock_ms)` 取的是「窗口均值 + 延迟输出」，笔在抬笔时变细却读到前面较粗的
  样本，所以末段突然变粗。
- **参考项目间距公式的两处刻意差异**：不照搬 `SPACED_SQUARE`(262/256) 与 `extra`(0.5+
  0.174px)，保持 PS 精确密度；ProCraft 的 `speed_spacing` 默认为 0，本仓库不实现速度间距。

注意：选项栏的 `pressure_size` 快捷开关保持上游默认 `false`。
「每个参数的控制默认 Off」由 `Dynamic::default()` 的 `Control::Off` 保证，两者是不同层级。

### 2.3 笔刷设置

- 面板 8 节：Tip / Basic / Motion / Texture / Color Mode / Wet Edge / Mix / Pigment
- 主/副笔刷双 lane
- 引擎模块：pigment（Kubelka-Munk）、material（grain/wet edge）、
  attributes（33 种颜色属性）、canvas_material、motion、transport
- 渲染算法**整套用自研实现**
- 选项栏不加滑条，只用数值框内拖拽

### 2.4 工具分组集合显示

- 工具栏置顶 4 槽：`[笔刷+铅笔] / [橡皮] / [涂抹] / [液化+模糊+锐化]`
- 液化组位于**第 4 个槽位**
- 选项栏**平铺显示当前工具所在组的全部工具**（不用 flyout）
- 保留上游新增工具：`MixerBrush`、`ArtHistoryBrush`、`Star`

### 2.5 默认工作区模板 D 与工具预设停靠

- 右侧面板自上而下固定为：**颜色（C2 布局）→ 图层 → 画笔 → 工具预设**，颜色之上不再有属性卡。
- 颜色卡的页签组合是 **颜色 | 色板 | 属性**（所有主题一致，2026-11-17）：属性面板
  （`properties`）并入颜色卡当第三页签；渐变、图案（`gradients` / `patterns`）离开颜色卡，
  改为独立的「预置」卡（`dock::Group::Presets`，默认关闭，`Window › Gradients / Patterns`
  照旧打开，菜单勾选与折叠语义不变）。
- `D` 是内置工作区模板（`Window › Workspace` 列表首位、顶栏工作区切换器第一项），也是
  **启动默认**：没有记忆布局（首次启动、新装、Reset Workspace）时一律应用 D
  （`crate::workspace_ui::DEFAULT`，`dock::launch_template` + `dock::restore`）。
  测试进程保留上游起始工作区（`launch_template` 在 `cfg(test)` 下返回 Essentials），否则 D
  不显示属性卡会让所有读属性面板的既有用例整片移位。
- 工具预设从浮动窗口改为**可停靠面板**（`dock::Group::ToolPresets`，面板 id `toolPresets`），
  与画笔面板同一套行为：拖到右侧停靠区、合并成 tab、拖出浮动、Close 关闭都能用；
  `Window › Tool Presets` 走 `window.panel.toolPresets → window.toggle.toolPresets`。
- 画笔面板与颜色面板滑块行原本没有无障碍名的数值框补了 `value_field_named`（如「Size Maximum」
  「Red」）：画笔画板随 D 显示后，`egui_kittest` 的无障碍检查在约 600 个既有用例上整片报错。
- 旧版布局里没有 `panels.tool_presets` 字段，按关闭处理（serde 默认 false）。

### 2.6 画笔面板的选择区（2026-11-16）

- **列表行只画笔迹**：去掉左侧笔尖缩略图和尺寸数字（`brushes_tab::list_row`）；S 形笔迹预览
  从行左边缘 6pt 起、铺到名称列之前（330pt 行宽下比旧版宽 52pt，上限 260pt 防止拉成直线）。
  网格视图不变，仍是笔尖 + 尺寸数字。
- **分组分页**：Brushes 面板一次只显示一个分组；分组标题固定在最上方，左右两个箭头切换分组
  （首尾循环），标题下面的内容单独滚动，每个分组记住各自的滚动位置。筛选框有内容时仍跨分组
  平铺匹配项（一次看全搜索结果）。Brush Preset picker 不分页，仍在一张滚动列表里平铺所有分组。
- **记住分组**：`BrushesPanelState.group` 存当前分组，写入 `Preferences::brush_panel`
  （`{"group": ...}`），启动时恢复；笔刷名本来由 `last_brush` 的 `preset` 恢复，若记住的分组
  已不存在，面板打开该笔刷所在的分组。
- 分页后跨分组拖动只能在当前分组的标题上放下（移到该组末尾）；跨分组整理用 Brush Preset picker。

---

## 3. 进度

### 已完成

| 项目 | 状态 | 提交 |
|---|---|---|
| 三方基线环境（上游基线 / 自研 / 上游新版） | ✅ | `f854202` |
| 四项自研功能移植 | ✅ | `dd54ac2` |
| 画布投影跟随旋转但光始终朝下 | ✅ | `a3194d4` |
| Esc 复位旋转角度 | ✅ | `a3194d4` |
| Esc 按住拖动导航（原 Shift 改为 Esc） | ✅ | `a3194d4` |
| 浮动面板投影 | ✅ | `a3194d4` |
| 解除过度净化（both_axes / 混色 / 湿边 / 颜色抖动 / 混合模式 / 间距 / 翻转抖动） | ✅ | `89fd09d` |
| 纹理采样改双线性 | ✅ | `89fd09d` |
| 间距用抖动前直径 | ✅ | `89fd09d` |
| 双笔刷合并方式对齐 C3 | ✅ | `13dd1e4` |
| 三种混色模式对齐 C3 | ✅ | `13dd1e4` |
| 模糊顺序（先模糊再混色再沉积） | ✅ | `13dd1e4` |
| 可分离高斯模糊 | ✅ | `f77c2a0` |
| `blurColorTarget` 与笔画起点比较 | ✅ | `f77c2a0` |
| 旧版色板（C2 布局）的实线/图案来源与吸管采样图案 | ✅ | 未提交（工作区） |
| 颜料混合强度 `painting.pigmentMixStrength` + 颜色面板菜单滑条 | ✅ | 未提交（工作区） |
| `pm.lut` 作为颜料混色的统一底层（涂抹 / 湿料拖色 / 传输 / 沉积） | ✅ | 未提交（工作区） |
| HEIC/HEIF 打开能力默认开启（`default = ["heif"]`，纯 Rust heic-rs，无需系统 libheif） | ✅ | 2026-11-14 |
| Windows 绿色版：默认 `portable-default`，数据跟着 exe（`PhotoCraftData\`），不写 `%APPDATA%` | ✅ | 2026-11-14 |
| 右侧面板模板 D（颜色→图层→画笔→工具预设）+ 工具预设改为可停靠面板 | ✅ | 未提交（工作区） |
| 便携包 `dist\photocraft-0.24.0-windows-x64-portable-panels-d.zip`（静态 CRT，含上述改动） | ✅ | 2026-11-14 |
| 颜色面板菜单恢复显示模式（C2/色相立方体/亮度立方体/色轮/灰度·RGB·HSB·CMYK·Lab·Web 滑块，选择记入偏好） | ✅ | 未提交（工作区） |
| 便携包 `dist\photocraft-0.24.0-windows-x64-portable-panels-d2.zip`（含显示模式菜单） | ✅ | 2026-11-14 |
| 颜色面板菜单改为窄版两页（第一页十种显示模式，第二页混合强度/复制/色谱，底部圆点翻页） | ✅ | 未提交（工作区） |
| 便携包 `dist\photocraft-0.24.0-windows-x64-portable-panels-d3.zip`（含两页窄菜单） | ✅ | 2026-11-14 |

**pm.lut 底层细节**（对照 `sssp_c3/backup/minimal_20261020_131735`，Swift/Metal 的 C3）：

参考版里所有**颜料色**的混色都收敛到一个函数：`pmPaintMix` → `pm_lerp`（38 波段 Kubelka-Munk，表是 32×32×320 的 `PM_lut.bin`），
alpha / 覆盖率始终由各自的管线负责（`pigmentMergedPixel`、`carrierTransportResult`、`transportC2LiquidColor`、
`mergeLayerTileKernel`）。这边的对应：

- `paint/src/pigment.rs`：新增 `lerp(from, to, t)`（= 参考版 `pm_lerp`：≤0.025 纯线性，之后按 0.025–0.16 的闸进入 KM）；
  `paint()` 改为调用它，公式与参考版 `pmPaintMix` 逐字对应（`support = smooth(0.02,0.22,dst α)`、`weight = strength * support`）。
- 沉积：`render.rs::lane_composite`（含半透明、软边、双笔刷副色、任意混合模式之后）。
- 混色 / 涂抹传输：`transport.rs` 的 V1/V2/V3 与 `over_carried`。
- **涂抹工具**：`retouch::Smudge` 新增 `with_pigment()`，颜色过 LUT、alpha 仍走原来的线性预乘混合；
  强度取自 `brush.pigment_mix_strength`（引擎 `paint.smudge` 直接读 session 笔刷，实时预览与提交同一份设置）。
- **湿料液态拖色**：`pigment_material::shade` 的 liquidity 混合改走 `pigment::lerp`（参考版
  `transportC2LiquidColor` 本来就不设强度闸，这里保持一致）。
- 不经过 LUT（与参考版一致，不是遗漏）：图层合成 / 图层混合模式 / 图层不透明度（`color/src/blend.rs`、`compose`）、
  桶填充、渐变与形状填充、液化、调整层、模糊与锐化。参考版的这几个 shader 里也没有 `pm_lerp`。
- 性能：`pigment::paint` 在 `strength*support ≤ 1e-4` 时直接返回、`lerp` 在 ≤0.025 时不查表，
  `render.rs` 保留 `pigment_mix_strength <= 0.0` 的 `native_only` 快速路径；单次混色 2 次查表，只在笔刷尺寸的 dab 内发生。

**色板移植细节**（对照旧版 `crates/ui-egui/src/color_panel.rs`）：

- 顶部 `实线 / 图案` 在 **C2 颜色** 布局里（面板菜单默认就是它）。图案模式下用吸管在画布上采样：
  `canvas::cursors::sample_eyedropper` 调 `document.samplePattern`（旧版同样逻辑），⌥ 采样图案、⌥ 之外按旧版行为。
- 图案采样后按笔刷大小取原生分辨率方块存在 `brush.sampled_pattern`，绘制时原样还原，不经过缩略图。
- 取纯色（场地/滑条/色块/数值）会退出图案来源但保留采样块，与旧版 `set_color` 一致。
- `混合强度` 在颜色面板右键菜单里（旧版位置），写 `painting.pigmentMixStrength`（0–100），
  引擎在偏好变化时同步到 `brush.pigment_mix_strength`（0–1），随机重启保留；
  不随无关偏好变动覆盖笔刷自身值。
- 底部「绘画设置…」入口按要求未移植。
- 性能：图案预览改为按采样块缓存一张最近邻纹理（旧版每帧 784 个矩形），采样只渲染笔刷大小的方块。

**测试**：`photocraft-paint` 207 全绿，`photocraft-engine` 3298 全绿，全工作区编译通过。

### 待办（需要作者决策）

| 项目 | 影响 | 说明 |
|---|---|---|
| 默认笔尖数学改 `c3_tip_math = true` | **会改变所有现有预设的观感** | 现在是 Photoshop 实测模型（软笔尖外溢到 2.17R），C3 是归一化高斯 |
| 笔刷光栅化 GPU 化 | 工作量约 900 行 WGSL | 现在盖印/颜料/混色/笔触合成全在 CPU |
| 散布副本各自抖动 | 改后观感变化 | C3 是「同一颗 dab 挪几个位置」，现在是「几颗不同的 dab」 |
| `anchor_smooth` 两级 EMA | 不改观感（当前是死字段） | C3 用它做基座与方向偏移的低通 |
| V1/V3 混色模型细节 | 会改观感 | 已在 `13dd1e4` 对齐主要部分 |
| 画布投影参数 | 可调 | 已按 C3 对齐（10pt / 8pt / 0.14），如需更浓可调 |

---

## 4. 跟随上游的固定流程

### 4.1 上游更新后

```bash
# 1. 下载上游新快照（tgz 比 git clone 稳）
curl -L -o /tmp/upstream.tar.gz \
  https://codeload.github.com/storytold/photocraft/tar.gz/refs/heads/main
mkdir -p /tmp/upstream && tar xzf /tmp/upstream.tar.gz -C /tmp/upstream --strip-components=1

# 2. 记录新基线
#    更新 UPSTREAM.lock 里的 upstream.commit / upstream.date

# 3. 定点合并（不要整体覆盖）
#    crates/paint/ 和 crates/ui-egui/ 这两块绝对不要整目录覆盖
```

### 4.2 合并边界

**冻结区**（上游改动不自动接受，只手工挑拣）：

- `crates/ui-egui/src/dialogs/liquify_ui.rs` — 上游 Filter 对话框，不启用
- `crates/engine/src/cmd/distort_cmds.rs` 的 `liquify` 部分
- `crates/ui-egui/src/dialogs/distort_ui.rs` 的 liquify 部分

**自定义区**（以本仓库实现为准）：

```
crates/paint/src/tablet_pressure.rs
crates/paint/src/material.rs
crates/paint/src/pigment.rs
crates/paint/src/pigment_material.rs
crates/paint/src/attributes.rs
crates/paint/src/canvas_material.rs
crates/paint/src/motion.rs
crates/paint/src/transport.rs
crates/ui-egui/src/panels/brush_sections.rs
crates/ui-egui/src/panels/brush_panel.rs
crates/engine/src/cmd/edit_scope.rs
crates/engine/src/cmd/cached_transform.rs
```

### 4.3 合并后必跑

```bash
cargo check --workspace --lib
cargo test -p photocraft-engine --lib
cargo test -p photocraft-paint --lib
```

### 4.4 已知的预期失败

`crates/paint/src/brush_tests.rs` 里有一批上游断言会因为
`bounded_for_render` 的设计而失败。判断标准：

- 若失败项来自 `bounded_for_render` 主动关闭的特性（Pose / 画笔投影 /
  Tilt Scale / 轮廓 / Build-up / 指数平滑）→ **预期**
- 否则视为真 bug，要修代码而不是改测试

`bounded_for_render` 关闭清单（C3 没有的功能）：

```rust
pose.enabled = false
canvas_material = None
saturation_record_strength = 0.0
saturation_preserve_output = 0.0
flip_x = false; flip_y = false
shape_dynamics.brush_projection = false
contour = None
noise = grain.is_some()
shape_dynamics.tilt_scale = 0.0
build_up = false
smoothing.enabled &= smoothing.pulled_string
```

**不要**再往这个清单里加 C3 有的功能（曾经误加了 `both_axes`、
混色参数、湿边抖动、颜色抖动、混合模式、间距、翻转抖动，已全部解除）。

---

## 5. 与 C3 的数学对照

完整的逐项对照见 `docs/custom/BRUSH-C3-AUDIT.md`（473 行）。
摘要：

| 领域 | 一致度 | 备注 |
|---|---|---|
| 笔尖与形状动力学 | 高 | `c3_tip_math=true` 时逐行等价 |
| 颗粒 | 高 | 只差噪声哈希函数（C3 自己注明不追求逐位一致） |
| 湿边 | 高 | 8 个函数逐条对齐，常数一致 |
| 运动/惯性 | 高 | tiltDelay、inertia 分叉 1:1 |
| 散布 | 中 | 公式一致，随机序列不同 |
| 纹理 | 中 | 采样已改双线性；`c3_math` 默认仍 false |
| 混色传输 | 中 | V1/V2/V3 已按 C3 重写 |
| 双笔刷 | 中 | 合并方式已对齐；强度作用点仍不同 |

---

## 6. 目录结构

```
source/sss.photocraft/           ← 本仓库工作树（工作区 = E:\000 app\s.PhotoCraft\）
├── UPSTREAM.lock                跟随上游的锁文件
├── docs/custom/
│   ├── README.md                快速索引
│   ├── MERGE-DIFF.md            三方差分清单（354 行）
│   └── BRUSH-C3-AUDIT.md        笔刷逐项对照（473 行）
├── crates/
│   ├── paint/                   笔刷引擎（自研为主）
│   ├── engine/                  文档/命令/历史
│   ├── ui-egui/                 界面
│   └── ...
└── dist/                        构建产物（不入库）
```

---

## 7. 构建

### macOS ARM release

```bash
cargo build --release -p photocraft
```

产物：`target/release/photocraft`

打成 .app 见 `packaging/macos/package.sh`，或手动：

```bash
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*= *"//;s/"//')
APP=/tmp/pc-app/PhotoCraft.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/photocraft "$APP/Contents/MacOS/PhotoCraft"
cp assets/app-icon/photocraft.icns "$APP/Contents/Resources/PhotoCraft.icns"
cp NOTICE ATTRIBUTION.md "$APP/Contents/Resources/"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$VERSION/g" \
    -e "s/@BUILD_SHA@/custom/g" packaging/macos/Info.plist.in > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
codesign --force --sign - --options runtime \
    --entitlements packaging/macos/entitlements.plist "$APP"
```

**注意**：release 构建需要约 10 GB 磁盘。`target/debug` 会占 20 GB 以上，
构建 release 前建议先 `rm -rf target/debug`。

---

## 8. 上游跟随记录

| 日期 | 上游改动 | 重叠文件 | 结果 |
|---|---|---|---|
| 2026-11-09 | 首次导入基线（0.20.1） | — | 四项自研功能全部移植 |
| 2026-11-10 | 221 个文件修改、32 个新增 | 4 个 | 逐个重放我们的增量 |

### 第二次跟随（2026-11-10）细节

上游新功能：Average 滤镜、color glyphs、cylinder warp、`toolbar_side` 面板偏好、
`damage_span` 增量重绘、perspective crop 改进等。

与我们重叠的 4 个文件：

| 文件 | 上游改动 | 我们的改动 | 处理 |
|---|---|---|---|
| `panels/toolbar.rs` | 673 行（缩进重排） | 35 行（工具分组） | 重放成功 |
| `canvas.rs` | 226 行 | 32 行（Esc 导航、压感接线） | 重放成功 |
| `panels/panel_docking.rs` | 371 行 | 28 行（浮动面板阴影） | 手工插入 |
| `tools/brush_picker.rs` | 226 行 | 2 行（tip_texture 签名） | 重放成功 |

另外把上游新增的 API 补进我们覆盖过的文件：

- `doc_state.rs`：`damage_span` 字段 + `damage_from()`
- `lib.rs`：`whole_redraws` 计数器
- `state.rs`：`ToolbarSide` 枚举、`brush_lane`/`brush_secondary_section`
- `brush_panel.rs`：`BrushesPanelState.show_search`
- `gesture.rs`：`finish_stroke` 的新 `line` 参数
- `tool_input.rs`：`perspective_crop_ui::pointer` 的新 `mods` 参数
- `options_bar.rs`：Gradient 分支改用上游新版

### 已知的上游自带失败（与合并无关）

`photocraft-engine` 有 3 个测试在**纯上游快照上同样失败**：

- `artboard_cmds::tests::a_board_past_the_origin_moves_the_color_samplers_and_symmetry_with_the_document`
- `artboard_cmds::tests::duplicating_a_board_beside_one_past_the_origin_moves_the_color_samplers_with_the_document`
- `warp_cmds::tests::untouched_cylinder_committed_from_warp_mode_stays_a_cylinder`

前者是色板采样器 0.5px 的像素中心偏移，后者是 cylinder warp 的模式判定。
**修它们不属于本仓库范围**，跟随上游自行修复即可。

## 9. 自研改动：Ctrl+S 写回（2026-11-19）

上游只有在能装下整份文档的格式里才把保存当成"写回"（`crates/engine/src/cmd/file_cmds/paths.rs`
的 `saves_in_place` / `holds`），平铺格式一律退回"另存为"，因此打开或存成 PNG/JPG 之后每次
Ctrl+S 都会再弹一次文件选择框。改动三处：

- `paths.rs`：`layered_extension` 增加 `sssp`，`holds` 里 SSSP 视为能容纳整份文档（含印刷调整层）。
- `menus/invoke.rs` 的 `file.save`：文档已有自己的文件、且平铺文件仍装得下当前内容
  （`services::plain_raster`，单层、满不透明度、无蒙版/效果、Normal/PassThrough）时直接原地写回；
  外部程序改过该文件仍先问（`disk_change_prompt`）。
- `app/files.rs`：新增 `save_own_flat`，平铺自有文件原地保存时只留 JPEG/TIFF/Targa/DDS 选项，
  步骤记为 `file.save` 而非 `file.saveAs`。

口径：**无文件名第一次保存仍弹另存为**；平铺文件装不下（加了图层/文字/蒙版等）仍走"另存为"，
不无声压平原图。测试见 `file_dialog/tests.rs`、`file_open/tests.rs`、`file_cmds/tests.rs`。

### 备份

旧版 0.23.6 完整源码：`../backup/PhotoCraft-source-20261110-v0.23.6-final.zip`（22MB，不入库）

2026-11-14 换版本时，被替换的上一版 0.23.6 WinTab 工作树整目录留档在工作区
`backup/photocraft-winTab-source-20261114-1442/`（约 16.9 GB，含 `target/`，不入库）。
