//! Quarry: a keyboard-first database IDE.

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
