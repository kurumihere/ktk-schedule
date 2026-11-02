use anyhow::Result;
use std::io::IsTerminal;

const HELP: &str = "\
ktk-schedule [--check | --version | --help]

Telegram and VK schedule bot for KTK. Settings come from the environment (.env in the
working directory for local runs) and secrets also from systemd credentials.

  --check    validate the configuration, open and migrate SQLite, and verify that
             CREDENTIALS_SECRET reads the saved sessions; nothing is sent anywhere
  --version  print the version";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" => {
            println!("ktk-schedule {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        [flag] if flag == "--help" => {
            println!("{HELP}");
            return Ok(());
        }
        [] => (),
        [flag] if flag == "--check" => (),
        _ => anyhow::bail!("unknown arguments; use --help"),
    }
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ktk_schedule=info,teloxide=warn".into()),
        )
        // journald stores raw bytes: colors only for a human at a terminal.
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let config = ktk_schedule::config::Config::load()?;
    if !args.is_empty() {
        let cipher = ktk_schedule::credentials::Cipher::new(&config.secret)?;
        let storage = ktk_schedule::storage::Storage::open(&config.database_path, cipher).await?;
        let checked = storage.check_secret().await;
        storage.close().await;
        checked?;
        println!("Configuration and SQLite are ready.");
        return Ok(());
    }
    ktk_schedule::app::run(config).await
}
