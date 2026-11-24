//! Desktop notifications (macOS Notification Center, Windows toasts,
//! freedesktop on Linux). Sent from a thread so a slow notification daemon
//! never blocks the UI.

/// Call once at startup. macOS: notifications must be sent "as" an app
/// bundle. Unset, `mac-notification-sys` looks up an app literally named
/// "use_default" (AppleScript), which pops up a "Where is use_default?"
/// dialog that also steals focus. Use our bundle id when running as a .app,
/// else Terminal's (a bare binary has none).
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        let bundle = objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "com.apple.Terminal".to_string());
        if let Err(e) = notify_rust::set_application(&bundle) {
            eprintln!("notifications: {e}");
        }
    }
}

pub fn notify(title: String, body: String) {
    if std::env::var_os("BITCRAFT_DEBUG").is_some() {
        eprintln!("notify: {title} — {body}");
    }
    if std::env::var_os("BITCRAFT_NO_NOTIFY").is_some() {
        return;
    }
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .appname("BitCraft Tracker")
            .show();
        if let Err(e) = result {
            eprintln!("notification failed: {e}");
        }
    });
}
