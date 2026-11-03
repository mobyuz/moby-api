FROM rust:1.98.1-slim-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY .sqlx/ .sqlx/
COPY migrations/ migrations/
COPY src/ src/
COPY vendor/moby-protocol/ vendor/moby-protocol/
ARG CARGO_BUILD_JOBS
RUN SQLX_OFFLINE=true cargo build --locked --release

FROM debian:12.13-slim
COPY --from=build /app/target/release/moby-api /usr/local/bin/moby-api
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/moby-api"]
