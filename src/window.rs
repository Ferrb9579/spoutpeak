//! Native window shaping. Let the compositor handle restored/maximized states.

#[cfg(not(windows))]
pub fn configure_corners() {}

#[cfg(windows)]
pub fn configure_corners() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DwmSetWindowAttribute,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId,
    };
    use windows::core::BOOL;

    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    unsafe extern "system" fn visit(hwnd: HWND, context: LPARAM) -> BOOL {
        let mut process = 0;
        // SAFETY: Win32 supplies a live top-level HWND and the process pointer
        // refers to a local u32 for the duration of this synchronous call.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut process));
        }
        if process != std::process::id() {
            return true.into();
        }
        let mut title = [0u16; 64];
        let length = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
        if String::from_utf16_lossy(&title[..length]) != "Spoutpeak" {
            return true.into();
        }

        let preference = DWMWCP_ROUND;
        // DWM automatically suppresses rounding for maximized, snapped and
        // fullscreen windows. No opaque masks or changes to the resize frame.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&preference).cast(),
                std::mem::size_of_val(&preference) as u32,
            )
        };
        let border = DWMWA_COLOR_NONE;
        // Rounding must not opt the frameless UI into a system-colored outline.
        let border_result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                std::ptr::from_ref(&border).cast(),
                std::mem::size_of_val(&border) as u32,
            )
        };
        if std::env::var_os("SPOUTPEAK_DEBUG").is_some() {
            eprintln!(
                "[window] rounded corner preference: {result:?}; border disabled: {border_result:?}"
            );
        }
        // SAFETY: EnumWindows is synchronous; context points to `found` below.
        unsafe {
            *(context.0 as *mut bool) = true;
        }
        false.into()
    }

    // The first retained-tree build can precede HWND creation. Wait for our
    // native window without blocking startup or requiring a navigation event.
    let worker = std::thread::Builder::new()
        .name("window-corners".into())
        .spawn(|| {
            for _ in 0..100 {
                let mut found = false;
                // Returning FALSE after a match intentionally stops enumeration.
                let _ =
                    unsafe { EnumWindows(Some(visit), LPARAM((&mut found as *mut bool) as isize)) };
                if found {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            STARTED.store(false, Ordering::Relaxed);
        });
    if worker.is_err() {
        STARTED.store(false, Ordering::Relaxed);
    }
}

/// Hides the mouse pointer until it next moves (the next move sets the
/// pointer shape again). For the player's idle fullscreen.
#[cfg(windows)]
pub fn hide_cursor_until_moved() {
    // SAFETY: SetCursor with no cursor only changes this thread's pointer shape.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursor(None);
    }
}

#[cfg(not(windows))]
pub fn hide_cursor_until_moved() {}
