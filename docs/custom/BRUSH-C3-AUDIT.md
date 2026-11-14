# 笔刷参数对照 C3 审查

任务来源：用户 2026-11-09 要求
1. 尽量全部使用 GPU，保证最高效率
2. 数学算法要与 C3 一致

## 0. GPU 现状（已确认）

| 环节 | C3 | 本仓库当前 |
|---|---|---|
| 笔尖盖印 | GPU compute `renderBrushStamp` | **CPU** `crates/paint/src/render.rs::BrushContext::rasterize_into` |
| 颜料盖印 | GPU compute `renderPigmentBrushStamp` | **CPU** `accumulate_pigment` |
| 混色盖印 | GPU compute `renderMixStamp` | **CPU** `transport.rs` |
| 笔触合成 | GPU compute `mergeStrokeTileKernel` | **CPU** `composite_tiles`（已并行，但仍是 CPU） |
| 画布显示 | GPU | GPU（`crates/ui-egui/src/canvas/gpu_canvas`） |

结论：**笔刷光栅化目前 100% 在 CPU**。GPU crate 只做图像合成与显示，不接触 dab。
要达到 C3 的 GPU 程度，需要把 `render.rs` 里三个 CPU 阶段整体搬到 wgpu compute，
这是独立的大工程（C3 对应 shader 合计约 900 行 Metal）。

## 1. 已确认的数学差异（待修）

### 1.1 笔尖衰减曲线

C3（`Sources/Shaders/SimpleShaders.metal:905`）：

```metal
static inline float brushTipFalloff(float dist, float radius, float hardness, bool subpixelSample = false) {
    if (hardness >= 0.999) {
        if (subpixelSample) return dist <= radius ? 1.0 : 0.0;
        return clamp(radius - dist + 0.5, 0.0, 1.0);      // 1px 线性抗锯齿
    }
    float h = clamp(hardness, 0.0, 0.999);
    float r = max(radius, 0.0005);
    float t = clamp((dist / r - h) / (1.0 - h), 0.0, 1.0);
    const float k = 2.5;
    return max(0.0, (exp(-k * t * t) - exp(-k)) / (1.0 - exp(-k)));   // 归一化高斯
}
```

本仓库（`crates/paint/src/lib.rs:177,186,241`）：

```rust
const TIP_SHAPE: [(f32, f32); 11] = [...];   // 查表：hardness → (core 比例, shoulder 宽度)
pub fn tip_shape(radius, hardness) -> (f32, f32) { ... }   // 线性插值查表
fn soft_exp(u: f32) -> f32 { ... }           // (1-u/64)^64 六次平方
```

差异：
- C3 是单条归一化高斯曲线，`t` 由 `(dist/r - h)/(1-h)` 归一化。
- 本仓库是「实心 core + 独立 shoulder」两段模型，肩部用多项式近似高斯。

两者在 hardness 中间值时覆盖率分布不同，视觉上笔尖边缘的软硬过渡会不一样。
**待定**：是否改为 C3 的归一化高斯（需同时改 `tip_reach`、`subpixel` 采样与
`TipProfile` 结构，影响所有现有笔刷预设的观感）。

---

## 2. 笔尖与形状动力学对照（agent 报告要点）

### 2.1 好消息：C3 数学已经存在

`render.rs:2209-2254` 里的 `brushTipFalloff` 当 `c3_tip_math = true` 时与 C3 shader
**逐行一致**（归一化高斯 k=2.5、硬笔尖 1px 线性 AA、采样笔尖 8 邻域模糊、三角笔尖）。

但 `brush.rs:819` 的默认值是 `c3_tip_math: false`，走的是 Photoshop 实测模型（TIP_SHAPE 查表）。

### 2.2 已确认需要修的真实差异

