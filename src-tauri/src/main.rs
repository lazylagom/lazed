fn main() {
    // Safety: in dev the app binary shares the daemon's name and sits in
    // the resource dir, so a bad lazed_bin() pick can spawn US as
    // `lazed server|api|term attach`. Booting the GUI here would fork-bomb
    // (every spawn opens attach streams that spawn more). Forward to the
    // real daemon binary instead.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(String::as_str),
        Some("server" | "api" | "term" | "status" | "stop")
    ) {
        let home = std::env::var("HOME").unwrap_or_default();
        let real = std::path::Path::new(&home).join(".local/bin/lazed");
        let real = if real.is_file() {
            real
        } else {
            std::path::PathBuf::from("lazed")
        };
        let status = std::process::Command::new(real)
            .args(&args)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or(1);
        std::process::exit(status);
    }
    lazed_lib::run()
}
