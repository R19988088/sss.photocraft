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

### 2026-11-21：涂抹的载体反馈上 GPU，工作面不再每 dab 重读

同一套 bench（900 px / 1% 间距，2520×1575，release）：

| 笔（900 px / 1%） | 上一版 p50 | 现在 p50 |
|---|---|---|
| 单色 | 11.0 ms | 11.0 ms |
| 图案 | 37.6 ms | 37.5 ms |
| 涂抹 V1 | 319 ms | 147 ms |

两块改动：

- **工作面不再每 dab 重读**：原来 `needed = rect.inflate(halo)`，`halo = radius*(0.18+0.82*blur)
  + 2*radius*spacing`（900 px 笔 ≈180 px），900 px 的一个 dab 并集就被推过 `MAX_PIXELS`（4 M），
  于是每 dab 都 `flush()` + 清空 + 从层里重读一遍（≈75 MB），涂抹的 `transport` 分项因此高达
  97 ms/帧。同一笔画的相邻 dab 只差一个间距步，上一笔的载体又落在这些方块内，所以只需要
  `rect.inflate(2)`：并集落回预算内，`grow_to` 只读新长出来的条带。**这一项同时把 CPU 路径从
  319 拉到 147 ms**（原来 CPU 路径也吃同一个浪费）。
- **C3 载体反馈（V1）做成 compute pass**：`crates/gpu/src/paint_transport.{rs,wgsl}` 在一帧内
  用一次 submit 跑完所有 dab 的 build/lay，层缓冲在 GPU 上跨 dab 连续，帧末只回读脏区域一次。
  V2/V3、颜料混合、工作面超 64 MB 时回落 CPU 逐 dab 路径。

分项（`PHOTOCRAFT_PAINT_PROFILE=1`，涂抹 147 ms/帧）：mask 回读 82、accumulate 22、tr-weights 12、
transport 10（GPU 通道自报：upload 10 ms、record 0.4、submit 4、wait 0、copy 3 for ~9 dabs）、
rest 40。**transport 从 97 掉到 10 ms，此后涂抹的账几乎全是 mask 回读与 CPU 累加。**

### 再一刀：mask 只读要用的道，涂抹的覆盖只留一份

| 笔（900 px / 1%） | 上一版 | 现在 | 纯 CPU 回落 |
|---|---|---|---|
| 涂抹 V1 | 147 ms | 115 ms | 273 ms |

- **mask 回读不再读 Wet Edges 的 depth 道**：`MaskBatch` 多了 `want_depth`（= 这一帧 `wet`），
  `read_masks` 只拷 coverage，depth 那条 2 B/px 的拷贝和整段解码都省了；`fill` 的尺寸闸门
  相应放宽到「不要 depth 时不比长度」。masks 82 → 55 ms。
- **一个 dab 的覆盖只留一份**：走到后端的那一支原来既给 `Footprint` 拷一份 `cov`、又给
  `TransportDab` 拷一份（900 px 一笔 13 MB × 2 × 9 dab/帧），现在后端支的 footprint 不带 cov
  （`feedback_plan` 只看 `dab`），CPU 支才建。transport 10 → 4.3 ms。
- **一帧的后端工作按 64 MB 预算切块**：`pending_area` 超预算就先 `flush_pending`，长笔画不会
  一次把整幅文档压在显存里，`takes_backend` 也不再依赖「region 已经长好」这个副作用。

分项：涂抹 115 ms/帧里 masks 55、accumulate 22–24、tr-weights 12、transport 4.3、rest 44。

### 画笔参数的账（2026-11-22 逐项查过）

先按「参数 → 落在哪条路径 → 大笔刷一帧的代价（900 px / 1%，2520×1575）」排一遍，慢的都在下面三档：

**A. 留在折叠帧（GPU 一次画完、一次小回读，10–37 ms/帧）**
`size`、`hardness(+dynamic)`、`spacing`、`flow`、`opacity`、`pressure_*`、`tip`（含采样位图）、
`angle`、`roundness`、`flip_x/y`、`aliased`、`triangle`、`c3_tip_math`、`square`、`texture`（含
`each_tip`）、`scattering`、`shape_dynamics`、`pose`、`transfer`、`color_dynamics`（走 colour lane，
多读一份 paint）、`wet_edges`（多读一份 depth）。

**B. 落回逐 dab 回读（mask 46–55 ms/帧 + CPU 累加 21–24）**
`mix_settings.enabled`（V1 涂抹）、`saturation_record_strength`/`saturation_preserve_output` > 0、
`erase`（直接拒绝折叠）、`pigment`/`canvas_material`、单 dab 湿边描边（`wet_edge.outline=Single`）。

**C. 掉到逐 dab CPU 光栅（约 15 ms/dab，一帧 9 dab ≈ 140 ms，最贵）**
`noise`、`grain`（`StampJob::of_with` 直接返回 None）、单个 dab 超过图集（>16.7 M px，直径约 >4100 px）。

**D. 掉到逐 dab CPU 涂抹（约 17 ms/dab，即 273 ms 那一档）**
`algorithm=V2/V3`、`blur_color_target`、`pigment_mix_strength>0`、`blur_strength>0`（模糊那条 lane 只有 CPU 实现）。

