// Hide the console window on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod api;
mod apps;
mod brew;
mod cli;
mod filesearch;
mod finder;
#[cfg(feature = "gui")]
mod gui;
mod icons;
mod locations;
mod macho;
mod sentinel;
mod tasks;
mod tools;
mod trash;
mod updater;
mod util;

/// Port the app window uses (fixed so the page's saved view settings persist between launches).
const APP_PORT: u16 = 47311;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |f: &str| args.iter().any(|a| a == f);

    // Helper modes (used by Pearfin itself)
    if flag("--askpass") {
        return brew::askpass_main();
    }
    if flag("--sentinel") {
        return sentinel::run();
    }
    if let Some(i) = args.iter().position(|a| a == "--thin") {
        // Run as root through the admin prompt when an app isn't writable by the user.
        let p = args.get(i + 1).cloned().unwrap_or_default();
        match api::thin_app(&p) {
            Ok(n) => println!("{n}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1)
            }
        }
        return;
    }
    // Terminal commands: `pearfin list`, `pearfin uninstall /Applications/Foo.app`, …
    if args.len() > 1 && cli::is_command(&args[1]) {
        std::process::exit(cli::run(&args[1..]));
    }

    let port: u16 = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(APP_PORT);
    let app = api::App::new();

    #[cfg(feature = "gui")]
    if !flag("--server-only") {
        // Paths passed by "Open With" on older launch paths, or by `open -a Pearfin Foo.app`
        let initial: Vec<String> = args.iter().skip(1).filter(|a| a.ends_with(".app") || a.starts_with("pearfin://")).cloned().collect();
        return gui::run(app, port, initial);
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = rt.block_on(api::serve(app, port, |_| {})) {
        eprintln!("Pearfin couldn't start: {e}");
        std::process::exit(1);
    }
}
