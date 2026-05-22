use wn_alerts::{Config, Scheduler};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    let mut scheduler = Scheduler::new(config)?;
    scheduler.run().await?;

    Ok(())
}
