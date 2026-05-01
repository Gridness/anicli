#[tokio::main]
async fn main() -> eyre::Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "anicli {} - Ratatui anime browser/player",
                    env!("CARGO_PKG_VERSION")
                );
                println!("Run without arguments to start the TUI.");
                println!("Environment variables follow ani-cli names where applicable.");
                return Ok(());
            }
            _ => {}
        }
    }
    anicli_tui::run().await
}
