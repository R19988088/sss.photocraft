# 画笔渲染搬上 GPU（B 路线）

目标：大笔刷（含图案/颗粒/纹理笔尖）和颜料混合都不掉帧。参考实现是 ProCraft
（`E:\000 app\procraft\source\procraft.C-main`，MIT OR Apache-2.0），它的做法是
「每个 dab 一条 GPU 实例、直接画进图层的 GPU 纹理、图案在片元里采样、全程不回读」。
我们的画笔目前是 `crates/paint/src/render.rs` 里的 CPU 逐像素循环，图案/颗粒/采样笔尖的
dab 连 GPU 内核都进不去（`crates/paint/src/stamp.rs:118`）。

**颜料混合（`render.rs` 的 `accumulate_pigment` / `shade_pigment` / `transported_output` /
`lane_composite` 等）是基础，CPU 版本原样保留，不做删改**：GPU 路径按同一算法重写，
逐位差异由保真测试给出（口径：单笔 ≤1 LSB，混色笔人工比对无可见差异）。

## 已落地（第一片）

- `crates/gpu/src/paint_surface.rs` + `paint_surface.wgsl`：一张 GPU 描边面
  - 覆盖 `Rgba16Float`（alpha 承载 dab 遮罩），混合 `(one, one − src alpha)`，等价于
    `CoverageMap::accumulate` 的 `c ← c + v·(ceil − c)`，按提交顺序逐 dab 累加；
  - Wet Edges 深度 `R16Float`，`max` 混合，等价于 `CoverageMap::depth`；
  - 一次 draw 画整批 dab（实例化 quad，实例数据在 storage buffer 里），不阻塞、不回读；
  - 覆盖「计算笔尖」全部变体（C3、传统 tip、抗锯齿、Block、Triangle、小笔尖 4×4 采样）。
- 纹理笔尖（`Texture › each tip`）：`set_texture` 把 `PatternImage` 上成 `R16F` + Repeat/Linear，
  片元里按 CPU `texture_at_point` 的三种坐标模式（画布 / 笔画 / 笔尖局部）取双线性值，再过
  `mask_combine` 的 14 个模式；`divide`/`mixd` 与 CPU 同名同式。
- 纹理深度是**每个 dab 自己的**（`Texture › Depth` 可逐笔尖抖动），所以 `StampJob` 带上了
  `texture_depth`（取自 `Dab::depth`），描边面按 `d.flip.z` 用它，而不是 brush 上的标称值。
- 测试 `crates/gpu/tests/it/paint_surface.rs`：与 CPU `rasterize_into` 逐像素对比
  （容差 1.5e-3，即 16 位浮点本身的步长），以及多 dab 累加顺序、跨帧保留、纹理笔尖
  （Multiply + 画布坐标 + 8×5 图案）逐像素对齐。**六项全绿**（RTX 3080 Ti / Vulkan）。

## 已落地（第二片：接进画布）

- `crates/gpu/src/paint_mask.rs`：`GpuMasker` 实现 `photocraft_paint::stamp::DabMaskBatch`。
  一帧的 dab 一次提交：每颗 dab 占图集里自己的一块（shelf 打包），`draw` 一遍、`read` 一次，
  按 dab 矩形切回掩码。**每颗 dab 一块，互不累加**，所以拿到的就是 CPU `rasterize_into`
  逐 dab 的 `val·alpha` 与 `1−rn`。
- `DabStamp` 多带 `atlas: [f32;4]`（quad 在面上的平移，片元里再加回去），实例仍是文档坐标；
  文档面直接用时是 0，行为与第一片一致。
- `crates/paint`：`StampJob` 多带 `texture_depth`（`Dab::depth`，逐笔尖抖动的纹理深度），
  并新增 `MaskBatch` / `MaskDab` / `DabMaskBatch` / `MaskBatchSlot`（与 `StamperSlot` 同一套注入方式：
  `set_process_mask_batch`，`BrushContext` 新建时取一份）。
- `StrokeRenderer::raster_pending`：帧首一次性向 backend 要这一帧的掩码，合格的 dab 用 GPU 的
  `scratch` / `depth_scratch`，不合格的（采样笔尖、Noise、Grain、单 dab 湿边、设备报错）逐颗回落
  CPU，顺序不变。**下游一行未动**：`accumulate_transport` / `accumulate_wet` /
  `accumulate_pigment`（颜料混合，CPU）照原样按 dab 顺序跑。
