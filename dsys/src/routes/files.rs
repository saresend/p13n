use actix_web::http::header::{self, HeaderMap};
use actix_web::{HttpRequest, HttpResponse, delete, get, post, put, web};
use serde::Deserialize;

use crate::errors::{ApiError, ApiResult};
use crate::files::{FileRoot, WriteCondition};

#[derive(Deserialize, Debug)]
pub(crate) struct PathQuery {
    path: String,
}

#[derive(Deserialize, Debug)]
pub(crate) struct TreeQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    include_content: bool,
    #[serde(default)]
    include_hidden: bool,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DeleteDirQuery {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize, Debug)]
pub(crate) struct MoveBody {
    from: String,
    to: String,
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string())
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
        Some("md" | "markdown") => "text/markdown; charset=utf-8",
        Some("json") => "application/json",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

#[get("/fs/tree")]
pub(crate) async fn tree(files: web::Data<FileRoot>, q: web::Query<TreeQuery>) -> ApiResult<HttpResponse> {
    let q = q.into_inner();
    let entry = web::block(move || files.tree(&q.path, q.include_content, q.include_hidden)).await??;
    Ok(HttpResponse::Ok().json(entry))
}

#[get("/fs/file")]
pub(crate) async fn read_file(files: web::Data<FileRoot>, q: web::Query<PathQuery>) -> ApiResult<HttpResponse> {
    let path = q.into_inner().path;
    let ct_path = path.clone();
    let (bytes, etag) = web::block(move || files.read_file(&path)).await??;
    let ct = if std::str::from_utf8(&bytes).is_ok() {
        content_type_for(&ct_path)
    } else {
        "application/octet-stream"
    };
    Ok(HttpResponse::Ok().content_type(ct).insert_header((header::ETAG, etag)).body(bytes))
}

/// Create or replace a file. `If-None-Match: *` makes it create-only;
/// `If-Match: <etag>` makes it replace-only-if-unchanged. Both fail with 412.
#[put("/fs/file")]
pub(crate) async fn write_file(
    req: HttpRequest,
    files: web::Data<FileRoot>,
    q: web::Query<PathQuery>,
    body: web::Bytes,
) -> ApiResult<HttpResponse> {
    let path = q.into_inner().path;
    let cond = WriteCondition {
        if_match: header_str(req.headers(), header::IF_MATCH),
        if_none_match_any: header_str(req.headers(), header::IF_NONE_MATCH).as_deref() == Some("*"),
    };
    let (entry, created) = web::block(move || files.write_file(&path, &body, &cond)).await??;
    let mut resp = if created { HttpResponse::Created() } else { HttpResponse::Ok() };
    if let Some(etag) = &entry.etag {
        resp.insert_header((header::ETAG, etag.clone()));
    }
    Ok(resp.json(entry))
}

#[delete("/fs/file")]
pub(crate) async fn delete_file(
    req: HttpRequest,
    files: web::Data<FileRoot>,
    q: web::Query<PathQuery>,
) -> ApiResult<HttpResponse> {
    let path = q.into_inner().path;
    let if_match = header_str(req.headers(), header::IF_MATCH);
    web::block(move || files.delete_file(&path, if_match.as_deref())).await??;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/fs/dir")]
pub(crate) async fn create_dir(files: web::Data<FileRoot>, q: web::Query<PathQuery>) -> ApiResult<HttpResponse> {
    let path = q.into_inner().path;
    let (entry, created) = web::block(move || files.create_dir(&path)).await??;
    Ok(if created { HttpResponse::Created() } else { HttpResponse::Ok() }.json(entry))
}

#[delete("/fs/dir")]
pub(crate) async fn delete_dir(files: web::Data<FileRoot>, q: web::Query<DeleteDirQuery>) -> ApiResult<HttpResponse> {
    let q = q.into_inner();
    web::block(move || files.delete_dir(&q.path, q.recursive)).await??;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/fs/move")]
pub(crate) async fn move_entry(files: web::Data<FileRoot>, body: web::Json<MoveBody>) -> ApiResult<HttpResponse> {
    let MoveBody { from, to } = body.into_inner();
    if from.trim().is_empty() || to.trim().is_empty() {
        return Err(ApiError::BadRequest("from and to are required".into()));
    }
    let entry = web::block(move || files.rename(&from, &to)).await??;
    Ok(HttpResponse::Ok().json(entry))
}

pub(crate) fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(tree)
        .service(read_file)
        .service(write_file)
        .service(delete_file)
        .service(create_dir)
        .service(delete_dir)
        .service(move_entry);
}
