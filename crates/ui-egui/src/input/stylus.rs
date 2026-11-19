//! Stylus input: real pen pressure (and tilt / barrel rotation where the platform reports them)
//! for the painting tools. A mouse always paints at pressure 1 with no tilt.
//!
//! Sources, by platform (eframe 0.36.2, egui-winit 0.36.2, winit 0.30.13):
//!
//! - **Windows**: winit turns `WM_POINTER` pen and touch input into `WindowEvent::Touch` with a
//!   normalised force (`POINTER_PEN_INFO::pressure / 1024`); egui-winit forwards it as
//!   `egui::Event::Touch { force }` alongside the emulated pointer. [`Stylus::update`] reads it.
//!   winit drops the pen's tilt and rotation, so those stay 0, and reports pressure 0 (a tap
//!   lighter than the pen's first pressure level) as no force, which [`Stylus::update`] reads as
//!   pressure 0 once it knows a pen is in use. The desktop app's pointer message hook
//!   (`photocraft-tablet`) reports which end of the pen is in use through `Services::pen_end`.
//! - **Web**: eframe forwards touch force but not pen pointer events, so the web runner listens
//!   to `pointerdown`/`pointermove` itself and writes `pressure`, `tiltX`, `tiltY`, `twist` and
//!   the eraser button of `pointerType == "pen"` events into the [`StylusFeed`].
//! - **macOS**: winit 0.30 drops `NSEvent` tablet data, so the desktop app installs an AppKit
//!   local event monitor (the `photocraft-tablet` crate) that writes pressure, tilt, rotation and
//!   the eraser end into the [`StylusFeed`] before winit handles each event.
//! - **Linux X11**: the desktop app reads XInput2 raw valuator events on its own X connection
//!   (`photocraft-tablet`, x11rb) and writes them into the [`StylusFeed`].
//! - **Linux Wayland**: compositors give a pen only to clients that bind `zwp_tablet_v2`, which
//!   winit doesn't (it would have to share winit's connection), so with a pen attached the
//!   desktop app opens its window through Xwayland, which reports tablet valuators to the X11
//!   reader. `PHOTOCRAFT_NATIVE_WAYLAND=1` keeps native Wayland, where the pen has no input.
//!
//! Preferences › Tools › Use Tablet Pressure off makes a pen paint like a mouse. Flipping the pen
//! to its eraser end selects the Eraser tool and flipping back restores the previous tool, as in
//! Photoshop.
//!
//! Automation simulates a pen with `ui.pointer` events carrying `pressure`, `tiltX`, `tiltY`
//! and `rotation`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

/// One stylus reading. Tilt is in degrees (-90..90, W3C Pointer Events convention), rotation in
/// degrees 0..360.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    /// The pen's eraser end is in use.
    pub eraser: bool,
    /// The pen's own position in virtual-screen pixels (`x` right, `y` down) when the backend
    /// reports one (WinTab's packets), so a stroke follows the pen's rate instead of the pointer's.
    pub pos: Option<[f32; 2]>,
}

impl Default for PenSample {
    fn default() -> Self {
        Self { pressure: 1.0, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0, eraser: false, pos: None }
    }
}

impl PenSample {
    /// Clamp to the documented ranges (and replace non-finite values).
    pub fn sanitized(self) -> Self {
        let f = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Self {
            pressure: f(self.pressure, 0.0, 1.0, 1.0),
            tilt_x: f(self.tilt_x, -90.0, 90.0, 0.0),
            tilt_y: f(self.tilt_y, -90.0, 90.0, 0.0),
            // `rem_euclid` rounds tiny negative angles up to exactly 360.
            rotation: if self.rotation.is_finite() { self.rotation.rem_euclid(360.0) % 360.0 } else { 0.0 },
            eraser: self.eraser,
            pos: self.pos.filter(|p| p[0].is_finite() && p[1].is_finite()),
        }
    }
}

/// Most samples a [`StylusFeed`] keeps between two frames (a stalled UI drops the oldest).
const FEED_QUEUE: usize = 1024;

/// The clock every backend's samples are stamped with, in milliseconds: one monotonic epoch for
/// the whole process, so samples written by a tablet thread and read by the UI share their spacing
/// (the UI aligns it to its own frame clock when it drains them).
fn pen_clock_ms() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_secs_f64() * 1000.0
}

