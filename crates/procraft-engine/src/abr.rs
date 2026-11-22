//! Reading the brushes in Photoshop brush files (ABR): each sampled tip's grey image (white
//! paints), with its preset's settings where the file gives them. Versions 1 and 2 (the tips one
//! after another; their computed tips left out) and 6 and later: a section of samples, one of
//! patterns and a descriptor of presets (Photoshop's action descriptor: typed values by key). A
//! preset there has its tip (sampled, or computed round with its hardness), its shape dynamics,
//! scattering, texture (a pattern from the file), dual brush (a second tip whose stroke masks the
//! first's), transfer and colour dynamics, noise, and its tool's flow, opacity and blend mode.

/// One brush tip from the file.
#[derive(Clone, Debug, PartialEq)]
pub struct AbrTip {
    pub name: String,
    /// Space between stamps, as a share of the tip's size, if the file gives it.
    pub spacing: Option<f32>,
    pub width: u32,
    pub height: u32,
    /// Grey, `width` x `height`, rows top to bottom: how much each pixel paints.
    pub pixels: Vec<u8>,
}

/// One of the file's brushes: a sampled tip, or (none) a computed round one, with its preset's
/// name, spacing, angle and roundness where the file gives them, and a computed one's hardness.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AbrBrush {
    pub name: String,
    pub tip: Option<AbrTip>,
    pub spacing: Option<f32>,
    /// Radians.
    pub angle: f32,
    /// How round the tip is, 1 a circle.
    pub roundness: Option<f32>,
    /// A computed tip's edge, 0 soft to 1 hard.
    pub hardness: Option<f32>,
    /// Its tip's diameter, pixels, if the preset gives it.
    pub diameter: Option<f32>,
    /// Whether its tip is drawn mirrored side to side, and top to bottom.
    pub flipped: (bool, bool),
    /// The pattern the preset paints through, if it uses one that is in the file.
    pub texture: Option<AbrTexture>,
    /// What the pen's pressure changes.
    pub dynamics: AbrDynamics,
    /// How its stamps are thrown off the stroke and how many fall at each step, if they are.
    pub scatter: Option<AbrScatter>,
    /// Its dual brush, if it has one on whose tip the file has: the second tip's own shape, size,
    /// spacing and scattering (a dual brush has no dual brush, texture or dynamics of its own), and
    /// how the two strokes combine ([`AbrBrush::dual_mode`]).
    pub dual: Option<Box<AbrBrush>>,
    /// How its dual brush's stroke combines with its own, by the preset's four-letter name for the
    /// mode ("CBrn", "hardMix"...), if it says.
    pub dual_mode: Option<String>,
    /// How its tool lays paint on the layer, by its four-letter name ("Nrml", "Mltp"...), if its
    /// tool's options say.
    pub blend: Option<String>,
    /// Its colour changed at random, if it is.
    pub color: Option<AbrColor>,
    /// Whether its tip's soft edges are speckled with noise.
    pub noise: bool,
}

/// The control that turns a tip the way the stroke goes (the bristle brushes in real files have it).
pub const ANGLE_BY_DIRECTION: i32 = 6;
/// The control that is the pen's pressure.
pub const BY_PRESSURE: i32 = 2;
/// The control that is the pen's tilt.
pub const BY_TILT: i32 = 3;

/// What a preset's shape and paint dynamics have the pen's pressure change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbrDynamics {
    /// Whether its shape dynamics are on; what controls the size (0 nothing, 2 the pen's
    /// pressure, 3 its tilt), the least share of the size it goes down to, and how much it jitters.
    pub tip: bool,
    pub size_control: i32,
    pub min_diameter: f32,
    pub size_jitter: f32,
    /// What turns the tip ([`ANGLE_BY_DIRECTION`] the way the stroke goes), with shape dynamics on,
    /// and how far each stamp turns at random (a share of a whole turn).
    pub angle_control: i32,
    pub angle_jitter: f32,
    /// What flattens the tip, how far each stamp is flattened at random, and the roundness neither
    /// takes it under (0 to 1).
    pub roundness_control: i32,
    pub roundness_jitter: f32,
    pub min_roundness: f32,
    /// Whether each stamp is mirrored at random side to side, and top to bottom.
    pub flip_x: bool,
    pub flip_y: bool,
    /// Whether its paint (transfer) dynamics are on; what controls the opacity and the flow, the
    /// least of each, and how much each jitters.
    pub paint: bool,
    pub opacity_control: i32,
    pub opacity_min: f32,
    pub opacity_jitter: f32,
    pub flow_control: i32,
    pub flow_min: f32,
    pub flow_jitter: f32,
    /// The flow and the opacity its tool options keep, 0 to 1, if they do.
    pub tool_flow: Option<f32>,
    pub tool_opacity: Option<f32>,
}

/// How a preset scatters its stamps: each thrown as far as `amount` of its diameter from the
/// stroke at most (across it, or `both_axes` along it too), `count` of them at each step (fewer by
/// up to `count_jitter` of them at random), each as far as its control lets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbrScatter {
    pub amount: f32,
    pub both_axes: bool,
    pub control: i32,
    pub count: u32,
    pub count_jitter: f32,
    pub count_control: i32,
}

/// A preset's colour dynamics: how far each stamp's (or, not `per_tip`, each stroke's) hue,
/// saturation and brightness go at random (0 to 1 each, a share of each one's whole range), and
/// how far it goes to the background colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbrColor {
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub background: f32,
    pub per_tip: bool,
}

/// A preset's texture: a pattern from the file, grey, and how the preset uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct AbrTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Grey, `width` x `height`, rows top to bottom.
    pub pixels: Vec<u8>,
    /// Its tile's size as a share of the pattern's own, if the preset gives it.
    pub scale: Option<f32>,
    /// How strongly it shows, 0 to 1, if the preset gives it.
    pub depth: Option<f32>,
    /// Whether dark lets paint through instead of light.
    pub invert: bool,
    /// The least it shows (pressure and the like lower it to this), 0 to 1, if given.
    pub min_depth: Option<f32>,
    /// What controls the depth (with `each_tip`; [`BY_PRESSURE`] the pen's pressure), and how far
    /// it jitters from stamp to stamp (0 to 1).
    pub depth_control: i32,
    pub depth_jitter: f32,
    /// Its brightness (-150 to 150) and contrast (-50 to 100), as the preset keeps them.
    pub brightness: i32,
    pub contrast: i32,
    /// How it is laid over the paint, by the preset's four-letter name ("Mltp", "Hght"...), if given.
    pub blend: Option<String>,
    /// Whether each stamp is textured as it is laid (the pattern still fixed to the canvas), else
    /// the stroke as a whole.
    pub each_tip: bool,
}

