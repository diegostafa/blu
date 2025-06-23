FROM rust:alpine as builder

RUN apk add --no-cache musl-dev openssl-dev pkgconfig sqlite-dev build-base
RUN cargo install sqlx-cli --no-default-features --features sqlite

WORKDIR /app
COPY . .

ARG DATABASE_URL
ENV DATABASE_URL=$DATABASE_URL

RUN sqlx database create
RUN sqlx migrate run
RUN cargo build --release

FROM alpine
RUN apk add --no-cache openssl sqlite libgcc

WORKDIR /app
COPY --from=builder /app/target/release/blu ./app
EXPOSE 3000
CMD ["./app"]