#[derive(Debug, Default)]
struct FeedState {
    /// The current sample (`None` = no pen down).
    latest: Option<PenSample>,
    /// Every sample set since the canvas last drained them, oldest first, with the time it came.
    queue: VecDeque<(PenSample, f64)>,
    /// A native backend (WinTab) supplies this pen's samples, so winit's own copy of the same
    /// stroke (the touch force Windows Ink reports) would double-report its pressure.
    native: bool,
}

/// Shared state a platform backend writes pen samples into. Cloning shares it. Each sample both
/// becomes the current one and is queued with its arrival time, so a frame sees every sample the
/// pen reported, in order, and a pointer move takes the pressure the pen had at its own time
/// rather than an index's share of the frame.
#[derive(Clone, Debug, Default)]
pub struct StylusFeed(Arc<Mutex<FeedState>>);

impl StylusFeed {
    /// The current sample (`None` = no pen down); a sample is also queued for [`Self::drain`].
    pub fn set(&self, s: Option<PenSample>) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let s = s.map(PenSample::sanitized);
        g.latest = s;
        if let Some(s) = s {
            if g.queue.len() >= FEED_QUEUE {
                g.queue.pop_front();
            }
            g.queue.push_back((s, pen_clock_ms()));
        }
    }

    pub fn get(&self) -> Option<PenSample> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).latest
    }

    /// Queue a sample stamped at `at` (milliseconds on [`pen_clock_ms`]'s clock): tests give the
    /// pen its own rate without waiting for it.
    #[cfg(test)]
    pub(crate) fn set_at(&self, s: PenSample, at: f64) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let s = s.sanitized();
        g.latest = Some(s);
        if g.queue.len() >= FEED_QUEUE {
            g.queue.pop_front();
        }
        g.queue.push_back((s, at));
    }

    /// Says a native backend (WinTab) supplies this pen's samples. WinTab and Windows Ink both
    /// report the same stroke on Windows: the pressure, tilt and rotation come from the driver's
    /// WinTab packets while they arrive (the finer data), and Windows Ink's touch force is only
    /// the fallback for a driver that gives no WinTab context.
    pub fn set_native(&self, native: bool) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).native = native;
    }

    /// Whether a native backend supplies the pen ([`Self::set_native`]).
    pub fn is_native(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).native
    }

    /// The samples set since the last drain, oldest first, each with the time it came (see
    /// [`pen_clock_ms`]).
    pub fn drain(&self) -> Vec<(PenSample, f64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).queue.drain(..).collect()
    }
}

/// One pen sample and when it came, in milliseconds on [`Stylus::clock_ms`]'s clock.
#[derive(Clone, Copy, Debug)]
struct TimedSample {
    sample: PenSample,
    at: f64,
}

/// `a` to `b` by `t`, rotation taking the shorter way round.
fn lerp_sample(a: PenSample, b: PenSample, t: f32) -> PenSample {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let round = (b.rotation - a.rotation + 540.0).rem_euclid(360.0) - 180.0;
    let pos = match (a.pos, b.pos) {
        (Some(a), Some(b)) => Some([lerp(a[0], b[0]), lerp(a[1], b[1])]),
        _ => None,
    };
    PenSample {
        pressure: lerp(a.pressure, b.pressure),
        tilt_x: lerp(a.tilt_x, b.tilt_x),
        tilt_y: lerp(a.tilt_y, b.tilt_y),
        rotation: a.rotation + round * t,
        eraser: b.eraser,
        pos,
    }
    .sanitized()
}

/// The sample of a track (oldest first) at time `t`, interpolated between the two it falls
/// between and held at the ends. Samples sharing an instant (a burst written between two frames,
/// automation) use the last of them.
fn sample_at(track: &[TimedSample], t: f64) -> Option<PenSample> {
    let first = *track.first()?;
    if !t.is_finite() || t <= first.at {
        return Some(first.sample);
    }
    let mut a = first;
    for b in track.iter().skip(1) {
        if b.at <= a.at {
            a = *b;
            continue;
        }
        if t <= b.at {
            let f = ((t - a.at) / (b.at - a.at)) as f32;
            return Some(lerp_sample(a.sample, b.sample, f));
        }
        a = *b;
    }
    Some(a.sample)
}

