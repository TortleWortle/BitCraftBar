//! Small platform-specific helpers GPUI doesn't provide.

use gpui::{App, Bounds, Pixels, Window};

/// Tray coordinates are physical pixels; GPUI uses logical ones.
#[cfg(target_os = "macos")]
pub fn scale_factor() -> f64 {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    MainThreadMarker::new()
        .and_then(NSScreen::mainScreen)
        .map(|s| s.backingScaleFactor())
        .unwrap_or(2.0)
}

#[cfg(target_os = "windows")]
pub fn scale_factor() -> f64 {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn scale_factor() -> f64 {
    1.0
}

/// Menu bar app: no Dock icon (GPUI starts as a regular app).
#[cfg(target_os = "macos")]
pub fn hide_dock_icon() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
}

/// Set the popup window's frame (size *and* position) in GPUI's logical
/// screen coordinates. GPUI only has `resize`, which on macOS keeps the
/// bottom edge fixed (the popup would slide under the menu bar) and can't
/// move a window. Applied asynchronously, like GPUI's own `resize`, so the
/// native resize callback doesn't re-enter a frame being drawn.
#[cfg(target_os = "macos")]
pub fn set_window_frame(window: &mut Window, frame: Bounds<Pixels>, cx: &App) {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSScreen, NSView};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Some(mtm) = MainThreadMarker::new() else { return };
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
    // SAFETY: GPUI's window handle points at a live NSView on the main thread.
    let Some(view) = (unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }) else { return };
    let Some(ns_window) = view.window() else { return };
    // GPUI: origin top-left of the primary screen, y down. AppKit: origin
    // bottom-left of the primary screen, y up.
    let Some(primary) = NSScreen::screens(mtm).firstObject() else { return };
    let primary_height = primary.frame().size.height;
    let (x, y, w, h) = (frame.origin.x.to_f64(), frame.origin.y.to_f64(), frame.size.width.to_f64(), frame.size.height.to_f64());
    let rect = NSRect::new(NSPoint::new(x, primary_height - y - h), NSSize::new(w, h));
    cx.foreground_executor()
        .spawn(async move { ns_window.setFrame_display(rect, true) })
        .detach();
}

#[cfg(target_os = "windows")]
pub fn set_window_frame(window: &mut Window, frame: Bounds<Pixels>, cx: &App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let scale = window.scale_factor();
    let px = |v: Pixels| (f32::from(v) * scale).round() as i32;
    let (x, y, w, h) = (px(frame.origin.x), px(frame.origin.y), px(frame.size.width), px(frame.size.height));
    cx.foreground_executor()
        .spawn(async move {
            // SAFETY: hwnd belongs to a live GPUI window on this thread.
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
        })
        .detach();
}

/// Linux: X11 keeps the top-left corner on resize; Wayland windows can't
/// position themselves anyway.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn set_window_frame(window: &mut Window, frame: Bounds<Pixels>, _cx: &App) {
    window.resize(frame.size);
}