| # | 差异 | Rust | C3 | 影响 |
|---|---|---|---|---|
| 1 | 间距用抖动后的直径 | `dynamics.rs:589` `diameter_at(...)` 含 jitter | `BrushDynamics.swift:492-496` 抖动前 | Size Jitter 开启时密度不同 |
| 2 | 最小半径 | `dynamics.rs:706` `.max(0.5)` px | `BrushDynamics.swift:563` `max(0.0005, ...)` | 渐隐尾部 Rust 留 1px 实心点 |
| 3 | Direction 坐标系 | `dynamics.rs:317` `(-dy).atan2(dx)` y-up | `BrushShapeDynamics.swift:156` `atan2(dy,dx)` y-down | 方向驱动的参数随笔画方向反向 |
| 4 | 散布副本抖动 | `dynamics.rs:657-681` 每副本独立抖动 | `BrushCPUEmissionAdapter.swift:148-166` 副本共享基础值 | 散布环形排列不同 |
| 5 | Range Jitter 上限 | 有效可达约 10000% | `BrushScatterResolver.swift:100` 钳 5000% | 极限时散布过大 |
| 6 | legacy PenTilt 角度 | 少 180° 偏移 | 有 | 倾角驱动的笔尖角度方向错 |
| 7 | ShapeDynamics.enabled 门控 | 门控全部通道含 size jitter | 只门控 angle/roundness/stroke jitter | 旧预设 size jitter 失效 |

### 2.3 Rust 多出、C3 没有的（按用户要求应保留或移除需确认）

- 逐 Dynamic 响应曲线 `Dynamic.curve`（C3 无）
- Tilt Scale / Brush Projection（C3 无，且渲染路径已强制关闭）
- `step_len` 的 flow 修正（C3 无）
- Control 的 Speed / PressureRelease / StylusWheel 用于形状（C3 形状控制里没有）

### 2.4 C3 有、Rust 没有的

- Buildup 作为控制通道（size/opacity/flow/hardness 都可选 buildup）
- 两段三次贝塞尔曲线模型 `BrushEffectPressureCurve`

---

## 3. 散布与纹理对照（agent 报告要点）

### 3.1 已用测试复现的缺陷

**Both Axes 完全失效**

```rust
// brush.rs:898
brush.scattering.both_axes = false;
brush.dual_brush.both_axes = false;
```

`bounded_for_render` 无条件关闭，所以 `dynamics.rs:742` 的平行散布分支永不执行。
仓库自带测试 `brush_tests::scattering_stays_in_bounds`（`brush_tests.rs:439`）因此失败。

C3（`BrushScatterResolver.swift:126`）是 `params.bothAxes ? offset(parallel) : 0`，正常工作。

### 3.2 纹理采样是最近邻，不是双线性

```rust
// render.rs:332
p.sample_wrap(x / scale - 0.5, y / scale - 0.5)   // floor + wrap
```

`PatternImage::sample_bilinear_wrap`（`tile.rs:311`）实现了但**只在测试里用**。
C3（`SimpleShaders.metal:1067-1078`）是 4 texel 双线性。

结果：纹理缩放 != 100% 时块状、走样明显。

### 3.3 纹理默认不走 C3 数学

`Texture.c3_math` 默认 `false`（`brush.rs:389`），且全仓库没有任何预设或命令置 true。
`c3_math=false` 时这些模式与 C3 方向不同：

| 模式 | Rust 默认（`texture_combine`） | C3 |
|---|---|---|
| Subtract | `v - d*t`（白纹理扣最多） | `max(v - d*(1-t), 0)`（白纹理不扣） |
| ColorDodge | `v/max(1-d*t, 1e-3)` | `mix(v, min(v/max(1-t),1), d)` |
| ColorBurn | `1-(1-v)/max(1-d+d*t,1e-3)` | `mix(v, 1-min((1-v)/max(t,1e-3),1), d)` |
| Height | `12*round(d*255)/255*v - t` | `t >= 1-d*v ? v : 0` |
| LinearHeight | reach 模型 | `v*clamp((t-(1-d*v))*4+0.5,0,1)` |

### 3.4 散布副本走完整动态（与 C3 不同）

