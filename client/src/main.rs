#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> Result<(), slint::PlatformError> {
    edenvoice_client::run()
}
