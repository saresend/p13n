use crate::errors::PError;

use crate::schema::{action, session, user};

use diesel::prelude::*;
use user::dsl::*;

use crate::models::{NewSession, Session, User};

use crate::load_db_connection;
use crate::models::{Action, NewAction, TransportAction};
use actix_web::{HttpResponse, Responder, get, post, web};
use diesel::dsl::insert_into;
use std::time::{SystemTime, UNIX_EPOCH};

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
async fn create_user_session(path: web::Path<i32>) -> impl Responder {
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
                let new_session = NewSession {
                    user_id,
                    date: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i32,
                };
                match new_session.insert_into(session::table).execute(&mut db_con) {
                    Ok(_) => HttpResponse::Ok().body("Ok"), // TODO: Load session object and return
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
                    Ok(_) => HttpResponse::Ok().json(result_value),
                    Err(e) => HttpResponse::from_error(PError::DBError(e)),
                }
            }
        }
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}

#[get("/user/{user_id}/session/{session_id}/action")]
async fn list_actions(path: web::Path<(i32, i32)>) -> impl Responder {
    let (_, sess_id) = path.into_inner();
    let mut db_conn = load_db_connection().await;

    let db_result = action::table
        .filter(action::session_id.eq(sess_id))
        .load::<Action>(&mut db_conn);
    match db_result {
        Ok(data) => HttpResponse::Ok().json(&data),
        Err(e) => HttpResponse::from_error(PError::DBError(e)),
    }
}
