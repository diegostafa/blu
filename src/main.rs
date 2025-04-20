use std::io::Cursor;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Multipart, Path};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use thumbnailer::{ThumbnailSize, create_thumbnails};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .unwrap();
    let pool = Arc::new(pool);

    let app = Router::new()
        .route("/boards", get(get_boards))
        .route("/{board_id}", get(get_threads))
        .route("/{board_id}/thread/{thread_id}", get(get_comments))
        .route("/create_board", post(create_board))
        .route("/create_thread", post(create_thread))
        .route("/create_comment", post(create_comment))
        .route("/media/{file_name}", get(get_media))
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .layer(Extension(pool.clone()))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Serialize, Deserialize)]
struct Board {
    code: String,
    name: String,
    desc: String,
    max_threads: i64,
    max_replies: i64,
    max_img_replies: i64,
    max_sub_len: i64,
    max_com_len: i64,
    max_file_size: i64,
    is_nsfw: bool,
    created_at: String,
}
#[derive(Serialize, Deserialize)]
struct Comment {
    id: i64,
    alias: Option<String>,
    file_name: Option<String>,
    media_name: Option<String>,
    thumb_name: Option<String>,
    sub: Option<String>,
    com: Option<String>,
    op: Option<i64>,
    board: Option<String>,
    created_at: String,
}
#[derive(Serialize, Deserialize)]
struct Thread {
    id: i64,
    file_name: Option<String>,
    media_name: Option<String>,
    thumb_name: Option<String>,
    sub: Option<String>,
    com: Option<String>,
    op: Option<i64>,
    board: Option<String>,
    replies: i64,
    images: i64,
}

#[derive(Serialize, Deserialize)]
struct CreateBoard {
    code: String,
    name: String,
    desc: String,
    max_threads: i64,
    max_replies: i64,
    max_img_replies: i64,
    max_sub_len: i64,
    max_com_len: i64,
    max_file_size: i64,
    is_nsfw: bool,
}
impl CreateBoard {
    fn validate(self) -> Result<Self, Box<dyn std::error::Error>> {
        if self.code.is_empty() || self.code.len() > 5 {
            return Err("invalid board code".into());
        }
        if self.name.is_empty() || self.name.len() > 100 {
            return Err("invalid board name".into());
        }
        if self.desc.is_empty() || self.desc.len() > 100 {
            return Err("invalid board description".into());
        }
        if self.max_threads < 0 {
            return Err("max_threads can't be negative".into());
        }
        if self.max_replies < 0 {
            return Err("max_replies can't be negative".into());
        }
        if self.max_img_replies < 0 {
            return Err("max_img_replies can't be negative".into());
        }
        Ok(self)
    }
}

#[derive(Serialize, Deserialize)]
struct CreateThread {
    sub: Option<String>,
    com: Option<String>,
    board: String,
}
impl CreateThread {
    fn validate(self) -> Result<Self, Box<dyn std::error::Error>> {
        if self.sub.is_none() && self.com.is_none() {
            return Err("sub or com is required".into());
        }
        if self.board.is_empty() {
            return Err("board is required".into());
        }
        Ok(self)
    }
}

#[derive(Serialize, Deserialize)]
struct CreateComment {
    alias: Option<String>,
    com: String,
    op: i64,
}
impl CreateComment {
    fn validate(self) -> Result<Self, Box<dyn std::error::Error>> {
        if self.op < 0 {
            return Err("op can't be negative".into());
        }
        if let Some(alias) = &self.alias {
            if alias.len() > 100 {
                return Err("alias is too long".into());
            }
        }
        Ok(self)
    }
}