/// The largest tip side read, pixels.
const MOST_SIDE: u32 = 5000;
/// The most bytes of tip images the brushes may have together (presets sharing one sample each
/// have a copy).
const MOST_TIP_BYTES: usize = 256 << 20;
/// The most bytes of patterns read, and of their copies the presets have together.
const MOST_PATTERN_BYTES: usize = 256 << 20;
/// The deepest a descriptor's objects and lists are read nested.
const MOST_NESTED: u32 = 32;

/// The sampled tips in an ABR file, in the file's order; none if it isn't one.
pub fn read(bytes: &[u8]) -> Option<Vec<AbrTip>> {
    Some(read_brushes(bytes)?.into_iter().filter_map(|b| b.tip).collect())
}

/// The brushes in an ABR file: its presets in order (from version 6; samples nothing names after
/// them), or its sampled tips (versions 1 and 2); none if it isn't one.
pub fn read_brushes(bytes: &[u8]) -> Option<Vec<AbrBrush>> {
    let mut r = Reader::new(bytes);
    let version = r.u16()?;
    match version {
        1 | 2 => Some(read_old(&mut r, version)?.into_iter().map(sampled).collect()),
        // 6, 7, 9 and 10 seen; any later one in the same sections is read the same way
        6.. if bytes.get(4..8) == Some(b"8BIM".as_slice()) => read_sections(&mut r),
        _ => None,
    }
}

/// A brush of a sampled tip alone.
fn sampled(t: AbrTip) -> AbrBrush {
    AbrBrush { name: t.name.clone(), spacing: t.spacing, tip: Some(t), ..AbrBrush::default() }
}

/// Versions 1 and 2: a count, then each tip: its kind, its length and (sampled) its image.
fn read_old(r: &mut Reader<'_>, version: u16) -> Option<Vec<AbrTip>> {
    let count = r.u16()?;
    let mut tips = Vec::new();
    for n in 1..=count {
        let kind = r.u16()?;
        let len = r.u32()? as usize;
        let body = r.take(len)?;
        if kind != 2 {
            continue;
        }
        let mut b = Reader::new(body);
        b.u32()?;
        let spacing = b.u16()?;
        let name = if version == 2 { b.unicode()? } else { String::new() };
        b.u8()?;
        // a short rectangle, then the one used
        b.take(8)?;
        let (top, left, bottom, right) = (b.i32()?, b.i32()?, b.i32()?, b.i32()?);
        let depth = b.u16()?;
        let compression = b.u8()?;
        if let Some(pixels) = image(&mut b, top, left, bottom, right, depth, compression) {
            tips.push(AbrTip {
                name: if name.trim().is_empty() { format!("Brush {n}") } else { name },
                spacing: (spacing > 0).then(|| f32::from(spacing) / 100.0),
                width: span(left, right)?,
                height: span(top, bottom)?,
                pixels,
            });
        }
    }
    Some(tips)
}

/// Version 6 on: "8BIM" sections; "samp" holds the tips by id, "patt" the patterns, "desc" the
/// presets.
fn read_sections(r: &mut Reader<'_>) -> Option<Vec<AbrBrush>> {
    let minor = r.u16()?;
    let mut samples: Vec<(String, u32, u32, Vec<u8>)> = Vec::new();
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut presets: Vec<Object> = Vec::new();
    while r.remaining() >= 12 {
        if r.take(4)? != b"8BIM" {
            break;
        }
        let key = r.take(4)?;
        let len = r.u32()? as usize;
        // a section cut short ends the file
        let Some(data) = r.take(len) else { break };
        // each is padded to a multiple of four
        r.take(((4 - len % 4) % 4).min(r.remaining()));
        match key {
            b"samp" => samples = read_samples(data, minor),
            b"patt" => patterns = read_patterns(data),
            b"desc" => presets = read_presets(data),
            _ => {}
        }
    }
    // a sample only a dual brush names isn't a brush of its own
    let used = samples
        .iter()
        .map(|s| presets.iter().any(|p| p.at(&["dualBrush", "Brsh"]).and_then(|b| b.text("sampledData")) == Some(&s.0)))
        .collect();
    let mut library = Library { samples, used, tip_bytes: MOST_TIP_BYTES, patterns, pattern_bytes: MOST_PATTERN_BYTES };
    let mut out: Vec<AbrBrush> = Vec::new();
    let named =
        |name: &str, n: usize| if name.trim().is_empty() { format!("Brush {}", n + 1) } else { name.to_owned() };
    for p in &presets {
        let name = named(p.text("Nm  ").unwrap_or_default(), out.len());
        // a sampled preset whose sample isn't there (or hasn't room for its copy) is left out
        if let Some(b) = preset(p, name, &mut library) {
            out.push(b);
        }
    }
    // samples nothing names, as they are
    let Library { samples, used, .. } = library;
    for ((_, width, height, pixels), u) in samples.into_iter().zip(used) {
        if !u {
            let name = named("", out.len());
            out.push(sampled(AbrTip { name, spacing: None, width, height, pixels }));
        }
    }
    Some(out)
}

/// The file's samples and patterns as the presets take copies of them, with room left for more.
struct Library {
    samples: Vec<(String, u32, u32, Vec<u8>)>,
    /// Whether a preset has taken each sample (or a dual brush names it).
    used: Vec<bool>,
    tip_bytes: usize,
    patterns: Vec<Pattern>,
    pattern_bytes: usize,
}

impl Library {
    /// A copy of the sample `id` as a tip named `name` spaced `spacing` apart, if the file has it
    /// and there is room for it.
    fn tip(&mut self, id: &str, name: &str, spacing: Option<f32>) -> Option<AbrTip> {
        let k = self.samples.iter().position(|s| s.0 == id)?;
        let ((_, width, height, pixels), used) = (self.samples.get(k)?, self.used.get_mut(k)?);
        self.tip_bytes = self.tip_bytes.checked_sub(pixels.len())?;
        *used = true;
        Some(AbrTip { name: name.to_owned(), spacing, width: *width, height: *height, pixels: pixels.clone() })
    }

    /// A copy of the pattern `id`, if the file has it and there is room for it.
    fn pattern(&mut self, id: &str) -> Option<&Pattern> {
        let p = self.patterns.iter().find(|t| t.id == id)?;
        self.pattern_bytes = self.pattern_bytes.checked_sub(p.pixels.len())?;
        Some(p)
    }
}

