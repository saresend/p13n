use actix_cors::Cors;
use actix_web::{App, HttpServer, middleware, web};
use intelligence_engine::api;
use intelligence_engine::engine::{Config, Engine};
use intelligence_engine::model::IdentityModel;
use std::sync::Arc;
use std::time::Duration;

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let defaults = Config::default();
    let config = Config {
        db_path: env_or("DATABASE_PATH", defaults.db_path),
        tick: Duration::from_millis(env_or("TICK_MS", 5_000)),
        min_new_actions: env_or("MIN_NEW_ACTIONS", defaults.min_new_actions),
        ..defaults
    };
    let bind: String = env_or("BIND", "127.0.0.1:8080".to_owned());
    // Browser apps allowed to call the API, e.g. the text editor's dev server.
    let origins: Vec<String> = env_or("CORS_ORIGINS", "http://localhost:5173".to_owned())
        .split(',')
        .map(|origin| origin.trim().to_owned())
        .filter(|origin| !origin.is_empty())
        .collect();

    let engine = Engine::open(config.clone(), Arc::new(IdentityModel)).map_err(|error| {
        std::io::Error::other(format!(
            "could not open {}: {error}",
            config.db_path.display()
        ))
    })?;
    log::info!(
        "listening on http://{bind} (database {}, tick {:?})",
        config.db_path.display(),
        config.tick
    );

    let data = web::Data::from(Arc::clone(&engine));
    let result = HttpServer::new(move || {
        let cors = origins
            .iter()
            .fold(Cors::default(), |cors, origin| cors.allowed_origin(origin))
            .allowed_methods(["GET", "POST"])
            .allowed_header(actix_web::http::header::CONTENT_TYPE)
            .max_age(3600);
        App::new()
            .wrap(middleware::Logger::default())
            .wrap(cors)
            .app_data(data.clone())
            .configure(api::configure)
    })
    .bind(&bind)?
    .run()
    .await;

    engine.shutdown();
    result
}
