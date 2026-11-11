//! Theta: a keyboard-first database IDE.

// A release build on Windows is a GUI application: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Before anything reads or creates the new directories (the panic hook included).
    theta_ui::legacy::migrate_app_dirs();
    theta::panic::install(theta_ui::config_dir());
    let demo_env = std::env::var("THETA_DEMO").is_ok_and(|v| v == "1");
    let args = match theta::cli::parse(std::env::args().skip(1), demo_env) {
        Ok(a) => a,
        Err(e) => {
            // A GUI-subsystem build on Windows has no console to print to; there the message
            // is still useful when started from a terminal that attached one.
            eprintln!("theta: {e}\n\n{}", theta::cli::USAGE);
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", theta::cli::USAGE);
        return;
    }
    if args.version {
        println!("{}", theta::cli::version_line());
        return;
    }
    if let Some(note) = theta::cli::ssh_auth_note(&args) {
        eprintln!("{note}");
    }
    theta::app::run(args);
}
