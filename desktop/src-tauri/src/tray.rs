//! Window visibility only: background voice and watching keep their lifetimes.
use tauri::{
    Manager, WebviewWindow,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

const ID: &str = "edenvoice-tray";

pub fn available(window: &WebviewWindow) -> bool {
    window.label() == "main" && window.app_handle().tray_by_id(ID).is_some()
}

pub fn hide_main(window: &WebviewWindow) -> tauri::Result<bool> {
    if !available(window) {
        return Ok(false);
    }
    window.hide()?;
    Ok(true)
}

fn restore(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Keep the user's compact/full layout; restoring is not a resize.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = crate::window_scale::apply_zoom(&window);
        let _ = window.set_focus();
    }
}

pub fn setup(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "tray-show", "打开 EdenVoice", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出 EdenVoice", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &separator, &quit])?;
    let icon = tray_image(app)?;
    TrayIconBuilder::with_id(ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("EdenVoice · 点击恢复窗口，右键退出")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                restore(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => restore(app),
            // Use the existing ExitRequested / shutdown_and_wait path so
            // devices, networking and our owned mpv process stop cleanly.
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    #[cfg(not(target_os = "macos"))]
    if let Some(window) = app.get_webview_window("main") {
        let handle = app.clone();
        window.on_window_event(move |event| {
            if matches!(
                event,
                tauri::WindowEvent::ThemeChanged(_) | tauri::WindowEvent::Focused(true)
            ) {
                if let (Some(tray), Ok(icon)) = (handle.tray_by_id(ID), tray_image(&handle)) {
                    let _ = tray.set_icon(Some(icon));
                }
            }
        });
    }
    Ok(())
}

// macOS templates are black + alpha; AppKit supplies the menu-bar foreground.
// Windows' taskbar can use a different theme from application windows.
fn tray_image(app: &tauri::AppHandle) -> tauri::Result<tauri::image::Image<'static>> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        tauri::image::Image::from_bytes(include_bytes!("../icons/tray-light.png"))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        let bytes: &[u8] = if taskbar_is_light() {
            include_bytes!("../icons/tray-light.png")
        } else {
            include_bytes!("../icons/tray-dark.png")
        };
        tauri::image::Image::from_bytes(bytes)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux panel themes are independent of window themes: the coloured
        // tile remains legible on both, without guessing the panel foreground.
        let _ = app;
        tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
    }
}

#[cfg(target_os = "windows")]
fn taskbar_is_light() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
    let mut value = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // All buffers are valid for this synchronous read; only a DWORD is accepted.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    result == 0 && value != 0
}
