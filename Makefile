.PHONY: build build-web test integration-test browser-test check fmt clippy install deploy

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

# The page itself, in a headless Chromium at a phone's size. Needs podman, and pulls about a
# gigabyte of browser the first time. This is the only test that can see the page: everything
# else reads what the server sends, which a clipped menu does not change.
browser-test:
	cargo nextest run --features web --run-ignored all -E 'binary(browser)' --no-capture

fmt:
	cargo fmt --check

clippy:
	cargo clippy --tests -- -D warnings
	cargo clippy --tests --no-default-features -- -D warnings
	cargo clippy --tests --features web -- -D warnings

check: fmt clippy test

install: check
	cargo install --path . --features web --locked

# Install, then restart the service that runs it. The unit is a user one by default, since
# the repositories worth serving are usually the ones in $HOME; DEPLOY_UNIT and SYSTEMCTL
# point this at a system unit instead. docs/deploy.md has both units.
#
# The state and the timestamp are printed rather than assumed. `codereview --version` cannot
# tell two builds of the same 0.0.x apart, so the way this fails is that everything succeeds
# and the page keeps serving the old code; the only facts that distinguish that are whether
# the unit came back up and when the binary underneath it landed.
DEPLOY_UNIT ?= codereview.service
SYSTEMCTL ?= systemctl --user

deploy: install
	$(SYSTEMCTL) restart $(DEPLOY_UNIT)
	@printf 'unit state: '; $(SYSTEMCTL) is-active $(DEPLOY_UNIT) || true
	@printf 'deployed:   '; ls -l --time-style=+%Y-%m-%dT%H:%M:%S $$(command -v codereview) | awk '{print $$6, $$7}'
