use super::appearance::{AppearanceRows, appearance_rows, system_theme};
use super::dialogs::{mismatch_body, presets_body, presets_io_body, settings_height};
use super::graphics::{gpu_memory_lines, gpu_status_rows, memory_usage_row, read_at_launch, rendering_mode_row};
use super::shortcuts::shortcuts_body;
use super::{Color32, Map, PhotocraftApp, RichText, SECTIONS, Sense, Tokens, Value, json, prefs, vec2};

// ------------------------------------------------------------------ bodies

/// Render one of our dialogs' bodies.
pub fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    match f.get("__prefsui").and_then(Value::as_str).unwrap_or("") {
        "prefs" => {
            let mut info = app.perf.gpu_info.lines();
            info.extend(gpu_memory_lines(app));
            f.insert("__gpuInfo".into(), json!(info));
            f.insert("__gpuAdapters".into(), json!(app.perf.gpu_info.adapters));
            let system = system_theme(app, ui.ctx());
            let pressure_peak = app.stylus.calibration_peak();
            prefs_body(ui, f, system, pressure_peak);
        }
        "shortcuts" => shortcuts_body(app, ui, f),
        "presets" => presets_body(app, ui, f),
        "presetsIO" => presets_io_body(ui, f),
        "mismatch" => mismatch_body(ui, f),
        _ => {}
    }
}

/// The label a preference key shows in the dialog: a few keys have their own wording, the rest
/// split camelCase into words (`convertToSrgb` → "Convert to srgb").
pub(super) fn humanize(key: &str) -> String {
    match key {
        "appearanceMode" => return "Appearance Mode".into(),
        "exrAcesCg" => return "Open color-less EXR as ACEScg".into(),
        "darkTheme" => return "Dark Theme".into(),
        "lightTheme" => return "Light Theme".into(),
        "automaticallyCheckUpdates" => return "Automatically Check for Updates".into(),
        _ => {}
    }
    // These controls appear only for WebP, so reuse the existing translated labels.
    if key == "webpLossless" {
        return "Lossless".into();
    }
    if key == "webpQuality" {
        return "Quality".into();
    }
    // The auto-hide notice settings read best with their own labels (Interface › Notification, #2022).
    if key == "notificationAutoHide" {
        return "Auto Hide Notifications".into();
    }
    if key == "notificationDurationSeconds" {
        return "Notification Duration (seconds)".into();
    }
    // Tools: arrow-key nudges bundle into one History state (#2259).
    if key == "bundleNudges" {
        return "Bundle Arrow-Key Nudges".into();
    }
    if key == "nudgeBundlePauseMs" {
        return "Nudge Bundle Pause (ms)".into();
    }
    // Performance: presentation without vsync (#1343); like the other graphics settings, it
    // takes effect when the window next opens.
    // File Handling: Photoshop's wording (#8080).
    if key == "doNotAppendCopy" {
        return "Do not append \"copy\" to filename when saving a copy".into();
    }
    if key == "pcraftFullComposite" {
        return "Save Full-Size Composite in .pcraft Files".into();
    }
    // Plug-ins: Photoshop's wording (#9392).
    if key == "generatorEnabled" {
        return "Enable Generator".into();
    }
    if key == "lowLatencyDrawing" {
        return "Low-Latency Drawing (vsync off, applies at next launch)".into();
    }
    // Performance: the canvas compositor's GPU memory, 0 for the automatic budget (L3).
    // Scratch Disks: how large the scratch file may grow; the free-space reserve always applies.
    if key == "sizeLimitMb" {
        return "Scratch Size Limit (MB, 0 = No Limit)".into();
    }
    if key == "gpuMemoryMb" {
        return "GPU Memory (MB, 0 = Automatic)".into();
    }
    let mut s = String::new();
    for (i, ch) in key.chars().enumerate() {
        if i == 0 {
            s.extend(ch.to_uppercase());
        } else if ch.is_uppercase() {
            s.push(' ');
            s.extend(ch.to_lowercase());
        } else {
            s.push(ch);
        }
    }
    s.replace("Psd", "PSD")
        .replace("Gpu", "GPU")
        .replace("Ui ", "UI ")
        .replace("Mb", "(MB)")
        .replace("Exif", "EXIF")
        .replace("Hud", "HUD")
        .replace("Lut ", "LUT ")
}

