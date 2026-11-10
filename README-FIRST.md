# ⚠️ 这是当前主目录

这个目录是 PhotoCraft 自定义版的**唯一开发目录**。所有修改都在这里进行。

---

## 基本信息

| 项 | 值 |
|---|---|
| 用途 | 上游 `storytold/photocraft` + 四项自研功能的合并版 |
| GitHub | https://github.com/R19988088/sss.photocraft |
| 当前分支 | `custom-merged` |
| 版本 | 0.24.0（上游快照 0.20.1） |
| 上游 | https://github.com/storytold/photocraft |

---

## 先读这两个

1. **`../README.md`** — 工作区全貌，哪几个目录是历史存档
2. **`docs/custom/PROJECT.md`** — 需求清单、进度、跟随上游的完整流程

---

## 不要做的事

- ❌ 不要改 `../code/`、`../code-liquify-023/`、`../integrate-detached-canvas-20261029/`、`../merge-shape-to-046/`
  —— 那些是历史快照
- ❌ 不要 `rsync`/`cp -r` 覆盖 `crates/paint/` 或 `crates/ui-egui/`
  —— 上游与自研改动重叠最密集，必须定点合并
- ❌ 不要用上游版覆盖液化的 Filter 对话框 —— 已冻结，改用独立的 `Tool::Liquify`
- ❌ 不要往 `bounded_for_render` 的关闭清单里加 C3 也有的功能

---

## 目录内标记

本目录是 git 仓库，提交历史完整可追溯：

```
d26073b fix(canvas): 画布投影改用 C3 的四边形方案
03c3bed release: 自定义版版本号 0.24.0
83de972 docs: 记录第二次上游跟随的改动与已知失败
14c0faa merge: 跟随上游 2026-11-10 快照
f2102b7 upstream: storytold/photocraft main 2026-11-10 快照
8435db6 docs: 项目总记录
...
f854202 vendor: upstream storytold/photocraft main (2026-11-09, 0.20.1) as merge base
```
