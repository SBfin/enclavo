use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{Shared, billing, error::ApiError};

pub fn file_object(id: &str, purpose: &str, filename: &str, bytes: i64, created_at: DateTime<Utc>) -> Value {
    json!({
        "id": id,
        "object": "file",
        "bytes": bytes,
        "created_at": super::unix(created_at),
        "filename": filename,
        "purpose": purpose,
        "status": "processed",
    })
}

pub async fn upload(
    State(state): State<Shared>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let mut purpose = None;
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Some(field) =
        form.next_field().await.map_err(|e| ApiError::bad_request(format!("Invalid upload: {e}")))?
    {
        match field.name() {
            Some("purpose") => purpose = Some(field.text().await.map_err(|e| ApiError::bad_request(e.to_string()))?),
            Some("file") => {
                let name = field.file_name().unwrap_or("upload.jsonl").to_string();
                let data =
                    field.bytes().await.map_err(|e| ApiError::bad_request(format!("Could not read the file: {e}")))?;
                file = Some((name, data.to_vec()));
            }
            _ => {}
        }
    }
    let purpose = purpose.ok_or_else(|| ApiError::bad_request("`purpose` is required."))?;
    if purpose != "batch" {
        return Err(ApiError::bad_request("Only `purpose=batch` files are supported."));
    }
    let (filename, content) = file.ok_or_else(|| ApiError::bad_request("`file` is required."))?;
    if content.len() > state.cfg.batch.max_file_bytes {
        return Err(ApiError::bad_request(format!("Files are limited to {} bytes.", state.cfg.batch.max_file_bytes)));
    }
    let id = format!("file-{}", Uuid::new_v4().simple());
    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO files (id, account_id, purpose, filename, bytes, content) VALUES ($1, $2, $3, $4, $5, $6) RETURNING created_at",
    )
    .bind(&id)
    .bind(account.id)
    .bind(&purpose)
    .bind(&filename)
    .bind(content.len() as i64)
    .bind(&content)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(file_object(&id, &purpose, &filename, content.len() as i64, created_at)))
}

pub async fn list(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let rows = sqlx::query(
        "SELECT id, purpose, filename, bytes, created_at FROM files WHERE account_id = $1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(account.id)
    .fetch_all(&state.db)
    .await?;
    let data = rows
        .iter()
        .map(|r| {
            Ok(file_object(
                r.try_get("id")?,
                r.try_get("purpose")?,
                r.try_get("filename")?,
                r.try_get("bytes")?,
                r.try_get("created_at")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Json(json!({ "object": "list", "data": data })))
}

pub async fn retrieve(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let row = sqlx::query("SELECT purpose, filename, bytes, created_at FROM files WHERE id = $1 AND account_id = $2")
        .bind(&id)
        .bind(account.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("No file with id {id}.")))?;
    Ok(Json(file_object(
        &id,
        row.try_get("purpose")?,
        row.try_get("filename")?,
        row.try_get("bytes")?,
        row.try_get("created_at")?,
    )))
}

pub async fn content(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let content: Vec<u8> = sqlx::query_scalar("SELECT content FROM files WHERE id = $1 AND account_id = $2")
        .bind(&id)
        .bind(account.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("No file with id {id}.")))?;
    let mut response = content.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/jsonl"));
    Ok(response)
}