/// The visible label of a stored choice value (`windowsInk` -> "Windows Ink (default)").
pub(super) fn choice_label(v: &str) -> String {
    match v {
        "windowsInk" => "Windows Ink (default)".into(),
        "winTab" => "WinTab".into(),
        "exr" => "EXR".into(),
        "cm" => "Centimeters".into(),
        "mm" => "Millimeters".into(),
        "75" | "80" | "85" | "90" | "95" | "100" | "105" | "110" | "115" | "120" | "125" | "150" | "175" | "200" | "250" | "300" => format!("{v}%"),
        "8" => "8 Bits/Channel".into(),
        "16" => "16 Bits/Channel".into(),
        "postScript" => "PostScript (72 points/inch)".into(),
        "traditional" => "Traditional (72.27 points/inch)".into(),
        "vulkan" => "Vulkan".into(),
        "dx12" => "DirectX 12".into(),
        "metal" => "Metal".into(),
        "gl" => "OpenGL".into(),
        "ahd" => "AHD (adaptive homogeneity)".into(),
        "mhc" => "Malvar–He–Cutler".into(),
        "cpu" => "CPU (no GPU acceleration)".into(),
        v => humanize(v),
    }
}

/// An editable `#rrggbb` field beside a colour swatch (#668): typing a valid value sets the
/// colour; while it's being typed a partial value is kept, and it shows the colour otherwise.
pub(super) fn hex_field(ui: &mut egui::Ui, key: &str, rgb: &mut [u8; 3]) {
    let id = egui::Id::unique(("pref-hex", key));
    let shown = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.clone());
    let r = crate::widgets::text_field_ex(ui, id, &mut text, "", 72.0, true, Some(7)).accessible_name(tl!("Color"));
    if r.changed()
        && let Some(c) = prefs::parse_hex(&text)
    {
        *rgb = c;
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
}

pub(super) fn color_of(s: &str) -> Color32 {
    prefs::parse_hex(s).map_or(Color32::GRAY, |c| Color32::from_rgb(c[0], c[1], c[2]))
}

/// Search the same visible, editable preference labels used in the dialog.
pub(super) fn preference_matches(section: &str, key: &str, query: &str) -> bool {
    let label = humanize(key);
    let en = crate::i18n::Lang::EN;
    en.contains_ignore_case(&format!("{section}.{key}"), query)
        || en.contains_ignore_case(&label, query)
        || crate::i18n::current().contains_ignore_case(tl!(&label), query)
        // The Tablet input row and its Pen Tablet note are the visible terms, not just the key.
        || (cfg!(target_os = "windows") && section == "tablet" && key == "tabletInput" && en.contains_ignore_case("windows ink pen tablet wintab tablet input", query))
}

pub(super) fn preference_sections(values: &Value, search: &str) -> Vec<(&'static str, &'static str)> {
    let query = search.trim();
    SECTIONS
        .iter()
        .copied()
        .filter(|(id, title)| {
            query.is_empty()
                || crate::i18n::Lang::EN.contains_ignore_case(title, query)
                || crate::i18n::current().contains_ignore_case(tl!(title), query)
                || values
                    .get(*id)
                    .and_then(Value::as_object)
                    .is_some_and(|fields| fields.keys().any(|key| !prefs::is_hidden(&format!("{id}.{key}")) && preference_matches(id, key, query)))
        })
        .collect()
}

