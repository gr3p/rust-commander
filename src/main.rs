mod ui;

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    ui::run(smoke);
}
