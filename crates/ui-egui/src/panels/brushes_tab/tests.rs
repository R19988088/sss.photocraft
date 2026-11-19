//! The preset grid (the Brush Preset picker's and the Brushes tab's) is centred: every row has the
//! same left margin, and the spare width is split between the two sides.

use egui::{Rect, accesskit::Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::*;

#[test]
fn dragging_a_brush_shows_its_tip_preview() {
    for theme in crate::theme::ThemeKind::ALL {
        for mode in 0..3 {
            let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
            app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
            app.ui.tool = crate::Tool::Brush;
            app.run("prefs.set", json!({"path": "interface.theme", "value": theme.id()})).unwrap();
            app.ui.panels.brush_settings = mode != 2;
            app.ui.brush_tab = 1;
            app.ui.brushes_panel.view = if mode == 1 { BrushesView::Grid } else { BrushesView::List };
            if mode == 2 {
                app.ui.brush_picker = Some([300.0, 120.0]);
            }
            let name = app.session.tools.presets[0].name.clone();
            let brush = app.session.tools.brush.clone();
            let presets = app.session.tools.presets.clone();
            let mut h = Harness::builder().with_size(vec2(1200.0, 900.0)).build_eframe(move |cc| {
                PhotocraftApp::setup_context(&cc.egui_ctx, theme);
                app
            });
            h.run_steps(4);
            let start = h.get_by_role_and_label(Role::Button, &name).rect().center();
            h.hover_at(start);
            h.run_steps(1);
            h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
            let end = start + vec2(50.0, 10.0);
            h.event(egui::Event::PointerMoved(end));
            h.run_steps(4);
            assert!(matches!(egui::DragAndDrop::payload::<BrushDrag>(&h.ctx).as_deref(), Some(BrushDrag::Preset(n)) if n == &name));
            assert!(h.query_by_role_and_label(Role::Image, &name).is_some(), "dragged brush needs a real tip thumbnail, not only its name");
            let renders = brush_preview::render_count(&h.ctx);
            h.run_steps(2);
            assert_eq!(brush_preview::render_count(&h.ctx), renders, "unchanged drag preview stays cached");
            h.event(egui::Event::PointerMoved(start));
            h.run_steps(1);
            h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
            h.run_steps(4);
            assert!(h.query_by_role_and_label(Role::Image, &name).is_none(), "preview disappears on release");
            assert_eq!(h.state().session.tools.brush, brush, "preview does not change brush settings");
            assert_eq!(h.state().session.tools.presets, presets, "dropping on the source preserves the library");
        }
    }
}

/// #5741: a brush dragged to the list's bottom edge (or below it) scrolls the list, so it can be
/// dropped on rows out of view; the list used to stay put, and the drag stopped at the last
/// visible row.
#[test]
fn dragging_a_brush_to_the_list_edge_scrolls_the_list() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
    app.ui.panels.brush_settings = true;
    app.ui.brush_tab = 1;
    app.ui.brushes_panel.view = BrushesView::List;
    app.ui.brushes_panel.collapsed.clear();
    let name = app.session.tools.presets[0].name.clone();
    let (w, h_px) = (1200.0, 520.0);
    let mut h = Harness::builder().with_size(vec2(w, h_px)).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, crate::theme::ThemeKind::ALL[0]);
        app
    });
    h.run_steps(4);
    let start = h.get_by_role_and_label(Role::Button, &name).rect();
    h.hover_at(start.center());
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: start.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerMoved(start.center() + vec2(0.0, 12.0)));
    h.run_steps(2);
    assert!(egui::DragAndDrop::has_payload_of_type::<BrushDrag>(&h.ctx), "the brush is being dragged");
    // Held at the bottom of the window, below the list's last visible row.
    let bottom = egui::pos2(start.center().x, h_px - 2.0);
    for _ in 0..12 {
        h.event(egui::Event::PointerMoved(bottom));
        h.run_steps(1);
    }
    let moved = h.query_by_role_and_label(Role::Button, &name).map(|n| n.rect());
    assert!(moved.is_none_or(|r| r.top() < start.top() - 1.0), "the list scrolled down: {start:?} → {moved:?}");
    h.event(egui::Event::PointerButton { pos: bottom, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn grid_columns_centre_whole_columns_past_the_indent() {
    // 6 × 44 + 5 × 3 = 279 of the 290 past a 4 pt indent: 5 spare on each side (rounded down).
    assert_eq!(grid_columns(294.0, 44.0, 4.0, GRID_GAP), (9.0, 6));
    // An exact fit leaves only the indent.
    assert_eq!(grid_columns(4.0 + 279.0, 44.0, 4.0, GRID_GAP), (4.0, 6));
    // One point short of a column: one fewer, centred.
    assert_eq!(grid_columns(4.0 + 278.0, 44.0, 4.0, GRID_GAP), (4.0 + 23.0, 5));
    // The picker's cards centre with their own tighter gap: 6 × 44 + 5 × 1 = 269 of the 290.
    assert_eq!(grid_columns(294.0, 44.0, 4.0, CARD_GAP), (4.0 + 10.0, 6));
    for (w, cell, indent) in [(294.0, 44.0, 4.0), (360.0, 52.0, 20.0), (500.0, 52.0, 20.0)] {
        let (left, cols) = grid_columns(w, cell, indent, GRID_GAP);
        let right = w - left - (cols as f32 * cell + (cols - 1) as f32 * GRID_GAP);
        assert!(((left - indent) - right).abs() <= 1.0, "{w}: left {left}, right {right}");
    }
}

#[test]
fn grid_columns_never_panic_or_return_no_columns() {
    for w in [0.0, -50.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
        for cell in [0.0, -1.0, f32::NAN, f32::INFINITY, 44.0, 1e-30] {
            let (left, cols) = grid_columns(w, cell, 4.0, GRID_GAP);
            assert!((1..=256).contains(&cols), "{w} {cell}: {cols}");
            assert!(left.is_finite() && left >= 0.0, "{w} {cell}: {left}");
        }
    }
    // Narrower than one cell: one column at the indent.
    assert_eq!(grid_columns(30.0, 44.0, 4.0, GRID_GAP), (4.0, 1));
}

/// The Brushes panel pages one group at a time. Checks each page's tiles against its fixed header
/// (which spans the list's width): the same left margin on every row, and full rows splitting the
/// spare width evenly. Walks `pages` groups with the header's next arrow, so the paging is
/// exercised with every page too.
fn rows_are_centred(h: &mut Harness<'_, PhotocraftApp>, indent: f32, pages: usize) {
    let presets = h.state().session.tools.presets.clone();
    let labels: Vec<String> = grouped_presets(&presets).into_iter().map(|(g, _)| g).collect();
    let next = crate::i18n::t("Next brush group");
    let mut seen: Vec<(String, Rect, Vec<Vec<Rect>>)> = Vec::new();
    for _ in 0..pages {
        h.run_steps(2);
        let found = labels.iter().find_map(|l| h.query_by_label(l).map(|n| (l.clone(), n.rect())));
        assert!(found.is_some(), "a page header");
        let (label, header) = found.unwrap();
        let tiles: Vec<Rect> = h
            .query_all_by_role(Role::Button)
            .filter(|n| n.accesskit_node().label().is_some_and(|l| presets.iter().any(|p| p.name == l)))
            .map(|n| n.rect())
            .filter(|r| r.top() > header.bottom())
            .collect();
        let mut rows: Vec<Vec<Rect>> = Vec::new();
        for r in tiles {
            match rows.iter_mut().find(|row| (row[0].top() - r.top()).abs() < 0.5) {
                Some(row) => row.push(r),
                None => rows.push(vec![r]),
            }
        }
        assert!(!rows.is_empty(), "page {label}: no tiles");
        seen.push((label, header, rows));
        h.get_by_label(&next).click();
        h.run_steps(1);
    }
    assert!(seen.iter().map(|(l, ..)| l.as_str()).collect::<std::collections::HashSet<_>>().len() > 1, "the pages didn't change");
    // A full row is as wide as the widest on any page (a short group fills only part of one).
    let cols = seen.iter().flat_map(|(_, _, rows)| rows.iter().map(Vec::len)).max().unwrap_or(0);
    let (list_left, list_right) = (seen[0].1.left(), seen[0].1.right());
    let left = |row: &Vec<Rect>| row.iter().map(|r| r.left()).fold(f32::MAX, f32::min) - list_left;
    let first = left(&seen[0].2[0]);
    for (label, _, rows) in &seen {
        for row in rows {
            assert!((left(row) - first).abs() < 0.5, "{label}: a row starts {} pt in, the first {first}", left(row));
            if row.len() == cols {
                let right = list_right - row.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
                assert!(((first - indent) - right).abs() <= 1.0, "{label}: {} pt left of the grid, {right} pt right", first - indent);
            }
        }
    }
    assert!(seen.iter().any(|(_, _, rows)| rows.len() > 1), "no page wrapped onto a second row, so the rows weren't compared");
}

/// The picker's cards: one full-width 2 × 2 card per preset (tip and stroke on top, the name
/// across the bottom), and a hidden part shrinks the card — with only the tip and the name on,
/// the name takes the stroke's cell and the card takes its own height ([`CARD_TIP_NAME_H`]) and
/// width ([`CARD_TIP_NAME_W`]) snapped to the list's left edge; with only the tip on, the card
/// takes the tip-only cell's own height, with the grid's whole columns centred.
#[test]
fn brush_picker_cards_follow_the_part_boxes() {
    let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    });
    h.state_mut().run("file.new", serde_json::json!({"width": 400, "height": 300})).unwrap();
    h.state_mut().ui.tool = crate::state::Tool::Brush;
    h.state_mut().ui.brush_picker = Some([300.0, 120.0]);
    // Every part on (Brush Tip starts off).
    h.state_mut().ui.brush_picker_list.show_tip = true;
    h.run_steps(8);
    let presets = h.state().session.tools.presets.clone();
    let cards = |h: &Harness<'_, PhotocraftApp>| -> Vec<Rect> {
        h.query_all_by_role(Role::Button)
            .filter(|n| n.accesskit_node().label().is_some_and(|l| presets.iter().any(|p| p.name == l)))
            .map(|n| n.rect())
            .collect()
    };
    let full = CARD_MAX_H;
    let one_row = CARD_MAX_H - CARD_NAME_H;
    let all_on = cards(&h);
    assert!(all_on.len() >= 2, "{} cards", all_on.len());
    assert!(all_on.iter().all(|r| (r.height() - full).abs() < 0.5 && r.width() > 250.0), "{all_on:?}");
    h.state_mut().ui.brush_picker_list.show_stroke = false;
    h.run_steps(2);
    let tip_name = cards(&h);
    assert!(
        tip_name.iter().all(|r| (r.height() - CARD_TIP_NAME_H).abs() < 0.5 && (r.width() - CARD_TIP_NAME_W).abs() < 0.5),
        "tip + name: the name in the stroke's cell, its own cell size"
    );
    assert!(tip_name.iter().all(|r| (r.left() - all_on[0].left()).abs() < 0.5), "snapped to the left edge: {tip_name:?}");
    h.state_mut().ui.brush_picker_list.show_name = false;
    h.run_steps(2);
    let tip_only = cards(&h);
    assert!(tip_only.iter().all(|r| (r.height() - CARD_TIP_ONLY_H).abs() < 0.5), "tip only: its own cell");
    assert!(tip_only[0].left() > all_on[0].left() + 5.0, "the tip-only grid's columns stay centred: {tip_only:?}");
    h.state_mut().ui.brush_picker_list.show_stroke = true;
    h.run_steps(2);
    assert!(cards(&h).iter().all(|r| (r.height() - one_row).abs() < 0.5), "tip + stroke: one row");
}

