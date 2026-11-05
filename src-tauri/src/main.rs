// Макет точки входа Windows (без консоли).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    jarvis3_lib::run()
}
