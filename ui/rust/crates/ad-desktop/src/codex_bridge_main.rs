//! Headless stdio MCP companion. No GUI, model credentials or SSH keys.
mod codex_protocol;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::var_os("AETHERDESK_CONNECTOR_DIRECTORY")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA").map(|p| std::path::PathBuf::from(p).join("AetherDesk-Rust"))
        });
    let result = match root {
        Some(root) => codex_protocol::run_stdio(root).await,
        None => Err("Application data directory is unavailable".into()),
    };
    if let Err(error) = result {
        eprintln!("AetherDesk connector: {error}");
        std::process::exit(1);
    }
}
