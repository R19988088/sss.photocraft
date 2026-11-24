# 接手备注（自定义合并版）

给下一个接手的人或 AI 的操作说明。规则先读根目录 `AGENTS.md` 的 house rules，这一份只写本仓库
特有的东西：分支怎么摆、怎么跟随上游、怎么出包、笔刷与间距的硬约定、以及已知会红的测试。

## 1. 分支模型

| 分支 | 含义 | 谁动它 |
|---|---|---|
| `main` | **上游原样**，不加任何自有改动（当前 = 2026-11-10 快照 `f2102b7`，版本号 0.20.1） | 只在换上游快照时更新，别往里塞改动 |
| `upstream-YYYYMMDD` | 每次跟随的上游快照存档，内容与上游逐字节一致 | 新增，不改旧 |
| `custom-merged` | **上游 + 我们自己的提交**，日常开发与出包都在这条线上 | 我们的提交都提交到这里 |
| `backup-*` | 危险操作（合并、强推）前的保险分支 | 确认不再需要后删 |

判断「哪边是上游」永远看 `main`；判断「哪版能跑」永远看 `custom-merged`（`origin/custom-merged`）。

## 2. 跟随上游的步骤

1. 新快照进 `upstream-YYYYMMDD`（内容 = 上游原样，一个自有改动都不许混进去）。
2. `git switch custom-merged` → `git merge upstream-YYYYMMDD`，只在我们真正改过的文件上解冲突。
3. **不要**整体覆盖 `crates/paint/src`、`crates/ui-egui/src`；`UPSTREAM.lock` 里 `frozen.paths`
   列出的文件，上游改动一律手工挑拣。
4. 更新 `UPSTREAM.lock` 的 `upstream.commit` / `upstream.date`。
5. 合并后按第 4 节跑测试，再按第 3 节出包。

## 3. 出包

### 3.1 macOS arm64（heif 与绿色模式已默认开启，无需再传 `--features heif`）

```bash
# 1) 编译
cargo build --release --locked -p photocraft --target aarch64-apple-darwin   # 约 5-7 分钟

# 2) 重建 bundle —— 必须先删旧 .app，否则里面的文件时间戳还是上一次的
rm -rf dist/release/PhotoCraft.app dist/release/PhotoCraft-0.24.0-arm64.zip
mkdir -p dist/release/PhotoCraft.app/Contents/{MacOS,Resources}
cp target/aarch64-apple-darwin/release/photocraft dist/release/PhotoCraft.app/Contents/MacOS/PhotoCraft
cp assets/app-icon/photocraft.icns dist/release/PhotoCraft.app/Contents/Resources/PhotoCraft.icns
cp NOTICE ATTRIBUTION.md dist/release/PhotoCraft.app/Contents/Resources/
sed -e "s/@VERSION@/0.24.0/g" -e "s/@SHORT_VERSION@/0.24/g" \
    -e "s/@BUILD_SHA@/$(git rev-parse --short HEAD)/g" \
    packaging/macos/Info.plist.in > dist/release/PhotoCraft.app/Contents/Info.plist
printf 'APPL????' > dist/release/PhotoCraft.app/Contents/PkgInfo
find dist/release/PhotoCraft.app -type f -exec touch {} +     # 统一时间戳
codesign --force --sign - --timestamp=none --options runtime \
    --entitlements packaging/macos/entitlements.plist dist/release/PhotoCraft.app/Contents/MacOS/PhotoCraft
codesign --force --sign - --timestamp=none --options runtime \
    --entitlements packaging/macos/entitlements.plist dist/release/PhotoCraft.app
dist/release/PhotoCraft.app/Contents/MacOS/PhotoCraft --version   # photocraft 0.24.0 (dev build)
spctl -a -t exec -vv dist/release/PhotoCraft.app                  # accepted

# 3) 打包
cd dist/release && ditto -c -k --keepParent PhotoCraft.app PhotoCraft-0.24.0-arm64.zip && touch PhotoCraft-0.24.0-arm64.zip
```

`ditto` 会把 bundle 里每项文件原有的修改时间封进 zip；只换二进制、不重建 bundle 的话，
用户在 Finder 里看到的还是旧日期——所以第 2 步必须整个删掉重做。

### 3.2 Windows x64 绿色版（默认开 heif，数据跟着 exe 走）

`apps/photocraft/Cargo.toml` 的默认特性是本仓库定死的：

```toml
default = ["heif", "portable-default"]
```

