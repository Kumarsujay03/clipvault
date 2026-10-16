//! ClipVault background service. Normally started at login (or by the panel on demand).

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("clipvaultd {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Err(e) = clipvault_core::daemon::run() {
        eprintln!("clipvaultd: {e}");
        std::process::exit(1);
    }
}
