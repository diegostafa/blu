FROM rust:alpine

RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    pkgconfig \
    sqlite-dev \
    build-base

RUN cargo install sqlx-cli --no-default-features --features sqlite

WORKDIR /app
COPY . .

ARG DATABASE_URL
ENV DATABASE_URL=${DATABASE_URL}
EXPOSE 3000

CMD sqlx database create && sqlx migrate run && cargo run --release