- `heif`：HEIC/HEIF 打开能力。解码是纯 Rust 的 `heic-rs`（`crates/heif`），**不需要系统
  libheif**，任何平台都能直接编进去；关掉后 HEIF 文件仍被识别，打开时给「不支持」。
- `portable-default`：Windows 上 `app_dirs.rs` 的 `PORTABLE_BY_DEFAULT` 为真，不需要
  `portable.txt`。首次启动就在 exe 旁建 `PhotoCraftData\`，preferences / Presets / logs /
  `ui.ron` / Recovery 全在里面，不写 `%APPDATA%`、`%LOCALAPPDATA%`；只有 exe 所在目录不可写
  （只读盘、Program Files）才回退到 `%APPDATA%\Photocraft` 并给出警告。

编译（静态 CRT，拷走 exe 就能跑，不需要 VC++ 运行库；约 13–20 分钟）：

```powershell
Set-Location -LiteralPath 'E:\000 app\s.PhotoCraft\source\sss.photocraft'
$env:RUST_MIN_STACK = '67108864'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
$env:PHOTOCRAFT_BUILD_SHA = (git rev-parse --short HEAD)
$env:PHOTOCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
cargo build --release --locked -p photocraft -j 6 2>&1 |
  Tee-Object -FilePath 'E:\000 app\s.PhotoCraft\build\build-<版本>-green.log'
```

`-j 6` 与 `RUST_MIN_STACK` 不能省（默认并发会让 rustc 崩，见根 `AGENTS.md`）。出包：

```powershell
$ver = '0.24.0'
$stage = "E:\000 app\s.PhotoCraft\dist\photocraft-$ver-windows-x64-portable-green"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\photocraft.exe $stage
Copy-Item LICENSE-APACHE, LICENSE-MIT, NOTICE, ATTRIBUTION.md, README.md $stage
Copy-Item packaging\windows\portable.txt $stage   # 只是给上游版式留个说明，本仓库不加也照样绿色
Compress-Archive -Path "$stage\*" -DestinationPath "$stage.zip"
```

验证：删掉 `$stage\PhotoCraftData` 后运行 `photocraft.exe`，它应当在 exe 旁边重建
`PhotoCraftData\`（含 `logs\`），而 `%APPDATA%\Photocraft` 的修改时间不变。

要打安装版（MSI，装进 Program Files，目录只读）时反过来关掉绿色默认，否则每次启动都会
走「不可写 → 回退 %APPDATA%」并提示：

```powershell
cargo build --release --locked -p photocraft -p photocraft-cli `
  --no-default-features --features heif,portable-default  # 需要绿色时
# 安装版：--no-default-features --features heif
```

## 4. 测试

```bash
cargo test -p photocraft-paint       # 应 209 passed / 0 failed / 1 ignored
cargo test -p photocraft-engine     # 应 3373 passed / 3 failed / 15 ignored
```

三条已知失败在本仓库里是既有的，与笔刷无关，别去「顺手修」：

- `cmd::artboard_cmds::tests::a_board_past_the_origin_moves_the_color_samplers_and_symmetry_with_the_document`
- `cmd::artboard_cmds::tests::duplicating_a_board_beside_one_past_the_origin_moves_the_color_samplers_with_the_document`
- `cmd::warp_cmds::tests::untouched_cylinder_committed_from_warp_mode_stays_a_cylinder`

## 5. 笔刷与间距的硬约定（动之前先读这里）

用户对笔迹的判断口径是画面：**直线棱角 = 点数不足；墨迹落后指针 / 每笔乱跑 = 定位被破坏**。
下面几条是踩过坑之后定下来的，不要回退：

1. **dab 间距跟着 dab 的实际尺寸走**（ProCraft 口径，2026-11-19 用户推翻旧规则后定下）：
   `dynamics.rs` 的 `DabBuilder::step_len`（以及 `DualBuilder::step_len`）= `max(0.5, 实际直径 × roundness × spacing%)`；
   实际直径取 `controlled_input` 按压力/倾斜/速度算出的 `base_diameter`（含 `tilt_size` 的加宽），
   所以笔画变细时间距同步变细、重叠密度恒定。旧规则（`size × spacing%`，dab 缩而间距不缩）正是
   「间距随速度变稀 / 一变细就断」的来源，别再改回去。
   `PathWalker` 用**相邻两个 gap 的均值**推进：`next_at += 0.5*(上一个 gap + 当前 gap)`，首个 gap
   就是首个点的 `step_len`（ProCraft `StrokeBuilder::step/step_as` 的 `gap()`）。
   **乘法必须在 f32 里做**：`b.spacing` 用 `f64::from` 展开会带上 f32 的 1e-9 误差，段长 100px 处
   `100.00000149 > 100 + 1e-9`，段末最后一个 dab 被吃掉（`render::material_regression` 会红）。
   ProCraft 的圆头 1.226 放大（`ROUND_TIP_SHARE`）与 `+extra` 半像素额外间距**没有**照搬：本仓库保持
   PS 的精确密度（size 20 / spacing 25% = 5px，21 个 dab）。
   `PathWalker` 里仍然**没有**按时间/速度出 dab 的分支（`speed_spacing`、`SPEED_SPACING_MS`、
   `MIN_SPEED_STEP` 已删除）；`spacing_enabled` 只为老预设兼容，不再改变规则。
   慢画、快画、带时间戳、不带时间戳、分块送入，dab 列表必须一致（`brush_tests::pitch_follows_...`）。
