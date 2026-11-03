use crate::errors::PError;

use crate::schema::{action, session, user};

use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::result::QueryResult;
use futures::{Stream, stream};
use std::convert::Infallible;
use tokio::sync::broadcast::error::RecvError;
use user::dsl::*;

use crate::models::{NewSession, Session, User};

use crate::AppState;
use crate::load_db_connection;
use crate::models::{Action, NewAction, TransportAction};
use crate::suggestions;
use actix_web::web::Bytes;
use actix_web::{HttpResponse, Responder, get, post, web};
use diesel::dsl::insert_into;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::Receiver;

// One SSE event: a `data:` line holding the JSON, then a blank line to end the event
fn sse_frame(actions: &[TransportAction]) -> Bytes {
    let json = serde_json::to_string(actions).unwrap();
    Bytes::from(format!("data: {json}\n\n"))
}

fn sse_stream(
    receiver: Receiver<Vec<TransportAction>>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream::unfold(receiver, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(actions) => return Some((Ok(sse_frame(&actions)), rx)),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    })
}

// Whether the session exists and belongs to the user in the path
fn session_owned_by(
    db_conn: &mut SqliteConnection,
    user_id: i32,
    sess_id: i32,
) -> QueryResult<bool> {
    let found = session::table
        .filter(session::id.eq(sess_id))
        .filter(session::user_id.eq(user_id))
        .first::<Session>(db_conn)
        .optional()?;
    Ok(found.is_some())
}

#[get("/user/{user_id}/session/{session_id}/suggestions/sse")]
async fn suggestions_stream(
    app_state: web::Data<AppState>,
    path: web::Path<(i32, i32)>,
) -> impl Responder {
    let (user_id, sess_id) = path.into_inner();
    let mut db_conn = load_db_connection().await;

    match session_owned_by(&mut db_conn, user_id, sess_id) {
        Ok(true) => {}
        Ok(false) => return HttpResponse::NotFound().body("Session not found"),
        Err(e) => return HttpResponse::from_error(PError::DBError(e)),
    }

    let receiver = app_state.create_receiver_for_session(sess_id).await;

    match receiver {
        Some(rx) => HttpResponse::Ok()
            .append_header(("Content-Type", "text/event-stream"))
            .streaming(sse_stream(rx)),
        None => HttpResponse::NotFound().body("Session not found"),
    }
}

#[post("/user")]
async fn create_user(user_input: web::Json<User>) -> impl Responder {
    let mut db_con = load_db_connection().await;
    let user_input = user_input.0;

    let res = insert_into(user).values(&user_input).execute(&mut db_con);
    if let Err(e) = res {
        HttpResponse::from_error(PError::DBError(e))
    } else {
        HttpResponse::Ok().json(user_input)
    }
}

#[post("/user/{user_id}/session")]
async fn create_user_session(
    app_state: web::Data<AppState>,
    path: web::Path<i32>,
) -> impl Responder {
    let mut db_con = load_db_connection().await;
    let user_id = path.into_inner();
    let results = user::table
        .filter(user::id.eq(user_id))
        .load::<User>(&mut db_con);
    match results {
        Ok(data) => {
            if data.is_empty() {
                HttpResponse::NotFound().body("Not Found")
            } else {
                let date = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i32;

                let new_session = NewSession { user_id, date };

                let inserted = new_session
                    .insert_into(session::table)
                    .returning(Session::as_returning())
                    .get_result(&mut db_con);

                match inserted {
                    Ok(session) => {
                        app_state.create_sender_for_session(session.id).await;
                        HttpResponse::Ok().json(session)
                    }
                    Err(e) => HttpResponse::from_error(PError::DBError(e)),
                }
            }
        }
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}

#[get("/user/{user_id}/session")]
async fn list_user_sessions(path: web::Path<i32>) -> impl Responder {
    let mut db_conn = load_db_connection().await;
    let user_id = path.into_inner();

    let sessions = session::table
        .filter(session::user_id.eq(user_id))
        .load::<Session>(&mut db_conn);

    match sessions {
        Ok(data) => HttpResponse::Ok().json(data),
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}

#[post("/user/{user_id}/session/{session_id}/action")]
async fn create_action(
    path: web::Path<(i32, i32)>,
    data: web::Json<TransportAction>,
    app_state: web::Data<AppState>,
) -> impl Responder {
    let mut db_conn = load_db_connection().await;
    let (user_id, sess_id) = path.into_inner();

    let sessions = session::table
        .filter(session::id.eq(sess_id))
        .filter(session::user_id.eq(user_id))
        .load::<Session>(&mut db_conn);

    match sessions {
        Ok(session_list) => {
            if session_list.is_empty() {
                HttpResponse::NotFound().body("Session not found")
            } else {
                // here we need to create a new action
                let string_payload = data.payload.to_string();
                let action_data = NewAction {
                    session_id: sess_id,
                    action_id: &data.action_id,
                    payload: &string_payload,
                };
                let result_value = action_data.clone();
                let db_result = action_data.insert_into(action::table).execute(&mut db_conn);

                match db_result {
                    Ok(_) => {
                        // Generate in the background so the POST returns without waiting on the model
                        if let Some(sender) = app_state.get_sender_for_session(sess_id).await {
                            let session = session_list[0].clone();
                            tokio::spawn(async move {
                                if let Err(e) = suggestions::try_generate(sender, session).await {
                                    eprintln!("suggestion generation failed: {e}");
                                }
                            });
                        }
                        HttpResponse::Ok().json(result_value)
                    }
                    Err(e) => HttpResponse::from_error(PError::DBError(e)),
                }
            }
        }
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}

#[get("/user/{user_id}/session/{session_id}/action")]
async fn list_actions(path: web::Path<(i32, i32)>) -> impl Responder {
    let (user_id, sess_id) = path.into_inner();
    let mut db_conn = load_db_connection().await;

    match session_owned_by(&mut db_conn, user_id, sess_id) {
        Ok(true) => {}
        Ok(false) => return HttpResponse::NotFound().body("Session not found"),
        Err(e) => return HttpResponse::from_error(PError::DBError(e)),
    }

    let db_result = action::table
        .filter(action::session_id.eq(sess_id))
        .load::<Action>(&mut db_conn);
    match db_result {
        Ok(data) => HttpResponse::Ok().json(&data),
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}
