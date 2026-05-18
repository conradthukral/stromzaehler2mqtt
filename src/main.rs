mod app;
mod config;
mod mqtt;
mod mqtt_client;
mod parser;
mod serial;

fn main() {
    init_logging();
    let config = config::load_config("config.yaml").expect("invalid config.yaml");
    app::run(config);
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stromzaehler2mqtt=info".into()),
        )
        .init();
}
