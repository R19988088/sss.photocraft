# The brush system, replaced with ProCraft's

The decision (the user's, not a fallback): stop patching this app's brush a parameter at a time.
Take ProCraft's brush system as it is, keep this app's UI, and let the two brushes' parameters
differ — anything ProCraft's brush has no counterpart for is marked **pink** in the brush panel
rather than quietly doing nothing.

## What is in the tree

| piece | where | state |
|---|---|---|
| ProCraft's document model, brushes, stroke geometry | `crates/procraft-engine` | verbatim, `0c85bc0c`, builds untouched |
| ProCraft's wgpu renderer (its shaders) | `crates/procraft-gpu` | verbatim, same commit, builds untouched |
| our settings read as ProCraft's `Brush` | `crates/brush-bridge` | done, tested |
| ProCraft's stamps laid as this app's dabs | `StrokeRenderer::push_dabs` | done, bench drives it |

The two vendored crates are **not** to be edited: a future upstream refresh is then a plain
re-copy. Our side of the seam lives in `crates/brush-bridge` and in this app.

`wgpu` is the reason this was cheap to try: `eframe 0.37` already pins `wgpu 30.0.1`, and ProCraft
asks for `wgpu 30`, so its renderer can share this app's device. No version bridge, no copying
textures across devices.

## What the seam keeps and what it drops

Kept: size, opacity, spacing, flow, hardness, the three pressure bleeds, roundness, angle,
streamline, the stamp diameter, and ProCraft's own stroke geometry (spacing rule, taper, scatter,
jitter).

Ported from the old brush (the spacing rule deliberately left as ProCraft's):

| ours | ProCraft's |
|---|---|
| end taper (`taper`) | `pen_taper` — same length, `size` is how much of the pressure it takes away |
| tilt angle / size / opacity, Follow Azimuth | `tilt.angle`, `tilt.size`, `tilt.opacity`, `shape.follow_azimuth` |
| angle jitter (share of ±180°) | `shape.scatter` (share of half turns) |
| size jitter | `shape.size_jitter` |
| angle control = Direction | `shape.follow = 1` |
| scatter (1 = one radius) | `shape.throw` (1 = one stamp diameter, so half of it) |
| dab count | `shape.count` |
| Transfer's opacity jitter | `shape.opacity_jitter` |
| wet edge's no-build-up rule | `Rendering::Strongest` |

`Kind::Partial` in `UNMAPPED` marks the sections carried in part — shape dynamics, scattering,
transfer, wet edge — so the panel can show them pink and name in the tooltip which of their controls
are left out.

Dropped for now (`UNMAPPED` in `crates/paint/src/procraft.rs`, `Kind::Pink`): the pigment/Kubelka-Munk
machinery (`pigment_mix_strength`, `pigment`, `contour`, `canvas_material`, `saturation_*`,
`mix_settings` with its V1/V2/V3, `mixer`), wet edge, our procedural patterns and sampled pattern
colour (`pattern_enabled`, `sampled_pattern`, `texture`, `grain`, `noise`), the dynamics sections
ProCraft's `Shape` has no field for (`shape_dynamics`, `scattering`, `dual_brush`,
`color_dynamics`, `transfer`, `pose`, `hardness_dynamic`), build-up, motion/inertia, and the tip
maths beyond a round tip with `hardness` (`tip`, `c3_tip_math`, `triangle`, `aliased`, `square`,
`flip_x/y`). Tool-level settings (colour, erase, behind, blend mode, seed, locks, smoothing,
taper, pressure_flow) are marked `Kind::Tool` instead: they are the app's, not the brush's.

## The numbers (900 px, 1 %, 2520x1575, RTX 3080 Ti, folded frame, p50)

| | before the replacement | with ProCraft's dabs | note |
|---|---|---|---|
| solid | 10.1 ms | 10.18 | unchanged: the dabs' places were never the cost |
| pattern (our colour lane) | 34.2 | 35.00 | unchanged, same reason |
| pattern as grain (GPU coverage) | — | **9.86** | what a ProCraft brush does with a texture |
| pattern, switched to grain | 34.2 | **9.67** | `procraft::effective`: the patch becomes a gray tile |
| smudge | 105.7 | 44.55 | ProCraft's stamps come out thinner; the transport lane is still ours |
| smudge + pattern | — | 156.48 | |
| erase | 10.2 | 10.00 | |
| pen-up | 0.1 s class | 0.19 ms | after calling `add_tipped` as ProCraft's canvas does |