/// A preset as a brush, named `name`: its tip (none if it names a sample the file hasn't), then
/// all else it has on.
fn preset(p: &Object, name: String, library: &mut Library) -> Option<AbrBrush> {
    let mut b = shape(p.object("Brsh")?, &name, library)?;
    b.name = name;
    b.dynamics = dynamics(p);
    if p.flag("useScatter") {
        b.scatter = Some(scatter(p));
    }
    if let Some(d) = p.object("dualBrush").filter(|d| d.flag("useDualBrush")) {
        let tip_name = d.at(&["Brsh"]).and_then(|t| t.text("Nm  ")).unwrap_or(&b.name).to_owned();
        if let Some(mut dual) = d.object("Brsh").and_then(|t| shape(t, &tip_name, library)) {
            dual.name = tip_name;
            dual.dynamics.flip_x = d.flag("Flip");
            dual.scatter = d.flag("useScatter").then(|| scatter(d));
            b.dual = Some(Box::new(dual));
            b.dual_mode = d.enumerated("BlnM").map(str::to_owned);
        }
    }
    if p.flag("useTexture") {
        b.texture = texture(p, library);
    }
    if p.flag("useColorDynamics") {
        b.color = Some(AbrColor {
            hue: p.percent("H   ").unwrap_or(0.0).clamp(0.0, 1.0),
            saturation: p.percent("Strt").unwrap_or(0.0).clamp(0.0, 1.0),
            brightness: p.percent("Brgh").unwrap_or(0.0).clamp(0.0, 1.0),
            background: p.object("clVr").and_then(|v| v.percent("jitter")).unwrap_or(0.0).clamp(0.0, 1.0),
            per_tip: p.get("colorDynamicsPerTip").is_none_or(|v| matches!(v, Value::Bool(true))),
        });
    }
    b.noise = p.flag("Nose");
    if let Some(tool) = p.object("toolOptions") {
        b.dynamics.tool_flow = tool.number("flow").map(|v| (v as f32 / 100.0).clamp(0.0, 1.0));
        b.dynamics.tool_opacity = tool.number("Opct").map(|v| (v as f32 / 100.0).clamp(0.0, 1.0));
        b.blend = tool.enumerated("Md  ").map(str::to_owned);
    }
    Some(b)
}

/// A brush of a preset's tip (or a dual brush's): sampled, its sample's picture (none if the file
/// hasn't it), or computed round with its hardness; its diameter, angle, roundness, spacing and
/// whether it is mirrored.
fn shape(t: &Object, name: &str, library: &mut Library) -> Option<AbrBrush> {
    let spacing = t.percent("Spcn").filter(|s| *s > 0.0);
    let tip = match t.text("sampledData") {
        Some(id) => Some(library.tip(id, name, spacing)?),
        None => None,
    };
    Some(AbrBrush {
        name: name.to_owned(),
        hardness: tip.is_none().then(|| t.percent("Hrdn").map(|v| v.clamp(0.0, 1.0))).flatten(),
        tip,
        spacing,
        angle: t.number("Angl").map_or(0.0, |v| v.to_radians() as f32),
        roundness: t.percent("Rndn").filter(|v| *v > 0.0 && *v <= 1.0),
        diameter: t.number("Dmtr").filter(|v| *v > 0.0).map(|v| v as f32),
        flipped: (t.flag("flipX"), t.flag("flipY")),
        ..AbrBrush::default()
    })
}

/// A dynamic setting's control (`bVTy`), jitter and least (each 0 to 1), as a "brVr" object keeps
/// them; nothing for one the preset hasn't.
fn varied(p: &Object, key: &str) -> (i32, f32, f32) {
    let Some(v) = p.object(key) else { return (0, 0.0, 0.0) };
    let share = |k: &str| v.percent(k).map_or(0.0, |x| x.clamp(0.0, 1.0));
    (v.number("bVTy").map_or(0, |c| c as i32), share("jitter"), share("Mnm "))
}

/// A preset's shape and transfer dynamics, each with whether it is on.
fn dynamics(p: &Object) -> AbrDynamics {
    let share = |k: &str| p.percent(k).map_or(0.0, |x| x.clamp(0.0, 1.0));
    let (size_control, size_jitter, _) = varied(p, "szVr");
    let (angle_control, angle_jitter, _) = varied(p, "angleDynamics");
    let (roundness_control, roundness_jitter, _) = varied(p, "roundnessDynamics");
    let (opacity_control, opacity_jitter, opacity_min) = varied(p, "opVr");
    let (flow_control, flow_jitter, flow_min) = varied(p, "prVr");
    AbrDynamics {
        tip: p.flag("useTipDynamics"),
        size_control,
        min_diameter: share("minimumDiameter"),
        size_jitter,
        angle_control,
        angle_jitter,
        roundness_control,
        roundness_jitter,
        min_roundness: share("minimumRoundness"),
        flip_x: p.flag("flipX"),
        flip_y: p.flag("flipY"),
        paint: p.flag("usePaintDynamics"),
        opacity_control,
        opacity_min,
        opacity_jitter,
        flow_control,
        flow_min,
        flow_jitter,
        tool_flow: None,
        tool_opacity: None,
    }
}

/// A preset's (or a dual brush's) scattering: how far (its jitter, which may pass 100%), on which
/// axes, by what, and how many stamps a step.
fn scatter(p: &Object) -> AbrScatter {
    let (control, _, _) = varied(p, "scatterDynamics");
    let amount = p.at(&["scatterDynamics"]).and_then(|v| v.percent("jitter")).unwrap_or(0.0);
    let (count_control, count_jitter, _) = varied(p, "countDynamics");
    AbrScatter {
        amount: amount.clamp(0.0, 10.0),
        both_axes: p.flag("bothAxes"),
        control,
        count: p.number("Cnt ").map_or(1, |c| c.round().clamp(1.0, 16.0) as u32),
        count_jitter,
        count_control,
    }
}

/// A preset's texture, if it names a pattern the file has (and there is room for its copy).
fn texture(p: &Object, library: &mut Library) -> Option<AbrTexture> {
    let share = |k: &str| p.percent(k).map(|x| x.clamp(0.0, 1.0));
    let (depth_control, depth_jitter, _) = varied(p, "textureDepthDynamics");
    let t = library.pattern(p.at(&["Txtr"])?.text("Idnt")?)?;
    Some(AbrTexture {
        name: t.name.clone(),
        width: t.width,
        height: t.height,
        pixels: t.pixels.clone(),
        scale: p.percent("textureScale").filter(|v| *v > 0.0),
        depth: share("textureDepth"),
        invert: p.flag("InvT"),
        min_depth: share("minimumDepth"),
        depth_control,
        depth_jitter,
        brightness: p.number("textureBrightness").map_or(0, |v| v.clamp(-150.0, 150.0) as i32),
        contrast: p.number("textureContrast").map_or(0, |v| v.clamp(-50.0, 100.0) as i32),
        blend: p.enumerated("textureBlendMode").map(str::to_owned),
        each_tip: p.flag("TxtC"),
    })
}

