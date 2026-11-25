//! Console helper for the Windows keeper (SPEC.md 14.1): see keeper/cli.rs.

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        myterm_lib::keeper::cli::main(&args)
    }
    #[cfg(not(windows))]
    {
        eprintln!("myterm-cli is only used on Windows");
        std::process::exit(2)
    }
}
