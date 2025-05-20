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
    thumb_name: Option<String>,
    media_size: Option<i64>,
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
    thumb_name: Option<String>,
    media_size: Option<i64>,
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
    alias: Option<String>,
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
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CreateComment {
    alias: Option<String>,
    com: Option<String>,
    op: i64,
}
impl CreateComment {
    fn validate(self) -> Result<Self, Box<dyn std::error::Error>> {
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
        })
    }
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
        c.media_size AS media_size,
        c.thumb_size AS thumb_size,
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
) -> (StatusCode, Json<Result<Board, String>>) {
    let Ok(data) = data.validate() else {
        return (StatusCode::BAD_REQUEST, Json(Err("invalid data".into())));
    };
    let res = sqlx::query_as!(Board,
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
    .await;

    match res {
        Ok(res) => (StatusCode::CREATED, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn create_thread(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> (StatusCode, Json<Result<Comment, String>>) {
    let Ok((thread, file_name, media_data)) = parse_multipart::<CreateThread>(multipart).await
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err("failed to process multipart".into())),
        );
    };
    let Ok(thread) = thread.validate() else {
        return (StatusCode::BAD_REQUEST, Json(Err("invalid data".into())));
    };

    let has_board = sqlx::query_as!(
        Board,
        r#"
        SELECT * FROM boards WHERE code = $1
        "#,
        thread.board,
    )
    .fetch_one(&*pool)
    .await;
    if has_board.is_err() {
        return (StatusCode::NOT_FOUND, Json(Err("board not found".into())));
    }

    let Some(media_data) = media_data else {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err("image is required".into())),
        );
    };
    let Ok((media_name, thumb_name, media_size, thumb_size)) = save_media(media_data).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Err("failed to process media".into())),
        );
    };

    let res = sqlx::query_as!(
        Comment,
        r#"
        INSERT INTO comments (file_name, media_name, thumb_name, media_size, thumb_size, alias, sub, com, board, op)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING *
        "#,
        file_name,
        media_name,
        thumb_name,
        media_size,
        thumb_size,
        thread.alias,
        thread.sub,
        thread.com,
        thread.board,
        None::<i64>,
    )
    .fetch_one(&*pool)
    .await;

    match res {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}
async fn create_comment(
    Extension(pool): Extension<Arc<SqlitePool>>,
    multipart: Multipart,
) -> (StatusCode, Json<Result<Comment, String>>) {
    let Ok((comment, file_name, media_data)) = parse_multipart::<CreateComment>(multipart).await
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err("failed to process multipart".into())),
        );
    };
    let Ok(comment) = comment.validate() else {
        return (StatusCode::BAD_REQUEST, Json(Err("invalid data".into())));
    };
    if comment.com.is_none() && media_data.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err("comment or image is required".into())),
        );
    }

    let res = if let Some(media_data) = media_data {
        let Ok((media_name, thumb_name, media_size, thumb_size)) = save_media(media_data).await
        else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Err("failed to process media".into())),
            );
        };
        sqlx::query_as!(
            Comment,
            r#"
            INSERT INTO comments (file_name, media_name, thumb_name, media_size, thumb_size, alias, com, op)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
            file_name,
            media_name,
            thumb_name,
            media_size,
            thumb_size,
            comment.alias,
            comment.com,
            comment.op,
        )
        .fetch_one(&*pool)
        .await
    } else {
        sqlx::query_as!(
            Comment,
            r#"
            INSERT INTO comments (alias, com, op)
            VALUES ($1, $2, $3)
            RETURNING *
            "#,
            comment.alias,
            comment.com,
            comment.op,
        )
        .fetch_one(&*pool)
        .await
    };

    match res {
        Ok(res) => (StatusCode::OK, Json(Ok(res))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(Err(e.to_string()))),
    }
}

async fn save_media(
    media_data: Vec<u8>,
) -> Result<(String, String, i64, i64), Box<dyn std::error::Error>> {
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
    let media_size = media_data.len();
    let thumb_size = thumb_data.get_ref().len();

    File::create(format!("media/{media_name}"))
        .await?
        .write_all(&media_data)
        .await?;

    File::create(format!("media/{thumb_name}"))
        .await?
        .write_all(thumb_data.get_ref())
        .await?;

    Ok((media_name, thumb_name, media_size as i64, thumb_size as i64))
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
