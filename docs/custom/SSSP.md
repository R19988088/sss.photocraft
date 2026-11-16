# SSSP：C3 原生工程格式（读写支持）

C3（`sssp_c3`，Swift/Metal）把作品存成 `.sssp`。它不是位图打包，是 **SQLite 数据库**；本仓库用 `crates/sssp` 读写它，和 C3 互通。PSD 存不下的东西（网点/半调、故障、网格、屏幕印刷…）只有 SSSP 留得住，所以图层行与菜单里给这类项打 `SSSP` 标记。

## 容器结构（与 `SSSPDocumentContainer.swift` 一致）

| 项 | 值 |
|---|---|
| 头 | `PRAGMA application_id = 0x53535350`（"SSSP"）、`user_version = 1` |
| 文档 | `document(id=1, document_id TEXT)` |
| 存档头 | `heads(kind ∈ manual/automatic, generation TEXT, saved_at REAL)`：一个文件里同时存手动保存与自动保存 |
| 元数据 | `metadata(head, scope, entity_id, json)`：项目 JSON 按实体拆开放——`root` 每个顶层键一行，`layer` 每层一行，`group` 每组一行，`layout` 存 `layers`/`groups` 的 id 顺序 |
| 像素 | `tile_payloads(version_id, width, height, tile_size, codec, data)` + `tile_refs(head, layer_id, x, y, version_id)`：每层按 `tile_size` 切块，**RGBA16Float 预乘、8 字节/像素**，`codec ∈ lzfse/raw` |
| 资源 | `resource_payloads` + `resource_refs`：JSON 里用 `{"$ssspResource": id}` 引用大块数据，C3 读出来时换成 base64 |
| 预览 | `previews(head, generation, data)`：PNG |
| 颜色身份 | `color_identity(head, layer_id, x, y, width, height, data)`：每像素 2 字节，纯黑处也能还原色相 |

图层 JSON 字段：`id/name/type/visible/locked/isSolidBackground/opacity/blendMode/clipsToLayerBelow/colorHex/style/adjustment/imageDataBase64/sourceProperties`。其中 `adjustment` 是 `{kind, payload, enabledKinds, strengths, processingOrder}`——**一个调整层可叠多种效果**，PSD 的单调整层表达不了。

## `crates/sssp` 提供的接口

- `read(path)` / `read_head(path, head)` / `heads(path)` / `is_sssp(path)`
- `write(path, &Project)`：写到临时邻居再改名，失败不会留下半个工程
- `split_layer` / `assemble_layer`：一层像素 ↔ 分块
- `encode_tile` / `decode_tile`：LZFSE 压不小就存 `raw`
- `inline` / `extract`：`{"$ssspResource": id}` ↔ base64
- `Project::new_document(w, h, tile)`

## 验证（真机）

`/Users/ddd/Desktop/小 p 设计/s2.sssp`：5000×6000、8 层、7789 块。

```
cargo test -p photocraft-sssp            # 4 项全过（含真文件回写比对）
cargo run -p photocraft-sssp --example dump -- <file.sssp>
```

回写后逐层比对：`id/name/type/adjustment` 一致，任取一块解码像素逐字节相同。

## UI 标记

- `panels/layer_row_ui.rs`：新增 `Indicator::Sssp`，网点调整层右侧画 `SSSP` pill，悬停说明「PSD 无此层类型」
- `menu_catalog::is_sssp_only` + `menus/render.rs`：`Layer → New Adjustment Layer → Halftone…`、`Image → Adjustments → Halftone…` 显示 `SSSP`

## 调整层栈：C3 合并、这里分面板

C3 把同类调整层里的所有效果合成**一个面板**：一层携带 `enabledKinds / strengths / processingOrder`，每个 kind 一个 payload；本工程是「一层一个 Adjustment、一种效果一个面板」，所以合并栈没有原生位置。

处理方式：**导入时把整段 C3 栈原样记在层上**（`layer.xmp` 里的 `sssp.adjustment` 包，PSD 也保留层 XMP），导出时再写回容器——