`dynamics.rs:657-749`：每个散布副本用 `next_index` 重新计算 `diameter_at` / `angle` / `roundness`。
C3（`BrushCPUEmissionAdapter.swift:147-163`）：副本只复制基 dab，改位置/颜色/纹理深度，
**尺寸角度圆度完全相同**。

count > 1 时观感差别明显：C3 是「同一颗 dab 挪几个位置」，Rust 是「几颗不同的 dab」。

### 3.5 range 抖动按副本采样

`dynamics.rs:726` 的 `i` 是副本索引；C3（`BrushScatterResolver.swift:99`）每个基 dab 只算一次，
所有副本共用。→ Rust 散布方差偏大。

### 3.6 其它已确认差异

| 项 | Rust | C3 |
|---|---|---|
| range 上限 | jitter≤50 但乘 (1+jitter) 后可达 10000% | 最终 clamp 0..5000% |
| each_tip depth 抖动 | `lerp*(1+jitter*(2r-1))` 对称 | `max((min+(1-min)*c)*(1-jitter*r), minimumDepth)` 只减有下限 |
| 整笔纹理 + 湿边 | 两者叠加 | 整笔纹理激活时关闭湿边（`ContentView.swift:9262`） |
| 整笔纹理 opacity | `combine(c,t)*opacity` | `combine(c/opacity,t)*opacity` 先反预乘 |
| StrokeLocked 原点 | 第一个 dab 中心 | 笔画第一个输入点 |
| TipLocal 整笔锚点 | 最后一个 dab | 第一个 dab |

### 3.7 已核对一致的部分

- 散布 count 控制、count 抖动代数、spacing 抖动、offset 公式、32 dab 上限
- range → amount 的「100% = 半径」语义
- 纹理 brightness/contrast/invert（`c3_math=true` 时）
- Multiply / Darken / Overlay / LinearBurn / HardMix（两条路径都对）
- 程序化纹理 hash/FBM/四种风格公式
- CanvasLocked 坐标、像素中心 0.5 偏移

### 3.8 RNG 说明

`rng.rs:33` 的 `c3_scatter_unit` 注释写「bit-for-bit so a scatter ring matches the reference」，
但它对应的 C3 函数 `BrushScatterResolver.randomUnit` **在生产路径中未被调用**。
C3 实际用的是 `BrushSeededRandom.bits`（6 字 avalanche 哈希，含 target 字）。

所以这个注释是错的，实际不能逐位复现 C3 的散布排列。

---

## 4. 运动 / 混色 / 双笔刷 / 平滑（agent 报告要点）

### 4.1 数学已确认 1:1 一致的部分（不需改）

- **tiltDelay 整套**（`motion.rs:56-108`）：延迟映射 `d^0.55`、离群抑制阈值、高/低延迟两档的
  死区/α/限幅，全部与 `BrushStrokeMotion.swift:134-196` 一致。
- **inertia 分叉**（`motion.rs:145-197`）：累计距离、转弯点积 < 0.3、分叉长度
  `max(step*8, diameter*10)*amount*weight`、压力锥减 `1-0.6*offset/distance`，与
  `BrushInertiaResolver.swift` 一致。
- **`dual_blend` 模式函数**（`render.rs:165-209`）：11 种模式的公式与
  `SimpleShaders.metal:3356-3370` 逐条相同。
- **9-tap 模糊核**（`transport.rs:228-287`）：权重 (0,0,.18) / (0,±1,.14) / (±1,0,.14) /
  (±.7,±.7,.10)，与 `SimpleShaders.metal:2395-2399` 一致。
- **`pressure_value` 钟形曲线**（`transport.rs:350-385`）：控制点、贝塞尔极值求最大强度，
  与 `BrushEffectPressureCurve.swift` 一致。
- **V2 (SAI) retention 公式**：`(1-(1-stretch)^2)^(32*spatialStep/quality)` 一致。

### 4.2 已确认需要修的问题

