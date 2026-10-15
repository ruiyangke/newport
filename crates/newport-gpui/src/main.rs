fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--check-backend") {
        let value =
            newport_client::backend::Backend::discover().call("snapshot", serde_json::json!({}))?;
        println!(
            "Backend connected: {} server(s), {} tunnel(s)",
            value["config"]["servers"].as_array().map_or(0, Vec::len),
            value["config"]["tunnels"].as_array().map_or(0, Vec::len)
        );
        return Ok(());
    }
    newport_gpui::run()
}
