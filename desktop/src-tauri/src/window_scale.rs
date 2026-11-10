//! Windows uses app-owned sizing while retaining DPI-aware, crisp rendering.
use tauri::{LogicalSize, PhysicalSize, Size, WebviewWindow};

pub fn pixels_per_unit(window: &WebviewWindow) -> tauri::Result<f64> {
    if cfg!(target_os = "windows") {
        let available = window.current_monitor()?.map(|m| m.work_area().size);
        Ok(available.map_or(1.0, |s| fit_scale(s.width, s.height)))
    } else {
        window.scale_factor()
    }
}

fn fit_scale(width: u32, height: u32) -> f64 {
    (f64::from(width) / 820.0)
        .min(f64::from(height) / 560.0)
        .min(1.0)
        .max(0.1)
}

pub fn apply_zoom(window: &WebviewWindow) -> tauri::Result<()> {
    let zoom = if cfg!(target_os = "windows") {
        pixels_per_unit(window)? / window.scale_factor()?
    } else {
        1.0
    };
    window.set_zoom(zoom)
}

pub fn size(window: &WebviewWindow, width: f64, height: f64) -> tauri::Result<Size> {
    if cfg!(target_os = "windows") {
        let unit = pixels_per_unit(window)?;
        Ok(Size::Physical(PhysicalSize::new(
            (width * unit).round() as u32,
            (height * unit).round() as u32,
        )))
    } else {
        Ok(Size::Logical(LogicalSize::new(width, height)))
    }
}

pub fn set_mode(window: &WebviewWindow, compact: bool) -> tauri::Result<()> {
    let (width, height, min_height) = if compact {
        (306.0, 480.0, 430.0)
    } else {
        (820.0, 700.0, 560.0)
    };
    window.set_min_size(Some(size(window, width, min_height)?))?;
    window.set_size(size(window, width, height)?)?;
    apply_zoom(window)?;
    #[cfg(target_os = "windows")]
    fit_to_work_area(window)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn fit_to_work_area(window: &WebviewWindow) -> tauri::Result<()> {
    use tauri::PhysicalPosition;
    if window.is_maximized()? || window.is_fullscreen()? {
        return Ok(());
    }
    if let Some(monitor) = window.current_monitor()? {
        let area = monitor.work_area();
        let outer = window.outer_size()?;
        let inner = window.inner_size()?;
        let border_w = outer.width.saturating_sub(inner.width);
        let border_h = outer.height.saturating_sub(inner.height);
        let width = inner.width.min(area.size.width.saturating_sub(border_w));
        let height = inner.height.min(area.size.height.saturating_sub(border_h));
        if width != inner.width || height != inner.height {
            window.set_size(PhysicalSize::new(width, height))?;
        }
        let p = window.outer_position()?;
        let right = area.position.x + area.size.width.saturating_sub(width + border_w) as i32;
        let bottom = area.position.y + area.size.height.saturating_sub(height + border_h) as i32;
        window.set_position(PhysicalPosition::new(
            p.x.clamp(area.position.x, right),
            p.y.clamp(area.position.y, bottom),
        ))?;
    }
    Ok(())
}

pub fn setup(window: &WebviewWindow) -> tauri::Result<()> {
    apply_zoom(window)?;
    #[cfg(target_os = "windows")]
    {
        use std::sync::{Arc, Mutex};
        use tauri::WindowEvent;
        set_mode(window, false)?;
        window.center()?;
        fit_to_work_area(window)?;
        // Cache app-space dimensions, ignoring the automatic OS resize that can
        // precede ScaleFactorChanged. Never undo an ordinary user resize.
        let unit = pixels_per_unit(window)?;
        let initial = window.inner_size()?;
        let state = Arc::new(Mutex::new((
            window.scale_factor()?,
            initial.width as f64 / unit,
            initial.height as f64 / unit,
        )));
        let target = window.clone();
        window.on_window_event(move |event| {
            match event {
                WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                    if target.is_maximized().unwrap_or(false)
                        || target.is_minimized().unwrap_or(false)
                    {
                        return;
                    }
                    if let (Ok(scale), Ok(unit), Ok(mut saved)) = (
                        target.scale_factor(),
                        pixels_per_unit(&target),
                        state.lock(),
                    ) {
                        if (scale - saved.0).abs() < 0.001 {
                            saved.1 = s.width as f64 / unit;
                            saved.2 = s.height as f64 / unit;
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    let (width, height) = {
                        let Ok(mut saved) = state.lock() else {
                            return;
                        };
                        saved.0 = *scale_factor;
                        (saved.1, saved.2)
                    };
                    let _ = apply_zoom(&target);
                    // Compact/full state is reflected in the cached width.
                    let compact = width < 600.0;
                    if let Ok(min) = size(
                        &target,
                        if compact { 306.0 } else { 820.0 },
                        if compact { 430.0 } else { 560.0 },
                    ) {
                        let _ = target.set_min_size(Some(min));
                    }
                    if !target.is_maximized().unwrap_or(false)
                        && !target.is_fullscreen().unwrap_or(false)
                    {
                        if let Ok(new_size) = size(&target, width, height) {
                            let _ = target.set_size(new_size);
                        }
                        let _ = fit_to_work_area(&target);
                    }
                }
                _ => {}
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_layout_fits_small_work_areas_without_system_magnification() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let fit = fit_scale(1920, 1040);
            let zoom = fit / dpi;
            assert!((820.0 * zoom * dpi - 820.0).abs() < 0.001);
            assert!((208.0 * zoom * dpi - 208.0).abs() < 0.001);
        }
        for (w, h) in [(800, 600), (1024, 500), (1920, 1040)] {
            let fit = fit_scale(w, h);
            assert!(820.0 * fit <= w as f64 + 0.001);
            assert!(560.0 * fit <= h as f64 + 0.001);
        }
    }
}
