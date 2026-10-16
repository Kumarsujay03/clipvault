//! `clipvault` — launcher and panel UI.
//!
//! Fast paths (`--toggle`, `--show`, `--hide`, `--quit`) talk to an already-running panel or
//! daemon over a Unix socket and exit within milliseconds, without loading GTK/WebKit.

use std::os::unix::process::CommandExt;

use clipvault_core::{autostart, desktop, ipc, paths::Paths, settings::Settings};

const HELP: &str = "\
ClipVault — clipboard history

Usage: clipvault [OPTION]
  (none), --show       open the clipboard panel
  --toggle             open the panel, or close it if it is focused
  --hide               close the panel
  --daemon             run the background capture service (started at login)
  --quit               stop ClipVault completely
  --setup              enable start at login, the keyboard shortcut and the GNOME extension
  --remove-integration undo --setup (history is kept)
  --update [--yes]     check GitHub for a new version and install it
  --restart            restart the background capture service
  --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    if has("--help") || has("-h") {
        println!("{HELP}");
        return;
    }
    if has("--version") {
        println!("clipvault {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if has("--daemon") {
        let daemon = std::env::current_exe().map(|e| e.with_file_name("clipvaultd"));
        match daemon {
            Ok(path) if path.exists() => {
                let err = std::process::Command::new(path).exec();
                eprintln!("clipvault: cannot start the capture service: {err}");
            }
            _ => eprintln!("clipvault: clipvaultd not found next to clipvault"),
        }
        std::process::exit(1);
    }
    // Relaunch after an update: wait for the old panel process to exit first.
    if let Some(pid) = args.iter().find_map(|a| a.strip_prefix("--wait-pid=")) {
        let proc_dir = std::path::PathBuf::from(format!("/proc/{pid}"));
        for _ in 0..100 {
            if !proc_dir.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if has("--restart") {
        if clipvault_lib::update::restart_service() {
            println!("ClipVault capture service restarted.");
        } else {
            eprintln!("clipvault: could not start the capture service");
            std::process::exit(1);
        }
        return;
    }
    if has("--update") {
        std::process::exit(cli_update(has("--yes")));
    }
    if has("--quit") {
        let _ = ipc::request(&ipc::Request::Quit, std::time::Duration::from_secs(2));
        ipc::send_ui("quit");
        return;
    }
    if has("--setup") || has("--remove-integration") {
        let paths = match Paths::from_env() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("clipvault: {e}");
                std::process::exit(1);
            }
        };
        let _ = paths.ensure();
        let (settings, _) = Settings::load(&paths);
        if has("--setup") {
            for line in desktop::setup_all(&paths, &settings.shortcut, settings.start_at_login) {
                println!("{line}");
            }
        } else {
            let r1 = desktop::remove_shortcut(&paths);
            let r2 = desktop::uninstall_extension();
            let r3 = autostart::sync(&paths, false);
            for (what, r) in [
                ("shortcut", r1),
                ("GNOME extension", r2),
                ("start at login", r3),
            ] {
                match r {
                    Ok(()) => println!("Removed {what}"),
                    Err(e) => println!("Could not remove {what}: {e}"),
                }
            }
        }
        return;
    }
    if has("--hide") {
        ipc::send_ui("hide");
        return;
    }
    let forwarded = if has("--toggle") {
        ipc::send_ui("toggle")
    } else {
        ipc::send_ui("show")
    };
    if forwarded {
        return;
    }
    clipvault_lib::run();
}

fn cli_update(yes: bool) -> i32 {
    use clipvault_lib::update;
    use std::io::{BufRead, IsTerminal, Write};
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("clipvault: {e}");
            return 1;
        }
    };
    let info = match update::check(&paths) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("clipvault: {e}");
            return 1;
        }
    };
    if !info.available {
        println!(
            "ClipVault {} is up to date (latest release: {}).",
            info.current, info.latest
        );
        return 0;
    }
    println!(
        "ClipVault {} is available (you have {}).\n{}\n",
        info.latest, info.current, info.url
    );
    if !yes {
        if !std::io::stdin().is_terminal() {
            eprintln!("Run `clipvault --update --yes` to install it.");
            return 2;
        }
        print!("Install it now? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !answer.trim().eq_ignore_ascii_case("y") {
            return 0;
        }
    }
    match update::install(&paths, &|m: String| println!("{m}")) {
        Ok(msg) => {
            println!("{msg}");
            if msg.starts_with("Updated") {
                ipc::send_ui("quit");
                if update::restart_service() {
                    println!("Capture service restarted. Press Super+V to open the new version.");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("clipvault: {e}");
            1
        }
    }
}