| # | 问题 | 位置 | 影响 |
|---|---|---|---|
| 1 | `anchor_smooth` 是死字段，`resolve()` 从不读 | `motion.rs:23`（仅声明） | Anchor 模式的两级 EMA（基座 + 方向偏移 + 死区）完全缺失，高 smooth 时无拖尾 |
| 2 | Anchor 半径用 `brush.size` 而非当前解析直径 | `motion.rs:113` | 压感变细时偏移量不对 |
| 3 | 笔迹平滑是「拉绳」模型，C3 是 EMA | `dynamics.rs:99-143` vs `BrushStrokeMotion.swift:88-98` | 模型不同；`MotionSettings.smoothing` 也是死字段 |
| 4 | V1 混色用 Smudge 线性 lerp，C3 用 carrier 反馈 | `transport.rs:112-115` | 拖尾长度/浓度的决定因素不同 |
| 5 | V3 推移上限 `min(dist, 2r*spacing)` vs C3 `min(r, max(dist, 0.45r*length))` | `transport.rs:134` | 默认 spacing=0.25 时最大推移只有 C3 一半 |
| 6 | V2 缺 `min(strength, 0.85)` 上限 | `transport.rs:206` | >85% 时保留过多旧颜料 |
| 7 | V2 缺「起笔透明重置」分支 | `transport.rs:198-205` | 空白画布起笔拾取更慢 |
| 8 | 双笔刷合成先各自 over 再按权重 lerp | `render.rs:1773-1779, 2090-2107` | 重叠处底图透出更多，双笔刷显得更淡 |
| 9 | 模糊用 9-tap 盒式且在沉积之后；C3 可分离高斯且在之前 | `transport.rs:228-287`, `render.rs:1000-1025` | 边缘与叠加顺序不同 |
| 10 | 模糊工具乘了钟形压感曲线，C3 不乘 | `dynamics.rs:794-808` | 模糊浓度随手压变化 |

### 4.3 `bounded_for_render` 清零的混色参数（已核实）

```rust
// brush.rs:952-965
brush.mix_settings.length = 0.5;
brush.mix_settings.concentration = 1.0;
brush.mix_settings.water = 0.0;
brush.mix_settings.smooth = 0.0;
brush.mix_settings.quality = 4.0;
brush.mix_settings.output_strength = 1.0;
brush.mix_settings.blur_color_target = None;
brush.mix_settings.strength_dynamic = Dynamic::default();
brush.mix_settings.length_dynamic = Dynamic::default();
brush.mix_settings.blur_dynamic = Dynamic::default();
brush.mix_settings.length_pressure_inverted = false;
brush.mix_settings.blur_pressure_inverted = false;
```

→ C3 预设里的这些参数**全线失效**。这与 3.1 的 `both_axes` 是同一类问题：
净化的范围过宽，把本来要用的功能也关掉了。

### 4.4 C3 有、Rust 没有

- `anchor_smooth` 的两级 EMA 平滑
- 笔迹平滑的 EMA 模型（`strokeSmoothingLevel`）
- Anchored 模式下跳过平滑（C3 `BrushStrokeMotion.swift:78-85`）

### 4.5 Rust 有、C3 没有

- Brush Pose（C3 完全没有此功能；Rust 有实现但被 `bounded_for_render` 关闭，
  只剩光标预览生效 → 半实现状态）
- `length_pressure_inverted` / `blur_pressure_inverted`
- 简化的双笔刷 legacy 路径


---

## 5. 本轮已修复（代码改动）

按「以 C3 为准」逐项修，每项都跑通了 paint 206 / engine 3298 全测试。

### 5.1 `bounded_for_render` 的净化范围过宽（根因）

原来无条件关闭/重置了一批 C3 本来要用的功能。已解除：

| 解除项 | 影响 |
|---|---|
| `scattering.both_axes = false` | 双向散布恢复（自带测试 `scattering_stays_in_bounds` 由失败转通过） |
| `dual_brush.both_axes = false` | 副笔刷双向散布恢复 |
| `mix_settings` 的 length/concentration/water/smooth/quality/output_strength/blur_color_target | 混色参数不再失效 |
| `transfer.wetness` / `transfer.mix` | 湿度/混合抖动恢复 |
| `color_dynamics.fg_bg` | 前景/背景颜色抖动恢复 |
| `mode = Normal`（非橡皮） | 笔刷混合模式恢复（引擎测试 `brush_blend_mode_multiply` 恢复为「蓝×黄=黑」） |
| `spacing_enabled = true` | 「间距」未勾选时的速度间距恢复 |
| `shape_dynamics.flip_x/y_jitter` | 翻转抖动恢复 |