/// The presets in the descriptor: its version, then an object whose "Brsh" list holds them. A
/// damaged one ends the list (those before it are kept).
fn read_presets(data: &[u8]) -> Vec<Object> {
    let mut out = Vec::new();
    // (what was read before a damaged part is kept either way)
    let _ = presets_into(&mut Reader::new(data), &mut out);
    out
}

fn presets_into(r: &mut Reader<'_>, out: &mut Vec<Object>) -> Option<()> {
    r.u32()?;
    r.unicode()?;
    r.key()?;
    for _ in 0..r.u32()? {
        if r.key()? != "Brsh" {
            value(r, 0)?;
            continue;
        }
        if r.take(4)? != b"VlLs" {
            return None;
        }
        for _ in 0..r.u32()? {
            match value(r, 1)? {
                Value::Object(o) if o.class == "brushPreset" => out.push(o),
                _ => {}
            }
        }
    }
    Some(())
}

/// A descriptor's value, as far as presets need them.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Object(Object),
    /// A double, a whole number, or a unit's value (its unit left out: "#Prc" percent, "#Pxl"
    /// pixels, "#Ang" degrees).
    Number(f64),
    Text(String),
    /// An enumerated value's own name.
    Enum(String),
    Bool(bool),
    /// What a preset's own settings don't use: a list, a reference, a class, raw data, units.
    Other,
}

/// A descriptor's object: its class and its values by key, in order.
#[derive(Clone, Debug, Default, PartialEq)]
struct Object {
    class: String,
    items: Vec<(String, Value)>,
}

impl Object {
    fn get(&self, key: &str) -> Option<&Value> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn object(&self, key: &str) -> Option<&Object> {
        match self.get(key)? {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The object down the path of keys.
    fn at(&self, path: &[&str]) -> Option<&Object> {
        path.iter().try_fold(self, |o, k| o.object(k))
    }

    fn number(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Value::Number(v) if v.is_finite() => Some(*v),
            _ => None,
        }
    }

    /// A percent's value as a share (100% is 1).
    fn percent(&self, key: &str) -> Option<f32> {
        self.number(key).map(|v| (v / 100.0) as f32).filter(|v| v.is_finite())
    }

    fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    fn enumerated(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Enum(s) => Some(s),
            _ => None,
        }
    }

    /// Whether the switch `key` is on (none is off).
    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }
}

/// An object: its name (unused), class and values, each a key and a typed value.
fn object(r: &mut Reader<'_>, nested: u32) -> Option<Object> {
    if nested > MOST_NESTED {
        return None;
    }
    r.unicode()?;
    let class = r.key()?;
    let mut items = Vec::new();
    for _ in 0..r.u32()? {
        let key = r.key()?;
        items.push((key, value(r, nested)?));
    }
    Some(Object { class, items })
}

/// A typed value: its four-letter type, then the value as that type keeps it.
fn value(r: &mut Reader<'_>, nested: u32) -> Option<Value> {
    if nested > MOST_NESTED {
        return None;
    }
    Some(match r.take(4)? {
        b"Objc" | b"GlbO" => Value::Object(object(r, nested + 1)?),
        b"VlLs" => {
            for _ in 0..r.u32()? {
                value(r, nested + 1)?;
            }
            Value::Other
        }
        b"doub" => Value::Number(r.f64()?),
        b"UntF" => {
            r.take(4)?;
            Value::Number(r.f64()?)
        }
        b"UnFl" => {
            r.take(4)?;
            let n = r.u32()? as usize;
            r.take(n.checked_mul(8)?)?;
            Value::Other
        }
        b"long" => Value::Number(f64::from(r.i32()?)),
        b"comp" => Value::Number(i64::from_be_bytes(r.take(8)?.try_into().ok()?) as f64),
        b"bool" => Value::Bool(r.u8()? != 0),
        b"TEXT" => Value::Text(r.unicode()?),
        b"enum" => {
            r.key()?;
            Value::Enum(r.key()?)
        }
        b"type" | b"GlbC" => {
            r.unicode()?;
            r.key()?;
            Value::Other
        }
        b"alis" | b"tdta" | b"Pth " => {
            let n = r.u32()? as usize;
            r.take(n)?;
            Value::Other
        }
        b"obj " => {
            reference(r)?;
            Value::Other
        }
        _ => return None,
    })
}

/// A reference, passed over: a count of items, each of its own form.
fn reference(r: &mut Reader<'_>) -> Option<()> {
    for _ in 0..r.u32()? {
        match r.take(4)? {
            b"prop" => {
                r.unicode()?;
                r.key()?;
                r.key()?;
            }
            b"Clss" => {
                r.unicode()?;
                r.key()?;
            }
            b"Enmr" => {
                r.unicode()?;
                r.key()?;
                r.key()?;
                r.key()?;
            }
            b"rele" => {
                r.unicode()?;
                r.key()?;
                r.i32()?;
            }
            b"Idnt" | b"indx" => {
                r.u32()?;
            }
            b"name" => {
                r.unicode()?;
                r.key()?;
                r.unicode()?;
            }
            _ => return None,
        }
    }
    Some(())
}

/// The samples: each its id and its image (version 6.1: straight after a short rectangle; later:
/// the one channel written of its array).
fn read_samples(data: &[u8], minor: u16) -> Vec<(String, u32, u32, Vec<u8>)> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while r.remaining() >= 4 {
        let Some(len) = r.u32() else { break };
        // samples are padded to a multiple of four
        let padded = (len as usize).div_ceil(4) * 4;
        let Some(body) = r.take(padded.min(r.remaining())) else { break };
        if let Some(sample) = read_sample(body.get(..len as usize).unwrap_or(body), minor) {
            out.push(sample);
        }
    }
    out
}

fn read_sample(body: &[u8], minor: u16) -> Option<(String, u32, u32, Vec<u8>)> {
    let mut b = Reader::new(body);
    let n = usize::from(b.u8()?);
    let id = String::from_utf8_lossy(b.take(n)?).into_owned();
    if minor == 1 {
        // a short rectangle and depth, then the one used
        b.take(10)?;
        let (top, left, bottom, right) = (b.i32()?, b.i32()?, b.i32()?, b.i32()?);
        let depth = b.u16()?;
        let compression = b.u8()?;
        let pixels = image(&mut b, top, left, bottom, right, depth, compression)?;
        return Some((id, span(left, right)?, span(top, bottom)?, pixels));
    }
    b.take(4)?;
    let (width, height, pixels) = channels(&mut b, 1)?.into_iter().next()?;
    Some((id, width, height, pixels))
}

