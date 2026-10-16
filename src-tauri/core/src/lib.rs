//! ClipVault engine: clipboard capture, storage and desktop integration. Contains no GUI
//! toolkit so the background daemon (`clipvaultd`) stays small.

pub mod autostart;
pub mod capture;
pub mod classify;
pub mod daemon;
pub mod desktop;
pub mod error;
pub mod export;
pub mod ingest;
pub mod ipc;
pub mod media;
pub mod model;
pub mod ocr;
pub mod paths;
pub mod restore;
pub mod sensitive;
pub mod settings;
pub mod store;
mod tray;

/// Return freed heap memory to the OS (glibc keeps it otherwise after large image decodes).
pub fn trim_memory() {
    // SAFETY: malloc_trim has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}
