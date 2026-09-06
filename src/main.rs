use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("ktk-schedule {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args == ["--help"] {
        println!(
            "ktk-schedule [--check | --version]\n\n--check validates configuration and initializes SQLite without connecting to Telegram."
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.is_empty() || args == ["--check"],
        "unknown arguments; use --help"
    );
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ktk_schedule=info,teloxide=warn".into()),
        )
        .init();
    let config = ktk_schedule::config::Config::load()?;
    if args == ["--check"] {
        let cipher = ktk_schedule::credentials::Cipher::new(&config.secret)?;
        let storage = ktk_schedule::storage::Storage::open(&config.database_path, cipher).await?;
        storage.close().await;
        println!("Configuration and SQLite are ready.");
        return Ok(());
    }
    ktk_schedule::app::run(config).await
}
