use actix_web::{App, HttpServer};
use diesel::SqliteConnection;
use diesel::prelude::*;
use dotenv::dotenv;
use std::env;

pub mod errors;
pub mod models;
pub mod routes;
pub mod schema;

use routes::{create_action, create_user, create_user_session, list_actions, list_user_sessions};

pub async fn load_db_connection() -> SqliteConnection {
    dotenv().ok();
    let db_url = env::var("DATABASE_URL").expect("Need a database to load from");
    SqliteConnection::establish(&db_url).unwrap_or_else(|_| panic!("Couldn't connect to db"))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .service(create_user)
            .service(create_user_session)
            .service(list_user_sessions)
            .service(create_action)
            .service(list_actions)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
