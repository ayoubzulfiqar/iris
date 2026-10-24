fn main() {
    if let Err(err) = iris_daemon::run() {
        eprintln!("iris-daemon: {err}");
        std::process::exit(1);
    }
}