async fn get_media(Path(file): Path<String>) -> impl IntoResponse {
    let Ok(mut file) = File::open(format!("./media/{}", file)).await else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };
    let mut data = Vec::new();
    if (file.read_to_end(&mut data).await).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read file").into_response();
    }
    let content_type = match infer::get(&data) {
        Some(kind) => kind.mime_type(),
        None => "application/octet-stream",
    };

    let headers = [(header::CONTENT_TYPE, content_type)];
    (StatusCode::OK, headers, data).into_response()
}
async fn get_boards(Extension(pool): Extension<Arc<SqlitePool>>) -> (StatusCode, Json<Vec<Board>>) {
    let Ok(res) = sqlx::query_as!(
        Board,
        r#"
        SELECT * FROM boards
        "#,
    )
    .fetch_all(&*pool)
    .await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![]));
    };

    (StatusCode::OK, Json(res))
}
async fn get_threads(
    Path(board_id): Path<String>,
    Extension(pool): Extension<Arc<SqlitePool>>,
) -> (StatusCode, Json<Vec<Thread>>) {
    let Ok(res) = sqlx::query_as!(
        Thread,
        r#"
        SELECT
        c.id AS id,
        c.file_name AS file_name,
        c.media_name AS media_name,
        c.thumb_name AS thumb_name,
        c.sub AS sub,
        c.com AS com,
        c.op AS op,
        c.board AS board,
        COUNT(r.id) AS replies,
        COUNT(r.media_name) AS images
        FROM comments c
        LEFT JOIN comments r ON r.op = c.id
        WHERE c.op IS NULL AND c.board = $1
        GROUP BY c.id
        "#,
        board_id,
    )
    .fetch_all(&*pool)
    .await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![]));
    };

    (StatusCode::OK, Json(res))
}
async fn get_comments(
    Path((board_id, thread_id)): Path<(String, i64)>,
    Extension(pool): Extension<Arc<SqlitePool>>,
) -> (StatusCode, Json<Vec<Comment>>) {
    let thread_id = Some(thread_id);
    let Ok(res) = sqlx::query_as!(
        Comment,
        r#"
        SELECT * FROM comments WHERE board = $1 AND id = $2 OR op = $2
        "#,
        board_id,
        thread_id,
    )
    .fetch_all(&*pool)
    .await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![]));
    };

    (StatusCode::OK, Json(res))
}
async fn create_board(
    Extension(pool): Extension<Arc<SqlitePool>>,
    Json(data): Json<CreateBoard>,
) -> (StatusCode, Json<String>) {
    let Ok(data) = data.validate() else {
        return (StatusCode::BAD_REQUEST, Json("invalid data".into()));
    };
    let query = sqlx::query(
        r#"
        INSERT INTO boards (code, name, desc, max_threads, max_replies, max_img_replies, max_sub_len, max_com_len, max_file_size, is_nsfw)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(data.code)
    .bind(data.name)
    .bind(data.desc)
    .bind(data.max_threads)
    .bind(data.max_replies)
    .bind(data.max_img_replies)
    .bind(data.max_sub_len)
    .bind(data.max_com_len)
    .bind(data.max_file_size)
    .bind(data.is_nsfw)
    .execute(&*pool)
    .await;

    if let Err(e) = query {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(format!("Error: {}", e)),
        );
    }

    (StatusCode::OK, Json("success".into()))
}
async fn create_thread(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> (StatusCode, Json<String>) {
    let Ok((thread, file_name, media_data)) = parse_multipart::<CreateThread>(multipart).await
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json("failed to process multipart".into()),
        );
    };
    let Ok(thread) = thread.validate() else {
        return (StatusCode::BAD_REQUEST, Json("invalid data".into()));
    };
    let Some(media_data) = media_data else {
        return (StatusCode::BAD_REQUEST, Json("image is required".into()));
    };
    let Ok((media_name, thumb_name)) = save_media(media_data).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("failed to process media".into()),
        );
    };

    let res = sqlx::query_as!(
        Comment,
        r#"
        INSERT INTO comments (file_name, media_name, thumb_name, sub, com, board, op)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#,
        file_name,
        media_name,
        thumb_name,
        thread.sub,
        thread.com,
        thread.board,
        None::<i64>,
    )
    .fetch_one(&*pool)
    .await
    .unwrap();

    (StatusCode::OK, Json(res.id.to_string()))
}
async fn create_comment(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> (StatusCode, Json<String>) {
    let Ok((comment, file_name, media_data)) = parse_multipart::<CreateComment>(multipart).await
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json("failed to process multipart".into()),
        );
    };
    let Ok(comment) = comment.validate() else {
        return (StatusCode::BAD_REQUEST, Json("invalid data".into()));
    };
    let res = if let Some(media_data) = media_data {
        let Ok((media_name, thumb_name)) = save_media(media_data).await else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json("failed to process media".into()),
            );
        };
        sqlx::query(
            r#"
            INSERT INTO comments (file_name, media_name, thumb_name, com, op)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(file_name)
        .bind(media_name)
        .bind(thumb_name)
        .bind(comment.com)
        .bind(comment.op)
        .execute(&*pool)
        .await
    } else {
        sqlx::query(
            r#"
            INSERT INTO comments (com, op)
            VALUES ($1, $2)
            "#,
        )
        .bind(comment.com)
        .bind(comment.op)
        .execute(&*pool)
        .await
    };

    let Ok(_) = res else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("failed to create comment".into()),
        );
    };

    (StatusCode::OK, Json("success".into()))
}

async fn save_media(media_data: Vec<u8>) -> Result<(String, String), Box<dyn std::error::Error>> {
    let uuid = Uuid::new_v4().to_string();
    let media_kind = infer::get(&media_data).ok_or("Failed to infer media type")?;
    let media_name = format!("{uuid}.{}", media_kind.extension());
    let thumb_name = format!("{uuid}t.jpg");

    let mut thumb_data = Cursor::new(Vec::new());
    let thumb = create_thumbnails(
        Cursor::new(&media_data),
        mime::Mime::from_str(media_kind.mime_type()).unwrap(),
        [ThumbnailSize::Medium],
    )?
    .pop()
    .ok_or("Failed to create thumbnails")?;
    thumb.write_jpeg(&mut thumb_data, 100)?;

    File::create(format!("media/{media_name}"))
        .await?
        .write_all(&media_data)
        .await?;

    File::create(format!("media/{thumb_name}"))
        .await?
        .write_all(thumb_data.get_ref())
        .await?;

    Ok((media_name, thumb_name))
}
async fn parse_multipart<T: DeserializeOwned>(
    mut multipart: Multipart,
) -> Result<(T, Option<String>, Option<Vec<u8>>), Box<dyn std::error::Error>> {
    let mut val: Option<T> = None;
    let mut file_name: Option<String> = None;
    let mut media_data: Option<Vec<u8>> = None;

    while let Some(mut field) = multipart.next_field().await.unwrap() {
        match field.name() {
            Some("data") => {
                let text = field.text().await?;
                val = Some(serde_json::from_str(&text)?);
            }
            Some("media") => {
                file_name = Some(field.file_name().unwrap().to_string());
                let mut chunks = Vec::new();
                while let Ok(Some(chunk)) = field.chunk().await {
                    chunks.extend(chunk.to_vec());
                }
                media_data = Some(chunks);
            }
            _ => {}
        }
    }

    let Some(val) = val else {
        return Err("invalid data".into());
    };
    Ok((val, file_name, media_data))
}
