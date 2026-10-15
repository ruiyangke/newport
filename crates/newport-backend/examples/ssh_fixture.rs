//! Loopback-only adverse SSH fixture, using the same Rust SSH dependencies as the app.
#[cfg(unix)]
#[path = "ssh_fixture/server.rs"]
mod server;
#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::run().await
}
#[cfg(not(unix))]
fn main() {
    panic!("The PTY fixture requires Unix");
}