Two conclusions come out of that table and both point at the same place:

1. **The dabs' places were never the cost.** Replacing the dab source moved solid, erase and pattern
   by nothing at all.
2. **The pattern's 35 ms is the per-pixel colour lane, and only that.** The same pattern reaching
   the coverage in the shader (`mask_combine` — the GPU path this app already has) costs 9.86 ms,
   the solid price. So the replacement keeps ProCraft's grain (a texture over the coverage) and drops
   our sampled-pattern colour; `procraft::effective` does it in one step: a sampled pattern's
   weighted luma (with its own alpha, so a transparent corner stays a hole) becomes a `GrayTile` on
   `texture.pattern`, `pattern_enabled` goes off, and the colour lane is never allocated. Measured
   9.67 ms — the solid price, as predicted.

   That is the whole of the "don't mix the pipelines" rule for the pattern: one lane, the shader's
   coverage. Nothing else of the brush reaches into the colour lane under ProCraft's brush.

A correction worth keeping: the 1.4 s pen-up in the first ProCraft run was the benchmark's error,
not the stroke's — it called `add` throughout, so at pen-up `add_tipped` returned the whole stroke
as the tip. ProCraft's canvas calls `add_tipped` every frame and draws the tip on a layer of its
own; done that way, pen-up is 0.19 ms.

## What is left

0. **Which strokes ProCraft draws**: a brush that lays paint (`procraft::draws`). A stroke that mixes
   — a smudge, a mixer, a pigment response — keeps this app's own dabs and its CPU transport lane
   (`Transport::smudge_dab`, the lane the 0.23.5 brushes mixed with). ProCraft's brush lays stamps
   and does not carry paint between them, and the GPU transport backend hands a dab's whole square
   to a rect-strength pass: either one leaves squares of the layer under the stroke (white patches
   in a smudge). `takes_backend` is off for the same reason.

1. **The smudge lane**: ProCraft's `smudge.wgsl` (pigment lay/carry, the warped window) instead of
   this app's transport. That is the 44.55 ms. Measured on its own canvas
   (`cargo run --release -p procraft-gpu --example smudge_stroke`): the API drives, no GPU errors,
   but a 900 px stroke with a 900 px stamp costs **53.6 ms at 100 stamps and 426.6 ms at 975**,
   after **2.7 s** to make a canvas — it is slower than our lane at this brush size, so renting its
   window is not a win. The lane's cost is the stamp count, and our own sweep says it plainly:
   p50 at 1 % 52.4 ms, 2 % 27.3, 4 % 11.9, 8 % 7.1. A smudge carries colour, it does not lay it, so
   `SMUDGE_SPACING` (4 %) is now its floor: **the 44.55 ms becomes 11.9 ms** without a new pipeline.
2. **The pink panel**: `UNMAPPED` is the data for it; the panel has to read it.
3. **The default**: `PHOTOCRAFT_PROCRAFT_BRUSH=1` is how a stroke is drawn by ProCraft's brush today.
   Flipping the default needs the paint suite's brush-behaviour tests re-based one brush at a time —
   they assert this app's brush, and ProCraft's brush draws differently on purpose. Not a bug to fix:
   a decision to take, brush by brush, with the pink list as the guide.

## How to drive it

```
PHOTOCRAFT_PROCRAFT_BRUSH=1 photocraft.exe      # strokes drawn by ProCraft's brush
PHOTOCRAFT_PROCRAFT_BRUSH=0 photocraft.exe      # this app's own generator (the default)
```

`cargo run --release -p photocraft-ui-egui --example paint_modes_bench -- 900 1` times the modes;
add `PC_PROCRAFT=1` to drive the stroke through ProCraft's builder and `PC_TRANSPORT_GPU=1` for the
GPU smudge.
