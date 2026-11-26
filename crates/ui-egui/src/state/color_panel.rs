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
    /// What the panel shows, picked from its panel menu.
    pub mode: ColorPanelMode,
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
            recent: recent_default(),
        }
    }
}

/// The C2 layout's starting swatches, as the reference application shows them.
fn recent_default() -> [[f32; 4]; 6] {
    [[0.0, 0.0, 0.0, 1.0], [0.86, 0.12, 0.12, 1.0], [0.04, 0.32, 0.92, 1.0], [0.05, 0.52, 0.24, 1.0], [0.0; 4], [0.0; 4]]
}

/// The Color panel's display. Two kinds of face, and a face is never the other kind: a disc
/// (the reference layout, the hue cube, the color wheel) puts the colour on a field, and a strip
/// (HSB, Lab) shows it as sliders. A disc carries no sliders of its own and a strip carries no
/// field, so the two never share a panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorPanelMode {
    /// The C2 layout: colour and pattern source, chips, a large saturation/brightness field with
    /// its hue strip, and the recent and image swatch rows.
    #[default]
    C2,
    /// Photoshop's Hue Cube: a saturation/brightness field with its hue strip.
    HueCube,
    /// Photoshop's Color Wheel: a hue ring around a saturation/brightness triangle.
    ColorWheel,
    /// The colour as H, S, B sliders.
    HsbSliders,
    /// The colour as L, a, b sliders.
    LabSliders,
}

impl ColorPanelMode {
    /// Every mode, in the panel menu's order.
    pub const ALL: [ColorPanelMode; 5] =
        [ColorPanelMode::C2, ColorPanelMode::HueCube, ColorPanelMode::ColorWheel, ColorPanelMode::HsbSliders, ColorPanelMode::LabSliders];

    /// The 色盘 group of the panel menu: the faces that put the colour on a disc or a square.
    pub const DISC: [ColorPanelMode; 3] = [ColorPanelMode::C2, ColorPanelMode::HueCube, ColorPanelMode::ColorWheel];

    /// The 色条 group of the panel menu: the faces that show the colour as sliders.
    pub const STRIP: [ColorPanelMode; 2] = [ColorPanelMode::HsbSliders, ColorPanelMode::LabSliders];

    /// The panel menu item.
    pub fn label(self) -> &'static str {
        match self {
            ColorPanelMode::C2 => "C2 Color",
            ColorPanelMode::HueCube => "Hue Cube",
            ColorPanelMode::ColorWheel => "Color Wheel",
            ColorPanelMode::HsbSliders => "HSB Sliders",
            ColorPanelMode::LabSliders => "Lab Sliders",
        }
    }

    /// The faces that show the colour as sliders (色条), never as a field.
    pub fn is_sliders(self) -> bool {
        matches!(self, ColorPanelMode::HsbSliders | ColorPanelMode::LabSliders)
    }
}
