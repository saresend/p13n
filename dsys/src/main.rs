use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use actix_web::{App, HttpResponse, HttpServer, middleware::Logger, web};
use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use dotenv::dotenv;

mod errors;
mod models;
mod routes;
mod schema;
mod storage;

use errors::ApiError;
use storage::TaskStorage;

pub(crate) type DbPool = Pool<ConnectionManager<SqliteConnection>>;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Hand-maintained API description; keep in sync with `routes/`.
const OPENAPI_SPEC: &str = include_str!("../openapi.json");

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Per-connection SQLite settings: wait on locks instead of failing, WAL for
/// concurrent readers, and enforce foreign keys.
#[derive(Debug)]
struct SqlitePragmas;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        diesel::sql_query(
            "PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;",
        )
        .execute(conn)
        .map(|_| ())
        .map_err(diesel::r2d2::Error::QueryError)
    }
}

fn build_pool(database_url: &str) -> DbPool {
    if let Some(parent) = std::path::Path::new(database_url).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).expect("creating database directory");
        }
    }
    let pool = Pool::builder()
        .connection_customizer(Box::new(SqlitePragmas))
        .build(ConnectionManager::<SqliteConnection>::new(database_url))
        .expect("building sqlite connection pool");
    pool.get()
        .expect("connecting to sqlite")
        .run_pending_migrations(MIGRATIONS)
        .expect("running migrations");
    pool
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "data/dsys.sqlite".into());
    let tasks_dir = env::var("DSYS_TASKS_DIR").unwrap_or_else(|_| "data/tasks".into());
    let bind = env::var("DSYS_BIND").unwrap_or_else(|_| "127.0.0.1:8081".into());

    let pool = build_pool(&database_url);
    let storage = TaskStorage::new(&tasks_dir)?;
    log::info!("db: {database_url}, task files: {}", storage.root().display());
    log::info!("listening on http://{bind}");

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(storage.clone()))
            .app_data(web::JsonConfig::default().error_handler(|err, _| {
                actix_web::error::InternalError::from_response(
                    err.to_string(),
                    actix_web::ResponseError::error_response(&ApiError::BadRequest(err.to_string())),
                )
                .into()
            }))
            .route("/health", web::get().to(|| async { HttpResponse::Ok().body("ok") }))
            .route(
                "/openapi.json",
                web::get().to(|| async {
                    HttpResponse::Ok().content_type("application/json").body(OPENAPI_SPEC)
                }),
            )
            .configure(routes::actions::configure)
            .configure(routes::tasks::configure)
    })
    .bind(bind)?
    .run()
    .await
}

#[cfg(test)]
mod tests {
    use super::OPENAPI_SPEC;

    #[test]
    fn openapi_spec_is_valid_json_and_lists_every_route() {
        let spec: serde_json::Value = serde_json::from_str(OPENAPI_SPEC).expect("openapi.json parses");
        let paths = spec["paths"].as_object().expect("paths object");
        for (path, methods) in [
            ("/health", &["get"][..]),
            ("/openapi.json", &["get"]),
            ("/actions", &["get", "post"]),
            ("/actions/{id}", &["get", "patch", "delete"]),
            ("/tasks", &["get", "post"]),
            ("/tasks/{id}", &["get", "patch", "delete"]),
            ("/tasks/{id}/description", &["get", "put"]),
            ("/tasks/{id}/log", &["get", "post", "put"]),
        ] {
            for m in methods {
                assert!(paths[path].get(*m).is_some(), "missing {m} {path}");
            }
        }
    }
}