/// The footer slider's widths take the raw scale; heights, padding and text stop at
/// [`CARD_MIN_H_SCALE`], so at the slider's 0.15 the cards stand at 42 pt ([`CARD_MAX_H`] at that
/// floor) with the widths a fraction of that.
#[test]
fn brush_picker_cards_floor_their_height_but_not_their_width() {
    let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    });
    h.state_mut().run("file.new", serde_json::json!({"width": 400, "height": 300})).unwrap();
    h.state_mut().ui.tool = crate::state::Tool::Brush;
    h.state_mut().ui.brush_picker = Some([300.0, 120.0]);
    h.state_mut().ui.brush_picker_list.scale = 0.15;
    h.run_steps(8);
    let presets = h.state().session.tools.presets.clone();
    let cards: Vec<Rect> = h
        .query_all_by_role(Role::Button)
        .filter(|n| n.accesskit_node().label().is_some_and(|l| presets.iter().any(|p| p.name == l)))
        .map(|n| n.rect())
        .collect();
    assert!(cards.len() >= 2, "{} cards", cards.len());
    for r in &cards {
        assert!((r.height() - CARD_MAX_H * CARD_MIN_H_SCALE).abs() < 0.5, "the height floors at the min scale: {r:?}");
        // A column's share can run up to 1/col past the raw max width ([`card_columns`]); the
        // floored scale's widths would be four times this (0.60 / 0.15).
        assert!(r.width() <= CARD_MAX_W * 0.15 * 1.5, "the width keeps the raw scale, not the floored one: {r:?}");
    }
}

