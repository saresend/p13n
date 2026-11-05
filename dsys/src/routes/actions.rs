use actix_web::{HttpResponse, delete, get, patch, post, web};
use diesel::prelude::*;
use serde::Deserialize;

use crate::errors::{ApiError, ApiResult};
use crate::models::{Action, ActionChangeset, ActionView, CreateAction, NewAction, UpdateAction};
use crate::schema::action;
use crate::{DbPool, now_ms};

fn require_non_empty(field: &str, v: &str) -> ApiResult<()> {
    if v.trim().is_empty() {
        Err(ApiError::BadRequest(format!("{field} must not be empty")))
    } else {
        Ok(())
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct ActionQuery {
    source_application: Option<String>,
    action_name: Option<String>,
    /// Inclusive lower bound, epoch ms.
    since: Option<i64>,
    /// Exclusive upper bound, epoch ms.
    until: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[post("/actions")]
pub(crate) async fn create_action(
    pool: web::Data<DbPool>,
    body: web::Json<CreateAction>,
) -> ApiResult<HttpResponse> {
    let body = body.into_inner();
    require_non_empty("source_application", &body.source_application)?;
    require_non_empty("action_name", &body.action_name)?;
    let new = NewAction {
        source_application: body.source_application,
        action_name: body.action_name,
        payload: body.payload.to_string(),
        timestamp: body.timestamp.unwrap_or_else(now_ms),
    };
    let created = web::block(move || -> ApiResult<Action> {
        let mut conn = pool.get()?;
        Ok(diesel::insert_into(action::table)
            .values(&new)
            .returning(Action::as_returning())
            .get_result(&mut conn)?)
    })
    .await??;
    Ok(HttpResponse::Created().json(ActionView::from(created)))
}

#[get("/actions")]
pub(crate) async fn list_actions(
    pool: web::Data<DbPool>,
    q: web::Query<ActionQuery>,
) -> ApiResult<HttpResponse> {
    let q = q.into_inner();
    let rows = web::block(move || -> ApiResult<Vec<Action>> {
        let mut conn = pool.get()?;
        let mut query = action::table.select(Action::as_select()).into_boxed();
        if let Some(app) = q.source_application {
            query = query.filter(action::source_application.eq(app));
        }
        if let Some(name) = q.action_name {
            query = query.filter(action::action_name.eq(name));
        }
        if let Some(since) = q.since {
            query = query.filter(action::timestamp.ge(since));
        }
        if let Some(until) = q.until {
            query = query.filter(action::timestamp.lt(until));
        }
        Ok(query
            .order((action::timestamp.desc(), action::id.desc()))
            .limit(q.limit.unwrap_or(100).clamp(1, 1000))
            .offset(q.offset.unwrap_or(0).max(0))
            .load(&mut conn)?)
    })
    .await??;
    Ok(HttpResponse::Ok().json(rows.into_iter().map(ActionView::from).collect::<Vec<_>>()))
}

#[get("/actions/{id}")]
pub(crate) async fn get_action(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
) -> ApiResult<HttpResponse> {
    let id = id.into_inner();
    let row = web::block(move || -> ApiResult<Option<Action>> {
        let mut conn = pool.get()?;
        Ok(action::table
            .find(id)
            .select(Action::as_select())
            .first(&mut conn)
            .optional()?)
    })
    .await??
    .ok_or(ApiError::NotFound("action"))?;
    Ok(HttpResponse::Ok().json(ActionView::from(row)))
}

#[patch("/actions/{id}")]
pub(crate) async fn update_action(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
    body: web::Json<UpdateAction>,
) -> ApiResult<HttpResponse> {
    let id = id.into_inner();
    let body = body.into_inner();
    if let Some(s) = &body.source_application {
        require_non_empty("source_application", s)?;
    }
    if let Some(s) = &body.action_name {
        require_non_empty("action_name", s)?;
    }
    let changes = ActionChangeset {
        source_application: body.source_application,
        action_name: body.action_name,
        payload: body.payload.map(|p| p.to_string()),
        timestamp: body.timestamp,
    };
    let is_empty = changes.source_application.is_none()
        && changes.action_name.is_none()
        && changes.payload.is_none()
        && changes.timestamp.is_none();
    if is_empty {
        return Err(ApiError::BadRequest("no fields to update".into()));
    }
    let updated = web::block(move || -> ApiResult<Option<Action>> {
        let mut conn = pool.get()?;
        Ok(diesel::update(action::table.find(id))
            .set(&changes)
            .returning(Action::as_returning())
            .get_result(&mut conn)
            .optional()?)
    })
    .await??
    .ok_or(ApiError::NotFound("action"))?;
    Ok(HttpResponse::Ok().json(ActionView::from(updated)))
}

#[delete("/actions/{id}")]
pub(crate) async fn delete_action(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
) -> ApiResult<HttpResponse> {
    let id = id.into_inner();
    let n = web::block(move || -> ApiResult<usize> {
        let mut conn = pool.get()?;
        Ok(diesel::delete(action::table.find(id)).execute(&mut conn)?)
    })
    .await??;
    if n == 0 {
        return Err(ApiError::NotFound("action"));
    }
    Ok(HttpResponse::NoContent().finish())
}

pub(crate) fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create_action)
        .service(list_actions)
        .service(get_action)
        .service(update_action)
        .service(delete_action);
}
