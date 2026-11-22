# procraft-engine (vendored)

Verbatim copy of `artcraft-android/procraft`'s `crates/engine`, commit
`0c85bc0c5c52f744af36a4409a08526e9118915d` (2026-11-18).

- Source: `E:\000 app\procraft\source\procraft.C-main\crates\engine`
- Copied: 2026-11-22
- Changes: **none** to the Rust sources. `Cargo.toml` only had the workspace inheritance
  (`version/edition/rust-version/publish/license = workspace`, `[lints] workspace = true`,
  workspace dependencies) replaced with literal values so it builds inside this workspace.
- Licence: MIT OR Apache-2.0, same as this repository.

Why it is here: the brush system is being**replaced** with ProCraft's rather than patched
incrementally (`docs/custom/PAINT-GPU.md` records the decision). The engine is serde-only
(`serde`, `serde_json`, `miniz_oxide`) and has no GPU or platform code, so it drops in as-is.

Where the equivalent of `crates/gpu` is: `crates/procraft-gpu` (also verbatim, same commit).
Both build against `wgpu 30`, which is the version `eframe 0.37` already pins in this workspace.

Keep these two crates untouched. Our own settings/model go in the bridge, not in the vendored
sources — that keeps a future upstream refresh a plain re-copy.