/// The gear's menu goes the moment a slider or the resize grip is grabbed. A menu closes on a
/// click, and a drag never becomes one, so the menu stayed up — and the picker's own interaction
/// pulled the picker over it. Each press is held down (no release yet, nothing classified) while
/// the menu is checked, so only the grab itself can have closed it.
#[test]
fn brush_picker_menu_closes_when_a_slider_or_the_grip_is_grabbed() {
    fn click_gear(h: &Harness<'_, PhotocraftApp>) {
        h.query_all_by_role(Role::Button).find(|n| n.accesskit_node().label().as_deref() == Some("Brush Preset Options")).expect("the gear button").click();
    }
    let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    });
    h.state_mut().run("file.new", serde_json::json!({"width": 400, "height": 300})).unwrap();
    h.state_mut().ui.tool = crate::state::Tool::Brush;
    h.state_mut().ui.brush_picker = Some([300.0, 120.0]);
    h.run_steps(8);
    let picker = h.ctx.memory(|m| m.area_rect(egui::Id::unique("canvas-brush-picker"))).expect("the picker is shown");
    assert!(!egui::Popup::is_any_open(&h.ctx), "nothing open to start with");
    click_gear(&h);
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx), "the gear opens its menu");
    // The Size slider, under the row that holds the "Size" label: the row stands as tall as its
    // 24 pt value field with the label centred in it, so the slider's middle sits 12 pt below the
    // label's, plus the style's spacing and half the slider's 18 pt — 27 pt at the 6 pt spacing.
    let size_label = h.query_all_by_label(crate::i18n::t("Size")).find(|n| picker.contains(n.rect().center())).expect("the picker's Size label").rect();
    let slider = egui::pos2(size_label.left() + 100.0, size_label.center().y + 27.0);
    h.drag_at(slider);
    h.run_steps(2);
    assert!(!egui::Popup::is_any_open(&h.ctx), "grabbing the Size slider dismisses the menu");
    h.drop_at(slider);
    h.run_steps(2);
    click_gear(&h);
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx), "the gear opens its menu again");
    // The grip is the frame's bottom-right 16 × 16, so its middle is 8 pt off the picker's corner.
    let grip = picker.right_bottom() - vec2(8.0, 8.0);
    h.drag_at(grip);
    h.run_steps(2);
    assert!(!egui::Popup::is_any_open(&h.ctx), "grabbing the resize grip dismisses the menu");
    h.drop_at(grip);
    h.run_steps(2);
}

