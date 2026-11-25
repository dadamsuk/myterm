// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("--keeper") => myterm_lib::keeper::server::run(),
            // The CLI, for debug builds, which are console programs (release
            // builds ship myterm-cli.exe).
            Some("--cli") => myterm_lib::keeper::cli::main(&args[2..]),
            _ => {}
        }
    }
    myterm_lib::run()
}
