// Hide the Windows console in release builds; keep it in debug for logs.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

fn main() {
    qobee_app_lib::run();
}