/// The upper controls (Size, Hardness) share a block at most [`crate::brush_picker::HEAD_MAX_W`]
/// wide, flush with the picker's left edge: on a picker dragged wider, a click on the Size slider
/// just inside the block jumps the size, and the same click just past the block lands on nothing.
/// A click on a slider sets it to the spot clicked, so a size that doesn't move is a click that
/// missed.
#[test]
fn brush_picker_upper_controls_stop_at_their_max_width() {
    fn click(h: &mut Harness<'_, PhotocraftApp>, pos: egui::Pos2) {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.run_steps(2);
        }
    }
    let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    });
    h.state_mut().run("file.new", serde_json::json!({"width": 400, "height": 300})).unwrap();
    h.state_mut().ui.tool = crate::state::Tool::Brush;
    // Dragged wide: 500 pt of content, well past the block.
    h.state_mut().ui.brush_picker = Some([300.0, 120.0]);
    h.state_mut().ui.brush_picker_size = Some([500.0, 420.0]);
    h.run_steps(8);
    let picker = h.ctx.memory(|m| m.area_rect(egui::Id::unique("canvas-brush-picker"))).expect("the picker is shown");
    let size_label = h.query_all_by_label(crate::i18n::t("Size")).find(|n| picker.contains(n.rect().center())).expect("the picker's Size label").rect();
    // The Size slider's middle: 12 pt below the label's (the row stands as tall as its 24 pt value
    // field, the label centred in it), the style's spacing, then half the slider's 18 pt — 27 pt
    // at the 6 pt spacing.
    let y = size_label.center().y + 27.0;
    let before = h.state().session.tools.brush.size;
    click(&mut h, egui::pos2(size_label.left() + crate::brush_picker::HEAD_MAX_W - 10.0, y));
    let inside = h.state().session.tools.brush.size;
    assert_ne!(inside, before, "the slider takes a click just inside the block");
    click(&mut h, egui::pos2(size_label.left() + crate::brush_picker::HEAD_MAX_W + 10.0, y));
    assert_eq!(h.state().session.tools.brush.size, inside, "just past the block the click lands on nothing");
}

