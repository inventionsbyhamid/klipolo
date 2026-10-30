#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Export runs out of process in this same binary: no second executable, always the same version. Before Tauri
    // starts and before any window exists; its log is stderr, which the app forwards into its own.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-worker") {
        std::process::exit(klipolo_engine::export::worker::main(&args[2..]));
    }
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("Klipolo could not start");
}
