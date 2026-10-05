// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Chrome starts us this way to ask where the running app is (extension pairing).
    if std::env::args().nth(1).as_deref() == Some("--native-host") {
        idmx_server::browser::run_native_host(&idmx_core::default_data_dir());
        return;
    }
    idmx_app_lib::run()
}
