//! PhoinixDR desktop entry point.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

fn main() {
    phoinix_desktop_lib::run();
}
