#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
#[path = "pl_workbench/win32.rs"]
mod win32;
fn main() {
    #[cfg(windows)]
    unsafe {
        win32::run();
    }
    #[cfg(not(windows))]
    eprintln!("此工作台目前使用 Windows 原生 UI。");
}