/// Preferences: section list on the left, the section's settings on the right.
fn prefs_body(ui: &mut egui::Ui, f: &mut Map<String, Value>, system: Option<egui::Theme>, pressure_peak: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    let mut section = f.get("section").and_then(Value::as_str).unwrap_or("general").to_string();
    let mut values = f.get("values").cloned().unwrap_or(Value::Null);
    // The dialog follows the language being edited, so a change shows before OK.
    let lang = crate::i18n::Lang::from_pref(values.pointer("/interface/language").and_then(Value::as_str).unwrap_or("auto"));
    let mut search = f.get("__search").and_then(Value::as_str).unwrap_or("").to_string();
    crate::widgets::text_field(ui, ui.scope_id().with("prefs-search"), &mut search, tl!("Search settings…"), ui.available_width(), false);
    let found = preference_sections(&values, &search);
    if !found.iter().any(|(id, _)| *id == section)
        && let Some((first, _)) = found.first()
    {
        section = (*first).to_string();
    }
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        // Section list.
        ui.vertical(|ui| {
            ui.set_width(170.0);
            if found.is_empty() {
                ui.label(RichText::new(tl!("No matching settings")).color(t.text_dim));
            }
            for &(id, title) in &found {
                let sel = section == id;
                let empty = !has_visible_fields(&values, id);
                let (rect, resp) = ui.allocate_exact_size(vec2(170.0, 22.0), Sense::click());
                if sel {
                    ui.painter().rect_filled(rect, t.radius_sm, t.row_selected);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                }
                let color = if sel {
                    t.text
                } else if empty {
                    t.text_faint
                } else {
                    t.text_dim
                };
                // Translated section names can be longer than the column: elide them (the full
                // name is the tooltip) instead of drawing over the settings.
                let mut job = egui::text::LayoutJob::simple_singleline(tl!(title).to_string(), crate::theme::medium(12.5), color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 12.0);
                let galley = ui.painter().layout_job(job);
                let elided = galley.elided;
                ui.painter().galley(rect.left_center() + vec2(8.0, -galley.size().y / 2.0), galley, color);
                let resp = if elided { resp.on_hover_text(tl!(title)) } else { resp };
                if resp.clicked() {
                    section = id.to_string();
                }
            }
        });
        // The settings fill the dialog's resizable area down to the button row (#7420). Sizing them
        // from the window instead left a resized dialog with empty space and the buttons mid-way.
        // The area grows to fit its content, so this must never ask for more than it was given.
        let content_height = settings_height(ui.available_height(), ui.spacing().item_spacing.y, crate::widgets::button_height(ui.ctx()));
        crate::widgets::vline(ui, content_height);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 12.0).max(340.0));
            let title = SECTIONS.iter().find(|(id, _)| *id == section).map_or("General", |(_, t)| *t);
            ui.label(RichText::new(tl!(&title)).font(crate::theme::semibold(14.0)).color(t.text));
            let show_all = search.trim().is_empty()
                || crate::i18n::Lang::EN.contains_ignore_case(title, search.trim())
                || lang.contains_ignore_case(tl!(&title), search.trim());
            let filter = if show_all { "" } else { search.trim() };
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(content_height - 30.0).id_salt("prefs-scroll").show(ui, |ui| {
                let order: Vec<String> =
                    f.get("__order").and_then(|o| o.get(&section)).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                if !has_visible_fields(&values, &section) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(tl!("These settings aren't available in PhotoCraft yet.")).color(t.text_faint));
                } else if let Some(obj) = values.get_mut(&section).and_then(Value::as_object_mut) {
                    section_fields(ui, &section, obj, &order, lang, system, filter, pressure_peak);
                    if section == "fileHandling" {
                        ui.label(
                            RichText::new(tl!("0 turns this threshold off; SVG groups nested deeper than 100 levels are always rasterized."))
                                .color(t.text_faint),
                        );
                    }
                    if section == "performance" {
                        gpu_status_rows(ui, f.get("__gpuInfo"), f.get("__gpuAdapters"), obj);
                    }
                    ui.add_space(8.0);
                }
                if search.trim().is_empty()
                    && has_visible_fields(&values, &section)
                    && crate::widgets::secondary_button(ui, tl!("Reset Section"), 110.0).clicked()
                    && let Some(mut def) = prefs::Preferences::default().get(&section)
                {
                    // The favourite fonts are the user's library, not a Type setting: kept, as
                    // `prefs.reset {"path": "type"}` keeps them.
                    if section == "type"
                        && let Some(favorites) = values.get(&section).and_then(|v| v.get("favoriteFonts")).cloned()
                        && let Some(def) = def.as_object_mut()
                    {
                        def.insert("favoriteFonts".into(), favorites);
                    }
                    values[&section] = def;
                }
            });
        });
    });
    f.insert("section".into(), json!(section));
    f.insert("__search".into(), json!(search));
    f.insert("values".into(), values);
    // A colour swatch was clicked: open the Color Picker on that value (#2144).
    if let Some((path, name)) = ctx_take_pick(ui) {
        crate::color_picker_ui::request_field(f, &format!("/values/{}", path.replace('.', "/")), &crate::color_picker_ui::title_for(&name));
    }
}

