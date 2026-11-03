use std::{env, error::Error, net::SocketAddr, time::Duration};

use moby_api::{AccessKeys, MIGRATOR, config::Config, openapi, router_with_usage};
use sqlx::postgres::PgPoolOptions;
use tokio::{net::TcpListener, signal};
use tracing::{Level, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    match args.next() {
        Some(command) if command == "openapi" && args.next().is_none() => {
            println!("{}", serde_json::to_string_pretty(&openapi())?);
            return Ok(());
        }
        Some(command) if command == "auth-public-key" && args.next().is_none() => {
            let seed = env::var("MOBY_AUTH_SIGNING_SEED")
                .map_err(|_| "MOBY_AUTH_SIGNING_SEED is required")?;
            println!("{}", AccessKeys::public_key_from_seed(&seed)?);
            return Ok(());
        }
        Some(_) => return Err("usage: moby-api [openapi|auth-public-key]".into()),
        None => {}
    }

    let config = Config::from_env()?;
    tracing_subscriber::fmt()
        .json()
        .with_max_level(Level::INFO)
        .init();

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy_with(config.database);
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|_| "database migration failed")?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    info!(address = %listener.local_addr()?, "API listening");

    axum::serve(
        listener,
        router_with_usage(
            pool,
            config.accounts,
            config.access_keys,
            config.google,
            config.usage,
        )
        .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            result = signal::ctrl_c() => result.expect("install Ctrl+C handler"),
            _ = terminate.recv() => {},
        }
    }

    #[cfg(not(unix))]
    signal::ctrl_c().await.expect("install Ctrl+C handler");
}
