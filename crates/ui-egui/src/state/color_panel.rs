use super::{Deserialize, Serialize};

/// Selected tab per dock card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockTabs {
    pub properties: usize,
    pub color: usize,
    pub layers: usize,
    /// Navigator | Histogram | Info.
    pub navigator: usize,
    /// History | Actions.
    pub history: usize,
    /// Character | Paragraph.
    pub character: usize,
    /// Brushes presets.
    pub brushes: usize,
    /// Tool presets.
    pub tool_presets: usize,
    /// Gradients | Patterns.
    pub presets: usize,
}

/// Color panel state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColorPanelState {
    /// The panel edits the background colour (its chip was clicked), not the foreground.
    pub background: bool,
    /// Color Picker › Only Web Colors, kept across opens like Photoshop (#4995). `web_only` is
    /// its name before the camelCase rename (#6000).
    #[serde(default, alias = "web_only")]
    pub web_only: bool,
    /// The 色盘 the panel shows on top, picked from its panel menu.
    pub mode: ColorPanelMode,
    /// 色条: the HSB sliders under the 色盘, switched on from the panel menu (never instead of it).
    pub hsb_strip: bool,
    /// 色条: the Lab sliders under the 色盘.
    pub lab_strip: bool,
    /// The colours picked most recently (newest first), drawn as the C2 layout's swatch column.
    #[serde(default = "recent_default")]
    pub recent: [[f32; 4]; 6],
}

impl Default for ColorPanelState {
    fn default() -> Self {
        Self {
            background: false,
            web_only: false,
            mode: ColorPanelMode::default(),
            hsb_strip: false,
            lab_strip: false,
            recent: recent_default(),
        }
    }
}

/// The C2 layout's starting swatches, as the reference application shows them.
fn recent_default() -> [[f32; 4]; 6] {
    [[0.0, 0.0, 0.0, 1.0], [0.86, 0.12, 0.12, 1.0], [0.04, 0.32, 0.92, 1.0], [0.05, 0.52, 0.24, 1.0], [0.0; 4], [0.0; 4]]
}
/// The 色盘 the panel draws on top: the reference layout, Photoshop's Hue Cube or its Color Wheel.
/// The panel keeps the disc whatever the 色条 below are switched to — a disc is the top half of the
/// panel and is never replaced by strips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorPanelMode {
    /// The C2 layout: colour and pattern source, chips, a large saturation/brightness field with
    /// its hue strip, and the recent swatches.
    #[default]
    C2,
    /// Photoshop's Hue Cube: a saturation/brightness field with its hue strip.
    HueCube,
    /// Photoshop's Color Wheel: a hue ring around a saturation/brightness triangle.
    ColorWheel,
}

impl ColorPanelMode {
    /// Every 色盘, in the panel menu's order.
    pub const ALL: [ColorPanelMode; 3] = [ColorPanelMode::C2, ColorPanelMode::HueCube, ColorPanelMode::ColorWheel];

    /// The panel menu item.
    pub fn label(self) -> &'static str {
        match self {
            ColorPanelMode::C2 => "C2 Color",
            ColorPanelMode::HueCube => "Hue Cube",
            ColorPanelMode::ColorWheel => "Color Wheel",
        }
    }
}

/// The 色条 under the 色盘: the colour as sliders. Switched on and off from the panel menu,
/// independently of the 色盘 above, so turning one on never takes the colour field away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorStrip {
    /// H, S, B.
    Hsb,
    /// L, a, b.
    Lab,
}

impl ColorStrip {
    /// Both strips, in the panel menu's order.
    pub const ALL: [ColorStrip; 2] = [ColorStrip::Hsb, ColorStrip::Lab];

    /// The panel menu item.
    pub fn label(self) -> &'static str {
        match self {
            ColorStrip::Hsb => "HSB Sliders",
            ColorStrip::Lab => "Lab Sliders",
        }
    }
}

impl ColorPanelState {
    /// Whether the strip is switched on.
    pub fn strip(self, strip: ColorStrip) -> bool {
        match strip {
            ColorStrip::Hsb => self.hsb_strip,
            ColorStrip::Lab => self.lab_strip,
        }
    }

    /// Switch the strip on or off; returns the new state.
    pub fn toggle_strip(&mut self, strip: ColorStrip) -> bool {
        let slot = match strip {
            ColorStrip::Hsb => &mut self.hsb_strip,
            ColorStrip::Lab => &mut self.lab_strip,
        };
        *slot = !*slot;
        *slot
    }
}