2. **输入点之间沿向心 Catmull–Rom 曲线走**（ProCraft `walk_curve` 口径，2026-11-19 用户
   「尽量跟 procraft 一样」后改定）：`PathWalker::push` 把「上一个点 → 这个点」这一段交给
   `curve_pieces`，切成最多 `MOST_CURVE_PIECES=64` 段、每段不超过 `CURVE_STEP=4` px，段内位置取
   `curve_point`（ProCraft `catmull_rom`，只有前邻点时是过三点的抛物线——和参考项目 `flush` 里
   `d: None` 的那一段同式），压力/倾斜/旋转/轮沿段线性变化，最后一段正好落在输入点上；
   dab 间距仍按每段长度累加推进（`next_at += 0.5*(gap+gap)`），只是段更细。
   **速度与软化仍在真实输入点上结算**（不按曲线小段结算）：段内时间被切碎会让
   `frames_since` 小到 0.04 帧，`soften` 的 `2/frames` 会把起笔压力拉毛成上百像素。
   这是「笔点稀（快画、60Hz WinTab）却看起来是折线」的解：曲线只补段内长度，端点一个不动。
   旧规则（采样点之间只走直线 `lerp_pt`）已废除。
   早年试过的「强制拉绳 / 把点挪到平滑后的位置」仍然禁止——那种做法会挪端点、让墨迹落后指针
   （用户判为「强制开拉绳、乱定位」）；现在的做法只弯曲段内的路径。
3. **指针稳定器只管压力**：`tablet_pressure.rs` 的 `TabletPressureFilter` 只在 `input/stylus.rs`
   里给压力用，而且**按样本**喂：一帧里笔报了几个样本就过几次（`Stylus::advance_pressure_filter`），
   再按 move 的时间取对应的那个（`filtered_at`）。按帧只喂一条会让 60Hz 的窗口滞后约 5 帧/80ms，
   抬笔尾巴偏粗。它不参与笔迹几何，不要把它接进 `DabGenerator`。同一时间轴上的配给见第 6 条。
4. **确定性的描边不插帧**：矢量 `path.stroke`（`vector_cmds/paths.rs`）与修图类工具
   （`retouch_cmds.rs::parse_brush`，减淡/加深/海绵/模糊/锐化/涂抹/修复/仿制）把 dabs 直接混进
   自己的像素、没有「待定段」这一层，所以这两处显式把间距当纯比例、不引入任何曲线。
5. **唯一特殊路径**：像素网格铅笔（`aliased && size × spacing ≤ 1px`）走 `PathWalker::pixel_grid`，
   一个像素一步，这是本仓库独有特性，别改成距离步进。
6. **压力按时间配给 move，抬笔守住最后一次读数**（ProCraft 口径）：`StylusFeed` 的每个样本带到达
   时间（`pen_clock_ms`），`update` 把这一帧的样本对齐到 `clock_ms`，`Stylus::select` 用 move 自己的
   时间去前后两个样本间插值——不再「按 index 均分一帧的样本」，那种配法在笔与指针速率不同时配错样本。
   抬笔帧没有样本：`held` 保留笔离开时的读数，`canvas/tool_input.rs` 的 `ToolEvent::Up` 用
   `release_pressure()` 收尾（掉回鼠标的 1.0 就是「抬笔压力接收极差」、每笔末尾突然变粗）。
   压力 0 的样本（悬停、离开）不算接触，帧里全是 0 就整帧丢掉；要瞬间高压（Windows 轻点）由 touch
   的 force 负责，别在 `feed` 里伪造 1.0。