/// Per-app stylus state.
#[derive(Clone, Debug)]
pub struct Stylus {
    /// Pen samples pushed by the platform (desktop tablet monitor, web runner) or by automation.
    pub feed: StylusFeed,
    /// Preferences › Tools › Use Tablet Pressure: off, a pen paints like a mouse.
    pub use_pressure: bool,
    /// Preferences › Tablet › Tablet optimization (off by default). On: the pen's samples are
    /// timed, each move takes the pressure the pen had at its own moment, every sample of a frame
    /// goes through the tablet response, and a stroke's lift carries the pressure the pen left
    /// with. Off: the earlier reception — a frame's moves split the frame's samples by index, the
    /// response runs once per frame on the frame's last sample, and the lift falls back.
    pub optimized: bool,
    /// Preferences › Tools › Pressure Curve (`None`: linear), with the points it was built from.
    pressure_curve: Option<photocraft_engine::prefs::PressureCurve>,
    curve_points: Vec<[f32; 2]>,
    /// The pen end last seen (`Some(true)` = eraser), for the eraser tool switch.
    end: Option<bool>,
    /// The tool to restore when the pen tip comes back after the eraser end switched tools.
    pub(crate) tool_before_eraser: Option<crate::state::Tool>,
    /// Force of the touch/pen contact currently down (egui `Event::Touch`).
    touch: Option<f32>,
    /// The touch contacts currently down, so a touch move outside them is a hovering pen.
    contacts: Vec<egui::TouchId>,
    /// A pen reported through touch events (it hovered, or a contact carried a force): its
    /// contacts without a force are at pressure 0, not a finger's full pressure.
    touch_pen: bool,
    /// The contact ended this frame: keep its force for this frame's last tool events, clear next frame.
    lifted: bool,
    /// This frame's pen samples, oldest first, each with the time it came (on [`Self::clock_ms`]'s
    /// clock).
    frame: Vec<TimedSample>,
    /// The last pen sample of an earlier frame: where this frame's moves interpolate from when the
    /// pen reported nothing new (the frame it lifts in, a frame the driver's queue starved).
    prev: Option<TimedSample>,
    /// The sample [`Self::select`] interpolated for the pointer move being processed.
    current: Option<PenSample>,
    /// The pen's last real reading: kept through the lift so the release carries the pressure the
    /// pen left with, never the mouse's 1 and never the lift's 0 (ProCraft's `pen::pressure` on a
    /// contact that has no force of its own).
    held: Option<PenSample>,
    /// A stroke is in progress: a pen that stops reporting keeps its readings for the stroke's
    /// release rather than clearing them at the frame's start.
    stroke_active: bool,
    /// The highest pen pressure (before the curve) since the drag began, for a tap's one dab.
    peak: f32,
    /// The pen's touch contacts in the raw input, so a held pen is no press-and-hold right click.
    pub(crate) pen_contacts: PenContacts,
    /// The egui frame [`Self::update_for_frame`] last ran in.
    updated_frame: Option<u64>,
    /// A native input backend (WinTab) currently owns pressure: winit's touch force for the
    /// same stroke would double-report it, so it is ignored while set.
    pub external_pen: bool,
    /// The custom tablet response (dead zone, full-pressure calibration, elasticity, pressure
    /// averaging) and its per-stroke state.
    tablet_response: Option<photocraft_paint::TabletPressureSettings>,
    pressure_filter: photocraft_paint::TabletPressureFilter,
    /// The response's pressure at each of this frame's samples, in the same order: every sample
    /// goes through it (ProCraft smooths each input sample), and a move takes the one at its time.
    filtered: Vec<TimedSample>,
    /// The filter has begun this stroke (its window is filling) or has been flushed.
    filter_open: bool,
    /// Without the optimization: the raw pressure the response last emitted a value for, so the
    /// same reading arriving in a later frame isn't fed to it twice.
    last_filtered_raw: Option<f32>,
    /// The feed's clock read at the last drain, subtracted from [`Self::clock_ms`] so the two
    /// clocks line up (the feed's newest sample is the frame's own).
    feed_offset: Option<f64>,
    /// Tilt X, tilt Y, rotation of each point of the current drag (parallel to its points).
    pub(crate) stroke: Vec<[f32; 3]>,
    /// Time in milliseconds of each point of the current drag (parallel to its points), from
    /// [`Stylus::clock_ms`]. Speed spacing, airbrush build-up and smoothing catch-up use it.
    pub(crate) times: Vec<f64>,
    /// Time of the tool event being processed, in milliseconds. The canvas spreads a frame's
    /// samples between the previous and the current frame time (egui's events carry none).
    pub(crate) clock_ms: f64,
}

