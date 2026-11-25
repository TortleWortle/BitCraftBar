//! Small platform-specific helpers GPUI doesn't provide.

#[allow(unused_imports)]
use gpui::{App, Bounds, Pixels, Point, Window, point, px};

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

/// The NSWindow behind a GPUI window.
#[cfg(target_os = "macos")]
fn ns_window(window: &Window) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return None };
    // SAFETY: GPUI's window handle points at a live NSView on the main thread.
    let view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?;
    view.window()
}

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return None };
    Some(win32.hwnd.get() as windows_sys::Win32::Foundation::HWND)
}

/// Height of the primary screen (AppKit's y axis is flipped vs GPUI's).
#[cfg(target_os = "macos")]
fn primary_height() -> Option<f64> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    Some(NSScreen::screens(mtm).firstObject()?.frame().size.height)
}

/// Popups that show without taking focus (so a game keeps keyboard focus)
/// need click-outside detection by polling (`pressed_mouse_location`),
/// which isn't available on Linux; there the popup takes focus and closes
/// on focus loss.
pub const NON_ACTIVATING_POPUP: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Mouse position (GPUI logical screen coordinates) while a mouse button is
/// held anywhere on screen, in any app. None when no button is down.
#[cfg(target_os = "macos")]
pub fn pressed_mouse_location() -> Option<Point<Pixels>> {
    use objc2_app_kit::NSEvent;
    if NSEvent::pressedMouseButtons() == 0 {
        return None;
    }
    let p = NSEvent::mouseLocation();
    Some(point(px(p.x as f32), px((primary_height()? - p.y) as f32)))
}

#[cfg(target_os = "windows")]
pub fn pressed_mouse_location() -> Option<Point<Pixels>> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    // SAFETY: plain Win32 queries.
    let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|&k| unsafe { GetAsyncKeyState(k as i32) } as u16 & 0x8000 != 0);
    if !down {
        return None;
    }
    let mut p = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    let scale = scale_factor();
    Some(point(px((p.x as f64 / scale) as f32), px((p.y as f64 / scale) as f32)))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pressed_mouse_location() -> Option<Point<Pixels>> {
    None
}

/// Windows: GPUI shows windows with `SW_SHOWNORMAL`, which activates them
/// even with `focus: false`. `WS_EX_NOACTIVATE` keeps the game focused
/// (also on clicks); turned off when the popup needs the keyboard. macOS
/// PopUp windows are non-activating panels already.
#[cfg(target_os = "windows")]
pub fn set_no_activate(window: &Window, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE};
    let Some(hwnd) = hwnd(window) else { return };
    // SAFETY: hwnd belongs to a live GPUI window on this thread.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = if on { style | WS_EX_NOACTIVATE as isize } else { style & !(WS_EX_NOACTIVATE as isize) };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_no_activate(_window: &Window, _on: bool) {}

/// Keep the info window above other windows. macOS: GPUI's PopUp windows
/// already float (popup level, on all Spaces, next to fullscreen apps).
/// Windows: topmost + no activation.
#[cfg(target_os = "windows")]
pub fn make_overlay(window: &Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};
    set_no_activate(window, true);
    let Some(hwnd) = hwnd(window) else { return };
    // SAFETY: hwnd belongs to a live GPUI window on this thread.
    unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
}

#[cfg(not(target_os = "windows"))]
pub fn make_overlay(_window: &Window) {}

/// Start dragging a titlebar-less window from a mouse-down handler.
/// macOS: `performWindowDragWithEvent` with the mouse-down being handled
/// (GPUI's `start_window_move` is a no-op there). Linux: GPUI's own.
/// Windows: handled by `WindowControlArea::Drag` (`HTCAPTION`) instead.
#[cfg(target_os = "macos")]
pub fn start_window_drag(window: &Window) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(ns_window) = ns_window(window) else { return };
    if let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() {
        ns_window.performWindowDragWithEvent(&event);
    }
}

#[cfg(target_os = "windows")]
pub fn start_window_drag(_window: &Window) {}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start_window_drag(window: &Window) {
    window.start_window_move();
}

/// Set a window's frame (size *and* position) in GPUI's logical screen
/// coordinates. GPUI only has `resize`, which on macOS keeps the bottom
/// edge fixed (the popup would slide under the menu bar) and can't move a
/// window. Applied asynchronously, like GPUI's own `resize`, so the native
/// resize callback doesn't re-enter a frame being drawn.
#[cfg(target_os = "macos")]
pub fn set_window_frame(window: &mut Window, frame: Bounds<Pixels>, cx: &App) {
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let Some(ns_window) = ns_window(window) else { return };
    // GPUI: origin top-left of the primary screen, y down. AppKit: origin
    // bottom-left of the primary screen, y up.
    let Some(primary_height) = primary_height() else { return };
    let (x, y, w, h) = (frame.origin.x.to_f64(), frame.origin.y.to_f64(), frame.size.width.to_f64(), frame.size.height.to_f64());
    let rect = NSRect::new(NSPoint::new(x, primary_height - y - h), NSSize::new(w, h));
    cx.foreground_executor()
        .spawn(async move { ns_window.setFrame_display(rect, true) })
        .detach();
}

#[cfg(target_os = "windows")]
pub fn set_window_frame(window: &mut Window, frame: Bounds<Pixels>, cx: &App) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
    let Some(hwnd) = hwnd(window) else { return };
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
