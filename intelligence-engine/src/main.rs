use actix_web::{App, HttpResponse, HttpServer, Responder, get};
use diesel::SqliteConnection;
use diesel::prelude::*;
use dotenv::dotenv;
use std::env;

pub mod models;
pub mod schema;

pub async fn load_db_connection_pool() -> SqliteConnection {
    dotenv().ok();
    let db_url = env::var("DATABASE_URL").expect("Need a database to load from");
    SqliteConnection::establish(&db_url).unwrap_or_else(|_| panic!("Couldn't connect to db"))
}

#[get("/")]
async fn hello() -> impl Responder {
    HttpResponse::Ok().body("Hello world")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| App::new().service(hello))
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
