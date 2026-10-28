fn main() {
    // Library tests call `run_with` and must not reload a saved wallpaper.
    std::env::set_var("IRIS_RESTORE_WALLPAPER", "1");
    if let Err(err) = iris_daemon::run() {
        eprintln!("iris-daemon: {err}");
        std::process::exit(1);
    }
}