/// Does `section` have any setting the dialog shows (see [`prefs::HIDDEN_UNTIL_IMPLEMENTED`])?
pub(super) fn has_visible_fields(values: &Value, section: &str) -> bool {
    values.get(section).and_then(Value::as_object).is_some_and(|o| o.keys().any(|k| !prefs::is_hidden(&format!("{section}.{k}"))))
}

/// JPEG and WebP have independent settings; unrelated format controls stay out of view.
pub(super) fn export_field_visible(obj: &Map<String, Value>, key: &str) -> bool {
    let format = obj.get("quickExportFormat").and_then(Value::as_str).unwrap_or("png");
    match key {
        "convertToSrgb" | "metadata" => format != "exr",
        "jpegQuality" => format == "jpg",
        "webpLossless" => format == "webp",
        "webpQuality" => format == "webp" && obj.get("webpLossless").and_then(Value::as_bool) != Some(true),
        _ => true,
    }
}

/// The pause that bundles arrow-key nudges means nothing with Bundle Arrow-Key Nudges off (#2259).
/// WinTab is a Windows-only opt-in.
pub(super) fn tools_field_visible(obj: &Map<String, Value>, key: &str) -> bool {
    match key {
        // The pen's Windows input source lives on the Tablet page now.
        "tabletInput" => false,
        "nudgeBundlePauseMs" => obj.get("bundleNudges").and_then(Value::as_bool) != Some(false),
        _ => true,
    }
}

/// A titled, outlined group of settings (Photoshop's Interface page has Appearance,
/// Presentation and Options).
fn pref_group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.card_border)).corner_radius(t.radius).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(tl!(title)).strong().color(t.text));
        ui.add_space(4.0);
        add(ui);
    });
    ui.add_space(8.0);
}

/// Preferences › Interface laid out like Photoshop's: the theme and canvas settings, then UI
/// language, font size and scaling, then everything else. Compact enough that the language
/// shows without scrolling in a small window (#2532). A search (`query`, lowercased) hides the
/// rows that don't match and skips a group left empty.
fn interface_groups(ui: &mut egui::Ui, obj: &mut Map<String, Value>, keys: &[String], lang: crate::i18n::Lang, system: Option<egui::Theme>, query: &str) {
    const APPEARANCE: [&str; 3] = ["canvasColor", "canvasCustomColor", "canvasBorder"];
    const PRESENTATION: [&str; 3] = ["language", "uiFontSize", "uiScale"];
    let shown = |k: &str| keys.iter().any(|key| key == k) && field_visible("interface", obj, k, query);
    // In Photoshop's order, not the stored one.
    let pick = |names: &[&str]| names.iter().filter(|n| shown(n)).map(|n| n.to_string()).collect::<Vec<_>>();
    let appearance = pick(&APPEARANCE);
    let presentation = pick(&PRESENTATION);
    let rest = keys.iter().filter(|k| !APPEARANCE.contains(&k.as_str()) && !PRESENTATION.contains(&k.as_str()) && shown(k));
    // Checkboxes line up at the left edge, as in Photoshop; the other options keep a label column.
    let (checks, other): (Vec<String>, Vec<String>) = rest.cloned().partition(|k| obj.get(k).is_some_and(Value::is_boolean));
    let rows = AppearanceRows::matching(query);
    let grid = |id: &str| egui::Grid::new(("prefs-grid", id.to_string())).num_columns(2).spacing([14.0, 7.0]).min_col_width(150.0);
    if rows.any() || !appearance.is_empty() {
        pref_group(ui, "Appearance", |ui| {
            grid("interface-appearance").show(ui, |ui| {
                appearance_rows(ui, obj, system, rows);
                field_rows(ui, "interface", obj, appearance, lang);
            });
        });
    }
    if !presentation.is_empty() {
        pref_group(ui, "Presentation", |ui| {
            grid("interface-presentation").show(ui, |ui| field_rows(ui, "interface", obj, presentation, lang));
        });
    }
    if !checks.is_empty() || !other.is_empty() {
        pref_group(ui, "Options", |ui| {
            egui::Grid::new("prefs-grid-interface-checks").num_columns(2).spacing([0.0, 7.0]).show(ui, |ui| field_rows(ui, "interface", obj, checks, lang));
            grid("interface-options").show(ui, |ui| field_rows(ui, "interface", obj, other, lang));
        });
    }
}

