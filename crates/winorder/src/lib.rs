//! AppKit child windows, behind a safe API.
//!
//! A floating window that has to stay above the app's main window cannot lean on AppKit's window
//! *sibling* order alone. `-[NSWindow orderWindow:relativeTo:]` puts it back above the main window,
//! but AppKit reorders the clicked window the moment a click arrives, so the document is in front
//! again only on the next frame: that interval is the one-frame flicker a click on the canvas shows.
//! A *child* window (`-[NSWindow addChildWindow:ordered:]`) is kept above its parent by AppKit
//! itself, so there is nothing to correct and nothing to flicker.
//!
//! The one call objc2 can only offer as `unsafe` lives here, like the AppKit event monitor in
//! `photocraft-tablet`; every entry point takes window numbers, so nothing here borrows a window.

#![allow(unsafe_code)]

/// Make the window `child` a child window of the window `parent`, ordered above it: AppKit then
/// keeps it in front of the parent whichever of the app's windows is clicked. Returns whether both
/// windows still exist (a window can be gone between the caller's lookup and this call).
pub fn attach(parent: isize, child: isize) -> bool {
    macos::attach(parent, child)
}

/// Detach a window [`attach`]ed earlier, before it goes away: AppKit's parent keeps its children
/// alive, so a closed floating window stays allocated until it is removed.
pub fn detach(child: isize) {
    macos::detach(child);
}

/// Whether the window is currently a child window of some other window.
pub fn is_attached(child: isize) -> bool {
    macos::is_attached(child)
}

#[cfg(target_os = "macos")]
mod macos {
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

/// Nothing to attach to on the platforms AppKit is not the window server: the app keeps its
/// windows above each other through the sibling order alone.
#[cfg(not(target_os = "macos"))]
mod macos {
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
    /// Off macOS (where these tests run) there is no window server to attach to: every entry point
    /// answers, and answers "not attached", so the caller keeps its own ordering.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn nothing_attaches_without_appkit() {
        assert!(!super::attach(1, 2));
        assert!(!super::is_attached(2));
        super::detach(2);
    }
}