**仍然保留关闭**（C3 没有对应功能）：`pose.enabled`、`canvas_material`、`brush_projection`、
`tilt_scale`、`contour`、`build_up`、指数平滑。

### 5.2 纹理采样改为双线性

```rust
// render.rs::texture_at_point
let (u, v) = (x / scale - 0.5, y / scale - 0.5);
let (x0, y0) = (u.floor(), v.floor());
p.sample_bilinear_wrap(x0, y0, u - x0, v - y0)
```

原来是 `sample_wrap`（最近邻），`sample_bilinear_wrap` 写好了却没人调用。C3 是 4 texel 双线性。

### 5.3 间距用抖动前的直径

```rust
// dynamics.rs::step_len
let diameter = base_diameter(b, &s).max(1.0);   // 原来 diameter_at()，含 jitter
```

C3 用 `spacingSize`（抖动前），抖动只影响 `radius`。开 Size Jitter 时笔迹密度不再忽密忽疏。

### 5.4 Transfer 的 Fade 保持步数语义

新增 `REFERENCE_TRANSFER_CONTROLS`：opacity / flow 的 `Fade` 不再被重映射成
`DistanceFade`（C3 里只有 shape/size 用距离渐变）。

### 5.5 补全 size 控制可选源

`REFERENCE_SIZE_CONTROLS` 补上 `PenPressureInverse`、`Speed`、`PressureRelease`——
C3 的硬度/尺寸支持这些控制，原来被降级成 Off。

### 5.6 测试对齐（不是放宽，是断言新行为）

`brush_tests.rs` 里 17 个断言依赖旧的净化行为，已改为断言 C3 行为：
Pose / TiltScale / Projection 断言「渲染时被关闭」，noise 改用 grain，
fade 按距离语义，dual brush 显式设 `secondary_strength`。

## 6. 仍未处理（需要你决定）

### 6.1 默认笔尖数学

`c3_tip_math` 默认 `false`，走 Photoshop 实测模型（TIP_SHAPE 查表 + 软笔尖外溢到 2.17R）。
C3 是归一化高斯，严格在 R 处归零。

改 `c3_tip_math: true` 会让**所有现有预设的笔尖形状改变**。

### 6.2 GPU 化

笔刷光栅化仍在 CPU（见第 0 节）。要真正「全部 GPU」需要把
`rasterize_into` / `accumulate_pigment` / `transport` / `composite_tiles`
搬到 wgpu compute，对应 C3 约 900 行 Metal。这是独立工程。

### 6.3 其它算法级差异（已在第 2-4 节列出）

散布副本各自抖动、range 抖动按副本、Anchor 的 `anchor_smooth` 缺失、
V1/V3 混色模型、双笔刷颜色合并方式、模糊的盒式 vs 高斯。
---

## 7. 颗粒与湿边对照（自查，第 4 份 agent 报告 API 失败后手工完成）

### 7.1 Grain（颗粒）

| 项 | Rust | C3 | 一致 |
|---|---|---|---|
| fbm 层数 | 3 | 3 | ✓ |
| fbm 权重 | 0.56 / 0.28 / 0.14，norm 0.98 | amp 0.56 → 0.28 → 0.14，norm 0.98 | ✓ |
| 频率递进 | ×2.07 | ×2.07 | ✓ |
| `noiseScale` | `0.7 + unit((size-1)/199)^1.35 * 20` | 同 | ✓ |
| 域扭曲 | `warp = scale*0.55*jitter^1.35`，双 fbm 偏移 (3.1,7.2)/(11.3,17.5) | 同 | ✓ |
| alpha overlay | `<0.5: 2a·n`，否则 `1-2(1-a)(1-n)` | 同 | ✓ |
| density jitter | `1-amount + noise(x+157,y+89)*amount`，amount=j^1.15 | 同 | ✓ |
| **噪声哈希** | `rng::hash2`（整数哈希） | `fract(sin(dot(p,(127.1,311.7)))*43758.5453)`（正弦哈希） | **有意不同** |

