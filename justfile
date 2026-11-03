set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

setup:
    cargo fetch --locked

dev:
    docker compose up --build

test:
    cargo test --locked

lint:
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings

build:
    cargo build --locked --release
