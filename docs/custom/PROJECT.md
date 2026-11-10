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
| 合并工作副本 | `photocraft-custom-0.24/` |

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
photocraft-custom-0.24/          ← 本仓库
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

### 备份

旧版 0.23.6 完整源码：`../backup/PhotoCraft-source-20261110-v0.23.6-final.zip`（22MB，不入库）
