//! Windows window lifecycle: the floating document windows are the main window's *owned* windows.
//!
//! A document dragged out of the tab strip is an ordinary Win32 top-level window (`canvas::tabs`
//! `extra_windows`), so clicking the main window would put it in front of the document: Windows
//! orders whichever window is clicked to the front, and the document would slide behind the app
//! it belongs to. An *owned* window (`SetWindowLongPtrW(GWLP_HWNDPARENT, main window)`, wrapped by
//! `photocraft-winorder`) is kept above its owner by the window manager itself, whichever of the
//! app's windows is clicked — the Win32 counterpart of AppKit's child window (`mac_window.rs`),
//! with the same absence of a one-frame flicker a sibling reorder cannot avoid.
//!
//! In front of the app, and above nothing else: the windows keep their ordinary band (ownership is
//! not `WS_EX_TOPMOST`), so another application in front still covers them. As AppKit's child
//! windows are, an owned window is also no window of its own for the taskbar and Alt+Tab: the app
//! stays one window there.
//!
//! The relationship is set once per document window, by the window manager's own API. There is
//! nothing to correct on later frames — [`keep_document_windows_above_main`] only looks for a
//! document window it has not attached yet.

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};

/// The main window's `HWND`, taken at startup ([`remember_main_window`]); `0` until then. The
/// floating document windows are owned by this window.
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The document window ids the last pass looked at, and whether that pass attached every document
/// window the app has open ([`keep_document_windows_above_main`]). While they still match, there is
/// no window to look for: the owner relationship the window manager holds is the ordering.
static PASS: Mutex<Pass> = Mutex::new(Pass { seen: Vec::new(), settled: false, main: 0 });

/// What the last pass saw.
struct Pass {
    /// The open document windows' ids (`canvas::tabs` gives each a `{name} — window {id}` title).
    seen: Vec<u64>,
    /// Whether every one of them was found and attached.
    settled: bool,
    /// The main window the pass attached them to; a new one means a new pass.
    main: isize,
}

/// The suffix `canvas::tabs` puts on a floating document window's title (`{name} — window {id}`):
/// what tells a document window from the main window or a panel's own window, without holding that
/// window's handle.
const DOCUMENT_WINDOW: &str = " — window ";

/// Take the main window's `HWND`, once at startup: the floating document windows become its owned
/// windows ([`keep_document_windows_above_main`]). Taken while it is the app's only window, so the
/// handle is the one eframe created for the app.
pub fn remember_main_window(cc: &eframe::CreationContext<'_>) {
    let Ok(handle) = cc.window_handle() else {
        log::warn!("window order: the main window has no handle; floating document windows are left unowned");
        return;
    };
    let RawWindowHandle::Win32(window) = handle.as_raw() else {
        log::warn!("window order: the main window is not a Win32 window; floating document windows are left unowned");
        return;
    };
    let main = window.hwnd.get();
    log::debug!("window order: main window {main}");
    MAIN_WINDOW.store(main, Ordering::Relaxed);
}

/// Keep the app's floating document windows in front of the main window, and above nothing else.
///
/// Each document window is made an owned window of the main window, which is what closes the gap a
/// click leaves: Windows keeps owned windows above their owner, so the document never slides behind
/// the app it belongs to — not even for the frame a sibling reorder would take. The window keeps
/// its ordinary band, so another application in front still covers it.
///
/// `ids` are the open document windows' ids. A window whose viewport is created later in the same
/// frame as its id first shows up is attached by the next pass: the pass reports itself unsettled
/// until it has found a window for every id.
pub fn keep_document_windows_above_main(ids: &[u64]) {
    let main = MAIN_WINDOW.load(Ordering::Relaxed);
    if main == 0 {
        return;
    }
    let mut pass = PASS.lock().unwrap_or_else(|poison| poison.into_inner());
    if pass.main == main && pass.settled && pass.seen.as_slice() == ids {
        return;
    }
    log::debug!("window order: pass ids={ids:?} main={main}");
    let documents = document_windows();
    let mut attached = 0;
    for &number in &documents {
        let owned = photocraft_winorder::attach(main, number);
        if owned {
            attached += 1;
        }
    }
    log::debug!("window order: ids={ids:?} documents={documents:?} attached={attached}");
    pass.main = main;
    pass.seen.clear();
    pass.seen.extend_from_slice(ids);
    pass.settled = documents.len() >= ids.len() && attached == documents.len();
}

/// The app's own top-level windows whose title says they are a floating document window.
///
/// The titles are read after enumeration, never inside it: `GetWindowText` sends `WM_GETTEXT` to the
/// window, and answering it from inside `EnumWindows` runs the window's own procedure re-entrantly,
/// in the middle of the message that is already being dispatched.
fn document_windows() -> Vec<isize> {
    let process = std::process::id();
    let mut windows = Vec::new();
    let _ = winsafe::EnumWindows(|window| {
        // Only the app's own, visible top-level windows: another process's title can come back empty
        // and is none of ours, and a hidden helper window (an IME's, wgpu's) has nothing to add.
        if window.IsWindowVisible() && window.GetWindowThreadProcessId().1 == process {
            windows.push(window);
        }
        true
    });
    log::debug!("window order: {} own window(s) enumerated", windows.len());
    windows
        .into_iter()
        .filter(|window| window.GetWindowText().is_ok_and(|title| is_document_window(&title)))
        .map(|window| window.ptr() as isize)
        .collect()
}

/// Whether a window title is a floating document window's (`{name} — window {id}`, `canvas::tabs`).
fn is_document_window(title: &str) -> bool {
    let Some((_, id)) = title.rsplit_once(DOCUMENT_WINDOW) else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
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