#[test]
fn the_brushes_tab_grid_is_centred_past_its_indent() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.ui.panels.brush_settings = true;
    app.ui.brush_tab = 1;
    app.ui.brushes_panel.view = BrushesView::Grid;
    let mut h = Harness::builder().with_size(vec2(1200.0, 900.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            crate::brush_panel::window(app, &ctx);
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(6);
    rows_are_centred(&mut h, PANEL_LIST.indent, 2);
}

/// #4454: a card's size number sat under the tip thumbnail of any medium or large brush (the
/// tip took up to 44.8 pt of a 42 pt cell, the number anchored to the cell's bottom), so the
/// "30" of Hard Round and the "45" of Soft Round couldn't be read. At every footer scale that
/// shows numbers, in each card kind, the tip now stays above the number, and bigger brushes still
/// get bigger tips.
#[test]
fn card_tips_never_cover_their_size_number() {
    let number_h = std::rc::Rc::new(std::cell::Cell::new(0.0_f32));
    let seen = number_h.clone();
    let mut h = Harness::new_ui(move |ui| {
        let g = ui.painter().layout_no_wrap("45".into(), egui::FontId::proportional(CARD_NUM_FONT), Color32::WHITE);
        seen.set(g.size().y);
    });
    PhotocraftApp::setup_context(&h.ctx, Default::default());
    h.run();
    let number_h = number_h.get();
    assert!(number_h > 8.0, "the number has a real height: {number_h}");
    let mut scale = CARD_COMPACT_SCALE + 0.01;
    while scale <= *crate::brush_picker::SCALE_RANGE.end() + 1e-3 {
        let scale_h = scale.max(CARD_MIN_H_SCALE);
        // The tip cell of a full card (name row under it), a tip-and-name card and a tip-only one,
        // as [`preset_card`] sizes them in a wide column.
        let cells = [
            vec2(CARD_TIP_W * scale, (CARD_MAX_H - CARD_PAD * 2.0 - CARD_NAME_H) * scale_h),
            vec2(CARD_TIP_W * scale, (CARD_TIP_NAME_H - CARD_PAD * 2.0) * scale_h),
            vec2((CARD_TIP_ONLY_W - 16.0) * scale, (CARD_TIP_ONLY_H - CARD_PAD * 2.0) * scale_h),
        ];
        for size in cells {
            let cell = Rect::from_min_size(egui::pos2(10.0, 20.0), size);
            let number_top = cell.bottom() - 1.0 - number_h;
            let mut last = 0.0;
            for brush in [1.0, 5.0, 13.0, 30.0, 45.0, 100.0, 2500.0] {
                let tip = card_tip_rect(cell, Some(number_h), brush, scale);
                assert!(tip.bottom() <= number_top + 1e-3, "scale {scale}, cell {size:?}, {brush} px: tip {tip:?} reaches the number at {number_top}");
                assert!(cell.expand(1e-3).contains_rect(tip), "scale {scale}, {brush} px: tip {tip:?} leaves its cell {cell:?}");
                assert!(tip.width() >= last, "scale {scale}, {brush} px: a bigger brush gets a smaller tip");
                last = tip.width();
            }
        }
        scale += 0.05;
    }
    // The tip-only grid's cells are tall enough for the number: a big brush's tip still spans
    // the cell's width.
    let cell = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(CARD_TIP_ONLY_W - 16.0, CARD_TIP_ONLY_H - CARD_PAD * 2.0));
    assert!((card_tip_rect(cell, Some(number_h), 45.0, 1.0).width() - cell.width()).abs() < 1e-3);
    // Compact cards: no number, the tip fills the cell; nonsense never makes a non-finite rect.
    assert_eq!(card_tip_rect(cell, None, 45.0, 0.5).width(), cell.width() - 2.0);
    for (n, size, scale) in [(f32::NAN, 30.0, 1.0), (500.0, f32::NAN, 1.0), (12.0, 30.0, f32::INFINITY), (12.0, -5.0, 0.0)] {
        let r = card_tip_rect(cell, Some(n), size, scale);
        assert!(r.min.is_finite() && r.max.is_finite(), "{n} {size} {scale}: {r:?}");
    }
}

