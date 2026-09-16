.PHONY: build build-web test check fmt clippy install run web

build:
	cargo build --release

build-web:
	cargo build --release --features web

test:
	cargo nextest run
	cargo nextest run --features web

fmt:
	cargo fmt --check

clippy:
	cargo clippy --tests -- -D warnings
	cargo clippy --tests --no-default-features -- -D warnings
	cargo clippy --tests --features web -- -D warnings

check: fmt clippy test

install: check
	cargo install --path . --features web --locked

run:
	cargo run --

web:
	cargo run --features web -- web
