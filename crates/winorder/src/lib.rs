//! Child (macOS) and owned (Windows) windows, behind a safe API.
//!
//! A floating window that has to stay above the app's main window cannot lean on the window
//! manager's *sibling* order alone. On macOS `-[NSWindow orderWindow:relativeTo:]` puts it back
//! above the main window, but AppKit reorders the clicked window the moment a click arrives, so the
//! document is in front again only on the next frame: that interval is the one-frame flicker a
//! click on the canvas shows. A *child* window (`-[NSWindow addChildWindow:ordered:]`) is kept
//! above its parent by AppKit itself, so there is nothing to correct and nothing to flicker.
//!
//! Windows has no AppKit child windows, and the same one-frame gap opens between a click and a
//! sibling reorder. The counterpart is an *owned* window:
//! `SetWindowLongPtrW(GWLP_HWNDPARENT, owner)` makes the window the owner's, and the window
//! manager then keeps every owned window above its owner, whichever of the app's windows is
//! clicked — nothing to correct per frame. Ownership is not `WS_EX_TOPMOST`: another application
//! in front still covers the owned window, while a topmost one would float over every application.
//!
//! The one call each platform can only offer as `unsafe` lives here, like the AppKit event monitor
//! in `photocraft-tablet`; every entry point takes window numbers, so nothing here borrows a
//! window.

#![allow(unsafe_code)]

/// Make the window `child` a child (macOS) or owned (Windows) window of `parent`, ordered above
/// it: the platform then keeps it in front of the parent whichever of the app's windows is
/// clicked. `parent` and `child` are AppKit window numbers on macOS and `HWND` values on
/// Windows. Returns whether both windows still exist (a window can be gone between the caller's
/// lookup and this call).
pub fn attach(parent: isize, child: isize) -> bool {
    platform::attach(parent, child)
}

/// Detach a window [`attach`]ed earlier, before it goes away: AppKit's parent keeps its children
/// alive, so a closed floating window stays allocated until it is removed.
pub fn detach(child: isize) {
    platform::detach(child);
}

/// Whether the window is currently a child (macOS) or owned (Windows) window of some other
/// window.
pub fn is_attached(child: isize) -> bool {
    platform::is_attached(child)
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSWindow, NSWindowOrderingMode};

    fn window(mtm: MainThreadMarker, number: isize) -> Option<Retained<NSWindow>> {
        NSApplication::sharedApplication(mtm).windowWithWindowNumber(number)
    }

    pub(super) fn attach(parent_number: isize, child_number: isize) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let (Some(parent), Some(child)) = (window(mtm, parent_number), window(mtm, child_number)) else {
            return false;
        };
        if child.parentWindow().is_some() {
            return true;
        }
        // SAFETY: both windows are live windows of this app, looked up by number on the main thread,
        // which is the only thread that may touch AppKit. A child window is attached to one parent
        // at a time and the caller only ever attaches an ordinary document window, never a window
        // that has children of its own, so no parent -> child -> parent cycle can be formed.
        unsafe { parent.addChildWindow_ordered(&child, NSWindowOrderingMode::Above) };
        true
    }

    pub(super) fn detach(child_number: isize) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(child) = window(mtm, child_number) else {
            return;
        };
        if let Some(parent) = child.parentWindow() {
            parent.removeChildWindow(&child);
        }
    }

    pub(super) fn is_attached(child_number: isize) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        window(mtm, child_number).is_some_and(|child| child.parentWindow().is_some())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::c_void;

    use winsafe::{co, HwndPlace, HWND, POINT, SIZE};

    /// The window with `number`, when Windows still has it. The caller passes an `HWND` it read
    /// from a live window of this process, so the wrap only has to reject a handle destroyed
    /// since (`IsWindow`).
    fn window(number: isize) -> Option<HWND> {
        if number == 0 {
            return None;
        }
        // SAFETY: wrapping an `HWND` allocates nothing and borrows nothing; the value is checked
        // by `IsWindow` before it is used.
        let window = unsafe { HWND::from_ptr(number as *mut c_void) };
        window.IsWindow().then_some(window)
    }

    /// The owner's handle value, `0` when the window has no owner.
    fn owner(child: &HWND) -> isize {
        child.GetWindowLongPtr(co::GWLP::HWNDPARENT)
    }

    pub(super) fn attach(parent_number: isize, child_number: isize) -> bool {
        let (Some(parent), Some(child)) = (window(parent_number), window(child_number)) else {
            return false;
        };
        let parent_value = parent.ptr() as isize;
        if owner(&child) == parent_value {
            return true;
        }
        // SAFETY: both are live top-level windows of this app (`window` checked `IsWindow`). The
        // value stored is an `HWND`, never a pointer into this process's memory, and Windows
        // drops the relationship when either window goes away or the owner is changed again.
        unsafe { child.SetWindowLongPtr(co::GWLP::HWNDPARENT, parent_value) };
        if owner(&child) != parent_value {
            return false;
        }
        // Once, not per frame: the window manager keeps owned windows above their owner from
        // here on. This only puts the new one into that place in the band the owner is in.
        let _ = child.SetWindowPos(HwndPlace::Hwnd(parent), POINT::new(), SIZE::new(), co::SWP::NOMOVE | co::SWP::NOSIZE | co::SWP::NOACTIVATE);
        true
    }

    pub(super) fn detach(child_number: isize) {
        let Some(child) = window(child_number) else {
            return;
        };
        if owner(&child) == 0 {
            return;
        }
        // SAFETY: as in `attach`; clearing the owner stores no handle at all.
        unsafe { child.SetWindowLongPtr(co::GWLP::HWNDPARENT, 0) };
    }

    pub(super) fn is_attached(child_number: isize) -> bool {
        window(child_number).is_some_and(|child| owner(&child) != 0)
    }
}

/// Nothing to attach to where neither AppKit nor the Win32 window manager is: the app keeps its
/// windows above each other through the sibling order alone.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub(super) fn attach(_parent_number: isize, _child_number: isize) -> bool {
        false
    }

    pub(super) const fn detach(_child_number: isize) {}

    pub(super) const fn is_attached(_child_number: isize) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    /// Off macOS and Windows (where these tests also run) there is no window manager to attach to:
    /// every entry point answers, and answers "not attached", so the caller keeps its own ordering.
    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn nothing_attaches_without_appkit() {
        assert!(!super::attach(1, 2));
        assert!(!super::is_attached(2));
        super::detach(2);
    }

    /// Fake handles are never live windows: both platforms have to answer "no" instead of
    /// touching a window they do not own.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn unknown_windows_are_not_attached() {
        assert!(!super::attach(1, 2));
        assert!(!super::is_attached(2));
        super::detach(2);
    }
}
