.PHONY: build build-web test integration-test check fmt clippy install

build:
	cargo build --release

build-web:
	cargo build --release --features web

test:
	cargo nextest run
	cargo nextest run --features web

# The browser UI behind nginx, which runs in a container. Needs podman, curl and openssl;
# set CODEREVIEW_CONTAINER_RUNTIME=docker to drive docker instead.
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
