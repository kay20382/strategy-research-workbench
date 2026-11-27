#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
#[path = "quote_manager/ui.rs"]
mod ui;
fn main() {
    #[cfg(windows)]
    unsafe {
        ui::run();
    }
    #[cfg(not(windows))]
    eprintln!("原生資料管理視窗僅支援Windows；可使用quote_manager CLI。");
}