- App 接入：`crates/ui-egui/src/app/gpu.rs` 的 `set_wgpu` 用窗口同一 device/queue 装上去，
  GPU 画布回落 CPU 时清空。
- 预算：一批 <10 万像素（小笔刷）直接留给 CPU（一次提交+回读不划算）；图集上限 4096²，
  尺寸按 2 的幂分桶，走动时不再重建纹理。
- 测试 `crates/gpu/tests/it/paint_mask.rs`：10 颗重叠/旋转/双尺寸/不同纹理深度的 dab，
  逐像素对 CPU（容差 4e-3，两个 16 位浮点步长）；同一批画两次结果一致（图集不串状态）。

## 下一片（按顺序）

1. **图集常驻 + 异步回读**：现在每帧一次 `poll(Wait)` 会同步等一次；改成双缓冲、上一帧的掩码用在
   下一帧（画笔对一帧延迟不敏感），并把 `read` 只读 cov/depth 两条 lane（现在连 `paint` 一起读）。
2. **采样笔尖进 shader**：mip 金字塔上传成纹理数组，`sample_clamped` 的 mip 选择在片元里算，
   这样 `sampled_tip` 也能脱离 CPU。
3. **Noise / Grain**：`rng::hash2` 是 64 位 mix64，WGSL 没有 u64；按可见等价重写（或保留 CPU）。
4. **颜料混合上 GPU**：把 `render.rs` 的混合通道按同一算法写成 pass，读改写同一张面，中间量
   `rgba32f`；先做 CPU vs GPU diff 基准图，再切默认。
5. **涂抹 / 湿边 / 模糊**：compute pass。
6. **回退与预算**：CMYK / Lab / 32bit 灰继续走 CPU；VRAM 预算 + 驱逐；分片提交防驱动超时。

## 已落地（第三片：采样笔尖 + 分块 + 便宜回读）

`C:\Users\画画的\Downloads\PW4-常用水彩2026(1).brushset`（104.6 MB，30 个 Procreate 水彩）里
每个 brush 的 `Shape.png` 都是采样笔尖，`spacing` 常见 1%，单笔几百到近万颗叠 dab。

- **采样笔尖进 shader**：`StampJob` 多带 `tip_inv_scale`（`big / 2r`，level 0 的缩放）与
  `tip_level`（`Mips::level_for(2 r ro)`）；`MaskTip` 是 level 0 的 8 位位图（Procreate 的
  `Shape.png` 本身就是 8 位，往返无损），随 `MaskBatch.tip` 交给 backend；`PaintSurface::set_tip`
  u8 纹理 + 自建 mip 链（floor 尺寸，`div_ceil` 的 CPU 版在奇数尺寸上差一级，容差内）。片元分支顺序
  照 CPU 的 `match mips`：triangle > Block > 小笔尖 > **sampled** > 计算笔尖；C3 采样笔尖照
  `c3_sampled_mask`（level 0、9 tap 模糊、`pow(2.2−1.2h)`、边界 faloff），非 C3 走 mip 双线性。
  测试 `a_sampled_tip_masks_like_the_cpu`（64×64 软笔尖、10 颗 dab，逐像素 ≤4e-3）。
- **分块提交**：一帧的 dab 可能远超一张图集（1% spacing 的单笔近万颗）。`batch_masks` 按
  `MASK_CHUNK_PIXELS`（8M 像素）切块，逐块提交，块内才受 backend 的图集上限约束。
- **回读瘦身**：`read_masks` 只读 cov/depth 两条 lane（不读 paint），只读图集里**实际填满的
  extent**（桶化的纹理可能大好几倍），并按行用 `bytemuck` 切片批量转 f32。cov 仍是 `Rgba16Float`：
  试过 `Rg16Float`（省一半字节），但这套混合靠 `src alpha`，无 alpha 的格式一律按 1 混合，累加就
  退化成覆盖 —— 已回退并在常量注释里写明。
- **一次成形**：`tip_mask` 在 `BrushContext::new` 里从 level 0 生成一次，`mask_tip()` 只 clone
  `Arc`；backend 用 `Arc::ptr_eq` 判断「这张笔尖已上传」，不重复上纹理。