7. **第 6 条是新管线，挂在开关后面，默认关**（2026-11-19 用户要求）：`tools.tabletOptimization`
   → `Stylus::optimized`（`Preferences › 数位板 › 数位板优化`）。关（默认）时走旧链路：`select` 按
   index 均分一帧的样本、`latest` 取帧尾或 feed、滤波每帧只喂一次（`filtered` 只有一个值）、
   `ToolEvent::Up` 用末点压力。开才走第 6 条（按时间插值、按样本滤波、`release_pressure()`）。
   改压力链路时两条都要过：`input::stylus` 的测试里时间相关的用例显式置 `optimized = true`，
   新增用例 `the_tablet_optimization_is_off_by_default_and_a_frames_moves_then_split_by_index`
   锁住默认关。
8. **压感开关与压感曲线的位置**：`tools.useTabletPressure` 的 UI 行已删除（`field_visible` 里和
   `tools.pressureCurve` 一起隐藏），压感永远开启——`Preferences::set` 与 `load_prefs_json` 都把它
   强制成 `true`，脚本/整段写入/旧文件都关不掉。压感曲线的编辑器搬到了 `数位板` 页
   （`prefs_ui::fields::tablet_fields` 直接调 `crate::pressure_curve_ui::editor`），
   存回 `tools.pressureCurve`（`tablet_storage_path` 负责映射）。
9. **压感不做窗口平均，抬笔读笔自己的最后读数**（2026-11-19，用户报「抬笔有错误的压力提升」）：
   - `TabletPressureSettings::pressure_window` 默认从 5 改成 **1**（ProCraft 的压力就是原始读数，
     ProCraft 只有一段 3 样本的一阶软化，没有均值窗口）；`load_prefs_json` 把旧文件里存的
     `pressureWindow` 也拉回 1，否则老配置永远钝。UI 从不暴露这一项，改它只能写配置文件。
   - 抬笔压力走 `TabletPressureFilter::mapped_pressure()`（同一份死区/满压/弹性映射，
     **不进 `Filter16` 窗口、不经坐标延迟线**），样本取 `held`（最后真实接触样本）。
     之前 `release_pressure()` 读 `filtered_at(clock_ms)`：那是窗口均值 + 延迟输出，笔在抬笔时
     变细，窗口里还留着前几个较粗的样本 → 抬笔点明显变粗，就是「抬笔压力提升」的来源。
     **别再让抬笔去读 `filtered_at` / `pressure_now` / `last_emitted`**（都有滞后）。
   - 抬笔的 hold 顺序：`held` → 本帧最后样本 → `prev` → `feed`；**不要**再用 `current`
     （它是某次 move 的插值，跟抬笔位置无关）。
10. **参考项目间距公式的两处刻意差异**（研究结论，2026-11-19）：ProCraft 是
   `(spacing * square_at(p) * thinned * SPACED_SQUARE * roundness + extra).max(0.25)`，其中
   `extra = 0.5 + (spacing - 0.005)/1.414`、`SPACED_SQUARE = 262/256`；本仓库用的是
   `实际直径 × roundness × spacing%`（PS 的精确密度，size20/25% 恰好 5px、21 个 dab），
   没有 `SPACED_SQUARE` 的 2.34% 放大与 `extra` 的 0.5–1.7px 加宽，下限 0.5px 而不是 0.25px。
   这两处是**有意的**，不是漏改：照搬会让每条笔画的 dab 数比 PS 多 10–15%。
   ProCraft 的 `gap()` 里还有 `p.speed * speed_spacing` 加宽项，默认 `speed_spacing = 0`
   （所以参考项目默认也不随速度变稀），本仓库没有这一项，别加回来。

11. **速度、起笔软化与参考项目同式**（2026-11-19，用户要求「要对齐」）：
   `dynamics.rs` 的 `PathWalker` 按 ProCraft 的 `pressed_on` 在**每个输入点**（不是每个 dab）
   结算两点，dab 用段内插值（`PathWalker::note_speed` / `soften` / `frames_since`）：
   - 速度 = 60 Hz 一帧的位移，`SPEED_SLOW=25`（px/帧）以下为 0，到 `SPEED_SLOW+SPEED_SPAN`
     （125 px/帧）满 1，`over.sqrt()`；`SPEED_FRAMES=3` 帧加权平滑，首点直取。`StepInput.speed`
     的语义因此是 0..1，不再是 px/ms —— 别再按 `distance/elapsed` 算，那会让 dab 间距
     （5px 级）永远测不到速度。
   - 压力 = `SOFTEN_FRAMES=2`：`held = 2/frames`、`ramp = min(points/held,1)*held`、
     `soft = (p + ramp*before)/(ramp+1)`，首点直通。作用在 `StrokePoint.pressure` 上（各 dynamics
     都从压力派生），头几个点渐入、之后是按帧计的低通（240Hz 笔约 9 点、约 40 ms）。`frames`
     取相邻输入点的时间差 `dt*0.06`，没有时间时按笔 0.25 / 手指 0.5 帧。
   - 没位移的点（抬笔、原地加压）不软化、不改速度：ProCraft 的 lift 事件不进 `pressed_on`，
     我们的抬笔点也不该被拖高。
   - `pitch_follows_the_stamp_so_the_overlap_holds_as_the_pen_thins` 的时间戳按 240Hz（4ms）
     且压力末段保持 0.2：软化让压力滞后几帧，这是行为不是 bug，别把断言改回去。

