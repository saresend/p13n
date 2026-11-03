use actix_web::{App, HttpServer, web};
use diesel::SqliteConnection;
use diesel::prelude::*;
use dotenv::dotenv;
use futures::lock::Mutex;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::broadcast::{Receiver, Sender};

pub mod errors;
pub mod models;
pub mod routes;
pub mod schema;
pub mod suggestions;

use routes::{
    create_action, create_user, create_user_session, list_actions, list_user_sessions,
    suggestions_stream,
};

use crate::models::TransportAction;

pub async fn load_db_connection() -> SqliteConnection {
    dotenv().ok();
    let db_url = env::var("DATABASE_URL").expect("Need a database to load from");
    SqliteConnection::establish(&db_url).unwrap_or_else(|_| panic!("Couldn't connect to db"))
}

#[derive(Clone)]
pub(crate) struct AppState {
    channel_capacity: usize,
    channels: Arc<Mutex<HashMap<i32, Sender<Vec<TransportAction>>>>>,
}

impl AppState {
    pub async fn create_receiver_for_session(
        &self,
        session_id: i32,
    ) -> Option<Receiver<Vec<TransportAction>>> {
        let inner = self.channels.lock().await;
        match inner.get(&session_id) {
            Some(t) => Some(t.subscribe()),
            None => None,
        }
    }

    pub async fn get_sender_for_session(
        &self,
        session_id: i32,
    ) -> Option<Sender<Vec<TransportAction>>> {
        let inner = self.channels.lock().await;
        match inner.get(&session_id) {
            Some(t) => Some(t.clone()),
            None => None,
        }
    }
    pub async fn create_sender_for_session(&self, session_id: i32) {
        let mut inner = self.channels.lock().await;
        inner.insert(
            session_id,
            tokio::sync::broadcast::Sender::new(self.channel_capacity),
        );
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let global_app_state = AppState {
        channel_capacity: 10,
        channels: Arc::new(Mutex::new(HashMap::new())),
    };
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(global_app_state.clone()))
            .service(create_user)
            .service(create_user_session)
            .service(list_user_sessions)
            .service(create_action)
            .service(list_actions)
            .service(suggestions_stream)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
