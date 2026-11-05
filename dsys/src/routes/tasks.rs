use actix_web::{HttpResponse, delete, get, patch, post, put, web};
use diesel::prelude::*;
use serde::Deserialize;

use crate::errors::{ApiError, ApiResult};
use crate::models::{CreateTask, NewTask, Task, UpdateTask, validate_status};
use crate::schema::tasks;
use crate::storage::TaskStorage;
use crate::{DbPool, now_ms};

const MARKDOWN: &str = "text/markdown; charset=utf-8";

fn load_task(conn: &mut SqliteConnection, id: i32) -> ApiResult<Task> {
    tasks::table
        .find(id)
        .select(Task::as_select())
        .first(conn)
        .optional()?
        .ok_or(ApiError::NotFound("task"))
}

/// Run blocking DB + filesystem work for one task off the async executor.
async fn with_task<T, F>(pool: web::Data<DbPool>, id: i32, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&mut SqliteConnection, Task) -> ApiResult<T> + Send + 'static,
{
    web::block(move || {
        let mut conn = pool.get()?;
        let task = load_task(&mut conn, id)?;
        f(&mut conn, task)
    })
    .await?
}

fn body_text(body: web::Bytes) -> ApiResult<String> {
    String::from_utf8(body.to_vec()).map_err(|_| ApiError::BadRequest("body must be UTF-8 text".into()))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
pub(crate) struct TaskQuery {
    status: Option<String>,
    model: Option<String>,
}

#[post("/tasks")]
pub(crate) async fn create_task(
    pool: web::Data<DbPool>,
    storage: web::Data<TaskStorage>,
    body: web::Json<CreateTask>,
) -> ApiResult<HttpResponse> {
    let body = body.into_inner();
    if body.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    let task = web::block(move || -> ApiResult<Task> {
        let files = storage.create(&body.title, &body.description)?;
        let new = NewTask {
            title: body.title,
            status: "Available".into(),
            task_description_path: files.description.to_string_lossy().into_owned(),
            progress_log_path: files.progress_log.to_string_lossy().into_owned(),
            model: body.model,
        };
        let inserted = pool.get().map_err(ApiError::from).and_then(|mut conn| {
            Ok(diesel::insert_into(tasks::table)
                .values(&new)
                .returning(Task::as_returning())
                .get_result(&mut conn)?)
        });
        if inserted.is_err() {
            // Don't leave an orphaned folder behind.
            let _ = storage.remove_task_dir(&new.task_description_path);
        }
        inserted
    })
    .await??;
    Ok(HttpResponse::Created().json(task))
}

#[get("/tasks")]
pub(crate) async fn list_tasks(
    pool: web::Data<DbPool>,
    q: web::Query<TaskQuery>,
) -> ApiResult<HttpResponse> {
    let q = q.into_inner();
    if let Some(s) = &q.status {
        validate_status(s)?;
    }
    let rows = web::block(move || -> ApiResult<Vec<Task>> {
        let mut conn = pool.get()?;
        let mut query = tasks::table.select(Task::as_select()).into_boxed();
        if let Some(s) = q.status {
            query = query.filter(tasks::status.eq(s));
        }
        if let Some(m) = q.model {
            query = query.filter(tasks::model.eq(m));
        }
        Ok(query.order(tasks::id.asc()).load(&mut conn)?)
    })
    .await??;
    Ok(HttpResponse::Ok().json(rows))
}

#[get("/tasks/{id}")]
pub(crate) async fn get_task(pool: web::Data<DbPool>, id: web::Path<i32>) -> ApiResult<HttpResponse> {
    let task = with_task(pool, id.into_inner(), |_, t| Ok(t)).await?;
    Ok(HttpResponse::Ok().json(task))
}

/// Partial update. When `status` is given, `expected_status` (query param)
/// makes the change conditional, e.g. `?expected_status=Available` to claim a
/// task only if nobody else has; a mismatch returns 409.
#[derive(Deserialize, Debug)]
pub(crate) struct UpdateQuery {
    expected_status: Option<String>,
}

#[patch("/tasks/{id}")]
pub(crate) async fn update_task(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
    q: web::Query<UpdateQuery>,
    body: web::Json<UpdateTask>,
) -> ApiResult<HttpResponse> {
    let id = id.into_inner();
    let mut changes = body.into_inner();
    let expected = q.into_inner().expected_status;
    if let Some(s) = &changes.status {
        validate_status(s)?;
        // Leaving In-Progress clears the claim, as kyogre does.
        if s != "In-Progress" {
            changes.taken_by_pid.get_or_insert(0);
            changes.taken_by_start.get_or_insert(0);
        }
    }
    if let Some(s) = &expected {
        validate_status(s)?;
    }
    if changes.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    let empty = changes.title.is_none()
        && changes.status.is_none()
        && changes.model.is_none()
        && changes.taken_by_pid.is_none()
        && changes.taken_by_start.is_none();
    if empty {
        return Err(ApiError::BadRequest("no fields to update".into()));
    }

    let task = web::block(move || -> ApiResult<Task> {
        let mut conn = pool.get()?;
        let updated = match &expected {
            Some(exp) => diesel::update(tasks::table.find(id).filter(tasks::status.eq(exp)))
                .set(&changes)
                .returning(Task::as_returning())
                .get_result(&mut conn)
                .optional()?,
            None => diesel::update(tasks::table.find(id))
                .set(&changes)
                .returning(Task::as_returning())
                .get_result(&mut conn)
                .optional()?,
        };
        match updated {
            Some(t) => Ok(t),
            None => {
                let current = load_task(&mut conn, id)?;
                Err(ApiError::Conflict(format!(
                    "task {id} has status {}, expected {}",
                    current.status,
                    expected.unwrap_or_default()
                )))
            }
        }
    })
    .await??;
    Ok(HttpResponse::Ok().json(task))
}

#[derive(Deserialize, Debug)]
pub(crate) struct DeleteQuery {
    /// Keep description.md / progress_log.md on disk.
    #[serde(default)]
    keep_files: bool,
}

#[delete("/tasks/{id}")]
pub(crate) async fn delete_task(
    pool: web::Data<DbPool>,
    storage: web::Data<TaskStorage>,
    id: web::Path<i32>,
    q: web::Query<DeleteQuery>,
) -> ApiResult<HttpResponse> {
    let keep = q.keep_files;
    with_task(pool, id.into_inner(), move |conn, task| {
        diesel::delete(tasks::table.find(task.id)).execute(conn)?;
        if !keep {
            storage.remove_task_dir(&task.task_description_path)?;
        }
        Ok(())
    })
    .await?;
    Ok(HttpResponse::NoContent().finish())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[get("/tasks/{id}/description")]
pub(crate) async fn read_description(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
) -> ApiResult<HttpResponse> {
    let text = with_task(pool, id.into_inner(), |_, t| TaskStorage::read(&t.task_description_path)).await?;
    Ok(HttpResponse::Ok().content_type(MARKDOWN).body(text))
}

#[put("/tasks/{id}/description")]
pub(crate) async fn write_description(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
    body: web::Bytes,
) -> ApiResult<HttpResponse> {
    let text = body_text(body)?;
    with_task(pool, id.into_inner(), move |_, t| TaskStorage::write(&t.task_description_path, &text)).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[get("/tasks/{id}/log")]
pub(crate) async fn read_log(pool: web::Data<DbPool>, id: web::Path<i32>) -> ApiResult<HttpResponse> {
    let text = with_task(pool, id.into_inner(), |_, t| TaskStorage::read(&t.progress_log_path)).await?;
    Ok(HttpResponse::Ok().content_type(MARKDOWN).body(text))
}

/// Append a dated entry to the progress log.
#[post("/tasks/{id}/log")]
pub(crate) async fn append_log(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
    body: web::Bytes,
) -> ApiResult<HttpResponse> {
    let text = body_text(body)?;
    if text.trim().is_empty() {
        return Err(ApiError::BadRequest("log entry must not be empty".into()));
    }
    let ts = now_ms();
    with_task(pool, id.into_inner(), move |_, t| TaskStorage::append_log(&t.progress_log_path, &text, ts))
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

/// Replace the whole progress log.
#[put("/tasks/{id}/log")]
pub(crate) async fn write_log(
    pool: web::Data<DbPool>,
    id: web::Path<i32>,
    body: web::Bytes,
) -> ApiResult<HttpResponse> {
    let text = body_text(body)?;
    with_task(pool, id.into_inner(), move |_, t| TaskStorage::write(&t.progress_log_path, &text)).await?;
    Ok(HttpResponse::NoContent().finish())
}

pub(crate) fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create_task)
        .service(list_tasks)
        .service(get_task)
        .service(update_task)
        .service(delete_task)
        .service(read_description)
        .service(write_description)
        .service(read_log)
        .service(append_log)
        .service(write_log);
}
