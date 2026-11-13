//! macOS window lifecycle: title bar dragging, window ordering and quitting.
//!
//! Dragging: the window draws under a transparent title bar (`main.rs`), so AppKit would
//! otherwise drag the window from anywhere in that strip, menus included: pressing a menu title
//! and dragging down to an item moved the window instead. Turning AppKit's own dragging off leaves
//! only the app's drag region (the free gap between the menus and the controls,
//! `panels::title_bar`), which starts a drag explicitly (`ViewportCommand::StartDrag`, AppKit's
//! `performWindowDragWithEvent:`, which works on an unmovable window).
//!
//! Ordering: a document dragged out of the tab strip is an ordinary AppKit window (`canvas::tabs`
//! `extra_windows`), so clicking the main window orders it in front of the document, which then
//! slides behind the app it belongs to. [`keep_document_windows_above_main`] puts it back above the
//! main window — in front of the app, and above nothing else: not `AlwaysOnTop`, which floats over
//! other applications too.
//!
//! Quitting: see [`terminate_later`]. All safe objc2 calls.

use std::sync::atomic::{AtomicIsize, Ordering};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindowOrderingMode};

/// The AppKit number of the main window, taken at startup ([`remember_main_window`]); `0` until
/// then. The floating document windows are kept above this window.
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The suffix `canvas::tabs` puts on a floating document window's title (`{name} — window {id}`):
/// the one thing AppKit-side that tells a document window from the main window or a panel's own
/// window, without holding that window's handle.
const DOCUMENT_WINDOW: &str = " — window ";

/// Stop AppKit dragging the app's windows by their title bar strip.
pub fn disable_native_title_drag() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("title bar: not on the main thread; AppKit window dragging left on");
        return;
    };
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        window.setMovable(false);
    }
}

/// Take the main window's AppKit number, once at startup: the floating document windows are kept
/// above it ([`keep_document_windows_above_main`]). Only the main window exists yet, so it is the
/// app's largest window. It is not visible at this point (eframe orders it front after the app
/// callback returns), which is why visibility is not part of the choice.
pub fn remember_main_window() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("window order: not on the main thread; floating document windows are left to AppKit");
        return;
    };
    let mut main = 0;
    let mut area = 0.0;
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        let size = window.frame().size;
        if size.width * size.height > area {
            area = size.width * size.height;
            main = window.windowNumber();
        }
    }
    log::debug!("window order: main window {main}");
    MAIN_WINDOW.store(main, Ordering::Relaxed);
}

/// Keep the app's floating document windows in front of the main window, and above nothing else.
///
/// The windows AppKit has ordered behind the main window are put back just above it, so the
/// document windows stay in front of the app without covering another application's windows: the
/// windows keep `NSNormalWindowLevel`, which is why another app in front still covers them.
pub fn keep_document_windows_above_main() {
    let main = MAIN_WINDOW.load(Ordering::Relaxed);
    if main == 0 {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // Ordered front to back: past the main window's place in it, a document window is behind the
    // app, which is what a click on the main window (or on a panel's window) leaves behind.
    let mut behind_the_main = false;
    let mut moved = 0;
    for window in app.orderedWindows().iter() {
        if window.windowNumber() == main {
            behind_the_main = true;
            continue;
        }
        if !behind_the_main || !window.isVisible() || !is_document_window(&window.title().to_string()) {
            continue;
        }
        // Taken front to back, each window lands just above the main window, so the documents keep
        // the order the user last clicked them in.
        window.orderWindow_relativeTo(NSWindowOrderingMode::Above, main);
        moved += 1;
    }
    if moved > 0 {
        log::debug!("window order: {moved} document window(s) back above the main window");
    }
}

/// Whether a window title is a floating document window's (`{name} — window {id}`, `canvas::tabs`).
fn is_document_window(title: &str) -> bool {
    let Some((_, id)) = title.rsplit_once(DOCUMENT_WINDOW) else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// Quit the way a Mac app does, with `-[NSApplication terminate:]` (`Services::quit`), instead of
/// letting eframe close the window. eframe drops the window while AppKit's run loop is still
/// running, and on Macs with a Touch Bar AppKit's next display cycle then tries to stop observing
/// a view that is gone: an uncaught exception that crashes (macOS 27) or leaves a windowless app
/// that only Force Quit ends (macOS 26) (#1458, #1575). `terminate:` ends the process before
/// another display cycle runs; winit's `applicationWillTerminate:` closes the windows and has
/// eframe save the window layout first, as the old ⌘Q (winit's default menu) did.
///
/// Scheduled on the main queue: winit has to be out of its event handler, or eframe's exit
/// handling (and the save) is dropped.
pub fn terminate_later() {
    dispatch2::DispatchQueue::main().exec_async(|| match MainThreadMarker::new() {
        Some(mtm) => NSApplication::sharedApplication(mtm).terminate(None),
        None => log::warn!("quit: the main queue ran off the main thread; not terminating"),
    });
}

#[cfg(test)]
mod tests {
    /// Only a floating document window's title is taken for one: the main window's (the document
    /// name and the app's) and a panel's are left alone.
    #[test]
    fn document_windows_are_told_by_their_title() {
        assert!(super::is_document_window("Untitled-1 — window 1"));
        assert!(super::is_document_window("图片_20261024 — window 12"));
        assert!(!super::is_document_window("Untitled-1 — PhotoCraft"));
        assert!(!super::is_document_window("Untitled-1 — window"));
        assert!(!super::is_document_window("Untitled-1 — window one"));
        assert!(!super::is_document_window("Untitled-1 — window 1a"));
    }
}
