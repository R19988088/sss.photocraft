use super::{PhotocraftApp, RichText, Tokens, label, vec2, widgets};

// ----------------------------------------------------------------------------- status bar

/// The status bar's height and the height of the controls in it (zoom field, Fit), which leaves
/// them an even margin above and below inside the bar (#2823).
pub fn status_bar_heights(t: &Tokens) -> (f32, f32) {
    if t.pro { (24.0, 18.0) } else { (30.0, 22.0) }
}

/// The status bar floats over the bottom of the canvas in two translucent groups, as Photoshop's
/// document window has it: the document's information at the reading start, the view controls at
/// its end. It reserves no layout space, sits just above the canvas's scrollbars, and keeps a
/// see-through chrome so the document shows behind it.
pub fn status_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    // The canvas panel's rect: the bar floats over it and takes nothing from it.
    let rect = ui.max_rect();
    let (height, control_h) = status_bar_heights(&t);
    let margin = ((height - control_h) / 2.0) as i8;
    // Just above the canvas's scrollbars, with the air the document window leaves them.
    let bottom = rect.bottom() - 6.0 - if app.ui.view.shows_scrollbars() { crate::scrollbars::THICKNESS } else { 0.0 };
    let rtl = crate::direction::is_rtl();
    let frame = egui::Frame::NONE.fill(t.chrome.gamma_multiply(0.78)).corner_radius(t.radius_sm).inner_margin(egui::Margin::symmetric(6, margin));
    let width = (rect.width() - 20.0).max(1.0);
    let has_actions = app.session.active().is_some()
        || app.session.has_jobs()
        || app.session.active().is_some_and(|st| app.redraws.contains_key(&st.doc.id))
        || crate::chrome_ui::render_path_label(app).is_some();
    // Separate areas leave the space between the readout and controls available to the canvas.
    let actions_width = if has_actions {
        egui::Area::new(egui::Id::new("status_bar_actions"))
            .layout(crate::direction::trailing())
            .order(egui::Order::Foreground)
            .pivot(if rtl { egui::Align2::LEFT_BOTTOM } else { egui::Align2::RIGHT_BOTTOM })
            .fixed_pos(egui::pos2(if rtl { rect.left() + 10.0 } else { rect.right() - 10.0 }, bottom))
            .default_size(vec2(0.0, height))
            .movable(false)
            .enabled(ui.is_enabled())
            .show(&ctx, |ui| {
                ui.set_max_width(width);
                frame.show(ui, |ui| {
                    ui.with_layout(crate::direction::trailing(), |ui| {
                        ui.set_min_height(control_h);
                        if !t.pro
                            && app.session.active().is_some()
                            && widgets::compact_button(ui, tl!("Fit"), control_h).clicked()
                            && let Some(i) = app.session.active_index()
                        {
                            app.ui.views[i].fit_pending = true;
                        }
                        // Background job progress with Cancel (#210).
                        if app.session.has_jobs() {
                            ui.add_space(6.0);
                            crate::jobs_ui::status_progress(app, ui);
                        }
                        crate::background_redraw::status(app, ui);
                        crate::chrome_ui::render_path_status(app, ui);
                    });
                });
            })
            .response
            .rect
            .width()
    } else {
        0.0
    };
    egui::Area::new(egui::Id::new("status_bar_info"))
        .layout(crate::direction::row())
        .order(egui::Order::Foreground)
        .pivot(if rtl { egui::Align2::RIGHT_BOTTOM } else { egui::Align2::LEFT_BOTTOM })
        .fixed_pos(egui::pos2(if rtl { rect.right() - 10.0 } else { rect.left() + 10.0 }, bottom))
        .default_size(vec2(0.0, height))
        .movable(false)
        .enabled(ui.is_enabled())
        .show(&ctx, |ui| {
            ui.set_max_width((width - actions_width - 8.0).max(1.0));
            frame.show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("status-info-scroll")
                    .max_width((width - actions_width - 20.0).max(1.0))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.with_layout(crate::direction::row(), |ui| {
                            ui.set_min_height(control_h);
                            if t.pro {
                                crate::chrome_ui::status_bar_pro(app, ui);
                                return;
                            }
                            if let (Some(st), Some(i)) = (app.session.active(), app.session.active_index()) {
                                let (w, h, mode, bits, layers) = (
                                    st.doc.size.width,
                                    st.doc.size.height,
                                    crate::canvas::mode_label(&st.doc),
                                    st.doc.depth.bits(),
                                    st.doc.layer_count(),
                                );
                                let mut pct = app.ui.views[i].zoom * 100.0;
                                let field = widgets::compact_wrapping_value_field(
                                    ui,
                                    &mut pct,
                                    crate::zoom_levels::percent_range(&app.ui.views[i]),
                                    "%",
                                    vec2(78.0, control_h),
                                )
                                .accessible_name(tl!("Zoom"));
                                if field.changed() {
                                    let animated = app.session.prefs().general.animated_zoom;
                                    crate::zoom_levels::set_from_field(
                                        &mut app.ui.views[i],
                                        field.dragged() || field.drag_started(),
                                        pct,
                                        animated,
                                    );
                                }
                                widgets::vline(ui, 16.0);
                                label(ui, &crate::i18n::fmt(tl!("{mode} Color · {bits} bit"), &[("mode", mode), ("bits", &bits.to_string())]));
                                widgets::vline(ui, 16.0);
                                label(ui, &format!("{w} × {h} px"));
                                widgets::vline(ui, 16.0);
                                label(ui, &crate::i18n::trn(crate::i18n::current(), layers as u64, "{n} layer", "{n} layers"));
                            } else {
                                label(ui, tl!("No document"));
                            }
                            if !app.ui.status.is_empty() {
                                widgets::vline(ui, 16.0);
                                let is_err = app.ui.status_error || app.ui.status.starts_with("Couldn");
                                ui.label(RichText::new(&app.ui.status).color(if is_err { t.warning } else { t.text_faint }));
                            }
                        });
                    });
            });
        });
}