12. **笔迹的点来自笔本身，不是系统光标**（2026-11-19，「速度还是影响了距离」的根因）：
   ProCraft 的 `desktop/src/pen/windows*.rs` 读 WinTab 包的 `PK_X`/`PK_Y`，位置按笔的采样率到；
   我们原来只用 winit 的 cursor 事件，实测约 60Hz（一帧一个点，`tool_input` 的 `PENDBG move#`
   相邻两条 17–31ms），快速划动时相邻两点被拉成 ~90px 的长弦，于是笔迹是等宽直线段拼的折线
   （不是 dab 间距问题：放大后看不到 dab 珠链）。
   - `photocraft-tablet` 的 `Sample` 多了 `pos: Option<[f32;2]>`，WinTab 的 `poll` 用
     `Context::to_screen(packet.x, packet.y)` 填（虚拟屏幕像素，y 向下）；Ink/macOS/X11 仍是 `None`。
   - `PenSample.pos` 一路带到 `ui-egui`，`Stylus::pen_track()` 给出这一帧的 `(位置, 时刻)`。
   - `canvas.rs` 的 freehand 分支用这些位置铺笔迹（**不看任何开关**：笔的真实位置不是选项）：
     系统光标只提供**锚点**（这一帧最新的指针位置），笔的位置按 `(p - 最新样本)/pixels_per_point`
     相对换算，因此不需要窗口原点或 DPR 绝对换算；比最后一个点旧的样本丢掉（`at > base`）。
   - 一帧的笔轨迹跨度 < `MIN_PEN_TRACK_PX`(1px) 时整段丢掉（驱动把每个包都映射到同一像素 =
     没有位置），退回 `pointer_moves`；Windows Ink / 鼠标本来就 `pos=None`，也走旧链路。
   - 驱动把数位板映射成非全屏区域时，相对换算的尺度会不对（笔迹比手快/慢），实测到再改成
     绝对换算（要拿窗口原点）。

13. **抬笔收细（end taper）只收压力，按参考项目的 classic 锥度来**（2026-11-19 重做，用户否掉第一版
   后给了两条硬约束：「非常卡」和「应该只收压力而不是其他任何参数，尤其是这个锥形是严重错误的」）：
   - 锥度是**笔刷属性**：`photocraft_paint::brush::Taper`（`enabled` / `end` / `size`）挂在
     `BrushSettings.taper`，默认 = ProCraft 的笔尖锥度 `{ end: 0.2, size: 0.8 }`。`Taper::NONE`
     关掉（`brush_tests::brush()` 与测预览一致性的用例用它，那些用例测的是别的机制）。
   - 长度按**路径距离**算，不按时间：`Taper::span_px()` = `end × CLASSIC_TAPER_SPAN(512)` = 默认
     102.4 canvas px，与笔刷尺寸无关——这就是参考截图里那条长而优雅的锥形，而不是 16.7 ms 的短锥。
     （第一版按 83.34 ms 时间窗做，短得看不出锥度，所以「锥形严重错误」。）
   - 实现在 `photocraft_paint::dynamics::taper_end(brush, &mut pts)`：沿点折线累计长度，
     `grown = 1 - (total - at) / span`（0 = 锥度起点，1 = 抬笔），只乘
     `p.pressure *= 1 - size × grown`，**别的参数一律不动**：尺寸/不透明度怎么跟压力走交给笔刷
     自己的 dynamics。taps（总长 0）和 `Taper::NONE` 原样返回。
   - `DabGenerator` 里**没有** hold / tail 机制：`walk` / `walk_dual` / `push` / `finish` 都是直接
     append，实时路径零额外开销（第一版每帧跑 `tail_preview` 生成器 + 区域回读，用户判定「非常卡」）。
   - ~~锥度在**抬笔时**落~~（2026-11-23 用户要求去掉压力的后处理）：`taper_end` 不再在
     `stroke_with` 里调用，抬笔不再整笔重渲染。原因是重渲染会在笔画尾部留下一块「压力重新计算」
     的痕迹，和预览画出来的尾巴对不上；现在**预览怎样，落笔怎样**，所有笔（实色或湿笔）都吃
     `live_commit` 的像素，`taper_lands` 判据删除。`Taper` 字段仍保留在笔刷/预设里（旧预设兼容），
     但不再生效；`dynamics::taper_end` 只剩单测在用。别再把它接回抬笔路径。
   - 还没照搬的参考项（用户没要求，别顺手加）：`taper.start`（起笔 0.15 渐显）、`opacity` 锥度、
     `tip_animation`、classic taper 的 `run_on`（`RUN_ON_PEN=1` / `RUN_ON_SHRINK=0.66`）。