这一轮顺手落地的一刀（对所有 A/B 两档都有效）：**回读缓冲不再每帧新分配**。
`read_masks`/`read_masks_color`/`read` 原来每次 `device.create_buffer` 一块 26 MB 的 MAP_READ 缓冲
（一帧 3 个 chunk 就是 9 块），页错误 + 清零比拷贝还贵；现在 `PaintSurface` 持有 `Staging`
（cov / depth / paint 三槽），够大就复用。masks 55 → 46，涂抹 115 → 103，单色 11.0 → 10.0。

| 笔（900 px / 1%） | 最初 | 现在 |
|---|---|---|
| 单色 | 14.3 ms | 10.0 ms |
| 图案 | 41.3 ms | 35.9 ms |
| 涂抹 V1 | 459 ms（更早）/ 319 ms | 103 ms |

图案这一档的分项（913×986 的并集，一帧 9–10 dab）：masks ≈ 7、**colour ≈ 19–20**、accumulate ≈ 8。
colour 就是 `lane_colors` 的逐像素五抽样，单线程实测 140 ms / 89 万 px ≈ 157 ns/px；
把 4×4 邻域沿行携带（`PigmentWalk`）只省 8 %，因为并行那一档已经被内存/调度吃掉。
所以图案再往下走不是换更快的采样器，而是让合成也在 GPU 上做、CPU 完全不碰逐像素色板。

下一个杠杆（按「一刀能影响多少参数」排序，尚未做）：
1. cov/depth 回读改成 compute 打包成 f32 storage buffer：8 B/px → 4 B/px，A/B 两档都受益，
   masks 46 → 约 28；
2. `noise`/`grain` 搬进 `paint_surface.wgsl`：C 档的 140 ms/帧直接变 A 档，粒度按呼吸感最强的
   一组参数（`grain` 的 4 张灰度图 + `noise` 的哈希）；
3. `erase` 解除折叠拒绝（累加定律 `c←c+ v(ceil−c)` 与顺序无关，折叠应与逐 dab 等价）；
4. V2/V3 与 `blur_strength` 的 compute pass（D 档，273 ms）。



### 试过、量过、没留下：图案色进 colour lane

把 `ColorPattern::sample_pigment` 的五抽样搬进 `paint_surface.wgsl`（RGBA32Float 纹理 +
`textureLoad` 手写双线性），图案色由 colour lane 直接产出：`lane_colors` 的 21 ms 消掉了，
但多出的那一路颜色要么走 `read_masks_color`（多读一份 paint 纹理 + 逐像素除法），要么借用
coverage lane 的空闲通道（g/b/a），两者的 CPU 解码/归一化都在 15–20 ms，正好抵掉省下的采样；
另外 20 次不可过滤 `textureLoad`/像素让 mask 分项从 8 涨到 17–19 ms。净结果 37.5 → 34–43 ms，
不稳定且改变像素算法，**已整段回退**，不留半成品。结论同涂抹：只要图案色还要回读一次到 CPU
参与 `accumulate`，GPU 采样就赢不回来；要赢必须让 accumulate 也在 GPU 上。


所以这两条路径都还停在 CPU 上：涂抹是 C3 输送（`transport.rs::feedback_dab` 每 dab 建 side² carrier
再铺一遍）加每 dab 一次 mask 回读；图案是逐像素 5-tap 采样。CPU 侧再怎么修都下不到 13 ms 量级，
要"对齐主画笔"必须落到 GPU：图案色进 `paint_surface.wgsl` 的 colour lane（`texture_at` 已有采样，
补 rgb 与 alpha），涂抹/模糊做成作用在层纹理上的 compute pass（下面「剩下的事」第 1、4 条）。

## 剩下的事

1. **零回读预览**：把描边面常驻在 `gpu_canvas` 的层纹理上，预览帧不回读，收笔时读脏 tile 一次，
   按 CPU 原算法（含颜料混合）落定。这是 p95 里 9–17 ms 的最后一截（并集越大 read 越贵，约 10 ns/px）。
2. `MAX_ATLAS_PIXELS`（4096²）下大位移的帧仍回落逐 dab，可放宽或分块。
3. 湿边 `edge` 分量、图案相位、颜色动态这三处帧级近似尚未与逐 dab 路径做过逐笔刷目视比对。
4. **图案色与涂抹搬上 GPU**：涂抹的 V1 载体反馈已经上 GPU（上一节），图案色试过 colour lane
   但被回读抵掉、已回退。下一步不再是"把采样搬过去"，而是把**覆盖折叠、权重、accumulate 一起
   留在 GPU**（层纹理常驻 + 帧末只回读脏区），否则 mask 回读 82 ms 和大笔刷的 CPU 累加注定
   让两条路径停在 30 ms 量级以上。


## 约定

- 每片都先跑 `cargo test --release --locked -p photocraft-gpu --test it`；
- CPU 路径保留，任何 GPU 面建不起来（`Unsupported`）都自动回落，不静默丢笔；
- 若开始逐文件复制 ProCraft 源码，需在 `NOTICE` / `ATTRIBUTION.md` 里署名
  （ProCraft, MIT OR Apache-2.0, ArtCraft Team）。