/// Where a clicked colour swatch leaves its preference path and label for the dialog body, which
/// holds the fields the Color Picker request goes in.
const PICK_ID: &str = "prefs-pick-color";

/// The colour swatch clicked this frame, if any: (preference path, label).
fn ctx_take_pick(ui: &egui::Ui) -> Option<(String, String)> {
    ui.ctx().data_mut(|d| d.remove_temp::<(String, String)>(egui::Id::unique(PICK_ID)))
}

/// Whether `section.key` gets a generic row for this search (`query`, lowercased; empty matches
/// everything). Settings nothing reads yet stay out of the dialog (issue #204), as do those drawn
/// by their own widgets; their stored values pass through untouched.
pub(super) fn field_visible(section: &str, obj: &Map<String, Value>, k: &str, query: &str) -> bool {
    let path = format!("{section}.{k}");
    let drawn_elsewhere = (section == "interface" && matches!(k, "theme" | "appearanceMode" | "darkTheme" | "lightTheme" | "favoriteThemes"))
        // View › Rulers (⌘R) sets it, as in Photoshop.
        || path == "unitsAndRulers.showRulers"
        || (section == "performance" && matches!(k, "useGpu" | "gpuBackend" | "gpuAdapter" | "renderingMode" | "memoryUsageMb"))
        // The browser presents frames itself: Low-Latency Drawing does nothing there.
        || (cfg!(target_arch = "wasm32") && path == "performance.lowLatencyDrawing")
        || (section == "export" && !export_field_visible(obj, k))
        || (section == "tools" && (!tools_field_visible(obj, k) || k == "tabletPressure"));
    (query.is_empty() || preference_matches(section, k, query))
        && !drawn_elsewhere
        && !prefs::is_hidden(&path)
        && obj.get(k).is_some_and(|v| (path == "performance.memoryUsagePercent" && v.is_null()) || !matches!(v, Value::Null | Value::Object(_)))
}

/// Generic editor for a section's fields: checkboxes, dropdowns for choices, colour swatches,
/// number fields with the preference's range, text fields.
pub(super) fn section_fields(
    ui: &mut egui::Ui,
    section: &str,
    obj: &mut Map<String, Value>,
    order: &[String],
    lang: crate::i18n::Lang,
    system: Option<egui::Theme>,
    search: &str,
    pressure_peak: Option<f32>,
) {
    let query = search.trim();
    if section == "tablet" {
        tablet_fields(ui, obj, pressure_peak);
        return;
    }
    let mut keys: Vec<String> = order.iter().filter(|k| obj.contains_key(*k)).cloned().collect();
    keys.extend(obj.keys().filter(|k| !order.contains(k)).cloned());
    if section == "interface" {
        interface_groups(ui, obj, &keys, lang, system, query);
        return;
    }
    if section == "performance" && (query.is_empty() || preference_matches(section, "renderingMode", query)) {
        rendering_mode_row(ui, obj);
    }
    if section == "performance" && (query.is_empty() || preference_matches(section, "memoryUsageMb", query)) {
        memory_usage_row(ui, obj);
    }
    keys.retain(|k| field_visible(section, obj, k, query));
    egui::Grid::new(("prefs-grid", section)).num_columns(2).spacing([14.0, 7.0]).show(ui, |ui| field_rows(ui, section, obj, keys, lang));
}


