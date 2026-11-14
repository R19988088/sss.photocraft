# 自定义合并版（photocraft-custom-0.24）

这个目录是「上游新版 main」与「你 0.23.6 的四项自研功能」的合并结果。

## 文件

- `AGENTS.md`：接手备注——分支模型、跟随上游步骤、出包命令、笔刷与间距的硬约定、已知失败的测试。
- `MERGE-DIFF.md`：完整的三方差分清单、功能域边界、已处理项、已知预期差异。
- `BRUSH-C3-AUDIT.md`：笔刷/笔迹与 C3 参考实现的逐项比对记录。
- `../UPSTREAM.lock`：跟随上游用的锁文件（当前基于哪个上游快照、哪些区域冻结）。

## 合并结果一览

| 功能域 | 来源 | 状态 |
|---|---|---|
| 多层液化 | 你的实现 | 已移植：`edit_scope` + `cached_transform` + 引擎多目标提交；上游 Filter 对话框入口冻结 |
| 压感优化 | 你的实现 | 已移植：`tablet_pressure.rs` + `is_pen` + C3 默认值（弹性 6 / 稳定度 2）+ 设置页 |
| 笔刷设置 | 你的实现 | 已移植：8 节面板 + 8 个引擎模块 + 双 lane；渲染用你的算法 + 上游并行调度 |
| 工具分组 | 你的实现 | 已移植：置顶 4 槽 + 选项栏平铺同组工具 |

## 从上游保留下来的新能力

`behind` 混合、并行光栅化、笔尖 mip 缓存、`Pose` 分轴、`ArtHistoryBrush` / `Star` 工具、
WinTab 后端、多通道液化、smart filter 编辑、透明滚动条下的浮窗前台。

## 以后跟随上游的步骤

1. 更新 `UPSTREAM.lock` 里的 `upstream.commit` / `upstream.date`。
2. 只用 `git merge` + 定点解决；**不要**整体覆盖 `crates/paint/src`、`crates/ui-egui/src`。
3. `UPSTREAM.lock` 里 `frozen.paths` 列出的文件，上游改动一律手工挑拣。
4. 合并后跑：
   ```
   cargo check --workspace --lib
   cargo test -p photocraft-engine --lib
   cargo test -p photocraft-paint --lib
   ```
5. `crates/paint/src/brush_tests.rs` 里 17 个上游断言失败是**预期**的：
   它们是 `bounded_for_render` 有意关闭的特性（见 `MERGE-DIFF.md` 第 6 节）。