/// The first `most` channels written in an array of them (its version, length and rectangle, then
/// each channel's rectangle, depth, compression and image), as grey images; none if there are none.
fn channels(b: &mut Reader<'_>, most: usize) -> Option<Vec<(u32, u32, Vec<u8>)>> {
    b.u32()?;
    b.u32()?;
    b.take(16)?;
    let count = b.u32()?;
    let mut out = Vec::new();
    // the colour channels, then a mask's and a sheet's
    for _ in 0..count.saturating_add(2).min(64) {
        if out.len() >= most {
            break;
        }
        let Some(written) = b.u32() else { break };
        if written == 0 {
            continue;
        }
        let len = b.u32()? as usize;
        if len == 0 {
            continue;
        }
        let mut c = Reader::new(b.take(len)?);
        c.u32()?;
        let (top, left, bottom, right) = (c.i32()?, c.i32()?, c.i32()?, c.i32()?);
        let depth = c.u16()?;
        let compression = c.u8()?;
        let pixels = image(&mut c, top, left, bottom, right, depth, compression)?;
        out.push((span(left, right)?, span(top, bottom)?, pixels));
    }
    (!out.is_empty()).then_some(out)
}

/// A pattern from the file, grey.
struct Pattern {
    id: String,
    name: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// The patterns: each its version, colour mode, size, name, id (an indexed one's palette after)
/// and its channels, made grey: an RGB one's three by how bright each looks, an indexed one's
/// through its palette. Up to [`MOST_PATTERN_BYTES`] of them.
fn read_patterns(data: &[u8]) -> Vec<Pattern> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    let mut left = MOST_PATTERN_BYTES;
    while r.remaining() >= 4 {
        let Some(len) = r.u32() else { break };
        // patterns are padded to a multiple of four
        let padded = (len as usize).div_ceil(4) * 4;
        let Some(body) = r.take(padded.min(r.remaining())) else { break };
        let Some(p) = read_pattern(body.get(..len as usize).unwrap_or(body)) else { continue };
        let Some(rest) = left.checked_sub(p.pixels.len()) else { break };
        left = rest;
        out.push(p);
    }
    out
}

fn read_pattern(body: &[u8]) -> Option<Pattern> {
    let mut b = Reader::new(body);
    b.u32()?;
    let mode = b.u32()?;
    b.take(4)?;
    let name = b.unicode()?;
    let n = usize::from(b.u8()?);
    let id = String::from_utf8_lossy(b.take(n)?).into_owned();
    let palette = if mode == 2 { Some(b.take(768)?) } else { None };
    let found = channels(&mut b, if mode == 3 { 3 } else { 1 })?;
    let (width, height, first) = found.first()?;
    let bright = |r: u8, g: u8, b: u8| ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000) as u8;
    let pixels = match (palette, found.as_slice()) {
        (_, [(_, _, r), (_, _, g), (_, _, bl)]) if r.len() == g.len() && g.len() == bl.len() => {
            r.iter().zip(g).zip(bl).map(|((&r, &g), &bl)| bright(r, g, bl)).collect()
        }
        (Some(colours), _) => first
            .iter()
            .map(|&i| {
                let c = |k: usize| colours.get(usize::from(i) * 3 + k).copied().unwrap_or(0);
                bright(c(0), c(1), c(2))
            })
            .collect(),
        _ => first.clone(),
    };
    Some(Pattern { id, name, width: *width, height: *height, pixels })
}

/// A tip's image: raw or packed (each row's length first), 8 or 16 bits; read as 8.
fn image(
    r: &mut Reader<'_>,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    depth: u16,
    compression: u8,
) -> Option<Vec<u8>> {
    let (w, h) = (span(left, right)? as usize, span(top, bottom)? as usize);
    let sample = match depth {
        8 => 1,
        16 => 2,
        _ => return None,
    };
    let row = w * sample;
    let raw = match compression {
        0 => r.take(row * h)?.to_vec(),
        1 => {
            let counts: Vec<usize> = (0..h).map(|_| r.u16().map(usize::from)).collect::<Option<_>>()?;
            let mut out = Vec::with_capacity(row * h);
            for c in counts {
                unpack(r.take(c)?, row, &mut out);
            }
            out
        }
        _ => return None,
    };
    Some(if sample == 2 { raw.as_chunks::<2>().0.iter().map(|&[hi, _]| hi).collect() } else { raw })
}

/// A rectangle's side, if it is a sensible one.
fn span(from: i32, to: i32) -> Option<u32> {
    u32::try_from(i64::from(to) - i64::from(from)).ok().filter(|&n| n > 0 && n <= MOST_SIDE)
}

/// PackBits: a row of `row` bytes unpacked onto `out` (short rows filled with zeros).
fn unpack(packed: &[u8], row: usize, out: &mut Vec<u8>) {
    let end = out.len() + row;
    let mut i = packed.iter();
    while out.len() < end {
        let Some(&n) = i.next() else { break };
        let n = n as i8;
        if n >= 0 {
            for _ in 0..=n {
                out.push(i.next().copied().unwrap_or(0));
            }
        } else if n != -128 {
            let v = i.next().copied().unwrap_or(0);
            out.extend(std::iter::repeat_n(v, usize::from(n.unsigned_abs()) + 1));
        }
    }
    out.resize(end, 0);
}

