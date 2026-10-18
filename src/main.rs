mod ui;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--mount-helper") {
        std::process::exit(rust_commander::mount::helper());
    }
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    ui::run(smoke);
}
