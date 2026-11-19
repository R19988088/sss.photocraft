# 笔刷格式导入（BRUSH-IMPORT）

PhotoCraft 的笔刷库可以直接读入各家的笔刷文件。实现分两层：`crates/io/src/brush_import/`
里是纯只读解析器（全部自写，不引入 zip/plist/rusqlite 依赖），引擎命令
`brush.presets.importBrushFile` 把解析结果加成笔刷库里的一个组，UI 的
「导入笔刷…」/文件拖放按扩展名路由到它。

## 支持的格式

| 格式 | 扩展名 | 来源 | 读到的内容 |
|---|---|---|---|
| Photoshop 笔刷 | `.abr` | `brush.presets.importAbr` | 笔尖、散布/纹理/双笔、文件夹结构 |
| Photoshop 工具预设 | `.tpl` | `[8BTP]` 容器里的 `Brsh` 描述符 | 参数齐全；旧式 `tpbd` 采样笔尖读不了，退化为圆头 + 警告 |
| Procreate | `.brush`、`.brushset` | ZIP + `Brush.archive`（NSKeyedArchiver） | `Shape.png` 笔尖、`Grain.png` 纹理、副笔刷、`brushset.plist` 组名与顺序 |
| Krita | `.kpp`、`.bundle` | PNG 文本块里的 `<Preset>` XML + 内嵌资源 | 直径/间距/角度/圆度、压力、纹理、散布，`.bundle` 逐支读 |
| GIMP | `.gbr`、`.gih` | 大端头像 + 位深 1/2/4 | 笔尖与间距；`.gih` 直管/散布管每格一支 |
| MyPaint | `.myb` | JSON 设置 + 压力曲线 | 半径、间距、不透明度、硬度、椭圆比、抖动、涂抹、橡皮 |
| SAI2 | `.sai2`、`.ini` | 素材包 ZIP 或 `[Parameters]` ini | 尺寸/间距/硬度/角度抖动/散布/笔尖 BMP |
| MediBang / FireAlpaca | `.mdp`、`.ini` | `mdipack` 容器 + `PAC` 图像段，或 `Brush2.ini` | 最大图层的 alpha 当笔尖，位图笔刷读 `brush_bitmap/` |
| 天生会画 | `.gobrush`、`.gobrushes` | ZIP + 记录流属性块 | 笔尖/纹理 PNG、尺寸区间、间距、alpha、flow、grain |
| CLIP STUDIO PAINT | `.sut` | SQLite 的 `Variant` 表 + `Node` 名 + `MaterialFile` tar | 全部笔刷参数、副笔、水边/混色/平滑；笔尖取素材预览（加密 stamp 层不解） |
| Corel Painter | `.brushlibrary`、`.brushcategory` | ZIP（可套一层）+ `<brush>.xml` + `.nib` | 半径/间距/流量/硬度/抖动/颗粒，nib 反相成笔尖 |

## 用法

- Brushes 面板 › 导入笔刷…，或直接把文件拖进窗口，或 File › Open。
- 同名组已存在时会问「替换 / 并存」；选并存则新组自动叫 `名字 (2)`。
- 导入在后台 job 里跑，完成时状态栏报数量与首条警告；失败会说明是哪种格式的哪一步读不动。
- 命令行 / 自动化可以直接调：

  ```
  photocraft-cli run --command brush.presets.importBrushFile \
      --params '{"path":"C:\\Users\\me\\Downloads\\Ink.sut","group":"CSP Inks","select":true}'
  ```

  参数：`path` 或 `data`（base64）+ 可选 `fileName`（走 data 时用于判格式）、`group`、
  `replace`、`select`。返回 `{group, imported, count, folders, format, warnings}`。

## 已知近似

- `.tpl` 里的旧式 `tpbd` 采样笔尖暂不支持，带这种笔尖的工具会以圆头导入并给警告。
- `.sut` 的 `data/material.layer` 是加密的 C2F 布局，笔尖改用素材包里的 PNG 预览（分辨率与
  真实笔尖可能不同）；纹理同理。参数本身是精确读出的。
- MediBang `.mdp` 只取最大图层当笔尖；多层笔刷按单笔尖近似。
- Corel Painter 的 `thickness`/`thinness` 按圆度映射，颗粒只取强度，不含纸纹图。

## 测试

```
cargo test -p photocraft-io --lib brush_import
```

单测覆盖 GIMP（`.gbr`/`.gih`）、MyPaint、Krita、ZIP 头、二进制 plist 与中性模型到
`BrushSettings` 的映射；其余格式靠真实样本手测。