/// #7256, then the slider's removal: the Brushes tab no longer shows a preview-size slider, and a
/// scale remembered from an older build is dropped instead of shrinking the rows.
#[test]
fn the_brushes_tab_draws_one_preview_size() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
    app.ui.brushes_panel.scale = 0.5;
    app.ui.brushes_panel.view = BrushesView::List;
    let mut ready = false;
    let mut h = Harness::builder().with_size(vec2(460.0, 760.0)).build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ALL[0]);
                ready = true;
                return;
            }
            ui.set_width(440.0);
            show(app, ui);
        },
        app,
    );
    h.run_steps(4);
    assert_eq!(h.state().ui.brushes_panel.scale, 1.0, "the tab draws its rows at the standard size");
}

/// The preset list's scrollbar is drawn from the start, with the pointer nowhere near the panel:
/// the theme's floating bar used to appear only while the list scrolled.
#[test]
fn the_preset_list_keeps_its_scrollbar_on_screen() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
    app.ui.brushes_panel.view = BrushesView::List;
    let mut ready = false;
    let mut h = Harness::builder()
        .with_size(vec2(340.0, 320.0))
        .with_accessibility_check(false)
        .build_ui_state(
            move |ui, app: &mut PhotocraftApp| {
                if !ready {
                    PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ALL[0]);
                    ready = true;
                    return;
                }
                ui.set_width(332.0);
                show(app, ui);
            },
            app,
        );
    h.run_steps(4);
    fn bar(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(r) if r.rect.width() <= 8.0 && r.rect.height() >= 24.0 => out.push(r.rect),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| bar(s, out)),
            _ => {}
        }
    }
    let mut bars = Vec::new();
    for shape in &h.output().shapes {
        bar(&shape.shape, &mut bars);
    }
    assert!(bars.iter().any(|r| r.right() >= 320.0), "the list draws its bar at its right edge: {bars:?}");
}