/// Preferences › Tablet: the pen's own page (the reference application's), gathered from
/// `tools.tabletPressure` and `tools.tabletInput` — the Windows input source, the three response
/// fields, and a calibration area that records how hard the pen is pressed while dragging in it.
fn tablet_fields(ui: &mut egui::Ui, obj: &mut Map<String, Value>, pressure_peak: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    ui.set_max_width(360.0);
    // The pen arrives two ways on Windows: WinTab (the tablet driver's API, with tilt, rotation
    // and the eraser end) or Ink (winit's pointer path, pressure only).
    if cfg!(target_os = "windows") {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Tablet input mode")).color(t.text_dim).strong());
            let mut value = obj.get("tabletInput").and_then(Value::as_str).unwrap_or("windowsInk").to_string();
            for (v, label) in [("windowsInk", tl!("Windows Ink")), ("winTab", tl!("WinTab"))] {
                if ui.selectable_label(value == v, label).clicked() {
                    value = v.to_string();
                    obj.insert("tabletInput".into(), json!(value));
                }
            }
        });
        ui.add_space(6.0);
    }
    egui::Grid::new("prefs-tablet-grid").num_columns(2).spacing([14.0, 7.0]).show(ui, |ui| {
        let mut row = |ui: &mut egui::Ui, key: &str, label: &str, range: std::ops::RangeInclusive<f64>, scale: f64, suffix: &str| {
            ui.label(RichText::new(tl!(label)).color(t.text_dim).strong());
            if let Some(v) = obj.get(key).and_then(Value::as_f64) {
                let mut shown = (v * scale) as f32;
                let (lo, hi) = (*range.start() as f32, *range.end() as f32);
                if crate::widgets::value_field(ui, &mut shown, lo..=hi, suffix, 172.0).changed() {
                    obj.insert(key.into(), json!(f64::from(shown) / scale));
                }
            }
            ui.end_row();
        };
        row(ui, "deadZone", "Pressure dead zone", 0.0..=30.0, 100.0, "%");
        row(ui, "elasticity", "Elasticity", 1.0..=100.0, 10.0, "");
        row(ui, "coordinateWindow", "Stroke stability", 1.0..=16.0, 1.0, "");
    });
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Pressure calibration")).color(t.text).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let peak = obj.get("maximumPressure").and_then(Value::as_f64).unwrap_or(1.0);
            ui.label(RichText::new(format!("{} {:.1}%", tl!("Retained peak"), peak * 100.0)).color(t.text_dim).strong());
        });
    });
    ui.add_space(4.0);
    let size = vec2(ui.available_width().min(420.0), 112.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    ui.painter().rect_filled(rect, 8.0, t.field);
    ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    let peak_id = response.id.with("calibration-peak");
    let trace_id = response.id.with("calibration-trace");
    if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        ui.data_mut(|d| d.insert_temp(peak_id, 0.0_f32));
        ui.data_mut(|d| d.insert_temp(trace_id, Vec::<egui::Pos2>::new()));
    }
    if (response.is_pointer_button_down_on() || response.drag_stopped() || response.clicked())
        && let Some(pressure) = pressure_peak
    {
        let peak = ui.data(|d| d.get_temp::<f32>(peak_id)).unwrap_or(0.0).max(pressure);
        ui.data_mut(|d| d.insert_temp(peak_id, peak));
        if let Some(pointer) = response.interact_pointer_pos() {
            ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<egui::Pos2>>(trace_id).push(pointer));
        }
        // The retained peak is the hardest the pen pressed: full pressure then maps to 100 %.
        if peak > 0.0 {
            obj.insert("maximumPressure".into(), json!(f64::from(peak)));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(peak_id));
        ui.data_mut(|d| d.remove::<Vec<egui::Pos2>>(trace_id));
    }
    let trace = ui.data(|d| d.get_temp::<Vec<egui::Pos2>>(trace_id).unwrap_or_default());
    if trace.len() > 1 {
        ui.painter().add(egui::Shape::line(trace, egui::Stroke::new(2.0, t.accent)));
    }
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            RichText::new(tl!(
                "Maximum pressure maps the pen's full press to 100%; the dead zone ignores a light resting touch, elasticity shapes the response curve, and stroke stability averages the last few positions."
            ))
            .small()
            .color(t.text_dim),
        )
        .wrap(),
    );
    if cfg!(target_os = "windows") {
        ui.add(
            egui::Label::new(
                RichText::new(tl!(
                    "WinTab reads pressure, tilt, rotation and the eraser end from the tablet driver. Turn off Windows Ink in your tablet's software utility (for example Wacom Center) before using it."
                ))
                .small()
                .color(t.text_dim),
            )
            .wrap(),
        );
    }
}