impl Default for Stylus {
    fn default() -> Self {
        Self {
            feed: StylusFeed::default(),
            use_pressure: true,
            optimized: false,
            pressure_curve: None,
            curve_points: Vec::new(),
            end: None,
            tool_before_eraser: None,
            touch: None,
            contacts: Vec::new(),
            touch_pen: false,
            lifted: false,
            frame: Vec::new(),
            prev: None,
            current: None,
            held: None,
            stroke_active: false,
            peak: 0.0,
            pen_contacts: PenContacts::default(),
            updated_frame: None,
            external_pen: false,
            tablet_response: None,
            pressure_filter: photocraft_paint::TabletPressureFilter::default(),
            filtered: Vec::new(),
            filter_open: false,
            last_filtered_raw: None,
            feed_offset: None,
            stroke: Vec::new(),
            times: Vec::new(),
            clock_ms: 0.0,
        }
    }
}

impl Stylus {
    /// [`Self::update`] once per egui frame: the canvas runs once per view, and egui can run a
    /// frame in several passes; a second update would drain an empty queue and lose the frame's
    /// pen samples.
    pub fn update_for_frame(&mut self, frame: u64, events: &[egui::Event]) {
        if self.updated_frame.replace(frame) != Some(frame) {
            self.update(events);
        }
    }

