# 把 V1 混色搬到 GPU 能快多少：实测

测量日期 2026-11-17。复跑：

```powershell
$env:RUST_MIN_STACK='134217728'
$env:CARGO_HOME='E:\000 app\Android_dev\cargo'
cargo run --release --locked -p photocraft-gpu --example mix_probe --features native
```

环境：Windows 11 x64，NVIDIA GeForce RTX 3080 Ti，Vulkan 后端，wgpu 30，release 构建。
workload：100 个 dab，1200×1200 画布，半径 80 px，flow 0.35，
被测量的循环是 V1 的逐像素 `carrier = mix(carrier, dest, amount)` / `dest = mix(dest, carrier, amount)`。

## 数据

| 路径 | 时间 |
|---|---|
| CPU，行并行，逐 dab box（≈引擎实际要做的量） | 9.5 ms |
| CPU，行并行，全图 × 100 dab（与 GPU 批量版同工作量） | 44.4 ms |
| GPU，单次 dispatch 覆盖全图 × 100 dab | 1.3 ms |
| GPU，每 dab 一次 dispatch（100 次） | 12.8 ms（其中 encode 0.2 / submit+wait 12.6） |

同工作量的算力比：**44.4 / 1.3 ≈ 34×**。

## 这组数说明什么

1. **算力确实在 GPU 上，而且很富裕**：它处理了 34 倍于 CPU 的时间预算下同样的循环。
2. **逐 dab 提交会把这个优势吃光**：每次 dispatch 的 encode+submit+wait 固定开销约 0.13 ms，
   100 个 dab 就是 12.6 ms，比 CPU 做完这 100 个 box 的 9.5 ms 还慢。
   流式渲染（每来一个 dab 就提交一次）等于把 GPU 退回成一台有 130 微秒延迟的串行机器。
3. **真正的瓶颈不在这个循环里**：现有 bench 里 `mix V1` 一笔 123.3 ms，而这个混合循环在 CPU 上只占 9.5 ms（7.7%）。
   剩下约 114 ms 花在采样取色、覆盖度、载体重建（`feedback_dab` 两趟）、合成与湿边记录上。
   只把混合循环搬到 GPU，端到端最多省 7.7%，还要倒贴同步开销，净收益约为零。

## 结论

- 只 GPU 化「混色那几行算式」：**不值得做**，端到端收益 ≈ 0。
- 要拿到 34× 那部分算力，必须把**整条 `transported_output`**（载体重建 + 采样 + 覆盖 + 混合 + 湿边）
  放进 GPU，并且**按笔画段批量提交**，而不是逐 dab 同步。
  在这种形态下，123 ms 的量级有希望进入 10–20 ms 区间，但这是待实测的估计，不是已验证结论。
- 这也是 C3 的做法：它的 `renderMixStamp` 不是把一个小循环挪上去，而是整条盖印管线在 GPU 上、一笔一次提交。

## 下一步的最小验证

1. 在 `crates/gpu` 里写一个只服务 V1 的实验 kernel：输入一段 dab 序列 + 画布 tile + 载体 tile，
   一次 dispatch 跑完整段，输出载体与结果 tile。
2. 与 CPU 的 `transported_output` 在同一份输入上比对像素（逐位或 1e-6 内），先要正确，再看时间。
3. 正确且明显更快之后，才谈把它接进 `StrokeRenderer`。

未验证的部分：真实笔画里 tile 的读回与写回、湿边随帧变化的增量、混合模式与颜色属性的合成，
这些都还没搬到 GPU，上述 10–20 ms 不含它们。