C3 自己的注释写明：「WebUI 用整数哈希；此 helper 保留 C2 的正弦哈希，不追求逐位一致」。
Rust 用整数哈希是为了跨 CPU 架构稳定（`material.rs:235` 注释）。**统计特性等价，排列不同**。

### 7.2 Wet Edge（湿边）

Rust 把 C3 的独立函数内联了，但逐条对比公式一致：

| C3 函数 | Rust 对应 | 一致 |
|---|---|---|
| `weContinuous01` | `continuous`（`material.rs`） | ✓ |
| `weStrength01` | `WetEdgeSettings::strength01` | ✓ |
| `weWidth01` | `WetEdgeSettings::width01` | ✓ |
| `weLiftMax` | `alpha()` 内联的 `lift_max` | ✓ |
| `weColorMask` | `WetEdgeSettings::color_mask` | ✓ |
| `weSoftnessMask` | `alpha()` 内联的 `soften` | ✓ |
| `weRemappedAlpha` | `WetEdgeSettings::alpha` | ✓ |
| `weFilterColor` | `WetEdgeSettings::color` / `color_with_effect` | ✓ |
| 逐 dab 遮罩 | `dab_edge` | ✓ |
| Watercolor2 空间检测 | `spatial_edge` | ✓ |

常数也对齐：Photoshop lift 0.55–0.85、Watercolor 0.40–0.70、Watercolor2 0.50–0.85；
润色上限 0.05/0.27/0.33 饱和度、0.10/0.45/0.54 亮度。

**结论：颗粒与湿边的数学与 C3 一致**，唯一差异是噪声哈希函数（有意为之）。

## 8. 四份对照总结

| 领域 | 数学一致度 | 本轮修复 |
|---|---|---|
| 笔尖与形状动力学 | 高（`c3_tip_math=true` 时逐行一致） | 间距改抖动前直径 |
| 散布 | 中（公式一致，随机序列不同） | 解除 `both_axes` 封锁 |
| 纹理 | 中（`c3_math=false` 时模式方向相反） | 改双线性采样 |
| 颗粒 | 高（仅哈希不同） | — |
| 湿边 | 高（逐条一致） | — |
| 运动/惯性 | 高（tiltDelay、inertia 1:1） | — |
| 混色传输 | 中（V2 基本一致，V1/V3 模型不同） | 解除 mix 参数封锁 |
| 双笔刷 | 中（模式函数一致，颜色合并方式不同） | 解除 both_axes 封锁 |

---

## 9. 用户 2026-11-09 指定三项算法对齐（已完成）

用户要求：「算法需要尽量接近，尤其是 1. 双重画笔的混合和强度控制方式 2. 三种混色模式 3. 模糊的参数」

### 9.1 双笔刷合并方式

**C3**（`SimpleShaders.metal:3404-3432` `dualBlendStrokeTileKernel`）：

```metal
float weight = d.a / max(p.a + d.a, 0.0001);
float3 primaryColor  = p.a > 0.0001 ? clamp(p.rgb / p.a, 0, 1) : float3(0);
float3 secondaryColor= d.a > 0.0001 ? clamp(d.rgb / d.a, 0, 1) : float3(0);
float3 straight = mix(primaryColor, secondaryColor, clamp01(weight));
float4 output = float4(straight * a, a);      // a = dualApplyBlend(p.a, d.a, mode)
// 非 shadow 模式：没有额外的 coverage/unon 因子
```

**原 Rust**（`render.rs::transported_output`）：两条 lane 各自先 `lane_composite` 到 destination，
再按 `weight` lerp，最后乘 `coverage/union` 因子。

