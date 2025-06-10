use std::error::Error;
use std::io::Cursor;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Multipart, Path};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use html_escape::encode_text;
use regex::Regex;
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

type Res<T> = Result<T, Box<dyn Error>>;

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
    created_at: i64,
}
#[derive(Serialize, Deserialize)]
struct Thread {
    id: i64,
    file_name: Option<String>,
    media_name: Option<String>,
    media_size: Option<i64>,
    media_ext: Option<String>,
    media_desc: Option<String>,
    thumb_name: Option<String>,
    thumb_size: Option<i64>,
    sub: Option<String>,
    com: Option<String>,
    op: Option<i64>,
    board: Option<String>,
    replies: i64,
    images: i64,
}
#[derive(Serialize, Deserialize)]
struct Comment {
    id: i64,
    alias: Option<String>,
    file_name: Option<String>,
    media_name: Option<String>,
    media_size: Option<i64>,
    media_ext: Option<String>,
    media_desc: Option<String>,
    thumb_name: Option<String>,
    thumb_size: Option<i64>,
    sub: Option<String>,
    com: Option<String>,
    op: Option<i64>,
    board: Option<String>,
    created_at: i64,
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
    fn validate(self) -> Res<Self> {
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
    alias: Option<String>,
    sub: Option<String>,
    com: Option<String>,
    media_desc: Option<String>,
    board: String,
}
impl CreateThread {
    fn validate(self) -> Res<Self> {
        if self.sub.is_none() && self.com.is_none() {
            return Err("sub or com is required".into());
        }
        if self.board.is_empty() {
            return Err("board is required".into());
        }
        if let Some(alias) = &self.alias
            && alias.len() > 100
        {
            return Err("alias is too long".into());
        }
        Ok(Self {
            alias: self.alias,
            sub: self.sub.map(encode_subject),
            com: self.com.map(encode_comment),
            board: self.board,
            media_desc: self.media_desc,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CreateComment {
    alias: Option<String>,
    com: Option<String>,
    media_desc: Option<String>,
    op: i64,
}
impl CreateComment {
    fn validate(self) -> Res<Self> {
        if self.op < 0 {
            return Err("op can't be negative".into());
        }
        if let Some(alias) = &self.alias
            && alias.len() > 100
        {
            return Err("alias is too long".into());
        }
        Ok(Self {
            alias: self.alias,
            com: self.com.map(encode_comment),
            op: self.op,
            media_desc: self.media_desc,
        })
    }
}

struct MediaInfo {
    media_name: String,
    media_size: i64,
    media_ext: String,
    thumb_name: String,
    thumb_size: i64,
}
struct MultiPartData<T> {
    data: T,
    file_name: Option<String>,
    file_bytes: Option<Vec<u8>>,
}

async fn get_media(Path(file): Path<String>) -> impl IntoResponse {
    let Ok(mut file) = File::open(format!("./media/{file}")).await else {
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
async fn get_boards(Extension(pool): Extension<Arc<SqlitePool>>) -> impl IntoResponse {
    let get_boards_impl = async || -> Res<Vec<Board>> {
        sqlx::query_as!(
            Board,
            r#"
            SELECT * FROM boards
            "#,
        )
        .fetch_all(&*pool)
        .await
        .map_err(|e| e.into())
    };
    match get_boards_impl().await {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn get_threads(
    Path(board_id): Path<String>,
    Extension(pool): Extension<Arc<SqlitePool>>,
) -> impl IntoResponse {
    let get_threads_impl = async || -> Res<Vec<Thread>> {
        sqlx::query_as!(
            Thread,
            r#"
            SELECT
            c.id AS id,
            c.file_name AS file_name,
            c.media_name AS media_name,
            c.thumb_name AS thumb_name,
            c.media_size AS media_size,
            c.media_desc AS media_desc,
            c.thumb_size AS thumb_size,
            c.media_ext AS media_ext,
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
        .map_err(|e| e.into())
    };
    match get_threads_impl().await {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn get_comments(
    Path((board_id, thread_id)): Path<(String, i64)>,
    Extension(pool): Extension<Arc<SqlitePool>>,
) -> impl IntoResponse {
    let get_comments_impl = async || -> Res<Vec<Comment>> {
        let thread_id = Some(thread_id);
        sqlx::query_as!(
            Comment,
            r#"
            SELECT * FROM comments WHERE board = $1 AND id = $2 OR op = $2
            "#,
            board_id,
            thread_id,
        )
        .fetch_all(&*pool)
        .await
        .map_err(|e| e.into())
    };
    match get_comments_impl().await {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn create_board(
    Extension(pool): Extension<Arc<SqlitePool>>,
    Json(data): Json<CreateBoard>,
) -> impl IntoResponse {
    let create_board_impl = async || -> Res<Board> {
        let data = data.validate()?;
        sqlx::query_as!(Board,
            r#"
            INSERT INTO boards (code, name, desc, max_threads, max_replies, max_img_replies, max_sub_len, max_com_len, max_file_size, is_nsfw)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
            data.code,
            data.name,
            data.desc,
            data.max_threads,
            data.max_replies,
            data.max_img_replies,
            data.max_sub_len,
            data.max_com_len,
            data.max_file_size,
            data.is_nsfw
        )
        .fetch_one(&*pool)
        .await.map_err(|e| e.into())
    };

    match create_board_impl().await {
        Ok(res) => (StatusCode::CREATED, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn create_thread(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> impl IntoResponse {
    let create_thread_impl = async || -> Res<Comment> {
        let MultiPartData {
            data,
            file_name,
            file_bytes,
        } = parse_multipart::<CreateThread>(multipart).await?;
        let thread = data.validate()?;

        sqlx::query_as!(
            Board,
            r#"SELECT * FROM boards WHERE code = $1"#,
            thread.board,
        )
        .fetch_one(&*pool)
        .await
        .map_err(|e| e.to_string())?;

        let media_data = file_bytes.ok_or("image is required")?;
        let MediaInfo {
            media_name,
            media_size,
            media_ext,
            thumb_name,
            thumb_size,
        } = save_media(media_data).await?;

        sqlx::query_as!(
        Comment,
        r#"
        INSERT INTO comments (file_name, media_name, thumb_name, media_size, thumb_size, media_ext, media_desc, alias, sub, com, board, op)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING *
        "#,
        file_name,
        media_name,
        thumb_name,
        media_size,
        thumb_size,
        media_ext,
        thread.media_desc,
        thread.alias,
        thread.sub,
        thread.com,
        thread.board,
        None::<i64>,
    )
    .fetch_one(&*pool)
    .await.map_err(|e| e.into())
    };
    match create_thread_impl().await {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn create_comment(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> impl IntoResponse {
    let create_comment_impl = async || -> Res<Comment> {
        let MultiPartData {
            data,
            file_name,
            file_bytes,
        } = parse_multipart::<CreateComment>(multipart).await?;

        let create_comment = data.validate()?;
        if create_comment.com.is_none() && file_bytes.is_none() {
            return Err("comment or image is required".into());
        }
        if let Some(media_data) = file_bytes {
            let MediaInfo {
                media_name,
                media_size,
                media_ext,
                thumb_name,
                thumb_size,
            } = save_media(media_data).await?;
            sqlx::query_as!(
            Comment,
            r#"
            INSERT INTO comments (file_name, media_name, thumb_name, media_size, thumb_size, media_ext, media_desc, alias, com, op)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
            file_name,
            media_name,
            thumb_name,
            media_size,
            thumb_size,
            media_ext,
            create_comment.media_desc,
            create_comment.alias,
            create_comment.com,
            create_comment.op,
        )
        .fetch_one(&*pool)
        .await.map_err(|e| e.into())
        } else {
            sqlx::query_as!(
                Comment,
                r#"
            INSERT INTO comments (alias, com, op)
            VALUES ($1, $2, $3)
            RETURNING *
            "#,
                create_comment.alias,
                create_comment.com,
                create_comment.op,
            )
            .fetch_one(&*pool)
            .await
            .map_err(|e| e.into())
        }
    };
    match create_comment_impl().await {
        Ok(comment) => (StatusCode::OK, Json(Ok(comment))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(Err(e.to_string()))),
    }
}

async fn parse_multipart<T: DeserializeOwned>(mut multipart: Multipart) -> Res<MultiPartData<T>> {
    let mut data: Option<T> = None;
    let mut file_name: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(mut field) = multipart.next_field().await.unwrap() {
        match field.name() {
            Some("data") => {
                let text = field.text().await?;
                data = Some(serde_json::from_str(&text)?);
            }
            Some("media") => {
                let Some(file) = field.file_name() else {
                    return Err("file namenot found".into());
                };
                file_name = Some(
                    std::path::Path::new(file)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(file)
                        .to_string(),
                );
                let mut chunks = Vec::new();
                while let Ok(Some(chunk)) = field.chunk().await {
                    chunks.extend(chunk.to_vec());
                }
                file_bytes = Some(chunks);
            }
            _ => {}
        }
    }

    let Some(data) = data else {
        return Err("invalid data".into());
    };
    Ok(MultiPartData {
        data,
        file_name,
        file_bytes,
    })
}
async fn save_media(media_data: Vec<u8>) -> Res<MediaInfo> {
    let uuid = Uuid::new_v4().to_string();
    let media_kind = infer::get(&media_data).ok_or("Failed to infer media type")?;
    let media_name = uuid.clone();
    let thumb_name = format!("{uuid}t");

    let mut thumb_data = Cursor::new(Vec::new());
    let thumb = create_thumbnails(
        Cursor::new(&media_data),
        mime::Mime::from_str(media_kind.mime_type()).unwrap(),
        [ThumbnailSize::Medium],
    )?
    .pop()
    .ok_or("Failed to create thumbnails")?;
    thumb.write_jpeg(&mut thumb_data, 100)?;
    let media_size = media_data.len() as i64;
    let thumb_size = thumb_data.get_ref().len() as i64;
    let media_ext = media_kind.extension().to_string();

    File::create(format!("media/{media_name}"))
        .await?
        .write_all(&media_data)
        .await?;

    File::create(format!("media/{thumb_name}"))
        .await?
        .write_all(thumb_data.get_ref())
        .await?;

    Ok(MediaInfo {
        media_name,
        media_size,
        media_ext,
        thumb_name,
        thumb_size,
    })
}

fn encode_comment(com: impl AsRef<str>) -> String {
    let re_replies = Regex::new(r"&gt;&gt;(\d+)").unwrap();
    let re_url = Regex::new(
        r"http[s]?://(?:[a-zA-Z]|[0-9]|[$-_@.&+]|[!*\(\),]|(?:%[0-9a-fA-F][0-9a-fA-F]))+",
    )
    .unwrap();

    let text = encode_text(&com);
    let text = text
        .lines()
        .map(|ln| {
            if ln.starts_with("&gt;") && !ln.starts_with("&gt;&gt;") {
                format!("<span>{ln}</span>")
            } else {
                ln.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("<br>");
    let text = re_url.replace_all(&text, "<a href=\"$0\">$0</a>");
    let text = re_replies.replace_all(&text, "<a href=\"#p$1\">&gt;&gt;$1</a>");
    text.to_string()
}
fn encode_subject(sub: impl AsRef<str>) -> String {
    let sub = encode_comment(sub);
    format!("<b>{sub}</b>")
}

#[test]
fn test_encode() {
    use crate::encode_comment;

    assert_eq!(encode_comment("hello >world"), "hello &gt;world");
    assert_eq!(encode_comment(">hello"), "<span>&gt;hello</span>");

    assert_eq!(
        encode_comment("https://google.com"),
        "<a href=\"https://google.com\">https://google.com</a>"
    );
    assert_eq!(
        encode_comment("hello >>11 >>22"),
        "hello <a href=\"#p11\">&gt;&gt;11</a> <a href=\"#p22\">&gt;&gt;22</a>"
    );
    assert_eq!(
        encode_comment("this\nis\nmultiline"),
        "this<br>is<br>multiline"
    );
}