- 当前能渲染的那项（`halftone`、`cmykScreenPrint`、`lineArt`、`psHueSaturation`、`gradientMap`）在导出时用编辑后的值刷新对应 payload；
- 其余 kind（`enabledKinds`、`processingOrder`、`strengths`、各 payload）原样保留，C3 打开后仍是合并面板、顺序不变；
- 因此「C3 → 这里改色相/饱和度/网点 → 存回 C3」不丢栈；将来补哪个 kind，就在映射表里加一条，记录结构不用动。

真文件 `s2.sssp` 里那条调整层是 `psHueSaturation`（`enabledKinds=[]`），已直接映射成 `Adjustment::HueSaturation`（hue/saturation/lightness/colorize + 中性六段范围）而不是「仅保真」；导出的单元测试会比对原文件与导出文件的 `processingOrder` / `enabledKinds` 是否一致。

## 接线（已完成）

- `crates/io/src/lib.rs`：`route()` 识别 `.sssp` 后缀或 `SQLite format 3` 签名 → `Route::Sssp`；`IoError::Sssp`
- `crates/io/src/sssp.rs`
  - **打开**：`import(name, bytes)` → `Document`（画布取容器尺寸、`canvasDPI` 作分辨率）。栅格层 `assemble_layer` → `f16 → f32` → `Surface(RGBA32F)`；`kind=halftone` 映射成 `Adjustment::Halftone`（`frequency` → `size = max(2, 180/frequency)`，即 C3 着色器的 `cellSize`；`colorMode == "color"` 才彩色；shape 认 circle/square/line/ellipse，缺字段给默认值）；`kind=cmykScreenPrint` 映射成 `Adjustment::CmykScreenPrint`（payload 里平铺的 `cmykDotSize/cmykAngle/cmykCyan/cmykMagenta/cmykYellow/cmykBlack`）；`kind=lineArt` 映射成 `Adjustment::LineArt`（payload 的 `lineArt{blendMode,color,intensity,sharpness,thickness,*Opacity}`）；其它 kind 存成 `Adjustment::Unsupported { psd_key: kind, raw: 原始 JSON }` 并给一条提示，帧不乱、数据不丢；组暂拍平并提示
  - **另存为**：`export(doc, ctl)` → 重新生成容器（tile 256、栅格层 `split_layer` 按块写、调整层回写 JSON、`Halftone` 反向生成 payload）。`Unsupported` 原样写回 C3 的 `{kind, payload, enabledKinds, strengths, processingOrder}`，所以「C3 打开 → 改 → 存回 C3」不丢效果
  - **版本号**：每层的块 id 以层 id 做种子（`split_layer(..., &id)`），跨层不撞 `tile_payloads.version_id`（曾撞过：同名列的块互相覆盖，读回报 `tile falls outside the layer`）
- `apps` 保存路径：`files.rs` 的 `writable` 认 `sssp`，`.sssp` 文档另存默认还是 `.sssp`
- 图层行 `sssp_only()`：网点层，以及从 SSSP 导入、无本地实现的 C3 调整层（key 是 `gradientFault` 这种词而不是 4 字节 PSD key），右侧都打 `SSSP`

C3 的三个印刷类调整层在本地都是**独立的调整层类型**（不再挤进 C3 的合并面板）：`Layer › New Adjustment Layer › CMYK… / Line Art…`、`Image › Adjustments › CMYK… / Line Art…`，参数走 `layer.setAdjustment` 的 `cmykScreenPrint` / `lineArt` 两种 kind。CPU 实现在 `compose::adjust::cmyk_screen` / `line_art`，GPU 侧：CMYK 是解析 case（`compose.wgsl` 的 `case 27`），线条强化有自己的入口 `fs_adjust_near`（`Kernel::AdjustNear`，`case 28`，按 C3 的 `lineArtSampleWeight` / `zoneWeights` 复刻，双线性采样 `tex_a`、钳制在作品矩形内）。
GPU 与 CPU 的逐像素一致性有 `crates/gpu/tests/it/parity.rs` 的 `c3_printing_adjustments_match_cpu` 守着；线条强化在大于 `CHUNK=1024` 的画布上块边界会有极细接缝（块内只读得到本块像素），导出走 CPU 所以成图不受影响。

验证：`cargo test -p photocraft-io --lib sssp` —— 真文件 `s2.sssp`（5000×6000、8 层、7789 块）导入 8 层、含网点/未知调整、栅格层有像素；再导出回读，层数/画布/head 一致，再导入仍是 8 层。
