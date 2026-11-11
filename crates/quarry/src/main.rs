//! Quarry: a keyboard-first database IDE.

// A release build on Windows is a GUI application: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let demo_env = std::env::var("QUARRY_DEMO").is_ok_and(|v| v == "1");
    let args = match quarry::cli::parse(std::env::args().skip(1), demo_env) {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "quarry: {e}\nusage: quarry [--demo] [--connect URL]... [--stress-catalog[=N]] [--theme NAME]"
            );
            std::process::exit(2);
        }
    };
    quarry::app::run(args);
}
