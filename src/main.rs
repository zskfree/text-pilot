#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod windows_app;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_app::run() {
        windows_app::show_fatal_error(&error.to_string());
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("TextPilot only supports Windows.");
}