- 诊断：`PHOTOCRAFT_PAINT_PROFILE=1` 打出每帧 `masks / cpu raster / rest` 三段耗时；
  `PC_MASK_DEBUG=1` 打出 backend 回落原因。

**实测**（1600×1000、24 个 stroke point、pressure 0.72、RTX 3080 Ti / Vulkan，
`cargo run --release -p photocraft-ui-egui --example pw4_bench -- <brushset> <n> [seg|dab]`）：

| 笔刷 | CPU 单笔 | GPU masks 单笔 | 提速 |
|---|---|---|---|
| PW-绿松木（size 256、spacing 1%、wet） | 66.5 s | 11.2–13.5 s | ~5× |
| PW-月光石（14.8、1%、wet） | 2.76 s | 0.90–1.01 s | ~3× |
| PotterW｜常用水彩套组笔刷规则（896.4、43%） | 1.30 s | 0.15–0.18 s | ~7–8× |
| —————水彩单色系列————（482.4、200%） | 90 ms | 36–39 ms | ~2.4× |

整笔仍慢的根因换了位置：不再是 `rasterize_into`，而是**逐 dab 回读**（大笔刷一颗 dab 就是
514²≈26 万像素，10 B/px）。单 dab 帧（`dab` 模式）PotterW 35.7 ms / 单色 12.6 ms。

## 已量到的关键数（`pw4_bench … draw`）

整笔画在**一张 GPU 面**上、**全程不回读**时（`PaintSurface` + `mask_job` 的真实 dab，含采样笔尖
`set_tip`），PotterW｜常用水彩套组笔刷规则（size 896.4、6 颗 dab、1600×1000）是 **0.20 ms/帧**，
水彩单色（482.4、3 颗）**0.21 ms/帧**。也就是说「逐 dab 实例化 + 片元算笔尖/图案」这一段已经是
ProCraft 的量级，卡的从来不是它。

同一次实验里读回**整张文档**（1.6M 像素、cov+paint+depth ≈16 MB）要 **41 ms** —— 这是「每帧回读」
的固定税，也解释了为什么逐 dab 回读（256 笔刷 6916 颗 × 26 万像素 ≈18 GB）会到 11 s。

所以第四片的形状确定了：描边面整笔常驻 → 直接接进 `crates/ui-egui/src/canvas/gpu_canvas`
（已有 egui_wgpu paint callback、tile + mip、预算与 damage），预览期间零回读；落笔/切层时只读脏
tile 一次，按 CPU 原算法（含颜料混合）落定。回读那 41 ms 也要顺手换掉（按行批量转 f32、只读脏区、
不读用不上的 lane）。

## 已落地（第四片：帧折叠 / aggregate frame）

一帧的所有 dab 在 GPU 上折成一张并集：`PaintSurface` 每帧清空一次、整批实例化 quad 一遍画完，
一次 `read_masks`（cov 8 B + depth 2 B/px，颜色动态再加 paint lane 8 B/px），CPU 只按并集跑一遍
`accumulate_wet` / `accumulate_pigment`。数学依据是

\[ c \leftarrow c + v\,(\mathrm{ceil} - c) \ \Longrightarrow\ (\mathrm{ceil} - c_n) = (\mathrm{ceil} - c_0)\prod_i (1 - v_i) \]

所以从空（ceil=1）折出的 `total` 除以 `ceil` 就是本帧等效的 `v`；cov 与 depth(`max`) 精确，
material 的 EMA 权重可乘故也精确，只有 `edge` 取本帧最终 depth（与整笔湿边同样是近似）。

- 单/双 lane 竖排在同一张 surface 上，一次 clear、两次 stamp、一次 read，按行切回；
- 图案色：整帧 union 上用该 lane 首个 dab 当参考逐像素采样（同帧 dab 相距数像素 → 相位亚纹素偏移）；
- 颜色动态：每 dab 自己的颜色随实例上传（`DabStamp` 28→32 float、WGSL `Dab.color`、
  `out.paint = vec4(color.rgb * cov, cov)`），回读后逐像素 `rgb / cov` 还原直色；
- 未使用时的回退：`ctx.secondary` 为 `None` 时生成的成对 dual dab 直接忽略（逐 dab 路径同样丢弃）；
- 拒绝理由打点：`PC_FOLD_DEBUG=1` 输出 `frame of N dabs kept per-dab: <reason>`，只剩空帧与
  `backend refused`（大位移单帧 union 超 `MAX_ATLAS_PIXELS`）。

