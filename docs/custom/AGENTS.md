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

## 3. 出包（macOS arm64，本机无 libheif，不加 `--features heif`）

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

1. **dab 间距只跟距离与尺寸有关**：`dynamics.rs` 的 `DabBuilder::step_len`（以及 Dual Brush 的
   `DualBuilder::step_len`）= `max(0.5, 直径 × spacing%)`，直径取 `base_diameter(...)` 且不小于 1px。
   `PathWalker` 里**没有**按时间/速度出 dab 的分支（`speed_spacing`、`SPEED_SPACING_MS`、
   `MIN_SPEED_STEP` 已删除）；`spacing_enabled` 只为老预设兼容，不再改变规则。
   慢画、快画、带时间戳、不带时间戳、分块送入，dab 列表必须一致。
2. **插值是采样点之间的线性等距插值**（`PathWalker::push` → `lerp_pt`）。不要引入 cubic / 曲线 /
   handle / 「曲线圆角」开关：试过，会让笔迹脱离指针（用户判为「强制开拉绳、乱定位」）。
3. **指针稳定器只管压力**：`tablet_pressure.rs` 的 `TabletPressureFilter` 只在 `input/stylus.rs`
   里给压力用；它不参与笔迹几何，不要把它接进 `DabGenerator`。
4. **确定性的描边不插帧**：矢量 `path.stroke`（`vector_cmds/paths.rs`）与修图类工具
   （`retouch_cmds.rs::parse_brush`，减淡/加深/海绵/模糊/锐化/涂抹/修复/仿制）把 dabs 直接混进
   自己的像素、没有「待定段」这一层，所以这两处显式把间距当纯比例、不引入任何曲线。
5. **唯一特殊路径**：像素网格铅笔（`aliased && size × spacing ≤ 1px`）走 `PathWalker::pixel_grid`，
   一个像素一步，这是本仓库独有特性，别改成距离步进。

## 6. 交付物

- `dist/release/PhotoCraft.app`、`dist/release/PhotoCraft-0.24.0-arm64.zip`（`custom-merged` 的构建产物）。
- 这两个目录不进 git；每次出包重建，别在旧 bundle 上原地替换二进制。

## 7. 别做的事

- 不 `git push` 到 `main`（`main` = 上游原样）；强推历史前先建 `backup-*` 分支。
- 不改根 `AGENTS.md` 里 `craft-house-rules` 标记块（那是上游同步来的规则文本）。
- 不把 `.DS_Store` 提交进仓库。
- 不以「改断言迁就失败」的方式让测试变绿：先弄清被测行为对应哪条真实规则。