## 5b. 笔刷文件导入（`.abr/.tpl/.gbr/…`，`crates/io/src/brush_import`）

格式表、嗅探与映射都在这里；工程命令是 `brush.presets.importBrushFile`（`preset_import_cmds.rs`），
参数 `path`/`data` + `group`/`replace`/`select`，返回 `{group, imported, count, folders, format, warnings}`。

- **`.tpl`（Photoshop 工具预设）必须是 `8BTP` + `8BIM` 块，画笔列表在 `tptp` 块里**：每条 =
  u32 名称字符数 + UTF-16BE 名称 + **带版本头的描述符**。版本头（16）由
  `VersionedDescriptor::parse_prefix` 自己读——2026-11-24 修过一次「先手读 4 字节再 parse」
  的双读 bug，那会让每个 `.tpl` 都报 `descriptor version 0`（用户报的「导入 tpl 失败」就是它）。
- 画笔定义有两种写法，`brush_of` 都认：tool 里有 `Brsh` item（`Objc` 或 `VlLs`），或者
  tool 的 class 就是 `Brsh`、画笔键（`Dmtr/Hrdn/Spcn/Angl/Rndn/Cntc`）直接铺在 tool 上；
  后一种会被收进一个 `Brsh` 对象再交给 `.abr` 的同一套映射。非画笔工具（移动/裁剪等）跳过并进 warning。
- 回归口径：`cargo test -p photocraft-io brush_import::` 全绿（`.tpl` 有 6 个合成用例：
  item / class 铺平 / list / 混入非画笔 / 无画笔报错 / version 3）。真实文件实测过
  `.abr`×2 与 `.brushset`（30 个预设）；其它格式（`kpp/.bundle/gbr/gih/.myb/.sai2/.gobrush/.mdp/.sut/`
  Painter 库）目前只有合成用例或完全没测，用户报障时优先要样本文件。
- 2026-11-24 复核：`engine.commands` 里 `brush.presets.importAbr/importBrushFile/importImage`、
  `gradient.presets.importGrd`、`style.presets.importAsl`、`pattern.import`、`swatches.import`、
  `shape.presets.importCsh` 全部注册；`preset_files_ui.rs` 的扩展名表（`PRESET_EXTS` /
  `BRUSH_FILE_EXTS`）与 `open()` 的 ext→命令表一一对上。`read_any` 实测：`0001.abr` → 1 个、
  `书法笔刷发D大.abr` → 2 个（warnings：`phry` 文件夹里 0 个 → 退回顶层；`useBrushSize` 无对应项）、
  `PW4-常用水彩2026(1).brushset` → 30 个；合成 `.gbr`/`.myb` 正常，垃圾字节给明确报错不 panic。
  **注意**：控制通道的 `importAbr/importBrushFile` 只接受 `data`（base64），不接受 `path`
  （automation 的 ambient-path 限制）；单行 payload 约 6 MB 以上会被断开，大文件只能走 UI 或库层测。

## 5c. 混色笔：笔触必须能盖住旧的（`MIX_SHARE`，2026-11-24）

用户口径：**混色可以，但新笔触必须能覆盖之前的内容**（「后来用力下笔的部分无法覆盖开始的部分」）。
根因是 carried（吸起来的画布颜色）在满覆盖时整块替换像素（GPU `smudge.wgsl::deposit` 的
`mix(w, moved, m*g)`、CPU `render.rs::transported_output::under` 的旧式 `above = out.a*(1-k)`），
而混合强度 100 % 时 paint 根本进不了像素——实测「橙底上画白」白占比 0 %（强度 0.5 是 67 %）。

