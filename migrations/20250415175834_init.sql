-- Add migration script here
DROP TABLE IF EXISTS boards;
CREATE TABLE boards (
    code TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    desc TEXT NOT NULL,
    max_threads INTEGER NOT NULL,
    max_replies INTEGER NOT NULL,
    max_img_replies INTEGER NOT NULL,
    max_com_len INTEGER NOT NULL,
    max_sub_len INTEGER NOT NULL,
    max_file_size INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    is_nsfw BOOLEAN NOT NULL
);
DROP TABLE IF EXISTS comments;
CREATE TABLE comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    alias TEXT,
    sub TEXT,
    com TEXT,
    op INTEGER,
    file_name TEXT,
    media_name TEXT,
    media_size INTEGER,
    media_ext TEXT,
    media_desc TEXT,
    thumb_name TEXT,
    thumb_size INTEGER,
    board TEXT,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    FOREIGN KEY (board) REFERENCES boards (code),
    FOREIGN KEY (op) REFERENCES comments (id)
);