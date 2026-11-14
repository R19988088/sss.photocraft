# 笔触管线 GPU 化方案（模糊 / 混色 / 湿边）

> 状态：方案稿（2026-11-14）。尚未动工，等作者确认开工顺序。

## 1. 现状：不是三条独立管线

模糊、混色、湿边共用**同一条 CPU 笔触循环**，每颗 dab 按固定顺序执行：

| 阶段 | 位置 | 说明 |
|---|---|---|
| 模糊 | `crates/paint/src/transport.rs::blur_dab` → `blur()` | 可分离高斯，半径 `r*(0.18+blurStrength*0.82)`，1.5–127 |
| 混色/运输 | `same file`：`feedback_dab`(V1) / `sai_dab`(V2) / `pickup_dab`(V3) | carrier 反馈、SAI、拾色，均持有 `blur_base` 起始快照 |
| 湿边 | `crates/paint/src/render.rs::accumulate_wet` + `wet_factor` | 跨 dab 累积，含 Single/Outline 边缘、颜色模式、饱和度记录 |
| 颜料/合成 | `render.rs` 主循环（`cov.accumulate_pigment`） | pigment 材质响应、饱和度保持、dual brush |

调用点：`render.rs:1482`（主画笔）、`render.rs:1565`（副画笔）每 dab 调用 `transport.dab()`，
顺序由 `transport.rs::dab` 决定：**先模糊，后运输**（与 C3 `ContentView.swift:9121 → 9140` 一致）。

GPU 侧目前只有 `crates/gpu/src/brush_stamp.wgsl`（272 行）：算单 dab 的覆盖率与湿边深度，
结果 `Vec<f32>` 回读到 CPU（`brush_stamp.rs::stamp` → `StampOutput`）。
`brush_stamp.rs` 文件头已注明：带湿边/Sampled tip/Texture/Grain 的 dab **不成 job**，直接走 CPU。

结论：**GPU 只承担了 dab 遮罩**，模糊、混色、湿边累积、颜料合成全部在 CPU。
`docs/custom/PROJECT.md:136` 亦记录同一事实（「笔刷光栅化 GPU 化 约 900 行 WGSL」）。

## 2. 目标

- 纯 GPU 模式：一次笔触的整条链（stamp → blur → transport → wet edge → pigment → composite）
  在 GPU 常驻图层 tile 上完成，只在历史提交/导出时读回。
- CPU 保底：GPU 不可用（无适配器、超出 dispatch 限制、未覆盖的笔刷特性）时整条链退回现有 CPU 实现，
  行为与今天完全一致；不允许「半 GPU 半 CPU」导致观感差异。

## 3. 需要的改动

1. **GPU 常驻笔触 target**：`gpu` 增加 stroke tile 纹理（RGBA16F/32F + 深度），
   替代 `paint::Region` 作为工作区；dab 之间不再回读。
2. **stamp 直出纹理**：`brush_stamp.wgsl` 从「写 coverage 数组」改为「写 coverage 纹理」，
   保留现有 CPU 遮罩路径用于保底与单测。
3. **blur pass**：复用 `gpu/src/fx.rs` 已有的可分离高斯（横向 + 纵向两次 dispatch，临时纹理），
   半径/权重与 `transport.rs::gaussian_weights` 逐位一致。
4. **transport pass**：carrier 纹理 + 三套算法（V1 keep/retain-alpha、V2 SAI、V3 pickup），
   `blur_base` 作为独立只读纹理在模糊前快照。
5. **wet edge pass**：`accumulate_wet` 的跨 dab 累积落成「上一 dab 边缘状态」纹理 + 本 dab 合并，
   Single / Outline(Edge/Sparse) / 颜色模式 / 饱和度记录逐一对应。
6. **合成**：pigment 响应若仍留 CPU，则只回读最终 RGBA（一次），不每 dab 回读。
7. **回退规则**：`GpuStrokeBackend` 在 setup 阶段探测并按笔刷特性给出 `Unsupported`，
   `StrokeRenderer` 整笔退回 CPU（不是逐 dab 混合）。

## 4. 分期与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| A | GPU stroke target + stamp 直出纹理 + 单 dab 合成，无混色/模糊/湿边 | 与 CPU 逐像素一致（≤1/255），无回读 |
| B | blur pass + wet edge pass | 含湿边/模糊的笔刷在 GPU 上出图；CPU 结果作为 golden |
| C | transport V1 → V3 → V2 | 三算法 parity 测试（`crates/gpu/tests/it/`）覆盖 keep/retain-alpha、carrier 走样 |
| D | 整笔 GPU + 单次回读 + 开关与回退 | 长笔触帧时间对比；无适配器时全 CPU 行为不变 |

每阶段跑：`cargo test -p photocraft-paint`、`cargo test -p photocraft-gpu`、
`cargo test -p photocraft-engine --lib`，并保留 CPU golden 图像对照。

## 5. 风险

- 观感：GPU 浮点精度与 CPU f32 路径在软边/低硬度处可能差 1–2/255，需要按阶段校准，不能靠「看起来差不多」。
- 湿边的跨 dab 状态是画面差异最大的部分（边缘累积顺序），移植时必须逐 dab 对照 golden。
- 现有 `DabStamper` 每 dab 同步等待（`device.poll(Wait)`），GPU 常驻后需改成一次性提交、末尾等待。