    /// Track touch/pen contacts from this frame's input events.
    pub fn update(&mut self, events: &[egui::Event]) {
        if std::mem::take(&mut self.lifted) {
            self.touch = None;
        }
        if let Some(last) = self.frame.last() {
            self.prev = Some(*last);
        }
        let now = self.clock_ms;
        let drained = self.feed.drain();
        if self.optimized {
            // The feed's clock and the canvas's are both monotonic milliseconds: the newest
            // sample is this frame's, so align once and keep the alignment.
            if let Some((_, at)) = drained.last() {
                self.feed_offset = Some(now - *at);
            }
            let offset = self.feed_offset.unwrap_or(0.0);
            self.frame = drained.into_iter().map(|(sample, at)| TimedSample { sample, at: at + offset }).collect();
        } else {
            // Without the optimization the samples are still the frame's own; each is tagged with
            // the frame's time, which nothing in this path reads.
            self.frame = drained.into_iter().map(|(sample, _)| TimedSample { sample, at: now }).collect();
        }
        // A native backend (WinTab) supplies this pen: Windows Ink's own copy of the same stroke
        // would double-report its pressure (see `StylusFeed::set_native`).
        let native = self.external_pen || self.feed.is_native();
        self.current = None;
        for e in events {
            if let egui::Event::Touch { id, phase, force, .. } = e {
                let force = force.filter(|f| f.is_finite()).map(|f| f.clamp(0.0, 1.0));
                match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::Move => {
                        let start = *phase == egui::TouchPhase::Start;
                        if start {
                            // A new contact keeps its force into the next frame, even when an
                            // earlier contact lifted in this one.
                            self.lifted = false;
                            if !self.contacts.contains(id) {
                                if self.contacts.len() >= 16 {
                                    self.contacts.remove(0);
                                }
                                self.contacts.push(*id);
                            }
                        }
                        let down = start || self.contacts.contains(id);
                        // Only a pen moves without touching, and only a pen (or a force-sensing
                        // screen) reports a force.
                        if !down || force.is_some() {
                            self.touch_pen = true;
                        }
                        // Windows reports a pen pressed lighter than its first pressure level with
                        // no force at all (winit maps pressure 0 to `None`): a light tap is
                        // pressure 0, never a mouse click's full pressure. A finger on a screen
                        // without force sensing stays at full pressure.
                        let force = force.or((down && self.touch_pen && (start || self.touch.is_none())).then_some(0.0));
                        if let Some(f) = force {
                            self.touch = Some(f);
                            // WinTab (or another native backend) supplies this pen's pressure:
                            // winit's copy of the same stroke would double-report it.
                            if !native {
                                // A touch carries its force at the moment egui handed it over, so
                                // it lands at the frame's end; the frame's moves before it take
                                // the pressure from the samples around them.
                                self.frame.push(TimedSample { sample: PenSample { pressure: f, ..Default::default() }, at: now });
                            }
                        }
                    }
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                        self.lifted = true;
                        self.contacts.retain(|c| c != id);
                    }
                }
            }
        }
        // A pen reports pressure 0 while it hovers and as it lifts (X11, macOS, WinTab): that is
        // no contact, so a frame that also touched keeps only its contact samples, and (with the
        // optimization on) a frame of nothing but zeros is no contact at all. A stroke's last
        // moves then keep the pressure the pen left with, and a tap its own (#1798).
        if self.frame.iter().any(|s| s.sample.pressure > 0.0) {
            self.frame.retain(|s| s.sample.pressure > 0.0);
        } else if self.optimized {
            self.frame.clear();
        }
        if let Some(last) = self.frame.last() {
            self.held = Some(last.sample);
        }
        self.peak = self.frame.iter().fold(self.peak, |m, s| m.max(s.sample.pressure));
        // A pen that has left keeps its last reading for one frame — the frame it lifts in, whose
        // moves have no sample of their own — and is a mouse after that; a stroke keeps it to its
        // end, so its release carries the pressure the pen left with.
        if self.optimized {
            let pen_here = !self.frame.is_empty() || self.feed.get().is_some() || self.touch.is_some();
            if !pen_here && !self.stroke_active {
                self.prev = None;
                self.held = None;
            }
        }
        // Every sample of the frame goes through the tablet response, in order.
        self.advance_pressure_filter();
    }

    /// The pen's current sample. When the pen touched during this frame but has lifted since
    /// (the feed is cleared or back at pressure 0 by the time the UI runs, as for a tap shorter
    /// than a frame), the last sample it reported: never full pressure as for a mouse, and never
    /// the lift's 0.
    fn latest(&self) -> Option<PenSample> {
        let feed = self.feed.get();
        if !self.optimized {
            // The earlier pairing: the pen's own reading while it reports one, else this frame's
            // last contact sample (never full pressure as for a mouse, never the lift's 0).
            return match self.frame.last().map(|s| s.sample) {
                Some(s) if s.pressure > 0.0 && feed.is_none_or(|f| f.pressure <= 0.0) => Some(s),
                _ => feed,
            };
        }
        if let Some(feed) = feed
            && feed.pressure > 0.0
        {
            return Some(feed);
        }
        if let Some(last) = self.frame.last() {
            return Some(last.sample);
        }
        self.prev.map(|p| p.sample).or(feed)
    }

    /// The highest pressure the pen pressed with this frame, for Preferences › Tablet's pressure
    /// calibration: pressing the pen while dragging there records its peak.
    pub(crate) fn calibration_peak(&self) -> Option<f32> {
        self.frame.iter().map(|s| s.sample).chain(self.feed.get()).map(|s| s.pressure).reduce(f32::max)
    }

    /// This frame's pen positions (virtual-screen pixels, `x` right and `y` down) with the time
    /// each came, oldest first, for the canvas to lay a stroke along the pen's own samples instead
    /// of the few pointer moves a frame gets. Empty for a mouse and for a backend that reports no
    /// position (the pointer's moves are then the only source, as before).
    pub fn pen_track(&self) -> Vec<([f32; 2], f64)> {
        self.frame.iter().filter_map(|s| s.sample.pos.map(|p| (p, s.at))).collect()
    }

    /// The current pen sample, `None` for a mouse (and for any pen while Use Tablet Pressure is
    /// off).
    pub fn sample(&self) -> Option<PenSample> {        if !self.use_pressure {
            return None;
        }
        let s = self.current.or_else(|| self.latest()).or(self.touch.map(|pressure| PenSample { pressure, ..Default::default() }))?;
        // The pen's pressure through Preferences › Tools › Pressure Curve, before any brush sees it.
        Some(match &self.pressure_curve {
            Some(c) => PenSample { pressure: c.eval(s.pressure), ..s },
            None => s,
        })
    }

    /// Preferences › Tools › Pressure Curve (rebuilt only when the points change).
    pub fn set_pressure_curve(&mut self, points: &[[f32; 2]]) {
        if self.curve_points != points {
            self.curve_points = points.to_vec();
            let c = photocraft_engine::prefs::PressureCurve::new(points);
            self.pressure_curve = (!c.is_linear()).then_some(c);
        }
    }

    /// Choose the pen sample for the pointer move being processed: the one the pen reported at
    /// that move's own time ([`Self::clock_ms`]), interpolated between the two samples it falls
    /// between. Each move then carries the pressure the pen had as it passed there, at whatever
    /// rate the pen and the pointer deliver — not an index's share of the frame, which paired the
    /// wrong sample with a move wherever either rate changed.
    ///
    /// `k` of `n` is kept for the callers that spread a frame's moves; the time is the canvas's.
    pub(crate) fn select(&mut self, k: usize, n: usize) {
        self.current = None;
        if !self.use_pressure {
            return;
        }
        if self.optimized {
            if self.prev.is_none() && self.frame.is_empty() {
                return;
            }
            self.current = self.at_time(self.clock_ms);
            return;
        }
        // Without the optimization a frame's moves split the frame's samples by index, from the
        // previous frame's last sample.
        if n == 0 || self.frame.is_empty() || self.sample().is_none() {
            return;
        }
        let m = self.frame.len();
        let start = self.prev.or(self.frame.first().copied()).map(|s| s.sample).unwrap_or_default();
        let at = |i: usize| if i == 0 { start } else { self.frame.get(i - 1).map(|s| s.sample).unwrap_or(start) };
        let pos = (k + 1).min(n) as f32 / n as f32 * m as f32;
        let i = pos.floor() as usize;
        let (a, b, t) = (at(i), at((i + 1).min(m)), pos - pos.floor());
        self.current = Some(lerp_sample(a, b, t));
    }

    /// The pen's reading at `t` (milliseconds, [`Self::clock_ms`]'s clock): this frame's samples
    /// interpolated, starting from the previous frame's last one.
    fn at_time(&self, t: f64) -> Option<PenSample> {
        let mut track: Vec<TimedSample> = Vec::with_capacity(self.frame.len() + 1);
        if let Some(prev) = self.prev
            && self.frame.first().is_none_or(|first| prev.at <= first.at)
        {
            track.push(prev);
        }
        track.extend(self.frame.iter().copied());
        sample_at(&track, t)
    }

    /// The released stroke's pressure: the reading the pen left with. A pen's lift has no force
    /// of its own (Windows reports none, WinTab a 0), so the last real sample is the one the
    /// stroke ends at — the pressure the user drew the tail with, never the mouse's full 1 that
    /// a lost sample used to fall back to, and never the response's averaged or delayed value:
    /// its window still holds the coarser samples just before the lift, so reading it here makes
    /// the tail's pressure jump *up* as the pen thins away. The last reading goes through the
    /// same calibration and elasticity as the stroke's samples (`mapped_pressure`), on its own.
    pub(crate) fn release_pressure(&self) -> f32 {
        let hold = self
            .held
            .or_else(|| self.frame.last().map(|s| s.sample))
            .or(self.prev.map(|p| p.sample))
            .or(self.feed.get());
        let Some(hold) = hold else { return 1.0 };
        if let Some(settings) = self.tablet_response {
            return self.pressure_filter.mapped_pressure(hold.pressure, settings);
        }
        match &self.pressure_curve {
            Some(c) => c.eval(hold.pressure),
            None => hold.pressure,
        }
    }

    /// Back to the current sample after a frame's moves.
    pub(crate) fn clear_selection(&mut self) {
        self.current = None;
    }

    /// Did the pen just flip to its eraser end (`Some(true)`) or back to its tip (`Some(false)`)?
    /// Each flip is reported once; a mouse in between changes nothing. `platform_end` is the end
    /// a platform reports outside the feed (Windows), read when the feed has no pen.
    pub(crate) fn take_end_flip(&mut self, platform_end: Option<bool>) -> Option<bool> {
        let eraser = self.feed.get().map(|s| s.eraser).or(platform_end)?;
        let flipped = self.end.map_or(eraser, |e| e != eraser);
        self.end = Some(eraser);
        flipped.then_some(eraser)
    }

    /// Switch to the Eraser when the pen's eraser end comes in, and back to the previous tool when
    /// the tip does (Photoshop). Not during a drag. Returns whether the tool changed.
    pub fn sync_eraser_tool(app: &mut crate::PhotocraftApp) -> bool {
        use crate::state::Tool;
        if app.drag.is_some() {
            return false;
        }
        let platform_end = app.services.pen_end.as_ref().and_then(|f| f());
        match app.stylus.take_end_flip(platform_end) {
            Some(true) if app.ui.tool != Tool::Eraser => {
                app.stylus.tool_before_eraser = Some(app.ui.tool);
                app.ui.tool = Tool::Eraser;
                // A tool switch like any other: Add New Artboard's placing mode ends (#11386).
                crate::tool_memory::ended_by_tool(app);
                true
            }
            Some(false) => match app.stylus.tool_before_eraser.take() {
                Some(t) if app.ui.tool == Tool::Eraser => {
                    app.ui.tool = t;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Pressure for the next tool event (1 for a mouse).
    pub fn pressure(&self) -> f32 {
        let Some(s) = self.sample() else { return 1.0 };
        match (self.tablet_response.is_some(), self.current_filtered()) {
            (true, Some(filtered)) => filtered,
            _ => s.pressure,
        }
    }

    /// The response's pressure for the event being processed: the one at the event's time with
    /// the optimization on, the last one the response emitted without it.
    fn current_filtered(&self) -> Option<f32> {
        if self.optimized { self.filtered_at(self.clock_ms) } else { self.filtered.last().map(|s| s.sample.pressure) }
    }

    /// Preferences › Tools › Tablet Pressure response (off when the filter is disabled).
    pub fn set_tablet_response(&mut self, settings: photocraft_paint::TabletPressureSettings) {
        self.tablet_response = settings.enabled.then_some(settings.clamped());
        if self.tablet_response.is_none() {
            self.pressure_filter.reset();
            self.filtered.clear();
            self.filter_open = false;
            self.last_filtered_raw = None;
        }
    }

    /// Begin a stroke through the tablet response, so its filter state and delay line start
    /// empty (`Stylus::pressure` then returns the filtered values).
    pub(crate) fn begin_pressure_filter(&mut self) {
        self.pressure_filter.reset();
        self.filtered.clear();
        self.filter_open = false;
        self.last_filtered_raw = None;
        self.advance_pressure_filter();
    }

    /// Feed this frame's pen samples through the response, in the order the pen reported them:
    /// the response averages the last few *samples* (C3's window), not the last few frames, so a
    /// 200 Hz pen is smoothed over 25 ms and the pressure follows the pen to its lift instead of
    /// trailing it by the frame rate.
    fn advance_pressure_filter(&mut self) {
        if !self.optimized {
            return self.advance_pressure_filter_by_frame();
        }
        self.filtered.clear();
        let Some(settings) = self.tablet_response else { return };
        for s in &self.frame {
            let point = photocraft_paint::StrokePoint { is_pen: true, x: 0.0, y: 0.0, pressure: s.sample.pressure, ..Default::default() };
            // The filter's returned point is the delayed one; the pressure below is the freshest.
            let _out = if self.filter_open {
                self.pressure_filter.push(point)
            } else {
                self.filter_open = true;
                self.pressure_filter.begin(point, settings)
            };
            // Coordinate stability holds points back a segment; the pressure the brush uses must
            // be the freshest one, so read the filter's current pressure rather than the point.
            let pressure = self.pressure_filter.pressure_now();
            self.filtered.push(TimedSample { sample: PenSample { pressure, ..s.sample }, at: s.at });
        }
    }

    /// The earlier response, run once per frame over the frame's last sample (the pen's real
    /// reading, or this frame's last contact): the delay line makes the filter emit a point a
    /// couple of samples behind, so this keeps the last emitted pressure for the whole frame.
    fn advance_pressure_filter_by_frame(&mut self) {
        let Some(settings) = self.tablet_response else {
            self.filtered.clear();
            return;
        };
        let Some(sample) = self.sample() else {
            self.filtered.clear();
            return;
        };
        if self.last_filtered_raw == Some(sample.pressure) && self.filter_open {
            return;
        }
        self.last_filtered_raw = Some(sample.pressure);
        let point = photocraft_paint::StrokePoint { is_pen: true, x: 0.0, y: 0.0, pressure: sample.pressure, ..Default::default() };
        let _out = if self.filter_open {
            self.pressure_filter.push(point)
        } else {
            self.filter_open = true;
            self.pressure_filter.begin(point, settings)
        };
        let pressure = self.pressure_filter.pressure_now();
        self.filtered.clear();
        self.filtered.push(TimedSample { sample: PenSample { pressure, ..sample }, at: self.clock_ms });
    }

    /// The response's pressure at `t` (milliseconds, [`Self::clock_ms`]'s clock).
    fn filtered_at(&self, t: f64) -> Option<f32> {
        sample_at(&self.filtered, t).map(|s| s.pressure)
    }

    /// Flush the tablet response at the end of a stroke.
    pub(crate) fn finish_pressure_filter(&mut self) {
        if self.tablet_response.is_some() {
            let _ = self.pressure_filter.finish(None);
        }
        self.filtered.clear();
        self.filter_open = false;
        self.last_filtered_raw = None;
        self.stroke_active = false;
    }

    /// Pressure for a tap, a drag released where it was pressed: the highest the pen pressed
    /// since the drag began (through the pressure curve), as the first sample of a contact is
    /// often far lighter than the tap. 1 for a mouse.
    pub(crate) fn tap_pressure(&self) -> f32 {
        let Some(s) = self.sample() else { return 1.0 };
        let peak = match &self.pressure_curve {
            Some(c) => c.eval(self.peak),
            None => self.peak,
        };
        if self.peak > 0.0 { peak.max(s.pressure) } else { s.pressure }
    }

    /// Start recording a drag's tilt/rotation.
    pub(crate) fn begin_stroke(&mut self) {
        // A new stroke's first moves interpolate from this frame's samples, never from a sample
        // left over from an earlier stroke.
        self.prev = None;
        self.held = None;
        self.stroke_active = true;
        self.peak = self.frame.iter().fold(0.0, |m, s| m.max(s.sample.pressure));
        self.stroke.clear();
        self.times.clear();
        self.record_point();
    }

    /// Record the tilt/rotation and time of a point just added to the drag.
    pub(crate) fn record_point(&mut self) {
        let s = self.sample().unwrap_or_default();
        self.stroke.push([s.tilt_x, s.tilt_y, s.rotation]);
        self.times.push(self.clock_ms);
    }

    /// Time of the drag's last recorded point.
    pub(crate) fn last_point_ms(&self) -> Option<f64> {
        self.times.last().copied()
    }

    /// Time of drag point `i` (the last known time past the end).
    pub(crate) fn point_ms(&self, i: usize) -> f64 {
        self.times.get(i).or(self.times.last()).copied().unwrap_or(0.0)
    }

    /// Stroke points for `paint.stroke`: `[x, y, pressure]`, extended with
    /// `tiltX, tiltY, rotation` when the drag carried any tilt or rotation, and with `timeMs`
    /// (from the drag's first point) when its points carry times.
    pub fn stroke_points(&self, points: &[[f64; 3]]) -> Vec<Vec<f64>> {
        let has_pose = self.stroke.iter().any(|t| t.iter().any(|v| *v != 0.0));
        let t0 = self.point_ms(0);
        let has_time = self.times.iter().any(|t| *t != t0);
        points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut v = vec![p[0], p[1], p[2]];
                if has_pose || has_time {
                    let t = self.stroke.get(i).or(self.stroke.last()).copied().unwrap_or_default();
                    v.extend(t.map(f64::from));
                }
                if has_time {
                    v.push(self.point_ms(i) - t0);
                }
                v
            })
            .collect()
    }
}

/// The touch contacts a pen makes, read from the raw input. egui takes every touch contact held
/// still past `max_click_duration` (0.8 s) for a touch screen's press-and-hold: a secondary click
/// (a context menu) that also drops the drag. On Windows a pen arrives as touch events, so a pen
/// held still opened a context menu. A pen held still is just a pen, as in Photoshop.
#[derive(Clone, Debug, Default)]
pub(crate) struct PenContacts {
    /// Touch ids that moved without touching: only a pen hovers.
    hovering: Vec<egui::TouchId>,
    /// The contacts down, and whether each is a pen.
    down: Vec<(egui::TouchId, bool)>,
}

/// Most touch ids [`PenContacts`] remembers (the oldest go first).
const CONTACTS: usize = 16;

impl PenContacts {
    /// Follow a frame's touch events. True when a pen contact was down at any point of the frame.
    pub(crate) fn observe(&mut self, events: &[egui::Event]) -> bool {
        let pressed = |f: &Option<f32>| f.is_some_and(|f| f > 0.0);
        let mut pen = self.down.iter().any(|(_, p)| *p);
        for e in events {
            let egui::Event::Touch { id, phase, force, .. } = e else { continue };
            match phase {
                egui::TouchPhase::Start => {
                    let is_pen = self.hovering.contains(id) || pressed(force);
                    self.down.retain(|(c, _)| c != id);
                    if self.down.len() >= CONTACTS {
                        self.down.remove(0);
                    }
                    self.down.push((*id, is_pen));
                    pen |= is_pen;
                }
                egui::TouchPhase::Move => match self.down.iter_mut().find(|(c, _)| c == id) {
                    Some(c) if pressed(force) => {
                        c.1 = true;
                        pen = true;
                    }
                    Some(_) => {}
                    None if !self.hovering.contains(id) => {
                        if self.hovering.len() >= CONTACTS {
                            self.hovering.remove(0);
                        }
                        self.hovering.push(*id);
                    }
                    None => {}
                },
                egui::TouchPhase::End | egui::TouchPhase::Cancel => self.down.retain(|(c, _)| c != id),
            }
        }
        pen
    }
}

/// Turn egui's touch press-and-hold off while a pen contact is down (and through the frame it
/// lifts in, so the release is the click a quick tap would be), back on otherwise for fingers.
pub(crate) fn pen_hold_is_no_right_click(ctx: &egui::Context, pen_down: bool) {
    let want = if pen_down { f64::INFINITY } else { egui::InputOptions::default().max_click_duration };
    if ctx.options(|o| o.input_options.max_click_duration) != want {
        ctx.options_mut(|o| o.input_options.max_click_duration = want);
    }
}

#[cfg(test)]
mod tests;
