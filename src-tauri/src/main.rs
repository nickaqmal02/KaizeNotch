// No console window behind the app in a Windows release build
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(json) = kaizenotch_lib::report_json_cli(&args) {
        println!("{json}");
        return;
    }
    kaizenotch_lib::run()
}