/// The Size at the top of the tab is a whole number: a 197.5 px brush reads and stores as 198,
/// so no field in the panel shows a decimal point.
#[test]
fn the_size_field_shows_whole_numbers() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
    app.ui.panels.brush_settings = true;
    app.ui.brush_tab = 1;
    app.session.tools.brush.size = 197.5;
    let mut h = Harness::builder()
        .with_size(vec2(1200.0, 900.0))
        .with_accessibility_check(false)
        .build_eframe(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, crate::theme::ThemeKind::ALL[0]);
            app
        });
    h.run_steps(4);
    assert_eq!(h.state().session.tools.brush.size, 198.0, "the field rounds a half pixel away");
    assert!(
        h.query_all_by_role(Role::SpinButton).any(|n| n.accesskit_node().numeric_value() == Some(198.0)),
        "the field shows the whole number"
    );
}

#[test]
fn preview_scale_is_clamped_to_its_range() {
    assert_eq!(list_row_height(1.0), 38.0);
    assert_eq!(list_row_height(100.0), 76.0);
    assert_eq!(list_row_height(f32::NAN), 38.0);
    assert_eq!(grid_cell_width(52.0, 0.0), 26.0);
    assert_eq!(grid_cell_width(52.0, f32::INFINITY), 52.0);
}

/// The stroke preview takes 60 % of the row, and the preset's name sits at its right in what is
/// left: the curve reads as the row's own stroke instead of a small squiggle.
#[test]
fn list_rows_give_the_stroke_sixty_percent_of_the_row() {
    assert_eq!(stroke_span(330.0), (6.0, 192.0), "60 % of 330, less the left pad");
    assert_eq!(stroke_span(140.0), (6.0, 78.0));
    assert_eq!(stroke_span(60.0), (6.0, 60.0), "a narrow row keeps a 60 point minimum");
    assert_eq!(stroke_span(1000.0), (6.0, STROKE_MAX_W), "a very wide panel stops growing the stroke");
    assert_eq!(stroke_span(f32::NAN), (6.0, 60.0));
    assert_eq!(stroke_span(f32::INFINITY), (6.0, 60.0));
    // The name's room is the other 40 % of the row.
    assert!(stroke_span(330.0).1 + 6.0 < 330.0 * 0.61);
}

/// A remembered group that no longer exists opens on the selected brush's group, so the brush
/// `last_brush` restored is on the page that opens; with neither, the first group.
#[test]
fn a_missing_remembered_group_falls_back_to_the_brushes_group() {
    let presets = photocraft_engine::Session::new().tools.presets;
    let groups = visible_groups(&presets, "");
    let name = presets.iter().find(|p| !p.group.is_empty()).map(|p| p.name.as_str()).expect("a grouped preset");
    let want = groups.iter().position(|(_, items)| items.iter().any(|p| p.name == name)).expect("its group is listed");
    assert_eq!(shown_group(&groups, "No Such Group", Some(name)), Some(want));
    assert_eq!(shown_group(&groups, "", None), Some(0));
    assert_eq!(shown_group(&groups, "No Such Group", None), Some(0));
    let empty: Vec<(String, Vec<&BrushPreset>)> = Vec::new();
    assert_eq!(shown_group(&empty, "", None), None);
}

