#![allow(deprecated, unexpected_cfgs)]

#[cfg(target_os = "macos")]
mod capture;
mod config;
mod geometry;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod ui;

fn main() {
    #[cfg(target_os = "macos")]
    unsafe {
        ui::run();
    }
    #[cfg(not(target_os = "macos"))]
    compile_error!("RingSwitch requires macOS");
}