30 个 PW4 水彩 `live` 模式 p50（1600×1000，每帧一个输入点、9 px 间距）从 130–212 ms 降到
0.5–7.7 ms；最重的白夜梦 / 昼梦里 / 绿松木 7.5–7.7 ms（≈130 fps），其余多在 1–4 ms。

## 量到的账：图案模式与混色/涂抹（2026-11-21）

探针 `crates/ui-egui/examples/paint_modes_bench.rs`（2520×1575、48 帧、每帧一个输入点、9 px 间距、
release、`masker_slot` 装上真实 GPU 描边面）：`paint_modes_bench <size> <spacing%> [solid|pattern|smudge|smudge+pattern]`，
`PHOTOCRAFT_PAINT_PROFILE=1` 出分项，`PC_FOLD_DEBUG=1` 出折叠拒绝理由。

| 笔（900 px / 1% 间距，≈50 dab 一帧） | 改前 p50 | 现在 p50 |
|---|---|---|
| 单色 | 14.3 ms | 11.9–14.7 ms |
| 图案 | 41.3 ms | 41.4 ms |
| 涂抹 V1 | 459 ms | 335 ms |
| 涂抹 + 图案 | 585 ms | 469 ms |

900 px / 5% 间距：涂抹 83 → 62 ms，涂抹+图案 147 → 82 ms。图案在这一档 41 → 39 ms。

两处改动：

- `CoverageMap::accumulate_transport` 按 tile 遍历，一 tile 一次 `tiles.get_mut`（原来一像素一次
  HashMap 查找）：该分项 183 ms → 19–24 ms/帧；
- `LaneTransport` 的工作面按笔画共享：`mix`/`blur` 的像素一整个笔画只读一次、每帧只写回一次
  （原来是每 dab 读一次、写一次），新增 `Region::grow_to` 只读新长出来的四条带；工作面超过
  4 M px 就借 `flush()` 后的新鲜像素重开一块，避免整幅文档存两份。

剩下的账（分项，同一次运行）：涂抹 335 ms 里 transport 254–282、mask 回读 94、tr-weights 19–24、
accumulate 25–30；图案 41 ms 里 mask 7.6、`lane_colors` 20.2、accumulate 8.1。

所以这两条路径都还停在 CPU 上：涂抹是 C3 输送（`transport.rs::feedback_dab` 每 dab 建 side² carrier
再铺一遍）加每 dab 一次 mask 回读；图案是逐像素 5-tap 采样。CPU 侧再怎么修都下不到 13 ms 量级，
要"对齐主画笔"必须落到 GPU：图案色进 `paint_surface.wgsl` 的 colour lane（`texture_at` 已有采样，
补 rgb 与 alpha），涂抹/模糊做成作用在层纹理上的 compute pass（下面「剩下的事」第 1、4 条）。

## 剩下的事

1. **零回读预览**：把描边面常驻在 `gpu_canvas` 的层纹理上，预览帧不回读，收笔时读脏 tile 一次，
   按 CPU 原算法（含颜料混合）落定。这是 p95 里 9–17 ms 的最后一截（并集越大 read 越贵，约 10 ns/px）。
2. `MAX_ATLAS_PIXELS`（4096²）下大位移的帧仍回落逐 dab，可放宽或分块。
3. 湿边 `edge` 分量、图案相位、颜色动态这三处帧级近似尚未与逐 dab 路径做过逐笔刷目视比对。
4. **图案色与涂抹搬上 GPU**：图案色走 mask pass 的 colour lane（`texture_at` 补 rgb/alpha，
   替掉 `lane_colors` 的 20 ms）；涂抹/模糊做成作用在层纹理上的 compute pass，先读后写的 hazard
   用 ping-pong 或同一张读、另一张写。这两条不落地，图案 41 ms、涂抹 335 ms 就是 CPU 的天花板。


## 约定

- 每片都先跑 `cargo test --release --locked -p photocraft-gpu --test it`；
- CPU 路径保留，任何 GPU 面建不起来（`Unsupported`）都自动回落，不静默丢笔；
- 若开始逐文件复制 ProCraft 源码，需在 `NOTICE` / `ATTRIBUTION.md` 里署名
  （ProCraft, MIT OR Apache-2.0, ArtCraft Team）。