现在的口径：**carried 与笔刷色共享像素，永不取代它**。两条管线同一个常量：

- GPU：`smudge.wgsl` 的 `MIX_SHARE = 0.25` + `clamped_paint_share(laid)`；`deposit()` 在两个 mode
  分支之后、`pigment_lay` 之前按 `share * paint_over` 混入画笔色；`paint_over` 由 `canvas.rs`
  的 `smudge_uniform` 填（仅 `StrokeTool::Paint` 且 `wet_pull.is_none()`，即干性混色笔；
  湿画 / Smudge / Erase / 预览不动）。
- CPU（`photocraft-paint`，也是测试口径）：`render.rs::transported_output` 里
  `const MIX_SHARE: f32 = 0.25`，`under(out, value, k, cover)` 的权重 = `k * (1 - (1-MIX_SHARE)*cover)`，
  直接线性混合（alpha 仍取笔触自己那份）。
- 实测（真实笔刷、画笔板、`paint.stroke` + `document.pixel`）：混合强度 1.0 时白占 75–80 %
  （改前 0 %），0.5 时 88 %，0.25 时 93 %；旧的暗色笔迹上叠白 75 %。混色拖色仍在（换 `MIX_SHARE`
  越小越偏覆盖，越大越偏混色）。
- 单测口径改成新契约：`brush_tests::a_mixing_stroke_covers_what_was_under_it`（满覆盖要上色）、
  `mixing_lays_the_carried_colour_under_the_strokes_own` 删掉，`mixing_drags_the_paint_under_the_stroke`
  的强度改成 0.6/0.9（满覆盖时拖色被 `MIX_SHARE` 压到 1/4，低于这个强度就低于 0.05 的断言阈值）、
  `low_mixing_keeps_...` 的边缘比较改成「开/关混色的像素差」，`the_live_renderer_only_mixes...`
  从「拖色取代笔刷色」改成「笔刷色为主、拖色着色」。**改常量时这 5 条要一起看**。

## 5d. 笔刷设置里的曲线 = 两端点 + 两拖拽手柄（2026-11-24）

用户口径：**把所有曲线的中间三个点去掉，只留左右两个端点和两个拖拽手柄**
（截图是 Brush Settings › 混合 › 混合强度 的曲线框，7 个白点被红框圈住中间 3 个）。

模型（`crates/paint/src/pigment_material.rs::SignedCurve`）从「两端点 + 中间锚点的双段三次
贝塞尔（7 点）」改成**单段三次贝塞尔**：

- 字段 `anchors: [[f32; 2]; 4]`，顺序 = 起点、起点手柄、终点手柄、终点；`enabled` 不变。
  起点 x 恒 0、终点 x 恒 1，两个手柄只在 x∈(0,1) 内、且彼此有序（`normalize_handles*`），
  所以曲线永远还是「输入 → 值」的函数。手柄默认落在 1/3、2/3。
- `line()` = 对角直线（未设置时的默认形状，代替旧的 `[x,x]` 七点）；
  `from_points` / `from_legacy` 把任意点列（含旧预设的 7 点）**最小二乘拟合**成这条单段曲线，
  旧预设的形状基本保留（不再有平台段，坡度会略柔）。
- 序列化：新的存 `"anchors"`；读的时候 `Deserialize` 先认 `anchors`，再认旧 `"points"`
  （7 点走 `from_legacy`、其它点数走 `from_points`），两者都没有才用默认。**旧笔刷文件不用手动迁移。**
  控制通道 `paint.stroke` 的 brush 里仍可传 7 点 `points`（走同一条兼容路径）。
- UI（`crates/ui-egui/src/panels/brush_sections.rs::effect_curve_graph`）只画 4 个点：
  两端点画大点、两手柄画小点并用细线连到各自端点；拖动时端点只改 y，手柄在邻居之间夹紧。
  `signed_curve`（混合强度/厚度/模糊强度…）与 `reference_effect_curve`（默认钟形）同此；
  画布材质曲线（`canvas_edit_curve`）也换成这套，写回时存 `anchors`。
- **不动**：动态里的「响应曲线」（`crates/ui-egui/src/pressure_curve_ui.rs::edit_points` 的
  点击加点 / 右键删点 + Preferences 压感曲线），那是变长点列的另一个模型。