/// One grid row per setting in `keys` (already filtered by `field_visible`).
fn field_rows(ui: &mut egui::Ui, section: &str, obj: &mut Map<String, Value>, keys: Vec<String>, lang: crate::i18n::Lang) {
    let t = Tokens::get(ui.ctx());
    for k in keys {
        let path = format!("{section}.{k}");
        let v = obj.get(&k).cloned().unwrap_or(Value::Null);
        let human = humanize(&k);
        let label = tl!(&human).to_string();
        if path == "performance.memoryUsagePercent" {
            ui.label(RichText::new(&label).color(t.text_dim));
            let mut enabled = v.is_number();
            let mut percent = v.as_u64().and_then(|p| u32::try_from(p).ok()).unwrap_or(70);
            ui.horizontal(|ui| {
                crate::widgets::checkbox(ui, &mut enabled, "");
                ui.add_enabled(enabled, egui::DragValue::new(&mut percent).range(1..=95).suffix("%"));
            });
            obj.insert(k, if enabled { json!(percent) } else { Value::Null });
            ui.end_row();
            continue;
        }
        match &v {
            Value::Bool(b) => {
                ui.label("");
                let mut b = *b;
                if read_at_launch(&path) {
                    ui.vertical(|ui| {
                        crate::widgets::checkbox(ui, &mut b, &label);
                        ui.label(RichText::new(tl!("Applies at next launch.")).color(t.text_dim));
                    });
                } else {
                    crate::widgets::checkbox(ui, &mut b, &label);
                }
                obj.insert(k, json!(b));
            }
            Value::String(s) if path == "interface.language" => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                let mut pairs: Vec<(String, &str)> = vec![("auto".into(), tl!("Auto"))];
                pairs.extend(crate::i18n::Lang::for_picker().into_iter().map(|l| (l.code().to_string(), l.name())));
                let mut cur = s.clone();
                crate::widgets::dropdown(ui, &format!("pref-{path}"), tl!(&label), &mut cur, &pairs, 220.0);
                obj.insert(k, json!(cur));
            }
            Value::String(s) if prefs::choices(&path).is_some() => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                let opts = prefs::choices(&path).unwrap_or(&[]);
                let labels: Vec<String> = opts.iter().map(|o| choice_label(o)).collect();
                let pairs: Vec<(String, &str)> = opts.iter().map(|o| o.to_string()).zip(labels.iter().map(String::as_str)).collect();
                let mut cur = s.clone();
                ui.vertical(|ui| {
                    crate::widgets::dropdown(ui, &format!("pref-{path}"), tl!(&label), &mut cur, &pairs, 220.0);
                    if read_at_launch(&path) {
                        ui.label(RichText::new(tl!("Applies at next launch.")).color(t.text_dim));
                    }
                });
                obj.insert(k, json!(cur));
            }
            Value::String(s) if prefs::is_color(&path) => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                let c = prefs::parse_hex(s).unwrap_or([128, 128, 128]);
                let mut rgb = c;
                ui.horizontal(|ui| {
                    // PhotoCraft's Color Picker, as everywhere else (#2144).
                    if crate::widgets::color_swatch_button(ui, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), &label).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(PICK_ID), (path.clone(), label.clone())));
                    }
                    hex_field(ui, &path, &mut rgb);
                });
                obj.insert(k, json!(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])));
                let _ = color_of(s);
            }
            Value::String(s) => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                let mut s = s.clone();
                ui.add(egui::TextEdit::singleline(&mut s).desired_width(260.0)).accessible_name(tl!(&label));
                obj.insert(k, json!(s));
            }
            Value::Number(n) => {
                let value_label = ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                let (lo, hi) = prefs::range(&path).unwrap_or((-1e9, 1e9));
                if n.is_u64() || n.is_i64() {
                    let mut x = n.as_i64().unwrap_or(0);
                    ui.add(egui::DragValue::new(&mut x).range(lo as i64..=hi as i64).custom_parser(crate::widgets::parse_num)).labelled_by(value_label.id);
                    obj.insert(k, json!(x));
                    // A row of its own, so the label stays level with its field.
                    if read_at_launch(&path) {
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(tl!("Applies at next launch.")).color(t.text_dim));
                    }
                } else {
                    let mut x = n.as_f64().unwrap_or(0.0);
                    ui.add(egui::DragValue::new(&mut x).range(lo..=hi).speed(0.1).max_decimals(3).custom_parser(crate::widgets::parse_num))
                        .labelled_by(value_label.id);
                    obj.insert(k, json!(x));
                }
            }
            Value::Array(_) if path == "tools.pressureCurve" => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                if let Some(next) = crate::pressure_curve_ui::editor(ui, &v) {
                    obj.insert(k, next);
                }
            }
            Value::Array(items) if k == "disks" => {
                ui.label(RichText::new(tl!("Scratch disks")).color(t.text_dim));
                let mut items = items.clone();
                ui.vertical(|ui| {
                    for d in &mut items {
                        ui.horizontal(|ui| {
                            let mut on = d.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                            let mut path = d.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                            crate::widgets::checkbox(ui, &mut on, tl!("Enabled"));
                            ui.add(egui::TextEdit::singleline(&mut path).desired_width(240.0)).accessible_name(tl!("Path"));
                            *d = json!({"enabled": on, "path": path});
                        });
                    }
                    if ui.small_button(tl!("Add disk")).clicked() {
                        items.push(json!({"enabled": true, "path": ""}));
                    }
                    let scratch = photocraft_raster::spill::stats();
                    if scratch.free_space_unmeasured {
                        let dir = scratch.dir.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| tl!("Unknown").into());
                        let warning = crate::i18n::fmt(
                            tl!("The current scratch directory ({path}) doesn't report its free space, so PhotoCraft can't stop before it fills up, and large files get no extra room from it without a Scratch Size Limit. This warning applies to the current configuration; changes above take effect on Apply or OK. Set a limit or use a local disk."),
                            &[("path", &dir)],
                        );
                        ui.add(egui::Label::new(RichText::new(warning).color(t.warning)).wrap());
                    }
                });
                obj.insert(k, Value::Array(items));
            }
            Value::Array(items) => {
                ui.label(RichText::new(tl!(&label)).color(t.text_dim));
                ui.label(RichText::new(crate::i18n::trn(lang, items.len() as u64, "{n} item", "{n} items")).color(t.text_faint));
                if !items.is_empty() && ui.small_button(tl!("Clear")).clicked() {
                    obj.insert(k, json!([]));
                }
            }
            _ => continue,
        }
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::preference_matches;

    /// The Tablet input row matches its visible search terms.
    #[test]
    fn wintab_row_matches_its_visible_terms() {
        if cfg!(target_os = "windows") {
            for query in ["windows ink", "pen tablet", "wintab", "tablet input"] {
                assert!(preference_matches("tablet", "tabletInput", query), "{query}");
            }
        }
        assert!(preference_matches("tools", "useTabletPressure", "tablet pressure"));
    }
}
