.PHONY: build build-web test integration-test check fmt clippy install run web

build:
	cargo build --release

build-web:
	cargo build --release --features web

test:
	cargo nextest run
	cargo nextest run --features web

# The browser UI behind nginx, which runs in a container. Needs docker, curl and openssl.
integration-test:
	cargo nextest run --features web --run-ignored all -E 'binary(nginx_proxy)'

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