/// Reads big-endian numbers and runs of bytes in order; None past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A descriptor's key: its length, then that many letters (a length of 0: four).
    fn key(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let bytes = self.take(if n == 0 { 4 } else { n })?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// A count of UTF-16 units, then the units (a trailing nul left off).
    fn unicode(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let units: Vec<u16> =
            self.take(n.checked_mul(2)?)?.as_chunks::<2>().0.iter().map(|&b| u16::from_be_bytes(b)).collect();
        Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn put32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn unicode(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
        put32(out, units.len() as u32);
        for u in units {
            put16(out, u);
        }
    }

    /// A descriptor's value as a test writes it.
    enum V {
        Obj(&'static str, Vec<(&'static str, V)>),
        List(Vec<V>),
        /// A unit and its value ("#Prc" percent, "#Pxl" pixels, "#Ang" degrees).
        Unit(&'static str, f64),
        Long(i32),
        Text(String),
        Bool(bool),
        Enum(&'static str, &'static str),
    }

    fn text(s: &str) -> V {
        V::Text(s.to_owned())
    }
    fn percent(v: f64) -> V {
        V::Unit("#Prc", v)
    }
    /// A dynamic setting: its control, jitter and least (percent).
    fn varied(control: i32, jitter: f64, least: f64) -> V {
        V::Obj(
            "brVr",
            vec![
                ("bVTy", V::Long(control)),
                ("fStp", V::Long(25)),
                ("jitter", percent(jitter)),
                ("Mnm ", percent(least)),
            ],
        )
    }

    /// A key: four letters as a length of 0 and them, any other as its length and its letters.
    fn key(out: &mut Vec<u8>, k: &str) {
        put32(out, if k.len() == 4 { 0 } else { k.len() as u32 });
        out.extend_from_slice(k.as_bytes());
    }

    fn put(out: &mut Vec<u8>, v: &V) {
        match v {
            V::Obj(class, items) => {
                out.extend_from_slice(b"Objc");
                object(out, class, items);
            }
            V::List(items) => {
                out.extend_from_slice(b"VlLs");
                put32(out, items.len() as u32);
                for i in items {
                    put(out, i);
                }
            }
            V::Unit(unit, v) => {
                out.extend_from_slice(b"UntF");
                out.extend_from_slice(unit.as_bytes());
                out.extend_from_slice(&v.to_be_bytes());
            }
            V::Long(v) => {
                out.extend_from_slice(b"long");
                out.extend_from_slice(&v.to_be_bytes());
            }
            V::Text(s) => {
                out.extend_from_slice(b"TEXT");
                unicode(out, s);
            }
            V::Bool(on) => {
                out.extend_from_slice(b"bool");
                out.push(u8::from(*on));
            }
            V::Enum(kind, name) => {
                out.extend_from_slice(b"enum");
                key(out, kind);
                key(out, name);
            }
        }
    }

    fn object(out: &mut Vec<u8>, class: &str, items: &[(&str, V)]) {
        unicode(out, "");
        key(out, class);
        put32(out, items.len() as u32);
        for (k, v) in items {
            key(out, k);
            put(out, v);
        }
    }

    /// The "desc" section of these presets: its version, then the object listing them.
    fn descriptor(presets: Vec<V>) -> Vec<u8> {
        let mut out = Vec::new();
        put32(&mut out, 16);
        object(&mut out, "null", &[("Brsh", V::List(presets))]);
        out
    }

    /// A preset named `name` of the brush `brush`, with more settings after it.
    fn preset(name: &str, brush: V, more: Vec<(&'static str, V)>) -> V {
        let mut items = vec![("Nm  ", text(name)), ("Brsh", brush)];
        items.extend(more);
        V::Obj("brushPreset", items)
    }

    /// A sampled tip of the sample `id`, spaced `spacing` percent.
    fn sampled_brush(id: &str, spacing: f64, more: Vec<(&'static str, V)>) -> V {
        let mut items = vec![("Dmtr", V::Unit("#Pxl", 30.0)), ("Spcn", percent(spacing))];
        items.extend(more);
        items.push(("sampledData", text(id)));
        V::Obj("sampledBrush", items)
    }

    /// A version 6 file with one 3 x 2 tip (raw) named by its preset.
    fn version6() -> Vec<u8> {
        with_presets(&descriptor(vec![preset("Chalky", sampled_brush("tip-one", 40.0, vec![]), vec![])]))
    }

    /// A version 6 file of the 3 x 2 tip "tip-one" (and, if asked, the 2 x 2 tip "tip-two") and
    /// the descriptor `desc`.
    fn with_presets(desc: &[u8]) -> Vec<u8> {
        with_tips(desc, &[("tip-one", 3, 2, &[0, 128, 255, 10, 20, 30])])
    }

    fn with_tips(desc: &[u8], tips: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
        let mut samp = Vec::new();
        for (id, w, h, pixels) in tips {
            let mut channel = Vec::new();
            put32(&mut channel, 8);
            for v in [0, 0, *h, *w] {
                put32(&mut channel, v);
            }
            put16(&mut channel, 8);
            channel.push(0);
            channel.extend_from_slice(pixels);
            let mut sample = vec![id.len() as u8];
            sample.extend_from_slice(id.as_bytes());
            sample.extend_from_slice(&[0, 1, 0, 0]);
            put32(&mut sample, 3);
            put32(&mut sample, 0);
            for v in [0, 0, *h, *w] {
                put32(&mut sample, v);
            }
            put32(&mut sample, 2);
            put32(&mut sample, 0);
            put32(&mut sample, 1);
            put32(&mut sample, channel.len() as u32);
            sample.extend_from_slice(&channel);
            put32(&mut samp, sample.len() as u32);
            samp.extend_from_slice(&sample);
            while !samp.len().is_multiple_of(4) {
                samp.push(0);
            }
        }
        let desc = desc.to_vec();
        let mut out = Vec::new();
        put16(&mut out, 6);
        put16(&mut out, 2);
        for (key, data) in [(b"samp", &samp), (b"desc", &desc)] {
            out.extend_from_slice(b"8BIM");
            out.extend_from_slice(key);
            put32(&mut out, data.len() as u32);
            out.extend_from_slice(data);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        }
        out
    }

    #[test]
    fn a_damaged_file_is_refused_not_a_crash() {
        let good = version6();
        assert!(read(&good).is_some());
        crate::damaged::survives("ABR", &good, |b| {
            let _ = read(b);
        });
    }

    #[test]
    fn a_version_9_file_is_read_like_version_6_and_other_files_are_not() {
        let six = version6();
        let mut nine = six.clone();
        nine[..2].copy_from_slice(&9u16.to_be_bytes());
        let (a, b) = (read_brushes(&six).unwrap(), read_brushes(&nine).unwrap());
        assert_eq!(a.len(), b.len());
        assert!(!b.is_empty());
        assert_eq!(a[0].name, b[0].name);
        // a zip (a brush bundle, say) starts with a large first number but no sections
        assert!(read_brushes(&[0x50, 0x4b, 3, 4, 20, 0, 0, 0, 8, 0]).is_none());
        let mut no_sections = nine;
        no_sections[4..8].copy_from_slice(b"XXXX");
        assert!(read_brushes(&no_sections).is_none());
    }

    #[test]
    fn a_version_6_1_tip_is_read_after_its_short_rectangle_and_padding_is_passed_over() {
        let id = "tip-old";
        let mut sample = vec![id.len() as u8];
        sample.extend_from_slice(id.as_bytes());
        for v in [0u16, 0, 2, 3, 8] {
            put16(&mut sample, v);
        }
        for v in [0u32, 0, 2, 3] {
            put32(&mut sample, v);
        }
        put16(&mut sample, 8);
        sample.push(0);
        sample.extend_from_slice(&[9, 8, 7, 6, 5, 4]);
        let mut samp = Vec::new();
        put32(&mut samp, sample.len() as u32);
        samp.extend_from_slice(&sample);
        let desc = descriptor(vec![preset("Old Stamp", sampled_brush(id, 25.0, vec![]), vec![])]);
        let mut out = Vec::new();
        put16(&mut out, 6);
        put16(&mut out, 1);
        // a section of three bytes, padded to four, before the ones read
        for (key, data) in [(b"patt", &vec![1, 2, 3]), (b"samp", &samp), (b"desc", &desc)] {
            out.extend_from_slice(b"8BIM");
            out.extend_from_slice(key);
            put32(&mut out, data.len() as u32);
            out.extend_from_slice(data);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        }
        let brushes = read_brushes(&out).unwrap();
        assert_eq!(brushes.len(), 1);
        let tip = brushes[0].tip.as_ref().unwrap();
        assert_eq!((brushes[0].name.as_str(), tip.width, tip.height), ("Old Stamp", 3, 2));
        assert_eq!(tip.pixels, [9, 8, 7, 6, 5, 4]);
    }

    #[test]
    fn a_presets_dynamics_say_what_pressure_changes_and_what_goes_at_random() {
        let desc = descriptor(vec![preset(
            "Soft",
            sampled_brush("tip-one", 25.0, vec![("flipX", V::Bool(true))]),
            vec![
                // shape dynamics: size by pressure (control 2) down to 40%, jittering 9%; turned
                // with the stroke and 30% at random; flattened at random down to a quarter;
                // mirrored at random side to side
                ("useTipDynamics", V::Bool(true)),
                ("flipX", V::Bool(true)),
                ("flipY", V::Bool(false)),
                ("minimumDiameter", percent(40.0)),
                ("minimumRoundness", percent(25.0)),
                ("szVr", varied(2, 9.0, 0.0)),
                ("angleDynamics", varied(ANGLE_BY_DIRECTION, 30.0, 0.0)),
                ("roundnessDynamics", varied(0, 50.0, 0.0)),
                // transfer: opacity by pressure jittering 5%, flow by nothing down to 20%
                ("usePaintDynamics", V::Bool(true)),
                ("prVr", varied(0, 10.0, 20.0)),
                ("opVr", varied(2, 5.0, 0.0)),
                // the tool's own: an opacity and a flow, multiplying (its own sizes after)
                (
                    "toolOptions",
                    V::Obj(
                        "PbTl",
                        vec![
                            ("flow", V::Long(76)),
                            ("Opct", V::Long(90)),
                            ("Md  ", V::Enum("BlnM", "Mltp")),
                            ("szVr", varied(3, 0.0, 0.0)),
                        ],
                    ),
                ),
            ],
        )]);
        let b = &read_brushes(&with_presets(&desc)).unwrap()[0];
        assert_eq!(b.flipped, (true, false));
        assert_eq!(
            b.dynamics,
            AbrDynamics {
                tip: true,
                size_control: 2,
                min_diameter: 0.4,
                size_jitter: 0.09,
                angle_control: ANGLE_BY_DIRECTION,
                angle_jitter: 0.3,
                roundness_jitter: 0.5,
                min_roundness: 0.25,
                flip_x: true,
                paint: true,
                opacity_control: 2,
                opacity_jitter: 0.05,
                flow_min: 0.2,
                flow_jitter: 0.1,
                tool_flow: Some(0.76),
                tool_opacity: Some(0.9),
                ..AbrDynamics::default()
            }
        );
        assert_eq!(b.blend.as_deref(), Some("Mltp"));
    }

    #[test]
    fn a_preset_paints_through_the_pattern_it_names() {
        let textured = |name: &str, on: bool| {
            preset(
                name,
                sampled_brush("tip-one", 25.0, vec![]),
                vec![
                    ("useTexture", V::Bool(on)),
                    ("TxtC", V::Bool(true)),
                    ("textureBlendMode", V::Enum("BlnM", "Hght")),
                    ("textureDepth", percent(40.0)),
                    ("minimumDepth", percent(17.0)),
                    ("textureDepthDynamics", varied(BY_PRESSURE, 8.0, 0.0)),
                    ("Txtr", V::Obj("Ptrn", vec![("Nm  ", text("Paper")), ("Idnt", text("pat-1"))])),
                    ("textureScale", percent(50.0)),
                    ("InvT", V::Bool(true)),
                    ("textureBrightness", V::Long(30)),
                    ("textureContrast", V::Long(-20)),
                ],
            )
        };
        let desc = descriptor(vec![textured("Grainy", true), textured("Smooth", false)]);
        // a grey 2 x 2 pattern, its channels after its name and id
        let mut pattern = Vec::new();
        put32(&mut pattern, 1);
        put32(&mut pattern, 1);
        put16(&mut pattern, 2);
        put16(&mut pattern, 2);
        unicode(&mut pattern, "Paper");
        pattern.push(5);
        pattern.extend_from_slice(b"pat-1");
        let mut channel = Vec::new();
        put32(&mut channel, 8);
        for v in [0u32, 0, 2, 2] {
            put32(&mut channel, v);
        }
        put16(&mut channel, 8);
        channel.push(0);
        channel.extend_from_slice(&[10, 20, 30, 40]);
        put32(&mut pattern, 3);
        put32(&mut pattern, 0);
        for v in [0u32, 0, 2, 2] {
            put32(&mut pattern, v);
        }
        put32(&mut pattern, 24);
        put32(&mut pattern, 1);
        put32(&mut pattern, channel.len() as u32);
        pattern.extend_from_slice(&channel);
        let mut patt = Vec::new();
        put32(&mut patt, pattern.len() as u32);
        patt.extend_from_slice(&pattern);
        let mut file = with_presets(&desc);
        file.extend_from_slice(b"8BIMpatt");
        put32(&mut file, patt.len() as u32);
        file.extend_from_slice(&patt);
        let brushes = read_brushes(&file).unwrap();
        assert_eq!(brushes.len(), 2);
        let t = brushes[0].texture.as_ref().unwrap();
        assert_eq!((t.name.as_str(), t.width, t.height, t.pixels.as_slice()), ("Paper", 2, 2, &[10, 20, 30, 40][..]));
        assert_eq!((t.scale, t.depth, t.invert, t.each_tip), (Some(0.5), Some(0.4), true, true));
        assert_eq!((t.min_depth, t.brightness, t.contrast, t.blend.as_deref()), (Some(0.17), 30, -20, Some("Hght")));
        assert_eq!((t.depth_control, t.depth_jitter), (BY_PRESSURE, 0.08));
        // the other's texture is off
        assert_eq!(brushes[1].texture, None);
        crate::damaged::survives("ABR with a pattern", &file, |b| {
            let _ = read_brushes(b);
        });
    }

    #[test]
    fn a_version_6_tip_comes_with_its_presets_name_and_spacing() {
        let tips = read(&version6()).unwrap();
        assert_eq!(tips.len(), 1);
        let t = &tips[0];
        assert_eq!((t.width, t.height), (3, 2));
        assert_eq!(t.pixels, [0, 128, 255, 10, 20, 30]);
        assert_eq!(t.name, "Chalky");
        assert_eq!(t.spacing, Some(0.4));
    }

    #[test]
    fn a_dual_brush_comes_with_its_preset_and_its_tip_is_not_a_brush_of_its_own() {
        let computed = |hardness: f64, more: Vec<(&'static str, V)>| {
            let mut items = vec![
                ("Dmtr", V::Unit("#Pxl", 12.0)),
                ("Hrdn", percent(hardness)),
                ("Angl", V::Unit("#Ang", 30.0)),
                ("Rndn", percent(40.0)),
                ("Spcn", percent(25.0)),
            ];
            items.extend(more);
            V::Obj("computedBrush", items)
        };
        let dual = V::Obj(
            "dualBrush",
            vec![
                ("useDualBrush", V::Bool(true)),
                ("Flip", V::Bool(true)),
                ("Brsh", sampled_brush("tip-two", 5.0, vec![("Nm  ", text("Spatter"))])),
                ("BlnM", V::Enum("BlnM", "CBrn")),
                ("useScatter", V::Bool(true)),
                ("Cnt ", V::Long(3)),
                ("bothAxes", V::Bool(false)),
                ("countDynamics", varied(0, 50.0, 0.0)),
                ("scatterDynamics", varied(0, 135.0, 0.0)),
            ],
        );
        let desc = descriptor(vec![
            // a computed round preset, scattered on both axes two at a time, whose dual brush is
            // the second sample
            preset(
                "Hard Round",
                computed(50.0, vec![]),
                vec![
                    ("useScatter", V::Bool(true)),
                    ("Cnt ", V::Long(2)),
                    ("bothAxes", V::Bool(true)),
                    ("countDynamics", varied(0, 0.0, 0.0)),
                    ("scatterDynamics", varied(BY_PRESSURE, 250.0, 0.0)),
                    ("dualBrush", dual),
                ],
            ),
            // a sampled preset turned a quarter, its dual brush off
            preset(
                "Leaf",
                sampled_brush("tip-one", 25.0, vec![("Angl", V::Unit("#Ang", 90.0)), ("Rndn", percent(100.0))]),
                vec![("dualBrush", V::Obj("dualBrush", vec![("useDualBrush", V::Bool(false))]))],
            ),
        ]);
        let tips: [(&str, u32, u32, &[u8]); 2] =
            [("tip-one", 3, 2, &[0, 128, 255, 10, 20, 30]), ("tip-two", 2, 2, &[1, 2, 3, 4])];
        let file = with_tips(&desc, &tips);
        let brushes = read_brushes(&file).unwrap();
        // the sample only the dual brush names isn't a brush of its own
        let names: Vec<&str> = brushes.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Hard Round", "Leaf"]);
        let round = &brushes[0];
        assert!(round.tip.is_none());
        assert_eq!((round.hardness, round.roundness, round.spacing), (Some(0.5), Some(0.4), Some(0.25)));
        assert!((round.angle - 30f32.to_radians()).abs() < 1e-6);
        assert_eq!(
            round.scatter,
            Some(AbrScatter {
                amount: 2.5,
                both_axes: true,
                control: BY_PRESSURE,
                count: 2,
                count_jitter: 0.0,
                count_control: 0
            })
        );
        let second = round.dual.as_deref().unwrap();
        assert_eq!(round.dual_mode.as_deref(), Some("CBrn"));
        let tip = second.tip.as_ref().unwrap();
        assert_eq!((second.name.as_str(), tip.width, tip.pixels.as_slice()), ("Spatter", 2, &[1, 2, 3, 4][..]));
        assert_eq!((second.diameter, second.spacing, second.dynamics.flip_x), (Some(30.0), Some(0.05), true));
        let s = second.scatter.unwrap();
        assert_eq!((s.amount, s.both_axes, s.count, s.count_jitter), (1.35, false, 3, 0.5));
        let leaf = &brushes[1];
        assert_eq!((leaf.tip.as_ref().map(|t| t.width), leaf.roundness, leaf.hardness), (Some(3), Some(1.0), None));
        assert!((leaf.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(leaf.dual.is_none() && leaf.scatter.is_none());
        // the tips alone: the sampled one
        assert_eq!(read(&file).unwrap().len(), 1);
        crate::damaged::survives("ABR with a dual brush", &file, |b| {
            let _ = read_brushes(b);
        });
    }

    #[test]
    fn colour_dynamics_and_noise_come_with_the_preset() {
        let desc = descriptor(vec![preset(
            "Leaves",
            sampled_brush("tip-one", 25.0, vec![]),
            vec![
                ("useColorDynamics", V::Bool(true)),
                ("clVr", varied(0, 40.0, 0.0)),
                ("H   ", percent(5.0)),
                ("Strt", percent(10.0)),
                ("Brgh", percent(20.0)),
                ("colorDynamicsPerTip", V::Bool(false)),
                ("Nose", V::Bool(true)),
            ],
        )]);
        let b = &read_brushes(&with_presets(&desc)).unwrap()[0];
        assert_eq!(
            b.color,
            Some(AbrColor { hue: 0.05, saturation: 0.1, brightness: 0.2, background: 0.4, per_tip: false })
        );
        assert!(b.noise);
    }

    #[test]
    fn presets_read_before_a_damaged_one_are_kept() {
        let one = |name: &str| preset(name, sampled_brush("tip-one", 25.0, vec![]), vec![]);
        let mut desc = descriptor(vec![one("First"), one("Second")]);
        // the second preset's brush given a type no descriptor has
        let at = desc.windows(4).rposition(|w| w == b"Objc").unwrap();
        desc[at..at + 4].copy_from_slice(b"XXXX");
        let brushes = read_brushes(&with_presets(&desc)).unwrap();
        assert_eq!(brushes.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["First"]);
    }

    #[test]
    fn a_version_2_packed_tip_and_bad_files() {
        // one sampled 2 x 2 tip, each row packed as a run
        let mut body = Vec::new();
        put32(&mut body, 0);
        put16(&mut body, 25);
        unicode(&mut body, "Old");
        body.push(1);
        body.extend_from_slice(&[0; 8]);
        for v in [0u32, 0, 2, 2] {
            put32(&mut body, v);
        }
        put16(&mut body, 8);
        body.push(1);
        put16(&mut body, 2);
        put16(&mut body, 2);
        body.extend_from_slice(&[0xFF, 200, 0xFF, 7]);
        let mut file = Vec::new();
        put16(&mut file, 2);
        put16(&mut file, 1);
        put16(&mut file, 2);
        put32(&mut file, body.len() as u32);
        file.extend_from_slice(&body);
        let tips = read(&file).unwrap();
        assert_eq!(tips[0].pixels, [200, 200, 7, 7]);
        assert_eq!(tips[0].name, "Old");
        assert_eq!(tips[0].spacing, Some(0.25));
        assert!(read(b"not a brush").is_none());
        assert_eq!(read(&version6()[..20]).map(|t| t.len()), Some(0));
    }
}