/// The Brushes panel shows one group at a time under a fixed header; its chevrons step to the
/// next and previous group (wrapping at either end), instead of every group stacking in one scroll
/// with the others only reachable by scrolling down.
#[test]
fn brushes_panel_pages_its_groups_under_a_fixed_header() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.edit_prefs(|p| p.general.auto_show_home_screen = false);
    app.ui.panels.brush_settings = true;
    app.ui.brush_tab = 1;
    app.ui.brushes_panel.view = BrushesView::List;
    let mut h = Harness::builder().with_size(vec2(1200.0, 900.0)).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, crate::theme::ThemeKind::ALL[0]);
        app
    });
    h.run_steps(6);
    let presets = h.state().session.tools.presets.clone();
    let groups: Vec<(String, Vec<String>)> = grouped_presets(&presets)
        .into_iter()
        .map(|(g, ids)| (g, ids.into_iter().filter_map(|i| presets.get(i).map(|p| p.name.clone())).collect()))
        .collect();
    assert!(groups.len() > 2, "{groups:?}");
    let header = |h: &Harness<'_, PhotocraftApp>, label: &str| h.query_by_label(label).expect("the page's header").rect();
    let (first, second) = (&groups[0], &groups[1]);
    let last = groups.last().expect("a last group");
    let top = header(&h, &first.0).top();
    assert!(h.query_by_label(&first.1[0]).is_some(), "the first group's rows are shown");
    assert!(h.query_by_label(&second.1[0]).is_none(), "the second group waits behind the arrow");
    // Next: the header stays where it was and the rows under it change.
    h.get_by_label(crate::i18n::t("Next brush group")).click();
    h.run_steps(3);
    assert_eq!(h.state().ui.brushes_panel.group, second.0);
    assert_eq!(header(&h, &second.0).top(), top, "the group header is fixed");
    assert!(h.query_by_label(&second.1[0]).is_some() && h.query_by_label(&first.1[0]).is_none());
    // Previous: back to the first group.
    h.get_by_label(crate::i18n::t("Previous brush group")).click();
    h.run_steps(3);
    assert_eq!(h.state().ui.brushes_panel.group, first.0);
    assert!(h.query_by_label(&first.1[0]).is_some());
    // Previous again wraps round to the last group.
    h.get_by_label(crate::i18n::t("Previous brush group")).click();
    h.run_steps(3);
    assert_eq!(h.state().ui.brushes_panel.group, last.0);
    assert!(h.query_by_label(&last.1[0]).is_some());
}

/// The group the panel pages is remembered in the preferences (`brush_panel`) and restored at
/// launch; the name of the selected brush itself is `last_brush`'s preset, and a saved group that
/// is gone opens on that preset's group ([`shown_group`]).
#[test]
fn the_brushes_panel_remembers_its_group() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.ui.brushes_panel.group = "Dry Media".into();
    persist(&mut app, &egui::Context::default());
    assert_eq!(app.session.prefs().brush_panel, json!({"group": "Dry Media"}));
    let mut launched = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    launched.session.edit_prefs(|p| p.brush_panel = json!({"group": "Ink"}));
    restore(&mut launched);
    assert_eq!(launched.ui.brushes_panel.group, "Ink");
    // A value that isn't a group name keeps the default, which opens on the selected brush's group.
    launched.session.edit_prefs(|p| p.brush_panel = json!({"group": 7}));
    launched.ui.brushes_panel.group.clear();
    restore(&mut launched);
    assert!(launched.ui.brushes_panel.group.is_empty());
}