**已改为**：直色按 weight 混合 → `dual_blend` 合并 alpha → 一次 over。
去掉了 `coverage/union` 因子（C3 普通笔刷路径没有它）。

### 9.2 三种混色模式

| | C3 | 原 Rust | 已改 |
|---|---|---|---|
| V1 | carrier 反馈：`keep = pow(0.2+0.75*length, max(0.08, spatialStep*4))`，`carrier = mix(fresh, previous, keep)`，再 `result = carrier*coverage + dst*(1-carrier*coverage)` | `Smudge` 预乘线性 lerp | 按 C3 重写为 `feedback_dab` |
| V2 | `blend` 上限 **0.85**（`min(strength, 0.85)`） | 直接用 `strength` | 已加 0.85 上限 |
| V3 | `pushDistance = min(r, max(dist, r*length*0.45))` | `min(dist, 2*r*spacing)` | 已改 |

### 9.3 模糊

| | C3 | 原 Rust | 已改 |
|---|---|---|---|
| 9-tap 核 | 权重 0.18/0.14×4/0.10×4 | 同 | 一致 |
| 半径 | `radius*(0.18+strength*0.82)`，钳 1.5..127 | 同 | 一致 |
| **执行顺序** | **先模糊**，再 smudge/pickup，再沉积 | 先 smudge，沉积，后模糊 | 已改为 blur → smudge → deposit |
| `blur_color_target` | 与**笔画起点图层内容**比较 | 与当前工作像素比较 | 未改（需额外快照，见下） |

### 9.4 仍未做的两处

1. **模糊用可分离高斯**：C3 新笔画走 `strokeUsesGaussianBlur=true`，9-tap 盒式只用于回放。
   这需要新增两趟可分离 pass，属性能优化而非公式差异。
2. **`blur_color_target` 的比较基准**：C3 与笔画起点的快照比，Rust 与当前像素比。
   改用快照需要 `LaneTransport` 保存一份起点副本（内存翻倍）。

### 9.5 测试

paint 206 全绿、engine 3298 全绿、全工作区编译通过。
`transport::tests::v1_carries_paint_forward_and_v2_...` 已按 C3 语义重写。

---

## 10. 模糊的最后两处（已完成）

### 10.1 可分离高斯

**C3**（`BlurPipeline.swift:29-45`）：`sigma = radius/3`，`support = ceil(radius)`，
权重 `exp(-i²/2σ²)` 归一化，偶数 tap 两两合并；两趟（水平、垂直）中间加 memory barrier。
半径 127 时是 129 个采样点。

**原 Rust**：9 个固定 tap（`0.18 / 0.14×4 / 0.10×4`），不论半径多大。
半径大时采样点间隔过远，出现块状/条纹。

**已改为**（`transport.rs::gaussian_weights` + `gaussian_blur_axis`）：

```rust
sigma = radius / 3
raw[i] = exp(-i² / (2σ²))
权重对 = (0, raw[0]/total)，然后每 2 个合并为 (i + b/(a+b), (a+b)/total)
```

两趟分开做：先水平进 `vertical`，再垂直进 `blurred`。
输出保持**预乘**，合并时按 alpha 反预乘——这样接近全透明的像素不会因除以极小 alpha 而过冲。

### 10.2 `blur_color_target` 比较基准

**C3**：与 `strokeBase`（笔画起点的图层快照）比较。
**原 Rust**：与当前工作像素比较——这一笔自己画上去的颜色会让本该跳过的地方变得"合格"。

**已改为**：`Transport` 新增 `blur_base: Option<Region>`，在笔画第一个 dab 时
捕获 `work` 的副本（仅当 `blur_color_target` 已设置时）。比较改用这份快照。

代价：每 lane 多一份与工作区同尺寸的副本（Blur 工具启用 target 时才会有）。

### 10.3 测试

新增 `gaussian_weights_normalise_and_widen_with_the_radius`：验证权重归一化、
半径越大 tap 越多、中心权重最大、偏移单调递增。

paint **207 全绿**、engine **3298 全绿**、全工作区编译通过。