- 参考钟形（`transport::pressure_value` 无曲线时）直接用单段 cubic：手柄高度取 `strength / 0.75`
  让峰值正好落在强度值上；鼠标笔（无压感）直接取强度，不再读曲线最大值。
  单测口径：`pigment_material` 的 `handles_keep_their_order…` / `a_seven_point_preset_is_read_as_the_nearest_cubic`，
  `transport` 的钟形断言从「0.3 压力 ≥0.69」放宽到「0.3 > 0.5、0.5 > 0.65」（单段没有平台段）。

### 5d.1 拖拽手感：`tools.setBrush` 的 `quiet`、位图不进 JSON、面板只能拖标题栏（2026-11-24）

- 曲线一次拖拽 = 每帧一次 `tools.setBrush`。实测（`PHOTOCRAFT_PROFILE_SETBRUSH` 打点，release）
  一次调用 ~28 ms，其中 **`merge_brush` 占 ~20 ms**：它把整把笔刷 `to_value` 后再深度合并，
  而 `touches()` 只摘掉了 `tip` / `dualBrush.tip` / `texture.pattern` 三个位图，
  `sampled_pattern`（`ColorPattern`，最长边 2048，RGBA f32）与 `canvas_material.pattern` 照样
  被序列化。现在 `merge_brush` 把这两个也摘出/回填；`brush_patch`（面板每帧算 patch）的
  `light_json` 同样跳过五个位图（只在真的改变时才进 patch）。
- 回执也不再白造：`set_brush` 认 `quiet: true`，成功时不回传整把笔刷（返回 `null`），
  **Brush Settings 面板的 `send()` 一律带上**（它从不读回执）。控制通道 / 其它调用方不传，
  行为不变。想测真实耗时别从控制通道看：那条路的 `quiet` 不保证透传，看 `build\logs` 的
  frame log 里 `command … (tools.setBrush)` 或开 `PHOTOCRAFT_PROFILE_SETBRUSH=1`。
- `ColorPattern` / `GrayTile` 的 `PartialEq` 改成手写，先 `Arc::ptr_eq` 再比内容：面板每帧
  `before == after` 与 `brush_patch` 都会比较这两张图，比 2048² 的像素本身是白白几十毫秒。
- 面板预览条（`panels/brush_preview.rs::preview_pixels`）里**关掉混合模拟**：命令降到 1 ms 后
  剩下的卡顿全在它身上——680×120、65 个笔尖，开混合的笔刷（如导入的 `勾线` 预设）一次重画
  `apply_stroke` 就要 ~400 ms，而面板里任何一处拖动都会触发一次重画（滑块、曲线、预览全都一样）。
  关掉混合后同一次重画 ~9 ms，预览跟着拖动实时更新。预览画在空白面上，混合本来也无画布可混，
  笔尖形状/间距/纹理都照旧，`color_output` 仍保留（Mixer Brush 的预览还显示它的颜色）。
  别再把它改回 `enabled = true`，否则整块面板的每个拖动都会回到 400 ms/帧。
- 面板（`panels/brush_panel.rs::window`）改成 `.movable(false)` + 每帧 `current_pos`：
  egui 在没有标题栏时默认"任意处可拖窗"（`WindowDrag::Anywhere`），所以在面板上拖滑块、
  拖曲线、拖预览条都会把整个面板拖走。现在只有 `panel_title` 那一行（22 pt）带
  `Sense::click_and_drag()`，按下即按指针位移改 `brush-settings-pos`，并在 canvas 内夹紧。
  改这一段时别把 `Sense::hover()` 还给标题行，也别开 `movable(true)`。
- 曲线框（`brush_sections.rs::effect_curve_graph`）画的是**形状**，不是取值：`SignedCurve::value`
  对 `enabled == false` 的曲线返回 0，直接画会得到"底部一条平线 + 左端点还在高处"的怪样子
  （同一个面板里勾选的曲线有弧度、没勾的是直线，看起来像两种控件）。现在画之前复制一份把
  `enabled` 置真再取样，只把线条颜色改成灰（和该区块其它被禁用的控件一致）。

## 6. 交付物

- `dist/release/PhotoCraft.app`、`dist/release/PhotoCraft-0.24.0-arm64.zip`（`custom-merged` 的构建产物）。
- 这两个目录不进 git；每次出包重建，别在旧 bundle 上原地替换二进制。

## 7. 别做的事

- 不 `git push` 到 `main`（`main` = 上游原样）；强推历史前先建 `backup-*` 分支。
- 不改根 `AGENTS.md` 里 `craft-house-rules` 标记块（那是上游同步来的规则文本）。
- 不把 `.DS_Store` 提交进仓库。
- 不以「改断言迁就失败」的方式让测试变绿：先弄清被测行为对应哪条真实规则。
