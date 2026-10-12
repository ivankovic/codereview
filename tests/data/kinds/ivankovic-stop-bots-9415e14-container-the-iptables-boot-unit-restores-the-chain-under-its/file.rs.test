/*  This file is part of the stop-bots project.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

//! End-to-end tests against a *real* NGINX and a *real* nftables, in a
//! container.
//!
//! Everything else in `tests/` asserts the text this project generates.
//! That is exactly the check that keeps passing when the text is wrong —
//! a directive NGINX rejects, a rule `nft` won't load, a block that
//! returns 403 to the wrong requests. These tests run the generated
//! output through the actual parsers and then send actual requests at it.
//!
//! **They do not run by default.** They need a container runtime, take
//! tens of seconds, and would break `cargo test` on any machine without
//! one. Set `STOP_BOTS_CONTAINER_TESTS=1` to enable them; `make
//! integration-test` does that for you. CI runs them as their own job.
//!
//! ## Which runtime
//!
//! `docker` unless `STOP_BOTS_CONTAINER_RUNTIME` says otherwise, which is
//! what CI uses. Setting it to `podman` runs the same suite rootless —
//! see [`runtime()`] for why that option is here, and [`Host::boot`] for
//! the one place the two need different flags. Both are exercised; the
//! systemd assertions below were confirmed to still fail when their
//! directive is removed under each.
//!
//! ## Two images, because faithfulness is not free
//!
//! [`Server`] (`Dockerfile`, ubuntu:24.04) has **no init system**. It is
//! fast, needs only `NET_ADMIN`, and is what the NGINX and nftables tests
//! use — `nginx -s reload` stands in for `systemctl reload nginx`. What it
//! is faithful about is the two parsers this project cannot check any
//! other way.
//!
//! [`Host`] (`Dockerfile.host`, debian:13) runs **real systemd as PID 1**.
//! It exists because that "no init system" line above was, for a while,
//! the reason three separate bugs reached a live server: `install web`
//! failing with `203/EXEC` because the unit's `ProtectHome=yes` hid the
//! binary, the firewall apply failing because `RestrictAddressFamilies`
//! omitted `AF_NETLINK`, and a deploy that replaced the binary without
//! restarting anything. Every one of those is a property of the generated
//! unit under a real service manager, and no amount of asserting on the
//! unit's *text* finds them — the text was what everyone had already
//! read.
//!
//! Tests on `Host` derive their probes from the unit `install web`
//! actually wrote (see [`Host::oneshot_under_web_sandbox`]) rather than
//! from a hand-written copy, and the sandbox ones each carry a negative
//! control that removes the directive under test and asserts the failure
//! comes back. Without that control, a container where the sandbox
//! silently did not apply would pass every one of them.
//!
//! ## And a stranger's host
//!
//! `Dockerfile.stranger` is a fresh Debian 12 or Ubuntu 24.04 with nothing
//! of ours on it, for [`a_stranger_follows_the_quick_start`]: the README's
//! quick start from `apt install` of the `.deb` to `uninstall all`. It
//! needs a package, named by `STOP_BOTS_STRANGER_DEB`, and skips without
//! one; `make stranger-test` builds one and runs it.

use std::process::Command;

/// Skips the test unless container tests are switched on. Returns whether
/// to proceed, and says why not — a silently skipped test is how a whole
/// suite quietly stops running.
fn enabled() -> bool {
    if std::env::var_os("STOP_BOTS_CONTAINER_TESTS").is_some() {
        return true;
    }
    stop_bots::say_err!(
        "skipping: container tests are off. Set STOP_BOTS_CONTAINER_TESTS=1 \
         (or run `make integration-test`) to enable them."
    );
    false
}

/// The container runtime to drive, from `STOP_BOTS_CONTAINER_RUNTIME`.
///
/// Defaults to `docker`, which is what CI has and what this suite was
/// written against. The override exists because the daemon Docker talks
/// to runs as root: on a machine that also hosts unrelated root-owned
/// containers, joining the `docker` group to run these tests hands over
/// every one of those containers too, since the socket has no notion of
/// per-container permission. Rootless Podman has no daemon and its own
/// per-user storage, so the suite can run without that trade.
///
/// Anything CLI-compatible works; `podman` is the one that is actually
/// exercised. Read once — a test run must not straddle two runtimes,
/// which is the sort of thing an env var changed mid-run would do.
fn runtime() -> &'static str {
    static RUNTIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        std::env::var("STOP_BOTS_CONTAINER_RUNTIME").unwrap_or_else(|_| "docker".to_string())
    })
}

/// Whether the runtime is Podman, which needs different flags to boot
/// systemd — see [`Host::boot`].
fn runtime_is_podman() -> bool {
    runtime().contains("podman")
}

/// Appended to every image tag and every container and network name, from
/// `STOP_BOTS_CONTAINER_SUFFIX`. Unset, the names are what they always
/// were.
///
/// Two checkouts running this suite at once otherwise share one image tag
/// and one set of container names: the second build replaces the image
/// under the first run, which then tests the other checkout's binary, and
/// each run's `rm -f` removes the other's containers. A suffix per
/// checkout keeps them apart.
fn suffix() -> &'static str {
    static SUFFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SUFFIX.get_or_init(|| std::env::var("STOP_BOTS_CONTAINER_SUFFIX").unwrap_or_default())
}

/// `name`, with [`suffix`] if there is one.
fn scoped(name: &str) -> String {
    match suffix() {
        "" => name.to_string(),
        suffix => format!("{name}-{suffix}"),
    }
}

/// The image tag, which is `latest` unless [`suffix`] says otherwise.
fn tagged(image: &str) -> String {
    match suffix() {
        "" => format!("{image}:latest"),
        suffix => format!("{image}:{suffix}"),
    }
}

fn image() -> String {
    tagged("stop-bots-test")
}

/// The image with a real init — see `Dockerfile.host` and [`Host`].
fn host_image() -> String {
    tagged("stop-bots-host")
}

/// Builds the image exactly once per test binary run.
///
/// The `Once` is not an optimisation. Tests run in parallel and every one
/// of them needs the image, so without it several threads copy the binary
/// into the build context while other threads' `docker build` are reading
/// it — producing a truncated binary inside the image and a scatter of
/// unrelated-looking command failures. That is precisely how this first
/// showed up: four tests failing together, each passing alone.
fn build_image() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| with_build_lock(|| build_unless_current(&image(), build_image_now)));
}

/// Builds `image` with `build` unless this run already built it from the
/// same binary and Dockerfiles, which a marker in the target directory
/// records.
///
/// Under nextest every test process gets here, one after another through
/// [`with_build_lock`]. Rebuilding each time is all cache hits, but on a
/// loaded machine a cached rebuild still costs seconds, and fifty-eight of
/// them in a row spent the first tests' whole time budget waiting for the
/// lock. The marker's key is the binary's and the build context's sizes
/// and mtimes, so a rebuilt binary is never tested through a stale image,
/// and the image has to still exist.
fn build_unless_current(image: &str, build: impl FnOnce()) {
    let marker = format!(
        "{}/container-image-{}.built",
        env!("CARGO_TARGET_TMPDIR"),
        image.replace([':', '/'], "_")
    );
    let key = build_key();
    let exists = Command::new(runtime())
        .args(["image", "inspect", image])
        .output()
        .is_ok_and(|out| out.status.success());
    if exists && std::fs::read_to_string(&marker).ok().as_deref() == Some(key.as_str()) {
        return;
    }
    build();
    std::fs::write(&marker, key).expect("failed to record the image build");
}

/// What an image is built from, as sizes and mtimes: the binary under test
/// and everything in the build context but the staged copy of it.
fn build_key() -> String {
    let stamp = |path: &std::path::Path| {
        let meta = std::fs::metadata(path).expect("a build input is missing");
        format!(
            "{} {} {:?}\n",
            path.display(),
            meta.len(),
            meta.modified().ok()
        )
    };
    let mut key = stamp(std::path::Path::new(env!("CARGO_BIN_EXE_stop-bots")));
    let ctx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/container");
    let mut inputs: Vec<_> = std::fs::read_dir(&ctx)
        .expect("the build context is missing")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.file_name().is_some_and(|name| name != "stop-bots"))
        .collect();
    inputs.sort();
    for path in inputs {
        key.push_str(&stamp(&path));
    }
    key
}

/// Runs `f` holding an exclusive `flock` shared by every process of this
/// checkout's test run.
///
/// The `Once`s here guard threads, and `cargo test` runs every test as a
/// thread of one process. `cargo nextest` — which CI uses for this suite,
/// for its retries — runs each test as a process of its own, where a
/// `Once` guards nothing: every test would stage the binary and build the
/// images at once, and one process's copy would truncate the binary under
/// another's build. That is the race `stage_binary` describes, back through
/// a door the `Once`s cannot close. Staging and building under this lock
/// closes it for both runners. A rebuild after the first is all cache hits,
/// so the cost is a second or so per test process.
///
/// In the target directory, so two checkouts (which already build under
/// different `STOP_BOTS_CONTAINER_SUFFIX`es) do not wait for each other.
fn with_build_lock<T>(f: impl FnOnce() -> T) -> T {
    use std::os::fd::AsRawFd;
    let path = concat!(env!("CARGO_TARGET_TMPDIR"), "/container-build.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .expect("failed to open the image build lock");
    // SAFETY: a valid descriptor, owned by `file` until it drops below.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    assert_eq!(locked, 0, "failed to lock {path}");
    let result = f();
    drop(file);
    result
}

/// The same, for the systemd image. A separate `Once` so a run that only
/// touches one of the two images only builds that one.
fn build_host_image() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        with_build_lock(|| build_unless_current(&host_image(), build_host_image_now))
    });
}

fn build_host_image_now() {
    let ctx = stage_binary();
    let out = Command::new(runtime())
        .args([
            "build",
            "-q",
            "-f",
            &format!("{ctx}/Dockerfile.host"),
            "-t",
            &host_image(),
            &ctx,
        ])
        .output()
        .expect("failed to run the image build");
    assert!(
        out.status.success(),
        "{} build (host image) failed:\n{}",
        runtime(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Copies the binary under test into the build context and returns the
/// context path.
///
/// `CARGO_BIN_EXE_stop-bots` is the exact binary this test run compiled —
/// not whatever happens to be on `PATH`.
///
/// Guarded by a `Once` of its own, and that is the point. The two image
/// builders above have one `Once` each, which stops two threads racing to
/// build *the same* image — but both of them call this, and both write the
/// same file. So a thread staging for the host image could truncate and
/// rewrite `stop-bots` while the other image's `docker build` was reading
/// it, putting a half-copied binary inside the image: exactly the failure
/// the per-image `Once`es were added for, arriving through the one door
/// they left open. It presents as a scatter of unrelated-looking command
/// failures, and it is far likelier on a slow two-core runner with a cold
/// Docker cache than on a developer's machine — which is where it was
/// found, as three consecutive red `container` jobs on CI with a green
/// suite locally.
///
/// `call_once` blocks every other caller until the copy has finished, so
/// the file is only ever read complete.
fn stage_binary() -> String {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let manifest = env!("CARGO_MANIFEST_DIR");
    let ctx = format!("{manifest}/tests/container");
    ONCE.call_once(|| {
        std::fs::copy(env!("CARGO_BIN_EXE_stop-bots"), format!("{ctx}/stop-bots"))
            .expect("failed to stage the stop-bots binary into the build context");
    });
    ctx
}

fn build_image_now() {
    let ctx = stage_binary();
    let out = Command::new(runtime())
        .args(["build", "-q", "-t", &image(), &ctx])
        .output()
        .expect("failed to run the image build");
    assert!(
        out.status.success(),
        "{} build failed:\n{}",
        runtime(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A running container, cleaned up on drop.
struct Server {
    name: String,
}

/// A user-defined Docker network, so containers on it get real addresses
/// and can reach each other by name. Removed on drop.
struct Network {
    name: String,
}

/// The subnet every test network is given.
///
/// **Not the runtime's default.** Both hand out RFC1918 addresses, and every
/// detector in this project deliberately skips those —
/// `accesslog::is_local_or_private` is what stops a NAT gateway or a
/// reverse proxy getting the whole office blocked. A client on
/// `192.168.x.x` is therefore invisible to detection, and an end-to-end
/// test built on one passes by finding nothing, for ever.
///
/// TEST-NET-2, reserved by RFC 5737 for documentation: it is not private,
/// so the detectors treat it as a real remote client, and it can never
/// route anywhere outside the container network.
///
/// Carved into /28s because tests run in parallel and both runtimes
/// refuse two networks whose pools overlap. Sixteen is far more than the handful of
/// networked tests here, and each gets thirteen usable addresses.
fn test_subnet(index: usize) -> String {
    format!("198.51.100.{}/28", (index % 16) * 16)
}

/// The same slicing inside 172.31.255.0/24: RFC1918, like the address a
/// Docker bridge hands a container. For the tests about a container
/// reaching a service on its own host, where being private is the point.
fn private_test_subnet(index: usize) -> String {
    format!("172.31.255.{}/28", (index % 16) * 16)
}

impl Network {
    fn create(name: &str) -> Network {
        Network::create_in(name, test_subnet, None)
    }

    fn create_private(name: &str) -> Network {
        Network::create_in(name, private_test_subnet, None)
    }

    /// [`Network::create`] with IPv6 as well, from the documentation
    /// prefix (RFC 3849) for the reason [`test_subnet`] uses TEST-NET-2:
    /// not private, and routed nowhere.
    fn create_dual_stack(name: &str) -> Network {
        Network::create_in(
            name,
            test_subnet,
            Some(|index| format!("2001:db8:{:x}::/64", index % 16)),
        )
    }

    fn create_in(
        name: &str,
        subnet_for: fn(usize) -> String,
        ipv6_subnet_for: Option<fn(usize) -> String>,
    ) -> Network {
        let name = &scoped(name);
        // A test killed at its time limit never ran its `Drop`s, so its
        // containers are still attached and `network rm` refuses. Under
        // nextest's retries the next try is exactly that case: it then
        // failed with "network already exists" on every slice below.
        // Podman's `-f` removes the attached containers too. Docker's only
        // ignores a missing network, so there the leftovers are found and
        // removed, but only when the plain removal was refused: listing
        // containers by network is slow with many on the host.
        let removed = Command::new(runtime())
            .args(["network", "rm", "-f", name])
            .output();
        let refused = removed
            .is_ok_and(|out| String::from_utf8_lossy(&out.stderr).contains("active endpoints"));
        if refused {
            if let Ok(out) = Command::new(runtime())
                .args(["ps", "-aq", "--filter", &format!("network={name}")])
                .output()
            {
                for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                    let _ = Command::new(runtime()).args(["rm", "-f", id]).output();
                }
            }
            let _ = Command::new(runtime())
                .args(["network", "rm", name])
                .output();
        }

        // Retry across slices rather than pick one and hope: a network
        // left behind by an aborted run still holds its pool, and the
        // failure ("Pool overlaps with other one on this address space")
        // names neither which network nor which test.
        //
        // Offset by the process id, because under nextest every test is a
        // process of its own with its own `NEXT` starting at zero, and
        // they would all try the same slice first.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let mut last = String::new();
        for _ in 0..16 {
            let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + std::process::id() as usize;
            let mut args = vec![
                "network".to_string(),
                "create".to_string(),
                "--subnet".to_string(),
                subnet_for(index),
            ];
            if let Some(v6) = ipv6_subnet_for {
                args.extend(["--ipv6".to_string(), "--subnet".to_string(), v6(index)]);
            }
            args.push(name.to_string());
            let out = Command::new(runtime())
                .args(&args)
                .output()
                .expect("failed to create a container network");
            if out.status.success() {
                return Network {
                    name: name.to_string(),
                };
            }
            last = String::from_utf8_lossy(&out.stderr).to_string();
        }
        panic!(
            "{} network create failed for every subnet slice:\n{last}",
            runtime()
        );
    }
}

/// Runs a shell command inside a container, returning (exit status
/// success, stdout, stderr). The one `docker exec` every assertion in this
/// file flows through.
fn exec_in(container: &str, cmd: &str) -> (bool, String, String) {
    let out = Command::new(runtime())
        .args(["exec", container, "sh", "-c", cmd])
        .output()
        .expect("failed to exec in the container");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// [`exec_in`], asserting the command succeeded.
fn sh_in(container: &str, cmd: &str) -> String {
    let (ok, stdout, stderr) = exec_in(container, cmd);
    assert!(
        ok,
        "command failed: {cmd}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

/// A container's address on its network: `field` is `IPAddress` or
/// `GlobalIPv6Address`, as `inspect` names them.
fn address_of(container: &str, field: &str) -> String {
    let out = Command::new(runtime())
        .args([
            "inspect",
            "-f",
            &format!("{{{{range .NetworkSettings.Networks}}}}{{{{.{field}}}}}{{{{end}}}}"),
            container,
        ])
        .output()
        .expect("failed to inspect a container");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `docker rm -f`, for the `Drop` impls. Errors are ignored: a container
/// that is already gone is the outcome wanted.
fn remove_container(name: &str) {
    let _ = Command::new(runtime()).args(["rm", "-f", name]).output();
}

impl Drop for Network {
    fn drop(&mut self) {
        let _ = Command::new(runtime())
            .args(["network", "rm", &self.name])
            .output();
    }
}

/// A second container that only makes requests — the remote client whose
/// packets the server's rules are supposed to stop.
struct Client {
    name: String,
}

impl Client {
    fn start(name: &str, net: &Network) -> Client {
        build_image();
        let name = &scoped(name);
        let _ = Command::new(runtime()).args(["rm", "-f", name]).output();
        let out = Command::new(runtime())
            .args([
                "run",
                "-d",
                "--name",
                name,
                "--network",
                &net.name,
                &image(),
            ])
            .output()
            .expect("failed to start the client container");
        assert!(
            out.status.success(),
            "{} run (client) failed:\n{}",
            runtime(),
            String::from_utf8_lossy(&out.stderr)
        );
        Client {
            name: name.to_string(),
        }
    }

    /// This container's address on the network, as the server will see it.
    fn address(&self) -> String {
        address_of(&self.name, "IPAddress")
    }

    /// Its IPv6 address, on a [`Network::create_dual_stack`] network.
    fn address_v6(&self) -> String {
        address_of(&self.name, "GlobalIPv6Address")
    }

    /// Fetches `path` from `host` and returns the status code.
    fn get_path(&self, host: &str, path: &str, extra: &str) -> String {
        let out = Command::new(runtime())
            .args([
                "exec",
                &self.name,
                "sh",
                "-c",
                &format!(
                    "curl -s -o /dev/null -w '%{{http_code}}' {extra} http://{host}:8080{path}"
                ),
            ])
            .output()
            .expect("failed to run curl in the client container");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Fetches `/` from `host` and returns the status code, tolerating a
    /// curl failure the same way `Server::status` does.
    fn get(&self, host: &str, extra: &str) -> String {
        let out = Command::new(runtime())
            .args([
                "exec",
                &self.name,
                "sh",
                "-c",
                &format!("curl -s -o /dev/null -w '%{{http_code}}' {extra} http://{host}:8080/"),
            ])
            .output()
            .expect("failed to run curl in the client container");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        remove_container(&self.name);
    }
}

impl Server {
    fn start(name: &str) -> Server {
        Server::spawn(name, None)
    }

    fn start_on_network(name: &str, net: &Network) -> Server {
        Server::spawn(name, Some(&net.name))
    }

    fn spawn(name: &str, network: Option<&str>) -> Server {
        build_image();
        let name = &scoped(name);
        // Leftover from a previous aborted run.
        let _ = Command::new(runtime()).args(["rm", "-f", name]).output();

        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.to_string(),
            // nft needs to actually manipulate the container's own
            // netfilter tables.
            "--cap-add=NET_ADMIN".to_string(),
        ];
        if let Some(network) = network {
            args.push("--network".to_string());
            args.push(network.to_string());
        }
        args.push(image());
        let out = Command::new(runtime())
            .args(&args)
            .output()
            .expect("failed to start the container");
        assert!(
            out.status.success(),
            "{} run failed:\n{}",
            runtime(),
            String::from_utf8_lossy(&out.stderr)
        );
        let server = Server {
            name: name.to_string(),
        };
        server.sh("mkdir -p /var/www/test && echo '<h1>hello</h1>' > /var/www/test/index.html");
        server.sh("nginx");
        server
    }

    /// Runs a shell command inside the container, returning
    /// (exit status success, stdout, stderr).
    fn run(&self, cmd: &str) -> (bool, String, String) {
        exec_in(&self.name, cmd)
    }

    /// Runs a command and asserts it succeeded.
    fn sh(&self, cmd: &str) -> String {
        sh_in(&self.name, cmd)
    }

    /// Seeds one blocked bot, in the real well-known-bots source format,
    /// through the real `update-bot-lists` path — so the parser is
    /// exercised too rather than bypassed by writing rows directly.
    ///
    /// Category `ai`, not `scanner`: this source's parser hardcodes
    /// `is_scanner: false` (only the nginx-bad-bots list sets it), so a
    /// bot tagged `scanner` here would carry no flags at all and never be
    /// blocked. AI is blocked by the default policy, so nothing further is
    /// needed to make this bot actually blocked.
    fn seed_bot(&self, id: &str, pattern: &str) {
        let json = format!(
            r#"[{{"id":"{id}","categories":["ai"],"pattern":{{"accepted":["{pattern}"],"forbidden":[]}},"url":"https://example.invalid/{id}"}}]"#
        );
        self.sh(&format!("cat > /tmp/bots.json <<'JSON'\n{json}\nJSON"));
        self.stop_bots("update-bot-lists --source /tmp/bots.json");
    }

    /// Runs `stop-bots` inside the container against a fixed database.
    fn stop_bots(&self, args: &str) -> String {
        self.sh(&format!("stop-bots {args} --db /tmp/db.sqlite3"))
    }

    fn stop_bots_expect_failure(&self, args: &str) -> String {
        let (ok, stdout, stderr) = self.run(&format!("stop-bots {args} --db /tmp/db.sqlite3"));
        assert!(!ok, "expected failure but it succeeded: {args}\n{stdout}");
        format!("{stdout}{stderr}")
    }

    /// The HTTP status code NGINX returns for a request, made from inside
    /// the container against the real server.
    ///
    /// Uses `run` rather than `sh`: curl exits non-zero when the server
    /// closes the connection without replying, which is exactly what the
    /// `444` response is *supposed* to do. Its `%{http_code}` of `000` is
    /// the result, not a failure to collect one.
    fn status(&self, path: &str, extra_curl_args: &str) -> String {
        let (_, stdout, _) = self.run(&format!(
            "curl -s -o /dev/null -w '%{{http_code}}' {extra_curl_args} \
             http://127.0.0.1:8080{path}"
        ));
        stdout.trim().to_string()
    }

    /// The same, against another port — for the two-site tests, where the
    /// second `server` block is what has to behave differently.
    fn status_on(&self, port: u16, path: &str, extra_curl_args: &str) -> String {
        let (_, stdout, _) = self.run(&format!(
            "curl -s -o /dev/null -w '%{{http_code}}' {extra_curl_args} \
             http://127.0.0.1:{port}{path}"
        ));
        stdout.trim().to_string()
    }

    fn body(&self, path: &str, extra_curl_args: &str) -> String {
        self.sh(&format!(
            "curl -s {extra_curl_args} http://127.0.0.1:8080{path}"
        ))
    }

    /// Validates the live config the way an admin would, and returns
    /// NGINX's own message so a failure names the directive.
    fn nginx_t(&self) -> (bool, String) {
        let (ok, stdout, stderr) = self.run("nginx -t");
        (ok, format!("{stdout}{stderr}"))
    }

    /// Whether the loaded `inet stop_bots` table has `element` in `set` —
    /// asked of the kernel with `nft get element`, which for an interval
    /// set also answers for an address inside a listed range.
    fn in_set(&self, set: &str, element: &str) -> bool {
        self.run(&format!(
            "nft get element inet stop_bots {set} '{{ {element} }}'"
        ))
        .0
    }

    /// Renders the firewall for `backend` into `/tmp/fw` and loads it the
    /// way the apply does. `--force` because a container has no SSH log.
    fn render_and_load(&self, backend: &str) {
        self.stop_bots(&format!(
            "render-firewall --backend {backend} --out /tmp/fw --force"
        ));
        let load = if backend == "nftables" {
            "nft -f /tmp/fw"
        } else {
            "sh /tmp/fw"
        };
        self.sh(load);
    }

    fn apply_and_reload(&self) {
        self.stop_bots("apply-blocks --root /etc/nginx/sites-enabled --no-reload");
        let (ok, output) = self.nginx_t();
        assert!(ok, "the generated config does not parse:\n{output}");
        self.sh("nginx -s reload");
        // Reload is asynchronous; give the worker a moment to swap in.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        remove_container(&self.name);
    }
}

// ---- a real Debian host, with a real init ----

/// A container running systemd as PID 1, cleaned up on drop.
///
/// Separate from [`Server`] because the privileges differ and the boot
/// cost is real: this one needs `SYS_ADMIN` and an unconfined seccomp
/// profile, and takes a couple of seconds to reach `running`. Tests that
/// only need NGINX and `nft` should keep using `Server`, which needs
/// neither.
struct Host {
    name: String,
}

/// Where `stop-bots install web` puts the console's database on a host.
const HOST_DB: &str = "/var/lib/stop-bots/db.sqlite3";

impl Host {
    /// A booted host with NGINX running — every test here wants that
    /// much, so it is not a line they each repeat. See [`Host::installed`]
    /// for the next step most of them take.
    fn start(name: &str) -> Host {
        let host = Host::boot(name);
        host.sh("systemctl start nginx");
        host
    }

    /// [`Host::start`] with the console installed as a service, the way
    /// `stop-bots install web` leaves a real host. For the tests that
    /// exercise the *installed* host rather than the install itself.
    fn installed(name: &str) -> Host {
        let host = Host::start(name);
        host.sh("stop-bots install web");
        host
    }

    /// [`Host::start`] with `install web --no-start`: the user, the
    /// database and the three units in place, nothing running. For the
    /// tests about what the units' sandboxes allow, which run their own
    /// probes under them and need no console.
    fn units_installed(name: &str) -> Host {
        let host = Host::start(name);
        host.sh("stop-bots install web --no-start");
        host
    }

    /// The `stop-bots` user's uid, as the host has it.
    fn console_uid(&self) -> String {
        self.sh("id -u stop-bots").trim().to_string()
    }

    /// Runs `stop-bots` on the host against the installed console's
    /// database.
    fn stop_bots(&self, args: &str) -> String {
        self.sh(&format!("stop-bots {args} --db {HOST_DB}"))
    }

    fn boot(name: &str) -> Host {
        build_host_image();
        Host::boot_image(name, &host_image(), None)
    }

    /// Boots `image` as a host, on `network` if one is given. Every
    /// systemd image here boots the same way; only what is in it differs.
    fn boot_image(name: &str, image: &str, network: Option<&Network>) -> Host {
        let name = &scoped(name);
        // Leftover from a previous aborted run.
        let _ = Command::new(runtime()).args(["rm", "-f", name]).output();

        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.to_string(),
            // systemd needs to mount things: every `Protect*` and
            // `Private*` directive in the generated unit is a mount
            // namespace, and without this they are silently not
            // applied — which would make every assertion below pass
            // for the wrong reason.
            "--cap-add=SYS_ADMIN".to_string(),
            // `nft` and `iptables` manipulate the container's own
            // netfilter tables, same as `Server`.
            "--cap-add=NET_ADMIN".to_string(),
            // The outer seccomp profile blocks syscalls systemd needs to
            // boot at all. Turning it off does *not* weaken what is under
            // test: `RestrictAddressFamilies` is enforced by a seccomp
            // filter systemd installs itself, inside the unit, and it was
            // verified to still refuse AF_NETLINK with this off — under
            // both runtimes.
            "--security-opt".to_string(),
            "seccomp=unconfined".to_string(),
        ];

        // How the two runtimes are told to host an init, and the one
        // place in this file where they genuinely differ.
        //
        // Podman knows what a systemd container needs and sets it up
        // itself: `--systemd=always` gives the container a writable
        // cgroup hierarchy of its own plus the tmpfs mounts on /run and
        // /run/lock. Docker has no such flag, so the same conditions have
        // to be assembled by hand — and bind-mounting the host's
        // /sys/fs/cgroup read-write, which is what that takes, is
        // precisely the kind of access rootless Podman exists to avoid.
        // Asking for it there fails outright rather than degrading.
        if runtime_is_podman() {
            args.push("--systemd=always".to_string());
        } else {
            // AppArmor confines the Docker daemon's containers; there is
            // no equivalent profile applied in the rootless case.
            args.push("--security-opt".to_string());
            args.push("apparmor=unconfined".to_string());
            args.push("--cgroupns=host".to_string());
            args.push("-v".to_string());
            args.push("/sys/fs/cgroup:/sys/fs/cgroup:rw".to_string());
            args.push("--tmpfs".to_string());
            args.push("/run".to_string());
            args.push("--tmpfs".to_string());
            args.push("/run/lock".to_string());
        }
        if let Some(network) = network {
            args.push("--network".to_string());
            args.push(network.name.clone());
        }
        args.push(image.to_string());

        let out = Command::new(runtime())
            .args(&args)
            .output()
            .expect("failed to start the host container");
        assert!(
            out.status.success(),
            "{} run (host) failed:\n{}",
            runtime(),
            String::from_utf8_lossy(&out.stderr)
        );

        let host = Host {
            name: name.to_string(),
        };
        host.wait_for_boot();
        host
    }

    /// Blocks until systemd says the system is up.
    ///
    /// `degraded` counts: this image has units masked out of the boot
    /// (see `Dockerfile.host`), and a masked unit is a failed job as far
    /// as `is-system-running` is concerned. What matters is that the
    /// manager is accepting jobs, which both states mean.
    fn wait_for_boot(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            let (_, stdout, stderr) = self.run("systemctl is-system-running");
            last = format!("{stdout}{stderr}");
            match last.trim() {
                "running" | "degraded" => return,
                _ => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        }
        panic!("systemd never finished booting; last state was {last:?}");
    }

    /// Runs a shell command inside the container, returning
    /// (exit status success, stdout, stderr).
    fn run(&self, cmd: &str) -> (bool, String, String) {
        exec_in(&self.name, cmd)
    }

    /// Waits for `path` to appear inside the container, up to 30s.
    ///
    /// The deploy tests prove a restart re-executed the *new* binary by
    /// having it `touch` a marker file. `systemctl` reporting the unit
    /// `active` does not mean that has happened yet: these units are
    /// `Type=simple`, where systemd calls the service started as soon as
    /// it has forked the process — everything the process then does,
    /// including running the first line of a shell wrapper, happens after
    /// the `systemctl` command has already returned. The gap is
    /// microseconds on an idle machine and wide enough to lose on a
    /// loaded one, which is how this reached CI as an occasional
    /// "re-executed the old binary" with nothing wrong: found here by
    /// running the suite while a full rebuild competed for the same cores.
    ///
    /// Only for assertions that something *will* appear. A test asserting
    /// a marker is absent must not wait, or it proves nothing — see the
    /// negative check in `redeploying_over_a_running_console_restarts_it`.
    fn wait_for_file(&self, path: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if self.run(&format!("test -e {path}")).0 {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// [`Self::run`], retried while SQLite says the database is locked.
    ///
    /// A CLI command and the console service are two processes sharing one
    /// file, which is the arrangement this project is built around: `Db`
    /// sets a five-second busy timeout precisely so the short collisions
    /// between them resolve by waiting. That is enough on any real host and
    /// enough here on an idle machine — but this suite runs a dozen
    /// containers at once, and on an oversubscribed two-core CI runner the
    /// same five seconds can elapse inside one query's scheduling gaps. The
    /// suite was three times red on CI and green locally until a loaded
    /// machine reproduced it here in one run.
    ///
    /// So the retry is about the test harness's own contention, not about
    /// papering over a lock bug: these assertions are about what the report
    /// *says*, and a reader that lost a race has not said anything yet.
    /// Raising the production timeout to suit a test bench would be the
    /// wrong direction — five seconds is chosen so that a genuine deadlock
    /// still surfaces as an error instead of a hang.
    fn run_uncontended(&self, cmd: &str) -> (bool, String, String) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let (ok, stdout, stderr) = self.run(cmd);
            let said = format!("{stdout}{stderr}");
            if !said.contains("database is locked") {
                return (ok, stdout, stderr);
            }
            if std::time::Instant::now() >= deadline {
                panic!("`{cmd}` never got the database lock in 30s:\n{said}");
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    /// Runs a command and asserts it succeeded.
    fn sh(&self, cmd: &str) -> String {
        sh_in(&self.name, cmd)
    }

    /// One property of a unit, as systemd itself reports it.
    ///
    /// Read through `systemctl show` rather than by parsing `systemctl
    /// status`: the status text is for humans and changes between
    /// versions, while these property names are stable and the values are
    /// exactly what the manager decided.
    fn unit(&self, unit: &str, property: &str) -> String {
        self.sh(&format!("systemctl show -p {property} --value {unit}"))
            .trim()
            .to_string()
    }

    /// What the journal has for a unit — the only place a sandbox refusal
    /// leaves its reason, and the reason is the whole point of these
    /// tests.
    fn journal(&self, unit: &str) -> String {
        let (_, stdout, stderr) = self.run(&format!("journalctl -u {unit} --no-pager -o cat"));
        format!("{stdout}{stderr}")
    }

    /// Starts a unit without asserting it worked, for the cases where
    /// failing *is* the expected outcome.
    fn try_start(&self, unit: &str) -> bool {
        self.run(&format!("systemctl start {unit}")).0
    }

    /// The live nftables ruleset.
    fn ruleset(&self) -> String {
        self.sh("nft list ruleset")
    }

    /// Copies a file from the working tree into the container.
    ///
    /// Not into `/tmp`: systemd mounts a tmpfs there during boot, which
    /// shadows whatever `docker cp` wrote into the image's own `/tmp`.
    /// The copy reports success and the file is not there.
    fn put(&self, local: &str, remote: &str) {
        let out = Command::new(runtime())
            .args(["cp", local, &format!("{}:{remote}", self.name)])
            .output()
            .expect("failed to copy into the container");
        assert!(
            out.status.success(),
            "{} cp {local} failed:\n{}",
            runtime(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Blocks until the console answers, meaning its start-up writes to
    /// the database are done.
    ///
    /// Needed because `Db::open` sets no `busy_timeout`, so a second
    /// process that wants the database *while the console is still
    /// registering sources and running due cron jobs* gets an immediate
    /// "database is locked" rather than waiting a moment. That is a real
    /// rough edge — see TODO.md — and this poll keeps these tests from
    /// racing it. It is not hiding it: nothing here would have found it,
    /// and a flaky test would have hidden it better.
    fn wait_for_console(&self) {
        self.wait_for_console_at("");
    }

    /// The same, for a console serving under a path prefix.
    ///
    /// A prefixed console does not answer on `/login` at all — the prefix
    /// is part of every route it matches, which is the whole reason it has
    /// to be told about one. Polling the unprefixed path after a Web
    /// Access change is how this helper first reported a healthy console
    /// as dead.
    fn wait_for_console_at(&self, base: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let (ok, code, _) = self.run(&format!(
                "curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:8787{base}/login"
            ));
            if ok && code.trim().starts_with('2') {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        panic!(
            "the console never answered on 127.0.0.1:8787{base}/login. journal:\n{}",
            self.journal("stop-bots-web.service")
        );
    }

    /// Installs the console and logs into it, returning a handle that can
    /// post actions the way a browser does.
    ///
    /// The password comes from the installer's own output, which is the
    /// only place it is ever shown — so this also exercises the claim that
    /// an operator can actually get in with what they were handed.
    ///
    /// Installed with an SSH log, as on a real Debian host: the console
    /// refuses to *apply* the firewall when it cannot read one, because the
    /// anti-lockout check could not run. The container runs no sshd, so the
    /// log is empty — readable, with nobody connected.
    fn console(&self) -> Console {
        let out =
            self.sh("touch /var/log/auth.log && stop-bots install web --ssh-log /var/log/auth.log");
        let password = out
            .lines()
            .map(str::trim)
            .find(|line| line.len() > 20 && !line.contains(' '))
            .unwrap_or_else(|| panic!("no password in the installer output:\n{out}"))
            .to_string();
        self.wait_for_console();
        self.login(&password)
    }

    /// Logs into the running console with `password`.
    fn login(&self, password: &str) -> Console {
        // A cookie jar, so the session survives across calls exactly as it
        // would in a browser.
        let login = self.sh(&format!(
            "curl -s -o /dev/null -w '%{{http_code}}' -c /tmp/jar              --data-urlencode 'password={password}' http://127.0.0.1:8787/login"
        ));
        assert!(
            login.trim().starts_with('2') || login.trim().starts_with('3'),
            "logging in with the installer's own password returned {login}"
        );
        Console {
            name: self.name.clone(),
        }
    }

    /// The stored console password hash, read with the tool's own
    /// database rather than by poking at SQLite's file format.
    ///
    /// With a lock timeout, like every `sqlite3` read here: the console
    /// this installed is registering sources and running due cron jobs in
    /// the same moment, and without one the CLI gives up at once with
    /// "database is locked" instead of waiting its turn.
    fn password_hash(&self) -> String {
        self.sh(&format!(
            "sqlite3 -cmd '.timeout 5000' {HOST_DB} \"select value from settings where key like 'web:password%'\""
        ))
        .trim()
        .to_string()
    }

    /// Seeds one blocked bot through the real parser, so an apply has
    /// something to write. Same format and same reasoning as
    /// [`Server::seed_bot`].
    fn seed_bot(&self, id: &str, pattern: &str) {
        let json = format!(
            r#"[{{"id":"{id}","categories":["ai"],"pattern":{{"accepted":["{pattern}"],"forbidden":[]}},"url":"https://example.invalid/{id}"}}]"#
        );
        self.sh(&format!("cat > /tmp/bots.json <<'JSON'\n{json}\nJSON"));
        self.stop_bots("update-bot-lists --source /tmp/bots.json");
    }

    /// Runs `command` as a oneshot unit carrying **the generated web
    /// unit's own sandbox** — as the console's user, with the console's
    /// groups — and returns whether it succeeded.
    ///
    /// The unit is derived from `/etc/systemd/system/stop-bots-web.service`
    /// as `install web` wrote it — every `User=`, `Protect*`, `Restrict*`
    /// and `Private*` line is carried over verbatim, and only `ExecStart`
    /// and `Type` are replaced. That is the whole point: a hand-written unit
    /// listing the directives this test expects would pass forever,
    /// including on the day the generated one stops emitting one of them.
    ///
    /// `mangle` edits the derived unit before it is installed, so a test
    /// can prove its own teeth by removing a directive and watching the
    /// command fail.
    fn oneshot_under_web_sandbox(&self, probe: &str, command: &str, mangle: &str) -> bool {
        self.oneshot_under_sandbox_of("stop-bots-web.service", probe, command, mangle)
    }

    /// The same under the root helper's sandbox, which is where applying
    /// happens since the console stopped being root.
    fn oneshot_under_helper_sandbox(&self, probe: &str, command: &str, mangle: &str) -> bool {
        self.oneshot_under_sandbox_of("stop-bots-helper.service", probe, command, mangle)
    }

    /// [`Self::oneshot_under_web_sandbox`] for any unit `install` wrote.
    ///
    /// Its `Condition*` lines go too: the firewall unit's
    /// `ConditionPathExists` would otherwise skip a probe whose script is
    /// not there, and systemd counts a skipped start as a successful one.
    ///
    /// The command goes into the unit from a file (`sed`'s `r`) rather
    /// than through a substitution, so it may hold `&`, `|` and quotes;
    /// systemd still splits it, so `$` has to be `$$`.
    fn oneshot_under_sandbox_of(
        &self,
        unit: &str,
        probe: &str,
        command: &str,
        mangle: &str,
    ) -> bool {
        self.sh(&format!(
            "cat > /run/{probe}.exec <<'STOP_BOTS_PROBE'\nExecStart={command}\nSTOP_BOTS_PROBE"
        ));
        self.sh(&format!(
            "set -e\n\
             sed -e '/^ExecStart=/{{r /run/{probe}.exec' -e 'd;}}' \
                 -e 's|^Type=.*|Type=oneshot|' \
                 -e 's|^Restart=.*||' \
                 -e '/^Condition/d' \
                 /etc/systemd/system/{unit} {mangle} \
                 > /etc/systemd/system/{probe}.service\n\
             systemctl daemon-reload",
        ));
        self.try_start(&format!("{probe}.service"))
    }

    /// Runs `script` with `sh` **as the console's own unit**:
    /// `stop-bots-web.service` exactly as `install web` wrote it, with a
    /// drop-in that replaces only its `ExecStart=` (and makes it a oneshot
    /// that is not restarted). Every other line is the real unit's, so
    /// nothing a copy could get wrong stands between the probe and what is
    /// under test. Returns whether the script exited 0, and what it
    /// printed.
    ///
    /// `background` starts it without waiting, for a probe that has to
    /// be running while the test changes something on the host; then
    /// [`Self::console_probe_result`] collects it.
    fn probe_in_console_unit(&self, probe: &str, script: &str) -> (bool, String) {
        self.start_console_probe(probe, script, false);
        self.console_probe_result(probe)
    }

    fn start_console_probe(&self, probe: &str, script: &str, background: bool) {
        // The output goes where the console may write: its own directory.
        self.sh(&format!(
            "cat > /run/{probe}.sh <<'STOP_BOTS_PROBE'\n\
             exec > /var/lib/stop-bots/{probe}.out 2>&1\n\
             {script}\n\
             STOP_BOTS_PROBE\n\
             chmod 644 /run/{probe}.sh\n\
             rm -f /var/lib/stop-bots/{probe}.out /var/lib/stop-bots/{probe}.status\n\
             mkdir -p /etc/systemd/system/stop-bots-web.service.d\n\
             cat > /etc/systemd/system/stop-bots-web.service.d/zz-probe.conf <<'STOP_BOTS_PROBE'\n\
             [Service]\n\
             Type=oneshot\n\
             Restart=no\n\
             ExecStart=\n\
             ExecStart=/bin/sh -c '/bin/sh /run/{probe}.sh; echo $$? > /var/lib/stop-bots/{probe}.status'\n\
             STOP_BOTS_PROBE\n\
             systemctl daemon-reload\n\
             systemctl reset-failed stop-bots-web.service 2>/dev/null || true"
        ));
        let block = if background { "--no-block " } else { "" };
        self.sh(&format!("systemctl start {block}stop-bots-web.service"));
    }

    /// Waits for the probe [`Self::start_console_probe`] started to
    /// finish, takes the drop-in away again, and returns what it did.
    fn console_probe_result(&self, probe: &str) -> (bool, String) {
        let status = format!("/var/lib/stop-bots/{probe}.status");
        assert!(
            self.wait_for_file(&status),
            "the probe {probe} never finished. journal:\n{}",
            self.journal("stop-bots-web.service")
        );
        let code = self.sh(&format!("cat {status}"));
        let out = self.run(&format!("cat /var/lib/stop-bots/{probe}.out")).1;
        self.sh(
            "rm /etc/systemd/system/stop-bots-web.service.d/zz-probe.conf \
             && systemctl daemon-reload",
        );
        (code.trim() == "0", out)
    }
}

/// A logged-in session against the console running inside a [`Host`].
///
/// Drives it over real HTTP through the container's own `curl`, rather
/// than through the router in-process the way `tests/web.rs` does. That is
/// the point: this is the only place the console is a listening server
/// with a session cookie, talking to a real NGINX and a real `nft`.
struct Console {
    name: String,
}

impl Console {
    fn run(&self, cmd: &str) -> (bool, String, String) {
        exec_in(&self.name, cmd)
    }

    /// Fetches a page as the logged-in user.
    fn get(&self, path: &str) -> String {
        let (ok, stdout, stderr) =
            self.run(&format!("curl -s -b /tmp/jar http://127.0.0.1:8787{path}"));
        assert!(ok, "GET {path} failed:\n{stderr}");
        stdout
    }

    /// The CSRF token currently on `path`. Every form carries one, and a
    /// post without it is refused — so reading it back out is part of
    /// behaving like a browser, not a way around the check.
    fn csrf(&self, path: &str) -> String {
        let body = self.get(path);
        let marker = r#"name="csrf" value=""#;
        let start = body
            .find(marker)
            .unwrap_or_else(|| panic!("no csrf token on {path}"))
            + marker.len();
        body[start..]
            .split('"')
            .next()
            .expect("an unterminated csrf value")
            .to_string()
    }

    /// Posts a form the way the page's own button would, token included.
    /// Returns the flash message the console redirected to.
    fn post(&self, path: &str, fields: &[(&str, &str)]) -> String {
        let csrf = self.csrf("/");
        let mut args = format!("--data-urlencode 'csrf={csrf}'");
        for (key, value) in fields {
            args.push_str(&format!(" --data-urlencode '{key}={value}'"));
        }
        let (ok, stdout, stderr) = self.run(&format!(
            "curl -s -L -b /tmp/jar -c /tmp/jar {args} http://127.0.0.1:8787{path}"
        ));
        assert!(ok, "POST {path} failed:\n{stderr}");
        stdout
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        remove_container(&self.name);
    }
}

// ---- `install web`: the unit, and the sandbox it puts the service in ----

/// The install path an operator actually runs, on a host that has an init
/// to talk to.
///
/// Every part of this was unit-tested against a fake `systemctl` before
/// today, and all of it passed while the real thing failed on a real host
/// — twice. The difference is that nothing was asking systemd.
#[test]
fn install_web_writes_a_unit_that_systemd_actually_starts() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-install");

    let out = host.sh("stop-bots install web");
    assert!(
        out.contains("stop-bots-web.service"),
        "install said nothing about the unit:\n{out}"
    );

    // The installer must be the one that generated the password, because
    // it is the only one that shows it to anybody. `install web` used to
    // start the service *before* doing its database work, so the service
    // could win the race, generate a password of its own, and leave the
    // installer reporting "a console password is already set, keeping it"
    // — an install that completes and hands the operator nothing. The
    // other two ways that race landed were a service that crash-looped on
    // "database is locked" and an install that failed after having already
    // enabled the unit.
    assert!(
        out.contains("Console password:"),
        "the installer did not generate and print the password:\n{out}"
    );

    assert_eq!(host.unit("stop-bots-web.service", "LoadState"), "loaded");
    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "active",
        "the service is not running. journal:\n{}",
        host.journal("stop-bots-web.service")
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "UnitFileState"),
        "enabled",
        "`enable --now` did not enable it, so it would not come back on boot"
    );

    // The resource limits reach the running process, not only the file:
    // a console on a 1 GB VPS must yield to the NGINX it protects.
    let pid = host.unit("stop-bots-web.service", "MainPID");
    let nice = host.sh(&format!("cat /proc/{pid}/stat | cut -d' ' -f19"));
    assert_eq!(nice.trim(), "10", "the console does not run at Nice=10");
    assert_eq!(
        host.unit("stop-bots-web.service", "IOSchedulingClass"),
        "3",
        "the console's I/O class is not idle (3)"
    );
    let high = host.unit("stop-bots-web.service", "MemoryHigh");
    assert!(
        high.parse::<u64>().is_ok(),
        "MemoryHigh is not a byte limit, so nothing throttles the console: {high}"
    );

    // A descriptor per held connection: the default soft limit of 1024
    // ran out at about 1,100 of them.
    let limits = host.sh(&format!("grep 'Max open files' /proc/{pid}/limits"));
    assert!(
        limits.split_whitespace().nth(3) == Some("16384"),
        "the console's soft descriptor limit is not 16384:\n{limits}"
    );
    assert_eq!(host.unit("stop-bots-web.service", "TasksMax"), "1024");
}

/// **The netlink bug, reproduced and then fixed, in one test.** Under the
/// root helper's unit since 0.1, which is what applies the firewall now.
///
/// `nft` and Debian's nft-backed `iptables` reach the kernel over a
/// netlink socket. The generated unit used to write
/// `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6` under a comment
/// asserting the service never applies the firewall — which stopped being
/// true the moment applying was added. The failure on the real host was
/// `Unable to initialize Netlink socket: Address family not supported by
/// protocol`, which names neither systemd nor this project.
///
/// The negative control is not decoration. Without it this test passes
/// whether or not the sandbox is being enforced at all, and "the sandbox
/// silently did not apply" is the one way a container can lie about this.
#[test]
fn the_generated_sandbox_lets_the_firewall_reach_netlink() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-netlink");

    host.stop_bots("add-firewall-rule --address 203.0.113.9");
    // `batch --apply` rather than a bespoke verb: it is the only thing
    // that applies the firewall unattended, and it is what a real cron
    // entry runs — so this probes the sandbox through the same call the
    // host does. `--force` because a container has no SSH log for the
    // lockout guard to read, `--no-fetch` to keep the run offline.
    let apply = format!(
        "/usr/local/bin/stop-bots batch --apply --force --no-fetch \
         --root /etc/nginx/sites-enabled --out /etc/stop-bots/firewall.nft --db {HOST_DB}"
    );

    // First the control: strip AF_NETLINK back out, and the apply must
    // fail the way the real host did.
    let without = host.oneshot_under_helper_sandbox(
        "probe-without-netlink",
        &apply,
        "| sed -e 's/ AF_NETLINK//'",
    );
    assert!(
        !without,
        "stripping AF_NETLINK from the unit did not break the apply, so this \
         test cannot tell whether the sandbox is enforced at all"
    );
    let journal = host.journal("probe-without-netlink.service");
    assert!(
        journal.contains("Netlink"),
        "the failure was not the netlink one, so the control proves nothing:\n{journal}"
    );

    // Then the unit as generated: the same command, through the same
    // sandbox, has to work.
    let with = host.oneshot_under_helper_sandbox("probe-with-netlink", &apply, "");
    assert!(
        with,
        "the generated unit's sandbox blocks the firewall apply. journal:\n{}",
        host.journal("probe-with-netlink.service")
    );
    assert!(
        host.ruleset().contains("203.0.113.9"),
        "the apply reported success but the rule is not in the live ruleset"
    );
}

/// **The `install web` bug, reproduced from the report.**
///
/// A binary sitting in `/root` and a unit setting `ProtectHome=yes` is a
/// service that dies with `status=203/EXEC` and "No such file or
/// directory" for a file that is plainly there. The fix is a preflight
/// refusal that names the directive; this checks both halves, because a
/// refusal for a danger that is not real would be just as wrong.
#[test]
fn installing_from_a_hidden_directory_is_refused_before_systemd_can_fail() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-hidden");
    host.sh("cp /usr/local/bin/stop-bots /root/stop-bots");

    let (ok, stdout, stderr) = host.run("/root/stop-bots install web");
    let said = format!("{stdout}{stderr}");
    assert!(!ok, "install from /root should have been refused:\n{said}");
    assert!(
        said.contains("ProtectHome=yes"),
        "the refusal must name the directive, or it is as useless as systemd's own report:\n{said}"
    );
    assert!(
        said.contains("203/EXEC"),
        "the refusal must name the failure it is preventing:\n{said}"
    );
    assert!(
        said.contains("/usr/local/bin/stop-bots"),
        "the refusal must give a command to paste:\n{said}"
    );

    // Now prove the danger is real rather than folklore: install properly,
    // then run the /root copy through the sandbox the generated unit
    // actually sets. This is the failure the check above exists to stop.
    // The helper's unit: it is root, so only `ProtectHome=` can be what
    // hides /root from it; the console's user could not enter /root anyway.
    host.sh("stop-bots install web --no-start");
    let ran =
        host.oneshot_under_helper_sandbox("probe-root-binary", "/root/stop-bots --version", "");
    assert!(
        !ran,
        "the generated unit does not hide /root, so the preflight refusal guards nothing"
    );
    assert_eq!(
        host.unit("probe-root-binary.service", "ExecMainStatus"),
        "203",
        "expected 203/EXEC. journal:\n{}",
        host.journal("probe-root-binary.service")
    );
}

/// `ProtectSystem=strict` makes `/etc` read-only, and the helper's unit
/// gives `/etc/nginx` back with `ReadWritePaths=`. Without that line the first
/// apply fails "an hour after the unit started cleanly". This runs a real
/// apply through the real sandbox, and then through the same sandbox with
/// that one line taken out, so the line is a test result.
#[test]
fn the_sandbox_still_lets_the_service_rewrite_nginx_config() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-protectsystem");

    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    host.seed_bot("badbot", "BadBot");
    let apply = format!("/usr/local/bin/stop-bots apply-blocks --root /etc/nginx/sites-enabled --no-reload --db {HOST_DB}");

    let strict = host.oneshot_under_helper_sandbox(
        "probe-without-etc-nginx",
        &apply,
        "| sed -e '/^ReadWritePaths=-\\/etc\\/nginx$/d'",
    );
    assert!(
        !strict,
        "taking /etc/nginx out of ReadWritePaths did not stop the apply, so either \
         the sandbox is not enforced or that line is not what allows it"
    );
    let journal = host.journal("probe-without-etc-nginx.service");
    assert!(
        journal.contains("Read-only file system"),
        "the apply failed, but not on the read-only /etc/nginx:\n{journal}"
    );

    let asgenerated = host.oneshot_under_helper_sandbox("probe-as-generated", &apply, "");
    assert!(
        asgenerated,
        "the generated sandbox blocks the apply it exists to allow. journal:\n{}",
        host.journal("probe-as-generated.service")
    );
    let applied = host.sh("cat /etc/nginx/sites-enabled/test-site.conf");
    assert!(
        applied.contains("BEGIN stop-bots"),
        "the apply reported success but wrote no block into the site config:\n{applied}"
    );
    assert!(
        applied.contains("BadBot"),
        "the block is there but does not carry the seeded pattern:\n{applied}"
    );
}

/// **The `make deploy` bug.** A new binary lands, everything reports
/// success, and the console keeps serving the old code because nothing
/// restarted the unit.
///
/// The marker is a wrapper rather than `--version`, for the reason the
/// Makefile now says out loud: two builds of the same `0.0.x` are
/// indistinguishable by version, so the only honest evidence is that the
/// *new file* is what got executed.
#[test]
fn replacing_the_binary_and_restarting_runs_the_new_one() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-redeploy");

    let before = host.unit("stop-bots-web.service", "MainPID");
    assert_ne!(before, "0", "the service is not running to begin with");

    // Stand in for `make deploy`: write beside the running binary, then
    // rename over it.
    //
    // Not a `>` redirect, and that is the point — truncating a running
    // executable fails with `Text file busy`, which is exactly why the
    // Makefile scps to `$(DEPLOY_PATH).new` and moves it into place. A
    // rename swaps the directory entry and leaves the running image
    // alone, which is also why the old process keeps serving old code
    // until something restarts it.
    host.sh("cp /usr/local/bin/stop-bots /usr/local/bin/stop-bots.real");
    host.sh("printf '#!/bin/sh\\ntouch /var/lib/stop-bots/new-build-ran\\nexec /usr/local/bin/stop-bots.real \"$@\"\\n' > /usr/local/bin/stop-bots.new");
    host.sh("chmod 755 /usr/local/bin/stop-bots.new");
    host.sh("mv /usr/local/bin/stop-bots.new /usr/local/bin/stop-bots");
    assert!(
        !host.run("test -e /var/lib/stop-bots/new-build-ran").0,
        "the marker exists before the restart, so it proves nothing"
    );

    host.sh("systemctl try-restart stop-bots-web.service");

    let after = host.unit("stop-bots-web.service", "MainPID");
    assert_ne!(
        before, after,
        "try-restart left the old process running, which is the bug exactly"
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "active",
        "the service did not come back. journal:\n{}",
        host.journal("stop-bots-web.service")
    );
    assert!(
        host.wait_for_file("/var/lib/stop-bots/new-build-ran"),
        "the service restarted but re-executed the old binary. journal:\n{}",
        host.journal("stop-bots-web.service")
    );
}

/// The built-in bot list has to reach a host that only ever runs the
/// console — which is every host `install web` sets up.
///
/// It did not. `register_all_sources` both registers the four sources and
/// *stores* the one compiled into the binary, and it was called from the
/// TUI's startup and nowhere else. A production host running
/// `stop-bots web` therefore had no `stop-bots-extras` source row, no
/// entries, and none of its patterns in the rendered config — including
/// Let's Encrypt, which the humans-only mode relies on being allowed.
///
/// Invisible from inside: nothing failed, because nothing was asked to.
/// The source simply was not in the database for any screen to report on,
/// and the list had been derived from the access log of the very host that
/// was not using it. Only an end-to-end check catches a missing call.
#[test]
fn the_console_stores_the_built_in_bot_list_on_startup() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-builtin-list");
    host.wait_for_console();

    let entries = host.sh(&format!(
        "sqlite3 -cmd '.timeout 5000' {HOST_DB} \"select count(*) from bot_source_entries where source_id = 'stop-bots-extras'\""
    ));
    let entries: i64 = entries.trim().parse().unwrap_or(0);
    assert!(
        entries > 0,
        "the console started without storing the built-in list; \
         bot_source_entries held {entries} row(s) for it"
    );

    // And the entry the humans-only allowlist is built around is one of
    // them, by pattern rather than by slug: the slug is this project's own
    // naming, the pattern is what actually has to match a request.
    let le = host.sh(&format!(
        "sqlite3 -cmd '.timeout 5000' {HOST_DB} \"select count(*) from bots where user_agent_pattern like '%Let%Encrypt%'\""
    ));
    assert_eq!(
        le.trim(),
        "1",
        "Let's Encrypt is not in the merged bot list: {le}"
    );
}

/// **The deploy half of the same bug.** `make deploy` pipes
/// `scripts/deploy-remote.sh` to the target over ssh; this runs that exact
/// file against a real systemd, because it is the only part of a deploy
/// that decides whether the host keeps serving the old build.
///
/// The case that matters is a unit that is *enabled but not running* —
/// where systemd is meant to be running the console and currently is not,
/// which is precisely where a crash-looping service ends up. `try-restart`
/// does nothing at all to such a unit, so the old script would have landed
/// the new binary and left the console down.
#[test]
fn deploying_onto_a_stopped_console_starts_it_again() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-deploy-stopped");
    host.wait_for_console();
    host.put("scripts/deploy-remote.sh", "/opt/deploy-remote.sh");

    // The state a crash-loop leaves behind: enabled, so systemd is
    // supposed to be running it, but not running.
    host.sh("systemctl stop stop-bots-web.service");
    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "inactive"
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "UnitFileState"),
        "enabled"
    );

    // A deploy arrives, exactly as `make deploy` stages it.
    host.sh("cp /usr/local/bin/stop-bots /usr/local/bin/stop-bots.real");
    host.sh("printf '#!/bin/sh\\ntouch /run/deployed-build-ran\\nexec /usr/local/bin/stop-bots.real \"$@\"\\n' > /usr/local/bin/stop-bots.new");
    let out = host.sh("sh /opt/deploy-remote.sh /usr/local/bin/stop-bots stop-bots-web.service");

    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "active",
        "the deploy left the console down. it said:\n{out}\njournal:\n{}",
        host.journal("stop-bots-web.service")
    );
    assert!(
        host.wait_for_file("/run/deployed-build-ran"),
        "the console came back but re-executed the old binary. it said:\n{out}\njournal:\n{}",
        host.journal("stop-bots-web.service")
    );
    assert!(
        out.contains("unit state: active"),
        "the deploy did not report the state it left the unit in:\n{out}"
    );
}

/// The other direction: a host where the console is run by hand has the
/// unit installed but disabled, and a deploy must not start a second copy
/// competing for the port.
#[test]
fn deploying_does_not_start_a_console_nobody_asked_systemd_to_run() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-deploy-byhand");
    host.wait_for_console();
    host.put("scripts/deploy-remote.sh", "/opt/deploy-remote.sh");

    host.sh("systemctl disable --now stop-bots-web.service");
    host.sh("cp /usr/local/bin/stop-bots /usr/local/bin/stop-bots.new");
    let out = host.sh("sh /opt/deploy-remote.sh /usr/local/bin/stop-bots stop-bots-web.service");

    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "inactive",
        "a deploy started a service that was deliberately disabled:\n{out}"
    );
}

/// Re-running `install web` must not silently replace an edited unit, and
/// must replace it with `--force`. An operator who tuned a directive and
/// lost it to a routine re-install finds out at the next reboot.
#[test]
fn reinstalling_leaves_an_edited_unit_alone_unless_forced() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-reinstall");
    host.wait_for_console();
    host.sh("printf '\\n# operator edit\\n' >> /etc/systemd/system/stop-bots-web.service");

    // A hard refusal, not a warning: re-running the installer is a
    // routine thing to do, and quietly reverting a directive someone
    // tuned would only be discovered at the next reboot.
    let (ok, stdout, stderr) = host.run("stop-bots install web");
    let said = format!("{stdout}{stderr}");
    assert!(
        !ok,
        "a plain re-install over an edited unit should fail:\n{said}"
    );
    assert!(
        host.sh("cat /etc/systemd/system/stop-bots-web.service")
            .contains("# operator edit"),
        "the re-install overwrote the edit. install said:\n{said}"
    );
    assert!(
        said.contains("--force"),
        "it left the edit but never said how to replace it:\n{said}"
    );

    host.sh("stop-bots install web --force");
    assert!(
        !host
            .sh("cat /etc/systemd/system/stop-bots-web.service")
            .contains("# operator edit"),
        "--force did not replace the edited unit"
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "active",
        "the forced re-install left the service down. journal:\n{}",
        host.journal("stop-bots-web.service")
    );
}

/// The other half of the refusal above: a unit an earlier release wrote,
/// and nobody edited, is replaced without `--force`. Before this, every
/// release that changed the template made a re-install refuse on every
/// host, which trains operators to reach for `--force` — the flag that
/// also overwrites the edits the refusal exists for.
#[test]
fn reinstalling_over_a_0_0_15_unit_upgrades_it_without_force() {
    if !enabled() {
        return;
    }
    reinstalling_upgrades(
        "stop-bots-upgrade-unit",
        "stop-bots-web-0.0.15.service",
        "0.0.7 to 0.0.15",
    );
}

/// The same from 0.1.0-rc.1, whose unit is recognised by its template
/// hash, and whose `ProtectSystem=yes` gives way to the stricter sandbox.
#[test]
fn reinstalling_over_an_0_1_0_rc_1_unit_upgrades_it_without_force() {
    if !enabled() {
        return;
    }
    let host = reinstalling_upgrades(
        "stop-bots-upgrade-rc1",
        "stop-bots-web-0.1.0-rc.1.service",
        "0.1.0-rc.1",
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "ProtectSystem"),
        "strict"
    );
}

/// And from 0.1.0-rc.2, whose console ran as root: replaced without
/// `--force`, by a unit whose console is the `stop-bots` user's.
#[test]
fn reinstalling_over_an_0_1_0_rc_2_unit_upgrades_it_without_force() {
    if !enabled() {
        return;
    }
    let host = reinstalling_upgrades(
        "stop-bots-upgrade-rc2-unit",
        "stop-bots-web-0.1.0-rc.2.service",
        "0.1.0-rc.2",
    );
    assert_eq!(host.unit("stop-bots-web.service", "User"), "stop-bots");
}

/// Puts `fixture` where `install web` writes its unit, re-installs with
/// no `--force`, and checks it was replaced and the console runs.
fn reinstalling_upgrades(name: &str, fixture: &str, releases: &str) -> Host {
    let host = Host::start(name);
    host.put(
        &format!(
            "{}/tests/fixtures/units/{fixture}",
            env!("CARGO_MANIFEST_DIR")
        ),
        "/etc/systemd/system/stop-bots-web.service",
    );

    let (ok, stdout, stderr) = host.run("stop-bots install web");

    let said = format!("{stdout}{stderr}");
    assert!(ok, "the {releases} unit was taken for an edit:\n{said}");
    assert!(
        said.contains(&format!("unedited since stop-bots {releases}")),
        "{said}"
    );
    let unit = host.sh("cat /etc/systemd/system/stop-bots-web.service");
    assert!(
        unit.contains("# stop-bots-template: ") && unit.contains("ProtectSystem=strict"),
        "the unit was not replaced:\n{unit}"
    );
    assert_eq!(
        host.unit("stop-bots-web.service", "ActiveState"),
        "active",
        "journal:\n{}",
        host.journal("stop-bots-web.service")
    );
    host
}

/// `uninstall` puts the host back as it was: every site file byte for
/// byte, no generated `conf.d` file, no nft table, no iptables chain, no
/// unit — against the real systemd, NGINX, nft and iptables, with the
/// console running and its internal cron free to put things back until
/// it is stopped.
///
/// Both firewall backends are applied, because a host that switched
/// backends has both, and `uninstall` removes whichever exist.
#[test]
fn uninstall_puts_the_host_back_as_it_was() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-uninstall");
    // Everything `uninstall` promises to put back, read the same way
    // before and after. Not /etc/stop-bots: this image creates it empty,
    // and `uninstall` removes it once it is.
    //
    // Nor the nft tables `iptables` itself owns. Debian's `iptables` is
    // iptables-nft, which creates `ip filter` the first time a rule is
    // added and never removes it; what is in it is exactly what
    // `iptables -S` lists, compared here too. Deleting that table would
    // delete ufw's and Docker's rules along with ours.
    let state = || {
        host.sh("set -e\n\
             find /etc/nginx -type f | sort | xargs sha256sum\n\
             echo '-- nft'\n\
             nft list tables | grep -v -e '^table ip filter$' -e '^table ip6 filter$' || true\n\
             echo '-- iptables'; iptables -S; ip6tables -S\n\
             echo '-- units'; find /etc/systemd/system -name 'stop-bots*' | sort\n\
             systemctl list-unit-files 'stop-bots*' --no-legend | sort")
    };
    let before = state();

    host.sh("stop-bots install web");
    host.wait_for_console();
    host.sh("stop-bots install firewall");
    host.seed_bot("badbot", "BadBot");
    host.stop_bots("set-rate-limit --enabled true");
    host.stop_bots("trust --address 192.0.2.77");
    host.stop_bots("apply-blocks");
    host.stop_bots("add-firewall-rule --address 203.0.113.60");
    host.stop_bots("render-firewall --backend nftables --out /etc/stop-bots/firewall.nft");
    host.sh("nft -f /etc/stop-bots/firewall.nft");
    host.stop_bots("render-firewall --backend iptables --out /etc/stop-bots/firewall.sh");
    host.sh("sh /etc/stop-bots/firewall.sh");
    let applied = state();
    for (what, needle) in [
        ("the site block", "stop-bots-limits.conf"),
        ("the nft table", "table inet stop_bots"),
        ("the iptables chain", "-N STOP-BOTS"),
        ("the web unit", "stop-bots-web.service"),
        ("the firewall unit", "stop-bots-firewall.service"),
    ] {
        assert!(
            applied.contains(needle),
            "{what} was never there:\n{applied}"
        );
    }

    let (ok, stdout, stderr) = host.run("stop-bots uninstall");

    let said = format!("{stdout}{stderr}");
    assert!(ok, "uninstall failed:\n{said}");
    assert_eq!(
        state(),
        before,
        "the host is not as it was. uninstall said:\n{said}"
    );
    // Only the host settings, which, like the database, stay unless
    // `--purge`.
    assert_eq!(
        host.sh("find /etc/stop-bots -type f 2>/dev/null || true")
            .trim(),
        "/etc/stop-bots/host.conf",
        "{said}"
    );
    let (valid, out, err) = host.run("nginx -t");
    assert!(
        valid,
        "NGINX does not load what uninstall left:\n{out}{err}"
    );
    // Polled: a reload is asynchronous, and an old worker can still answer.
    let mut served = String::new();
    for _ in 0..50 {
        served = host
            .sh("curl -s -o /dev/null -w '%{http_code}' -A 'BadBot/1.0' http://127.0.0.1:8080/");
        if served.trim() == "200" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(served.trim(), "200", "the running NGINX still blocks");
    assert!(
        host.run(&format!("test -f {HOST_DB}")).0 && said.contains(HOST_DB),
        "the database must be kept, and the output must say where:\n{said}"
    );
}

/// The password is generated once and printed once. A second install must
/// not roll it, or every re-install locks the operator out of a console
/// they had bookmarked.
#[test]
fn reinstalling_keeps_the_first_password() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-password");

    let first = host.sh("stop-bots install web");
    let hash = host.password_hash();
    assert!(!hash.is_empty(), "no password hash was stored:\n{first}");

    let second = host.sh("stop-bots install web");

    // The hash is the claim that matters. A quieter second message could
    // just as easily mean the password was rolled and not printed.
    assert_eq!(
        hash,
        host.password_hash(),
        "the stored password hash changed on re-install:\n{second}"
    );
}

/// The database holds the console's password hash, so it must not be
/// readable by anything else on the host.
#[test]
fn the_database_is_not_readable_by_other_users() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-perms");

    // The property that matters is reachability, not the mode digits:
    // `install web` chmods the *directory* to 0700 and leaves the file at
    // whatever the umask gave it, so the file is typically 0644 inside a
    // directory nothing else can traverse. Asserting 0600 on the file
    // would fail today for a reason that is not a vulnerability; asserting
    // that another account cannot read it is the real claim.
    host.sh("useradd --create-home --shell /bin/sh snoop");
    let (ok, stdout, stderr) = host.run("su snoop -c 'cat /var/lib/stop-bots/db.sqlite3'");
    assert!(
        !ok,
        "an unprivileged local user read the database holding the console's password hash:\n{stdout}{stderr}"
    );

    let dir_mode = host.sh("stat -c %a /var/lib/stop-bots");
    assert_eq!(
        dir_mode.trim(),
        "700",
        "the state directory is the first thing keeping that file private"
    );
    // And the file's own mode, which is what survives a backup or a `cp`
    // out of that directory.
    let file_mode = host.sh("stat -c %a /var/lib/stop-bots/db.sqlite3");
    assert_eq!(
        file_mode.trim(),
        "600",
        "the database holding the password hash is readable beyond its owner"
    );
    // And that owner is the console's user, not root: the console is not
    // root, and has to be able to open what it serves.
    for path in ["/var/lib/stop-bots", "/var/lib/stop-bots/db.sqlite3"] {
        assert_eq!(
            host.sh(&format!("stat -c %U:%G {path}")).trim(),
            "stop-bots:stop-bots",
            "{path} is not the console's"
        );
    }
}

/// `systemd-analyze verify` is systemd's own parser. A directive this
/// project misspells is accepted silently by `systemctl daemon-reload`
/// and simply does not apply — which is the quietest possible way for
/// hardening to stop hardening.
#[test]
fn systemd_itself_accepts_every_directive_in_the_generated_unit() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-verify");

    for unit in [
        "stop-bots-web.service",
        "stop-bots-helper.socket",
        "stop-bots-helper.service",
    ] {
        let (ok, stdout, stderr) = host.run(&format!(
            "systemd-analyze verify /etc/systemd/system/{unit}"
        ));
        let output = format!("{stdout}{stderr}");
        assert!(
            ok,
            "systemd-analyze verify rejected the generated {unit}:\n{output}"
        );
        assert!(
            !output.to_lowercase().contains("unknown"),
            "systemd did not recognise something in {unit}:\n{output}"
        );
    }
}

/// Debian's `iptables` is nft-backed, so it reaches the kernel over
/// netlink exactly like `nft` does. The sandbox has to let both through,
/// and the backend that ships on a host this project targets is the one
/// most likely to be reached for.
#[test]
fn the_generated_sandbox_lets_the_iptables_backend_reach_netlink() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-iptables-sandbox");

    host.stop_bots("add-firewall-rule --address 203.0.113.11");
    let apply = format!("/usr/local/bin/stop-bots batch --apply --force --no-fetch --backend iptables --root /etc/nginx/sites-enabled --out /etc/stop-bots/firewall.sh --db {HOST_DB}");

    let ran = host.oneshot_under_helper_sandbox("probe-iptables", &apply, "");
    assert!(
        ran,
        "the generated sandbox blocks an iptables apply. journal:\n{}",
        host.journal("probe-iptables.service")
    );
    assert!(
        host.sh("iptables -S STOP-BOTS").contains("203.0.113.11"),
        "the apply reported success but the rule is not in the live iptables chain"
    );
}

/// **The security review's finding, closed for writes**, for the one
/// process that is still root. The root helper writes the NGINX config and
/// the firewall script, and under `ProtectSystem=yes` a write it was
/// tricked into could land in `/etc/cron.d` or a systemd unit — root again
/// at the next cron minute or boot, outside any sandbox. Under the
/// generated unit each of those is refused by the filesystem itself.
///
/// The control runs first: the same write under 0.1.0-rc.1's
/// `ProtectSystem=yes` lands, so the refusals below are the sandbox and
/// not the probe failing for some other reason.
#[test]
fn the_helper_cannot_write_cron_units_or_binaries() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-no-persistence");
    // This image has no cron; the directories are what matter.
    host.sh("mkdir -p /etc/cron.d /var/spool/cron/crontabs");

    let loose = host.oneshot_under_helper_sandbox(
        "probe-cron-under-yes",
        "/usr/bin/touch /etc/cron.d/stop-bots-probe",
        "| sed -e 's/^ProtectSystem=.*/ProtectSystem=yes/'",
    );
    assert!(
        loose && host.run("test -e /etc/cron.d/stop-bots-probe").0,
        "the control did not write to /etc/cron.d, so the refusals below prove nothing. journal:\n{}",
        host.journal("probe-cron-under-yes.service")
    );
    host.sh("rm /etc/cron.d/stop-bots-probe");

    for (index, path) in [
        "/etc/cron.d/stop-bots-probe",
        "/var/spool/cron/crontabs/root",
        "/etc/systemd/system/stop-bots-probe.service",
        "/usr/local/bin/stop-bots-probe",
    ]
    .iter()
    .enumerate()
    {
        let probe = format!("probe-persist-{index}");
        let wrote =
            host.oneshot_under_helper_sandbox(&probe, &format!("/usr/bin/touch {path}"), "");
        let journal = host.journal(&format!("{probe}.service"));
        assert!(
            !wrote && !host.run(&format!("test -e {path}")).0,
            "the helper's sandbox let it write {path}"
        );
        assert!(
            journal.contains("Read-only file system"),
            "{path} was refused, but not by the sandbox:\n{journal}"
        );
    }
}

// ---- the console, as the `stop-bots` user ----

/// **The console is not root, and its unit says so twice.** It cannot
/// write the three places a write would become root: the NGINX config a
/// root master loads, the firewall script the boot unit runs, and cron.
/// `ProtectSystem=strict` refuses each with "Read-only file system"; with
/// the sandbox taken away the account still refuses it, with "Permission
/// denied". The account is the boundary; the sandbox is a second wall.
///
/// And what it is for, it can still write: its database's directory.
#[test]
fn the_console_cannot_write_nginx_config_the_firewall_script_or_cron() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-writes");
    host.sh("mkdir -p /etc/cron.d");

    for (index, path) in [
        "/etc/nginx/conf.d/stop-bots-probe.conf",
        "/etc/stop-bots/firewall.nft",
        "/etc/cron.d/stop-bots-probe",
    ]
    .iter()
    .enumerate()
    {
        let touch = format!("/usr/bin/touch {path}");
        for (how, mangle, refusal) in [
            ("its unit", "", "Read-only file system"),
            (
                "its account alone",
                "| sed -e '/^ProtectSystem=/d' -e '/^ReadWritePaths=/d'",
                "Permission denied",
            ),
        ] {
            let probe = format!("probe-console-write-{index}-{}", refusal.len());
            let wrote = host.oneshot_under_web_sandbox(&probe, &touch, mangle);
            let journal = host.journal(&format!("{probe}.service"));
            assert!(
                !wrote && !host.run(&format!("test -e {path}")).0,
                "under {how} the console wrote {path}"
            );
            assert!(
                journal.contains(refusal),
                "{path} was refused under {how}, but not with {refusal:?}:\n{journal}"
            );
        }
    }

    assert!(
        host.oneshot_under_web_sandbox(
            "probe-console-own-directory",
            "/usr/bin/touch /var/lib/stop-bots/probe",
            "",
        ),
        "the console cannot write its own directory. journal:\n{}",
        host.journal("probe-console-own-directory.service")
    );
}

/// The console cannot load firewall rules, or even list them: it has no
/// capability and no netlink. The control is the same command with root,
/// `CAP_NET_ADMIN` and `AF_NETLINK` given back, which creates the table —
/// so the refusal is the unit, not a broken probe.
#[test]
fn the_console_cannot_touch_the_firewall() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-nft");
    let add = "/usr/sbin/nft add table inet stop_bots_probe";
    let table = "nft list table inet stop_bots_probe";

    assert!(
        !host.oneshot_under_web_sandbox("probe-console-nft-add", add, ""),
        "the console added an nft table"
    );
    assert!(!host.run(table).0, "the table is there");
    assert!(
        !host.oneshot_under_web_sandbox("probe-console-nft-list", "/usr/sbin/nft list ruleset", ""),
        "the console listed the ruleset"
    );

    let as_root = host.oneshot_under_web_sandbox(
        "probe-console-nft-as-root",
        add,
        "| sed -e '/^User=/d' -e '/^Group=/d' \
           -e 's/^CapabilityBoundingSet=.*/CapabilityBoundingSet=CAP_NET_ADMIN/' \
           -e 's/^RestrictAddressFamilies=.*/& AF_NETLINK/'",
    );
    assert!(
        as_root && host.run(table).0,
        "the control could not add the table either, so the refusal proves nothing. journal:\n{}",
        host.journal("probe-console-nft-as-root.service")
    );
}

/// **The escape the old console had, closed.** Root that can reach systemd
/// can have it run anything outside every sandbox, and the rc.2 console,
/// root because it ran `systemctl reload nginx`, could. From the console's
/// unit now, `systemd-run` is refused: it is not root, so polkit refuses
/// it, and its unit hides systemd's sockets besides.
///
/// Three runs of the same command. As root with the sockets visible —
/// the control — it writes `/etc/cron.d`. As the console's user with the
/// sockets visible it is refused: the account alone holds. As generated,
/// refused.
#[test]
fn systemd_run_is_refused_to_the_console() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-escape");
    host.sh("mkdir -p /etc/cron.d && systemctl start dbus.socket dbus.service");
    let escape = "/usr/bin/systemd-run --wait -q /usr/bin/touch /etc/cron.d/stop-bots-escaped";
    let escaped = || host.run("test -e /etc/cron.d/stop-bots-escaped").0;

    let as_root = host.oneshot_under_web_sandbox(
        "probe-escape-as-root",
        escape,
        "| sed -e '/^User=/d' -e '/^Group=/d' -e '/^InaccessiblePaths=/d'",
    );
    assert!(
        as_root && escaped(),
        "the control could not reach systemd, so the refusals below prove nothing. journal:\n{}",
        host.journal("probe-escape-as-root.service")
    );
    host.sh("rm /etc/cron.d/stop-bots-escaped");

    for (how, probe, mangle) in [
        (
            "with systemd's sockets visible",
            "probe-escape-unhidden",
            "| sed -e '/^InaccessiblePaths=/d'",
        ),
        ("as generated", "probe-escape", ""),
    ] {
        let ran = host.oneshot_under_web_sandbox(probe, escape, mangle);
        assert!(
            !ran && !escaped(),
            "{how}, the console had systemd run a command for it"
        );
    }
}

/// The console's sandbox, directive by directive, under the systemd that
/// will run it. Each row is a probe that does what the directive forbids:
/// refused under the unit as generated, and let through once that one
/// line is removed — so a directive systemd ignores, misspelt, too new
/// for it, or not applied in this container, fails here.
///
/// The probes call system calls by number through Perl, which every
/// Debian has; the numbers are x86_64's.
#[test]
fn the_console_sandbox_holds_under_real_systemd() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-sandbox");
    if host.sh("uname -m").trim() != "x86_64" {
        stop_bots::say_err!("skipping: the probes call system calls by their x86_64 numbers");
        return;
    }
    host.sh("systemctl start dbus.socket dbus.service");
    // PTRACE_TRACEME is allowed to anyone below Yama's scope 3.
    let ptrace_allowed = host.sh("cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo 0");

    let mut rows = vec![
        (
            "memory both writable and executable",
            "MemoryDenyWriteExecute=",
            // mmap(NULL, 4096, PROT_READ|PROT_WRITE|PROT_EXEC, MAP_PRIVATE|MAP_ANONYMOUS)
            r#"/usr/bin/perl -e "exit(syscall(9, 0, 4096, 7, 34, -1, 0) == -1 ? 1 : 0)""#,
        ),
        (
            "a capability in its bounding set",
            "CapabilityBoundingSet=",
            r#"/bin/sh -c "grep -q '^CapBnd:.0000000000000000' /proc/self/status && exit 1; exit 0""#,
        ),
        (
            "systemd over D-Bus",
            "InaccessiblePaths=",
            "/usr/bin/systemctl show -p Version --value",
        ),
        (
            "systemd's private socket",
            "InaccessiblePaths=",
            // Root's 0700 socket, which this user could not use anyway;
            // hidden, it is a node of mode 0.
            r#"/bin/sh -c "stat -c %%a /run/systemd/private | grep -qvx 0""#,
        ),
    ];
    if ptrace_allowed.trim() != "3" {
        rows.push((
            "a system call outside @system-service (ptrace)",
            "SystemCallFilter=",
            r#"/usr/bin/perl -e "exit(syscall(101, 0, 0, 0, 0) == -1 ? 1 : 0)""#,
        ));
    }
    for (index, (what, directive, probe)) in rows.into_iter().enumerate() {
        let generated = format!("probe-sandbox-{index}");
        assert!(
            !host.oneshot_under_web_sandbox(&generated, probe, ""),
            "the console's unit allowed {what}. journal:\n{}",
            host.journal(&format!("{generated}.service"))
        );
        let without = format!("probe-sandbox-{index}-without");
        assert!(
            host.oneshot_under_web_sandbox(&without, probe, &format!("| sed -e '/^{directive}/d'")),
            "{what} is refused even without {directive}, so this row proves nothing. journal:\n{}",
            host.journal(&format!("{without}.service"))
        );
    }
}

/// The D-Bus call every probe of the bus makes: harmless, and answered to
/// any user who can reach the bus at all.
const PING_SYSTEMD: &str =
    "busctl --system call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
     org.freedesktop.DBus.Peer Ping";

/// **`/run` is read-only to the console, and D-Bus out of its reach,
/// whichever starts first.** `ProtectSystem=strict` alone left `/run`
/// writable here (`ProtectKernelTunables=` and `ProtectControlGroups=`
/// mount under it); the unit says `ReadOnlyPaths=/run`. And
/// `InaccessiblePaths=` hides nothing that is not there when the console
/// starts, so D-Bus started after it would have been reachable; the unit
/// hides D-Bus's directory, made first if need be.
///
/// Inside the real unit: `/run/lock` — which anyone may write outside it,
/// the control — and `/run` are read-only; and the bus, reachable from
/// outside as the console's user, is not reachable from inside, with
/// D-Bus started before the console and with D-Bus started while it runs.
#[test]
fn the_console_sees_run_read_only_and_no_bus_whichever_starts_first() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-run");
    let as_console = |cmd: &str| {
        host.run(&format!("/usr/sbin/runuser -u stop-bots -- {cmd}"))
            .0
    };
    assert!(
        as_console("touch /run/lock/stop-bots-control"),
        "the console's user cannot write /run/lock even outside its unit, so the check below \
         proves nothing"
    );

    // D-Bus after the console: not running, its directory not there.
    host.sh("systemctl stop dbus.socket dbus.service; rm -rf /run/dbus");
    host.start_console_probe(
        "probe-bus-later",
        &format!(
            "while [ ! -e /var/lib/stop-bots/go ]; do sleep 0.1; done\n\
             {PING_SYSTEMD} && exit 0\n\
             exit 1"
        ),
        true,
    );
    host.sh("systemctl start dbus.socket dbus.service");
    assert!(
        as_console(PING_SYSTEMD),
        "the console's user cannot reach the bus even outside its unit, so the checks below \
         prove nothing"
    );
    host.sh("touch /var/lib/stop-bots/go && chown stop-bots: /var/lib/stop-bots/go");
    let (reached, said) = host.console_probe_result("probe-bus-later");
    assert!(
        !reached,
        "the console reached D-Bus started after it:\n{said}"
    );

    // D-Bus before the console, as on a host that boots with it.
    let (reached, said) = host.probe_in_console_unit("probe-bus-first", PING_SYSTEMD);
    assert!(!reached, "the console reached D-Bus:\n{said}");

    for path in ["/run/stop-bots-probe", "/run/lock/stop-bots-probe"] {
        let (wrote, said) = host.probe_in_console_unit("probe-run", &format!("touch {path}"));
        assert!(
            !wrote && !host.run(&format!("test -e {path}")).0,
            "the console wrote {path}"
        );
        assert!(
            said.contains("Read-only file system"),
            "{path} was refused, but not by the sandbox:\n{said}"
        );
    }
}

/// **Every way up from the console's user, tried from inside its unit.**
/// Each row is what code running as the console would try next to become
/// root or to outlive it, and each is refused. Where the account alone
/// would allow it, the row's control proves it: the same command, as the
/// console's user but outside the unit, works — so `NoNewPrivileges=`,
/// `ProtectSystem=` or the hidden bus is what refuses it, not a broken
/// probe. `sudo` is given to the console's user and root's password is
/// emptied for the purpose.
#[test]
fn the_console_unit_refuses_every_way_up() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-ways-up");
    host.sh("systemctl start dbus.socket dbus.service \
         && echo 'stop-bots ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/stop-bots-probe \
         && chmod 440 /etc/sudoers.d/stop-bots-probe && passwd -d root >/dev/null");
    let present = |program: &str| host.run(&format!("command -v {program}")).0;
    let as_console = |cmd: &str| {
        host.run(&format!(
            "cd /var/lib/stop-bots && /usr/sbin/runuser -u stop-bots -- /bin/sh -c {}",
            sh_quoted(cmd)
        ))
        .0
    };
    let escaped = "/etc/stop-bots-escaped";

    // (what, program it needs, the attempt, whether the account alone
    // allows it, so the control must succeed)
    let rows: Vec<(&str, &str, String, bool)> = vec![
        ("sudo", "sudo", format!("sudo -n touch {escaped}"), true),
        (
            "su",
            "su",
            format!("su -c 'touch {escaped}' root < /dev/null"),
            true,
        ),
        ("pkexec", "pkexec", format!("pkexec touch {escaped}"), false),
        (
            "a crontab, as `crontab -e` writes it",
            "crontab",
            "echo '* * * * * true' | crontab -".to_string(),
            true,
        ),
        (
            "an at job",
            "at",
            "echo true | at now + 1 hour".to_string(),
            true,
        ),
        (
            "systemd-run",
            "systemd-run",
            format!("systemd-run --wait -q /usr/bin/touch {escaped}"),
            false,
        ),
        (
            "a call to systemd's manager",
            "busctl",
            PING_SYSTEMD.to_string(),
            true,
        ),
        ("writing /etc", "touch", format!("touch {escaped}"), false),
        (
            "writing /etc/nginx",
            "touch",
            "touch /etc/nginx/stop-bots-probe.conf".to_string(),
            false,
        ),
        (
            "writing /etc/stop-bots",
            "touch",
            "touch /etc/stop-bots/stop-bots-probe".to_string(),
            false,
        ),
        (
            "writing /usr/local/bin",
            "touch",
            "touch /usr/local/bin/stop-bots-probe".to_string(),
            false,
        ),
        (
            "a hard link to a root file in its directory",
            "ln",
            "ln /etc/stop-bots/host.conf /var/lib/stop-bots/host.conf.link".to_string(),
            false,
        ),
        (
            "writing /run",
            "touch",
            "touch /run/lock/stop-bots-probe".to_string(),
            true,
        ),
    ];
    let mut tried = 0;
    for (index, (what, program, attempt, control)) in rows.iter().enumerate() {
        // pkexec comes with polkit, which this image does not have.
        if !present(program) {
            continue;
        }
        tried += 1;
        if *control {
            assert!(
                as_console(attempt),
                "{what} fails as the console's user even outside its unit, so this row proves \
                 nothing"
            );
            // Undo what the control did, so the probe starts from the same
            // place.
            host.sh(&format!(
                "rm -f {escaped} /run/lock/stop-bots-probe; crontab -u stop-bots -r 2>/dev/null; \
                 for job in $(atq 2>/dev/null | cut -f1); do atrm $job; done; true"
            ));
        }
        let (worked, said) = host.probe_in_console_unit(&format!("probe-up-{index}"), attempt);
        assert!(
            !worked,
            "from inside its unit the console managed {what}:\n{said}"
        );
    }
    assert!(tried >= rows.len() - 1, "only {tried} rows ran");
    for (left, what) in [
        (escaped, "a root command ran"),
        (
            "/var/spool/cron/crontabs/stop-bots",
            "a crontab was written",
        ),
        ("/etc/nginx/stop-bots-probe.conf", "/etc/nginx was written"),
        (
            "/etc/stop-bots/stop-bots-probe",
            "/etc/stop-bots was written",
        ),
        (
            "/usr/local/bin/stop-bots-probe",
            "/usr/local/bin was written",
        ),
        ("/var/lib/stop-bots/host.conf.link", "a hard link was made"),
        ("/run/lock/stop-bots-probe", "/run was written"),
    ] {
        assert!(!host.run(&format!("test -e {left}")).0, "{what}: {left}");
    }
    assert_eq!(
        host.sh("atq 2>/dev/null | wc -l").trim(),
        "0",
        "an at job was queued"
    );
}

/// **A trigger the console plants runs nowhere as root.** Written as the
/// console's user straight into its database: one trigger on every write
/// to `settings`. Root CLI runs that write a setting then store theirs,
/// and the trigger's table stays empty. The control is the console's user
/// writing a setting the same way, which fires it.
#[test]
fn a_trigger_the_console_plants_does_not_fire_in_a_root_cli_run() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-planted-trigger");
    host.sh(&format!(
        "cat > /run/plant.sql <<'SQL'\n\
         CREATE TABLE fired (what TEXT);\n\
         CREATE TRIGGER planted_insert AFTER INSERT ON settings\n\
           BEGIN INSERT INTO fired VALUES ('insert'); END;\n\
         CREATE TRIGGER planted_update AFTER UPDATE ON settings\n\
           BEGIN INSERT INTO fired VALUES ('update'); END;\n\
         SQL\n\
         chmod 644 /run/plant.sql && \
         /usr/sbin/runuser -u stop-bots -- sqlite3 {HOST_DB} < /run/plant.sql"
    ));
    let fired = || {
        host.sh(&format!("sqlite3 {HOST_DB} 'SELECT count(*) FROM fired'"))
            .trim()
            .to_string()
    };

    host.stop_bots("set-auto-apply --enabled true");
    host.stop_bots("trust --address 192.0.2.5");
    assert_eq!(fired(), "0", "a planted trigger ran in a root CLI run");

    host.sh(&format!(
        "/usr/sbin/runuser -u stop-bots -- sqlite3 {HOST_DB} \
         \"INSERT OR REPLACE INTO settings (key, value) VALUES ('probe', 'x')\""
    ));
    assert_ne!(fired(), "0", "the control did not fire either");
}

/// **A link the console names like a pre-upgrade copy does not stop an
/// upgrade.** `install web` skips it, says so, follows nothing, and
/// finishes.
#[test]
fn a_planted_copy_beside_the_database_does_not_stop_install_web() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-planted-copy");
    let canary = "/etc/stop-bots-canary";
    host.sh(&format!("echo secret > {canary} && chmod 600 {canary}"));
    host.sh(&format!(
        "/usr/sbin/runuser -u stop-bots -- ln -s {canary} {HOST_DB}.bak-v99"
    ));

    let (ok, stdout, stderr) = host.run("stop-bots install web --no-start");

    assert!(
        ok,
        "install web failed over the planted copy:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains(&format!("{HOST_DB}.bak-v99")),
        "it did not say it skipped it:\n{stdout}"
    );
    assert_eq!(
        host.sh(&format!("stat -c '%U %a' {canary}")).trim(),
        "root 600",
        "the link's target was handed over"
    );
}

/// **The console reads no log of the host's.** Under its unit, as
/// `stop-bots`, it cannot read the auth log (`root:adm 0640`), NGINX's
/// access log (`www-data:adm 0640`), a log anyone may read, or the journal:
/// neither other units' entries nor sshd's. The helper reads the two logs
/// the detectors need for it. Each probe has its control: with the log
/// groups the unit used to add, and `/var/log` no longer hidden, the same
/// probe reads.
#[test]
fn the_console_can_read_no_log_of_the_hosts() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-console-no-logs");
    host.sh(
        "echo '127.0.0.1 - - [01/Oct/2026:00:00:00 +0000] \"GET / HTTP/1.1\" 200 1 \"-\" \"x\"' \
         >> /var/log/nginx/access.log && \
         echo 'Oct  1 00:00:00 host sshd[1]: Accepted publickey for m from 192.0.2.10 port 2 ssh2' \
         > /var/log/auth.log && chown root:adm /var/log/auth.log && chmod 640 /var/log/auth.log && \
         echo 'readable by anyone' > /var/log/world.log && chmod 644 /var/log/world.log",
    );
    // sshd's own unit, as far as the journal is concerned: what
    // `journalctl -u ssh` returns.
    host.sh("systemctl start dbus.socket dbus.service && systemd-run --wait --unit=ssh -p SyslogIdentifier=sshd \
         /bin/echo 'Failed password for root from 203.0.113.79 port 1 ssh2'");
    for (log, mode) in [
        ("/var/log/nginx/access.log", "www-data:adm 640"),
        ("/var/log/auth.log", "root:adm 640"),
    ] {
        assert_eq!(
            host.sh(&format!("stat -c '%U:%G %a' {log}")).trim(),
            mode,
            "{log} is not as Debian ships it"
        );
    }

    // Each succeeds only if it read something: an unprivileged
    // `journalctl` that can see nothing still exits 0.
    let journal = |filter: &str| {
        format!(
            "/bin/sh -c \"journalctl -q -n 1 {filter} -o cat > /var/lib/stop-bots/journal-probe \
             && test -s /var/lib/stop-bots/journal-probe\""
        )
    };
    let rows = [
        (
            "the NGINX access log",
            "/bin/cat /var/log/nginx/access.log".to_string(),
        ),
        ("the auth log", "/bin/cat /var/log/auth.log".to_string()),
        (
            "a log anyone may read",
            "/bin/cat /var/log/world.log".to_string(),
        ),
        ("other units' journal", journal("_PID=1")),
        ("sshd's journal", journal("-u ssh")),
    ];
    for (index, (what, probe)) in rows.iter().enumerate() {
        let name = format!("probe-no-logs-{index}");
        assert!(
            !host.oneshot_under_web_sandbox(&name, probe, ""),
            "the console read {what}. journal:\n{}",
            host.journal(&format!("{name}.service"))
        );
        let with = format!("{name}-with-groups");
        assert!(
            host.oneshot_under_web_sandbox(
                &with,
                probe,
                "| sed -e '/^InaccessiblePaths=-\\/var\\/log/d' \
                   -e '/^User=/a SupplementaryGroups=adm systemd-journal'"
            ),
            "{what} is unreadable even with the log groups, so this proves nothing. \
             journal:\n{}",
            host.journal(&format!("{with}.service"))
        );
    }
}

/// Sends `request`, one line of the helper's protocol, to its socket as
/// `user`, and returns the reply line, or `"<no connection>"`. Perl,
/// because it is on every Debian and speaks to a Unix socket.
fn ask_helper_with(host: &Host, user: &str, request: &str) -> String {
    host.sh(&format!(
        "cat > /run/ask-helper.pl <<'PERL'\n\
         use IO::Socket::UNIX;\n\
         my $s = IO::Socket::UNIX->new(Peer => '/run/stop-bots/helper.sock')\n\
           or do {{ print '<no connection>'; exit 0 }};\n\
         print $s q({request}), \"\\n\";\n\
         my $reply = <$s>;\n\
         print defined $reply ? $reply : '';\n\
         PERL\n\
         chmod 644 /run/ask-helper.pl"
    ));
    host.sh(&format!(
        "/usr/sbin/runuser -u {user} -- perl /run/ask-helper.pl"
    ))
}

/// **The helper applies NGINX to a site that listens on port 80.** `nginx
/// -t` binds every `listen` address it tests, and a port under 1024 needs
/// `CAP_NET_BIND_SERVICE` in the helper's bounding set. The suite's own
/// site listens on 8080 only, which is how a helper that could not test a
/// real site's config reached a real host before a test noticed. Asked by
/// the console's user, as the console asks.
#[test]
fn the_helper_applies_nginx_to_a_site_on_port_80() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-helper-port-80");
    host.sh("sed -i 's/listen 8080;/listen 8080;\\n    listen 80;/' \
         /etc/nginx/sites-enabled/test-site.conf \
         && nginx -t && systemctl reload nginx \
         && systemctl start stop-bots-helper.socket");
    assert!(
        host.sh("cat /etc/nginx/sites-enabled/test-site.conf")
            .contains("listen 80;"),
        "the site does not listen on 80, so this proves nothing"
    );
    // Something to apply, so that the apply writes the site and runs
    // `nginx -t` on it, rather than finding nothing to do.
    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    host.stop_bots("set-site-rule --site test.example --rule no-accept-language --enabled true");
    let reply = ask_helper_with(
        &host,
        "stop-bots",
        r#"{"ApplyNginx":{"site":null,"reload":true}}"#,
    );
    assert!(
        reply.starts_with(r#"{"Ok""#),
        "the helper could not apply NGINX to a site on 80: {reply}\njournal:\n{}",
        host.journal("stop-bots-helper.service")
    );
    assert!(
        host.sh("cat /etc/nginx/sites-enabled/test-site.conf")
            .contains("# BEGIN stop-bots"),
        "the helper said it applied, but the site has no block"
    );
}

/// **From the journal, the helper returns sshd's authentication lines and
/// nothing else.** Three entries: a failed login from sshd's own unit; the
/// same text from another program claiming to be sshd (`logger -t
/// 'sshd[1]'`, which any local user can run); and another service's
/// secret. Asked by the console's user for the SSH log, the helper — on
/// a host with no auth.log, so reading the journal — returns the first
/// alone.
#[test]
fn the_helper_returns_nothing_from_the_journal_but_sshds() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-helper-journal");
    host.wait_for_console();
    // No SSH log named, and none to find: the journal it is.
    host.sh(
        "systemctl start dbus.socket dbus.service && stop-bots set-log-paths --ssh-log '' && rm -f /var/log/auth.log /var/log/secure && \
         systemd-run --wait --unit=ssh -p SyslogIdentifier=sshd \
           /bin/echo 'Failed password for root from 203.0.113.79 port 1 ssh2' && \
         logger -t 'sshd[1]' 'Failed password for root from 203.0.113.80 port 1 ssh2' && \
         systemd-run --wait --unit=other-service \
           /bin/echo 'Accepted password for root from 203.0.113.81 port 1 ssh2 token=s3cr3t'",
    );

    let reply = ask_helper_with(
        &host,
        "stop-bots",
        r#"{"ReadLog":{"log":"Ssh","cursors":[],"legacy_offset":null,"since":0,"max_bytes":4194304}}"#,
    );

    assert!(
        reply.contains("203.0.113.79"),
        "sshd's own line did not come back: {reply}\njournal:\n{}",
        host.journal("stop-bots-helper.service")
    );
    for (what, needle) in [
        ("a line another program logged as sshd", "203.0.113.80"),
        ("another unit's entry", "203.0.113.81"),
        ("another unit's secret", "s3cr3t"),
    ] {
        assert!(!reply.contains(needle), "{what} came back: {reply}");
    }
}

// ---- root and the console's database ----

/// **Root opening the console's database leaves it the console's.** The
/// CLI, the TUI and the helper are root, and the database is the console
/// user's. SQLite is meant to give a `-wal` and `-shm` it creates as root
/// to the database file's owner; this checks that it does rather than
/// assuming it, because a root-owned `-wal` would leave the console unable
/// to open its own database. And the copy an upgrade takes before it
/// migrates goes to that owner too.
#[test]
fn root_opening_the_consoles_database_leaves_it_the_consoles() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-db-owner");
    let owner = |path: &str| {
        host.sh(&format!("stat -c '%U:%G %a' {path}"))
            .trim()
            .to_string()
    };
    assert_eq!(owner(HOST_DB), "stop-bots:stop-bots 600");
    assert_eq!(owner("/var/lib/stop-bots"), "stop-bots:stop-bots 700");

    // A root process holding the database open and writing, with nobody
    // else: a console run by hand as root, as rc.2's unit ran one.
    host.sh(&format!(
        "systemd-run --unit=root-holder /usr/local/bin/stop-bots web --db {HOST_DB} \
         --bind 127.0.0.1:8799 --no-apply"
    ));
    for side in ["-wal", "-shm"] {
        assert!(
            host.wait_for_file(&format!("{HOST_DB}{side}")),
            "the root console made no {side}. journal:\n{}",
            host.journal("root-holder.service")
        );
        assert_eq!(
            owner(&format!("{HOST_DB}{side}")),
            "stop-bots:stop-bots 600",
            "SQLite left the {side} it created as root to root"
        );
    }
    host.sh("systemctl stop root-holder.service");

    // An upgrade: a 0.0.15 database that is the console's, migrated by a
    // root CLI run.
    let old = "/var/lib/stop-bots/old.sqlite3";
    host.put(
        &format!(
            "{}/tests/fixtures/db/db-0.0.15.sql",
            env!("CARGO_MANIFEST_DIR")
        ),
        "/var/lib/stop-bots/old.sql",
    );
    host.sh(&format!(
        "sqlite3 {old} < /var/lib/stop-bots/old.sql && chown stop-bots:stop-bots {old} \
         && chmod 600 {old}"
    ));
    host.sh(&format!("stop-bots list-detectors --db {old}"));
    assert_eq!(
        owner(&format!("{old}.bak-v0")),
        "stop-bots:stop-bots 600",
        "the copy root took before upgrading is root's"
    );
    assert_eq!(owner(old), "stop-bots:stop-bots 600");
    // And the console's user opens both, as the console would.
    host.sh(&format!(
        "/usr/sbin/runuser -u stop-bots -- stop-bots list-detectors --db {old} \
         && /usr/sbin/runuser -u stop-bots -- stop-bots list-detectors --db {HOST_DB}"
    ));
}

/// **The console's directory is the console's, so root follows nothing in
/// it.** A compromised console can put a link where its database's
/// `-wal` would be, pointing at a root file, or replace the database
/// itself with a link to a file root would then create. Root — the CLI,
/// `install web` — refuses either, and the targets are untouched.
#[test]
fn root_follows_no_link_the_console_leaves_beside_its_database() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-db-links");
    let canary = "/etc/stop-bots-canary";
    host.sh(&format!(
        "echo 'root:x:0:0:root:/root:/bin/sh' > {canary} && chmod 644 {canary} && mkdir -p /etc/cron.d"
    ));
    let canary_state = || {
        host.sh(&format!(
            "stat -c '%U:%G %a' {canary} && sha256sum {canary}"
        ))
    };
    let before = canary_state();
    let as_console = |cmd: &str| host.sh(&format!("/usr/sbin/runuser -u stop-bots -- {cmd}"));

    as_console(&format!("ln -sf {canary} {HOST_DB}-wal"));
    for command in [
        format!("stop-bots trust --address 192.0.2.5 --db {HOST_DB}"),
        "stop-bots install web --no-start".to_string(),
    ] {
        let (ok, stdout, stderr) = host.run(&command);
        let said = format!("{stdout}{stderr}");
        assert!(!ok, "`{command}` went through a link to {canary}:\n{said}");
        assert!(
            said.contains("symbolic link"),
            "`{command}` failed, but not on the link:\n{said}"
        );
    }
    assert_eq!(canary_state(), before, "a file the link pointed at changed");

    as_console(&format!(
        "rm {HOST_DB}-wal && mv {HOST_DB} /var/lib/stop-bots/kept.sqlite3 \
         && ln -s /etc/cron.d/stop-bots-planted {HOST_DB}"
    ));
    let (ok, stdout, stderr) = host.run(&format!("stop-bots list-detectors --db {HOST_DB}"));
    assert!(
        !ok,
        "root opened a database through a link:\n{stdout}{stderr}"
    );
    assert!(
        !host.run("test -e /etc/cron.d/stop-bots-planted").0,
        "root created the file the console's link pointed at"
    );
}

/// `uninstall --purge` removes the console's user and group with the
/// database they own; without it, both stay.
#[test]
fn uninstall_removes_the_consoles_user_only_with_purge() {
    if !enabled() {
        return;
    }
    let host = Host::units_installed("stop-bots-uninstall-user");
    assert!(host.run("id stop-bots").0, "install web made no user");

    let kept = host.sh("stop-bots uninstall");
    assert!(
        host.run("id stop-bots").0,
        "uninstall without --purge removed the user:\n{kept}"
    );
    assert!(kept.contains("stop-bots user"), "{kept}");

    let purged = host.sh("stop-bots uninstall --purge");
    assert!(
        !host.run("id stop-bots").0,
        "--purge left the user:\n{purged}"
    );
    assert!(
        !host.run("getent group stop-bots").0,
        "--purge left the group:\n{purged}"
    );
    assert!(
        !host.run("test -e /var/lib/stop-bots").0,
        "--purge left the database directory:\n{purged}"
    );
}

// ---- the console and its helper, running ----

/// The console `install web` starts is the `stop-bots` user's, holds no
/// capability and can gain none, is in none of the log groups, and runs
/// under a syscall filter; the helper's socket is root's and the console
/// group's.
/// Read from the running process, not the unit.
#[test]
fn the_installed_console_runs_as_its_own_user_with_nothing_of_roots() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-console-account");
    host.wait_for_console();
    let pid = host.unit("stop-bots-web.service", "MainPID");
    let status = host.sh(&format!("cat /proc/{pid}/status"));
    let field = |name: &str| -> Vec<String> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}:")))
            .unwrap_or_else(|| panic!("no {name} in:\n{status}"))
            .split_whitespace()
            .map(String::from)
            .collect()
    };
    let uid = host.console_uid();
    assert!(
        field("Uid").iter().all(|id| *id == uid),
        "the console is not the stop-bots user: {:?}",
        field("Uid")
    );
    for (name, want) in [
        ("CapBnd", "0000000000000000"),
        ("CapEff", "0000000000000000"),
        ("CapPrm", "0000000000000000"),
        ("NoNewPrivs", "1"),
        ("Seccomp", "2"),
    ] {
        assert_eq!(field(name), vec![want.to_string()], "{name}");
    }
    // The status line is empty without supplementary groups.
    let groups: Vec<String> = status
        .lines()
        .find_map(|line| line.strip_prefix("Groups:"))
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect();
    for group in ["adm", "systemd-journal"] {
        let gid = host.sh(&format!("getent group {group} | cut -d: -f3"));
        assert!(
            !groups.contains(&gid.trim().to_string()),
            "the console is in {group}, which reads the host's logs: {groups:?}"
        );
    }
    assert_eq!(
        host.sh("stat -c '%U:%G %a' /run/stop-bots/helper.sock")
            .trim(),
        "root:stop-bots 660"
    );
    assert_eq!(
        host.unit("stop-bots-helper.socket", "ActiveState"),
        "active"
    );
}

/// The request a test sends the helper, when the bytes do not matter but
/// the answer does. One well-formed request of the helper's protocol
/// (`privileged::Op`, as JSON), for something that changes nothing: the
/// statuses of the scanned sites.
const HELPER_PROBE_REQUEST: &str = r#""SiteStatuses""#;

/// Sends [`HELPER_PROBE_REQUEST`] to the helper's socket as `user`, and
/// returns what came back: see [`ask_helper_with`].
fn ask_helper_as(host: &Host, user: &str) -> String {
    ask_helper_with(host, user, HELPER_PROBE_REQUEST)
}

/// **The peer check.** A user put in the socket's group can connect —
/// the socket's mode lets it — and the helper still refuses it, because
/// it checks every peer's uid. The console's user, sending the same bytes,
/// gets an answer.
#[test]
fn the_helper_refuses_a_peer_that_is_not_the_console() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-helper-peer");
    host.wait_for_console();
    host.sh("useradd --create-home --shell /bin/sh --groups stop-bots snoop");

    let console = ask_helper_as(&host, "stop-bots");
    let snoop = ask_helper_as(&host, "snoop");

    assert!(
        !console.trim().is_empty() && console != "<no connection>",
        "the console's own user got no answer: {console:?}. journal:\n{}",
        host.journal("stop-bots-helper.service")
    );
    assert_ne!(
        snoop, "<no connection>",
        "the socket refused the connection itself, so the peer check was never reached"
    );
    assert_ne!(
        snoop, console,
        "a user in the socket's group got the console's answer"
    );
    let snoop_uid = host.sh("id -u snoop");
    let journal = host.journal("stop-bots-helper.service");
    assert!(
        journal.contains(snoop_uid.trim()),
        "the helper did not log the refused peer's uid:\n{journal}"
    );
}

/// **Detection still works, with the helper reading the logs.** An access
/// log of `www-data:adm 0640` with a request for `/.env` in it, and an
/// auth log of `root:adm 0640` with a burst of failed logins: the console,
/// which can read neither, asks its helper for both, and the detectors
/// block both addresses on their first pass.
#[test]
fn the_console_detects_from_logs_it_reads_through_its_helper() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-console-detects");
    host.sh(
        "now=$(date +'%d/%b/%Y:%H:%M:%S %z') && \
         echo \"203.0.113.77 - - [$now] \\\"GET /.env HTTP/1.1\\\" 404 0 \\\"-\\\" \\\"curl/8\\\"\" \
           >> /var/log/nginx/access.log",
    );
    let mut auth = String::new();
    for attempt in 0..25 {
        auth.push_str(&format!(
            "$(date +'%b %e %H:%M:%S') host sshd[{}]: Failed password for root from 203.0.113.78 port {} ssh2\n",
            1000 + attempt,
            40000 + attempt
        ));
    }
    host.sh(&format!(
        "printf '%s' \"{auth}\" > /var/log/auth.log && chown root:adm /var/log/auth.log \
         && chmod 640 /var/log/auth.log"
    ));
    for (log, mode) in [
        ("/var/log/nginx/access.log", "www-data:adm 640"),
        ("/var/log/auth.log", "root:adm 640"),
    ] {
        assert_eq!(
            host.sh(&format!("stat -c '%U:%G %a' {log}")).trim(),
            mode,
            "{log} is not as Debian ships it"
        );
    }

    host.sh("stop-bots install web --ssh-log /var/log/auth.log");
    host.wait_for_console();
    assert!(
        host.sh("cat /etc/stop-bots/host.conf")
            .contains("/var/log/auth.log"),
        "install web did not store the SSH log for the helper"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut rules = String::new();
    while std::time::Instant::now() < deadline {
        rules = host
            .run_uncontended(&format!("stop-bots list-firewall-rules --db {HOST_DB}"))
            .1;
        if rules.contains("203.0.113.77") && rules.contains("203.0.113.78") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(
        rules.contains("203.0.113.77"),
        "the access log's prober was not blocked:\n{rules}\njournal:\n{}",
        host.journal("stop-bots-web.service")
    );
    assert!(
        rules.contains("203.0.113.78"),
        "the auth log's guesser was not blocked:\n{rules}\njournal:\n{}",
        host.journal("stop-bots-web.service")
    );
    // And it was the helper that read them.
    let helper = host.journal("stop-bots-helper.service");
    assert!(
        helper.contains("op=ReadLog") && helper.contains(&host.console_uid()),
        "the helper read no log for the console:\n{helper}"
    );
}

/// **The upgrade from 0.1.0-rc.2.** A host as rc.2 left it — its unit, a
/// console running as root, the NGINX commands and root in the database,
/// and the database root's — runs `install web`, and comes out with the
/// console running as `stop-bots`, the database the console's, the host
/// settings in `/etc/stop-bots/host.conf` and gone from the database, and
/// the console still applying, through its helper.
#[test]
fn an_rc_2_host_upgrades_to_an_unprivileged_console() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-upgrade-rc2");
    host.put(
        &format!(
            "{}/tests/fixtures/units/stop-bots-web-0.1.0-rc.2.service",
            env!("CARGO_MANIFEST_DIR")
        ),
        "/etc/systemd/system/stop-bots-web.service",
    );
    // rc.2's database, as rc.2's root console and CLI made it. Every root
    // CLI run comes before the rows below: this release's CLI moves them
    // into host.conf, and no root command may run between them and
    // `install web`, or the test exercises that move instead of the
    // upgrade's.
    host.sh(&format!(
        "mkdir -p -m 700 /var/lib/stop-bots && stop-bots scan-sites --root /etc/nginx/sites-enabled --db {HOST_DB}"
    ));
    host.seed_bot("badbot", "BadBot");
    // Not the defaults, so that what reaches host.conf is these.
    host.sh(&format!(
        "sqlite3 {HOST_DB} \"INSERT OR REPLACE INTO settings (key, value) VALUES \
           ('nginx:test_command', '/usr/sbin/nginx -t'), \
           ('nginx:reload_command', '/usr/sbin/nginx -s reload'), \
           ('nginx:root', '/etc/nginx')\""
    ));
    // rc.2 had no host settings file. The root commands above are this
    // release's, and made one, before there was anything to move into
    // it; an rc.2 host has the rows and no file.
    host.sh("rm -f /etc/stop-bots/host.conf");
    host.sh("systemctl daemon-reload && systemctl enable --now stop-bots-web.service");
    host.wait_for_console();
    let user_of_console = || {
        let pid = host.unit("stop-bots-web.service", "MainPID");
        // Not `ps`: the image has no procps.
        host.sh(&format!("stat -c %U /proc/{pid}"))
            .trim()
            .to_string()
    };
    assert_eq!(
        user_of_console(),
        "root",
        "the fixture is not rc.2's root console"
    );

    let out = host.sh("stop-bots install web");

    assert!(
        out.contains("unedited since stop-bots 0.1.0-rc.2"),
        "the rc.2 unit was not recognised:\n{out}"
    );
    host.wait_for_console();
    assert_eq!(
        user_of_console(),
        "stop-bots",
        "the console still runs as root after the upgrade:\n{out}"
    );
    for path in [HOST_DB, "/var/lib/stop-bots"] {
        assert!(
            host.sh(&format!("stat -c %U:%G {path}"))
                .trim()
                .starts_with("stop-bots:stop-bots"),
            "{path} is not the console's"
        );
    }
    // `hostconf`'s keys: `nginx_test_command = /usr/sbin/nginx -t`, and so on.
    let conf = host.sh("cat /etc/stop-bots/host.conf");
    for command in ["/usr/sbin/nginx -t", "/usr/sbin/nginx -s reload"] {
        assert!(
            conf.contains(command),
            "{command:?} did not reach host.conf:\n{conf}"
        );
    }
    let left = host.sh(&format!(
        "sqlite3 {HOST_DB} \"SELECT count(*) FROM settings WHERE key IN \
           ('nginx:test_command', 'nginx:reload_command', 'nginx:root')\""
    ));
    assert_eq!(
        left.trim(),
        "0",
        "the host settings are still in the database"
    );

    let password = host
        .sh(&format!("stop-bots web --set-password --db {HOST_DB}"))
        .lines()
        .find_map(|line| line.strip_prefix("New password: "))
        .expect("no new password")
        .trim()
        .to_string();
    let console = host.login(&password);
    let flash = console.post("/apply-all", &[]);
    let mut blocked = String::new();
    for _ in 0..50 {
        blocked = host
            .sh("curl -s -o /dev/null -w '%{http_code}' -A 'BadBot/1.0' http://127.0.0.1:8080/");
        if blocked.trim() == "403" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        blocked.trim(),
        "403",
        "the upgraded console could not apply. console said:\n{flash}\nhelper:\n{}",
        host.journal("stop-bots-helper.service")
    );
}

/// **The helper treats the database as hostile.** Rows a compromised
/// console could write, written as its user straight into its database:
/// NGINX commands and a root, a record of a "generated" file that is
/// `/etc/stop-bots-canary`, a link in its own directory to that file, a
/// site whose path is in `/etc/cron.d`, and the rc.2 hostile user agents.
/// "Apply everything" through the console then runs none of the commands,
/// deletes and writes nothing outside the NGINX config and
/// `/etc/stop-bots`, and turns away exactly the hostile agents' clients.
#[test]
fn the_helper_ignores_what_a_compromised_console_wrote_in_the_database() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-hostile-db");
    let console = host.console();
    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    host.seed_bot("badbot", "BadBot");
    let canary = "/etc/stop-bots-canary";
    host.sh(&format!(
        "echo 'root:x:0:0:root:/root:/bin/sh' > {canary} && mkdir -p /etc/cron.d"
    ));
    host.sh(&format!(
        "/usr/sbin/runuser -u stop-bots -- ln -s {canary} /var/lib/stop-bots/stop-bots-limits.conf"
    ));

    let mut sql = String::from(
        "INSERT OR REPLACE INTO settings (key, value) VALUES \
           ('nginx:test_command', 'touch /run/stop-bots-pwned-test'), \
           ('nginx:reload_command', 'touch /run/stop-bots-pwned-reload'), \
           ('nginx:root', '/etc');\n\
         INSERT OR REPLACE INTO managed_files (path, kind, written_at, version) VALUES \
           ('/etc/stop-bots-canary', 'nginx-limits', 0, 'x'), \
           ('/var/lib/stop-bots/stop-bots-limits.conf', 'nginx-limits', 0, 'x');\n\
         UPDATE sites SET config_path = '/etc/cron.d/stop-bots-site';\n",
    );
    for (user_agent, _) in HOSTILE_USER_AGENTS {
        sql.push_str(&format!(
            "INSERT INTO blocked_user_agents (user_agent, blocked_at) VALUES ('{}', 0);\n",
            user_agent.replace('\'', "''")
        ));
    }
    host.sh(&format!(
        "cat > /run/hostile.sql <<'SQL'\n{sql}SQL\n\
         chmod 644 /run/hostile.sql && \
         /usr/sbin/runuser -u stop-bots -- sqlite3 -cmd '.timeout 5000' {HOST_DB} < /run/hostile.sql"
    ));
    host.sh("touch /run/stop-bots-before && sleep 1");

    let flash = console.post("/apply-all", &[]);

    for pwned in ["/run/stop-bots-pwned-test", "/run/stop-bots-pwned-reload"] {
        assert!(
            !host.run(&format!("test -e {pwned}")).0,
            "the helper ran a command the database named. console said:\n{flash}"
        );
    }
    assert!(
        host.run(&format!("test -f {canary}")).0,
        "the helper deleted a file the database's record named"
    );
    assert!(
        !host.run("test -e /etc/cron.d/stop-bots-site").0,
        "the helper wrote where a site row said"
    );
    let changed = host.sh("find / -xdev -newer /run/stop-bots-before -type f \
           -not -path '/proc/*' -not -path '/sys/*' -not -path '/run/*' -not -path '/tmp/*' \
           -not -path '/var/log/*' -not -path '/var/lib/stop-bots/*' \
           -not -path '/etc/nginx/*' -not -path '/etc/stop-bots/*' \
           -not -path '/var/lib/systemd/*' -not -path '/var/cache/*' 2>/dev/null || true");
    assert_eq!(
        changed.trim(),
        "",
        "the apply changed files outside the NGINX config and /etc/stop-bots"
    );
    let applied = host.sh("cat /etc/nginx/sites-enabled/test-site.conf");
    assert!(
        applied.contains("BEGIN stop-bots"),
        "the site found on disk was not applied. console said:\n{flash}"
    );
    let (ok, out, err) = host.run("nginx -t");
    assert!(ok, "the hostile rows broke the config:\n{out}{err}");

    let status = |user_agent: &str| {
        host.run(&format!(
            "curl -s -o /dev/null -w '%{{http_code}}' -A {} http://127.0.0.1:8080/",
            sh_quoted(user_agent)
        ))
        .1
        .trim()
        .to_string()
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    for (user_agent, near_miss) in HOSTILE_USER_AGENTS {
        assert_eq!(status(user_agent), "403", "{user_agent:?} was served");
        if let Some(near_miss) = near_miss {
            assert_eq!(status(near_miss), "200", "{near_miss:?} was refused");
        }
    }
    assert_eq!(
        status("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0"),
        "200"
    );
}

/// And split in two the console still does every job it has, through its
/// own buttons and its root helper: NGINX and the firewall applied, its
/// database written in WAL mode, the apply lock taken in the file every
/// other stop-bots uses, and Web Access set up.
///
/// Three things here are the ways a narrower sandbox broke during this
/// change. NGINX is restarted after the console starts, which replaces
/// `/run/nginx.pid` — the file `nginx -t` opens — and a grant of that file
/// alone did not survive it. The site logs outside /var/log/nginx, and
/// `nginx -t` opens its log for writing. And the site file belongs to
/// www-data, so rewriting it with its owner and mode kept takes CAP_CHOWN
/// and CAP_FOWNER.
#[test]
fn the_console_under_its_unit_still_does_everything_it_is_for() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-sandboxed-console");
    let site = "/etc/nginx/sites-enabled/test-site.conf";
    host.sh(&format!(
        "mkdir -p /srv/www/test/logs \
         && sed -i 's|^    root |    access_log /srv/www/test/logs/access.log;\\n    root |' {site} \
         && nginx -t && systemctl reload nginx \
         && chown www-data:www-data {site} && chmod 0640 {site}"
    ));
    assert!(
        host.sh(&format!("cat {site}"))
            .contains("/srv/www/test/logs"),
        "the fixture did not move the site's log"
    );
    let console = host.console();
    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    host.seed_bot("badbot", "BadBot");
    host.stop_bots("add-firewall-rule --address 203.0.113.80");
    // A generated conf.d file, which the console records in its database
    // before writing it.
    host.stop_bots("set-rate-limit --enabled true");
    host.sh("systemctl restart nginx && rm -f /run/stop-bots.lock");

    let flash = console.post("/apply-all", &[]);

    assert!(
        host.ruleset().contains("203.0.113.80"),
        "the firewall half did not reach the kernel. console said:\n{flash}"
    );
    let mut blocked = String::new();
    for _ in 0..50 {
        blocked = host
            .sh("curl -s -o /dev/null -w '%{http_code}' -A 'BadBot/1.0' http://127.0.0.1:8080/");
        if blocked.trim() == "403" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        blocked.trim(),
        "403",
        "the NGINX half did not take effect. console said:\n{flash}"
    );
    assert_eq!(
        host.sh(&format!("stat -c '%U:%G %a' {site}")).trim(),
        "www-data:www-data 640",
        "the site file lost its owner or mode when the console rewrote it"
    );

    // The lock file holds the pid of whoever last took it: the helper,
    // which applies for the console, in the same file the CLI and the TUI
    // lock — and never the console, which is not root.
    assert_eq!(
        host.sh("cat /run/stop-bots.lock").trim(),
        host.unit("stop-bots-helper.service", "MainPID"),
        "the helper applied without taking the shared apply lock"
    );

    let sqlite = |sql: &str| host.sh(&format!("sqlite3 -cmd '.timeout 5000' {HOST_DB} \"{sql}\""));
    assert_eq!(sqlite("PRAGMA journal_mode").trim(), "wal");
    assert!(
        host.run(&format!("test -e {HOST_DB}-wal")).0,
        "the console's database has no write-ahead log beside it"
    );
    let recorded = sqlite("SELECT path FROM managed_files");
    assert!(
        recorded.contains("/etc/nginx/conf.d/stop-bots-limits.conf")
            && host
                .run("test -f /etc/nginx/conf.d/stop-bots-limits.conf")
                .0,
        "the console did not record and write its conf.d file. recorded:\n{recorded}"
    );

    let flash = console.post(
        "/web-access",
        &[
            ("mode", "path"),
            ("site", "test.example"),
            ("prefix", "/stop-bots/"),
            ("host", ""),
        ],
    );
    let config = host.sh(&format!("cat {site}"));
    assert!(
        config.contains(r#"location "/stop-bots/""#),
        "Web Access wrote no location block. console said:\n{flash}"
    );
    let (ok, out, err) = host.run("nginx -t");
    assert!(ok, "Web Access left a config NGINX refuses:\n{out}{err}");

    for unit in ["stop-bots-web.service", "stop-bots-helper.service"] {
        let journal = host.journal(unit);
        assert!(
            !journal.contains("Read-only file system"),
            "{unit} hit its sandbox somewhere:\n{journal}"
        );
    }
}

/// The boot unit runs, as root, a script the console writes. Under its
/// sandbox that script can load rules and nothing else: it cannot write
/// to `/etc/cron.d`, and it cannot ask systemd to — the escape the web
/// console, which needs `systemctl`, still has.
///
/// The control proves the escape is real: the same sandbox with `AF_UNIX`
/// put back lets `systemd-run` write where the sandbox itself cannot.
#[test]
fn the_boot_unit_can_load_rules_but_not_write_or_reach_systemd() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-boot-sandbox");
    host.stop_bots("add-firewall-rule --address 203.0.113.90");
    host.stop_bots("render-firewall --apply --force");
    host.sh("stop-bots install firewall");
    host.sh("mkdir -p /etc/cron.d && systemctl start dbus.socket dbus.service");
    let unit = "stop-bots-firewall.service";
    let escape = "/usr/bin/systemd-run --wait -q /usr/bin/touch /etc/cron.d/stop-bots-escaped";

    let with_unix = host.oneshot_under_sandbox_of(
        unit,
        "probe-boot-with-unix",
        escape,
        "| sed -e 's/^RestrictAddressFamilies=.*/& AF_UNIX/'",
    );
    assert!(
        with_unix && host.run("test -e /etc/cron.d/stop-bots-escaped").0,
        "the control could not reach systemd, so the refusal below proves nothing. journal:\n{}",
        host.journal("probe-boot-with-unix.service")
    );
    host.sh("rm /etc/cron.d/stop-bots-escaped");

    let escaped = host.oneshot_under_sandbox_of(unit, "probe-boot-escape", escape, "");
    assert!(
        !escaped && !host.run("test -e /etc/cron.d/stop-bots-escaped").0,
        "a script run by the boot unit had systemd write /etc/cron.d for it"
    );
    let wrote = host.oneshot_under_sandbox_of(
        unit,
        "probe-boot-write",
        "/usr/bin/touch /etc/cron.d/stop-bots-probe",
        "",
    );
    assert!(
        !wrote && !host.run("test -e /etc/cron.d/stop-bots-probe").0,
        "a script run by the boot unit wrote /etc/cron.d"
    );

    // And it still does its one job, the way a boot runs it.
    host.sh("nft delete table inet stop_bots");
    host.sh("systemctl restart stop-bots-firewall.service");
    assert!(
        host.run("nft get element inet stop_bots block_v4 '{ 203.0.113.90 }'")
            .0,
        "the hardened boot unit did not load the applied rules. journal:\n{}",
        host.journal(unit)
    );
}

/// The iptables boot unit is a shell script under the same sandbox, plus
/// what iptables-legacy needs. It has to bring the chain back.
#[test]
fn the_iptables_boot_unit_restores_the_chain_under_its_sandbox() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-boot-iptables");
    host.stop_bots("set-firewall-backend --backend iptables");
    host.stop_bots("add-firewall-rule --address 203.0.113.91");
    host.stop_bots("render-firewall --apply --force");
    host.sh("stop-bots install firewall");

    host.sh(
        "iptables -D INPUT -j STOP-BOTS && iptables -D FORWARD -j STOP-BOTS \
         && iptables -F STOP-BOTS && iptables -X STOP-BOTS && rm -f /run/xtables.lock",
    );
    let started = host.try_start("stop-bots-firewall.service");

    assert!(
        started,
        "the iptables boot unit failed under its sandbox. journal:\n{}",
        host.journal("stop-bots-firewall.service")
    );
    assert!(
        host.sh("iptables -S STOP-BOTS").contains("203.0.113.91"),
        "the unit ran but the chain did not come back"
    );
}

// ---- the console, as a listening server on a real host ----

/// "Apply everything" from the browser, on a host where both planes are
/// real: NGINX genuinely reloaded, `nft` genuinely loaded.
///
/// This is the end-to-end version of the netlink bug. The console is
/// running under the sandbox `install web` generated, the apply goes
/// through a button rather than a probe unit, and the evidence is a live
/// ruleset and a request that actually gets refused.
#[test]
fn apply_everything_from_the_console_enforces_on_both_planes() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-apply-all");
    let console = host.console();

    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    host.seed_bot("badbot", "BadBot");
    host.stop_bots("add-firewall-rule --address 203.0.113.40");

    let flash = console.post("/apply-all", &[]);

    let ruleset = host.sh("nft list ruleset");
    assert!(
        ruleset.contains("203.0.113.40"),
        "the firewall half did not reach the kernel. console said:\n{flash}\nruleset:\n{ruleset}"
    );

    // A reload is asynchronous: `systemctl reload nginx` returns once the
    // master has the signal, and an old worker can still answer the next
    // request. Polled rather than slept on, so a slow runner waits longer
    // and a fast one not at all.
    let mut blocked = String::new();
    for _ in 0..50 {
        blocked = host
            .sh("curl -s -o /dev/null -w '%{http_code}' -A 'BadBot/1.0' http://127.0.0.1:8080/");
        if blocked.trim() == "403" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        blocked.trim(),
        "403",
        "the NGINX half did not take effect — config written but not reloaded"
    );
    let served =
        host.sh("curl -s -o /dev/null -w '%{http_code}' -A 'Mozilla/5.0' http://127.0.0.1:8080/");
    assert_eq!(served.trim(), "200", "everyone else must still be served");
}

/// The Web Access panel writes NGINX config that puts the console behind
/// the very server it is protecting. New codegen, validated by `nginx -t`
/// here for the first time against a config that then has to actually
/// proxy.
///
/// Also pins the documented limitation: the path prefix is read when the
/// router is built, so the console answers on it only after a restart.
#[test]
fn web_access_path_mode_serves_the_console_through_nginx() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-webaccess");
    let console = host.console();
    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");

    let flash = console.post(
        "/web-access",
        // Every field, including the one this mode does not use: the
        // page's own form posts all four, and the handler deserialises
        // them as a unit.
        &[
            ("mode", "path"),
            ("site", "test.example"),
            ("prefix", "/stop-bots/"),
            ("host", ""),
        ],
    );

    let site = host.sh("cat /etc/nginx/sites-enabled/test-site.conf");
    assert!(
        site.contains(r#"location "/stop-bots/""#),
        "no console location block was written. console said:\n{flash}\nconfig:\n{site}"
    );
    let (ok, output, err) = host.run("nginx -t");
    assert!(
        ok,
        "the generated console config does not parse:\n{output}{err}"
    );

    // The proxy reaches the console: not a 502, which is what a wrong
    // upstream or a `proxy_pass` trailing slash would give.
    host.sh("systemctl reload nginx");
    let through =
        host.sh("curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/stop-bots/");
    assert_ne!(
        through.trim(),
        "502",
        "NGINX could not reach the console it was just pointed at"
    );

    // And after the restart the limitation calls for, the prefixed path
    // really serves the console.
    host.sh("systemctl restart stop-bots-web.service");
    host.wait_for_console_at("/stop-bots");
    let body = host.sh("curl -s -L http://127.0.0.1:8080/stop-bots/");
    assert!(
        body.contains("password"),
        "the console is not being served under its prefix:\n{body}"
    );
}

/// The console's own `location` sits in a site's `server` block, and every
/// check in the site's sentinel block runs before a `location` is chosen:
/// a request-shape rule that refuses the operator's client locked them out
/// of the one page that turns it off. The server that serves the console
/// exempts its prefix, and the rest of the site stays blocked.
#[test]
fn a_request_rule_that_refuses_curl_still_lets_the_console_through() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-console-exempt");
    let console = host.console();
    host.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    console.post(
        "/web-access",
        &[
            ("mode", "path"),
            ("site", "test.example"),
            ("prefix", "/stop-bots/"),
            ("host", ""),
        ],
    );
    host.sh("systemctl restart stop-bots-web.service");
    host.wait_for_console_at("/stop-bots");

    // curl sends no Accept-Language.
    host.stop_bots("set-site-rule --site test.example --rule no-accept-language --enabled true");
    host.stop_bots("apply-blocks --root /etc/nginx/sites-enabled");

    // Polled until it answers `want`: a reload is asynchronous, and an old
    // worker can still answer the first request after one. Unpolled, this
    // passed on podman and failed every try on CI's runner.
    let status = |path: &str, want: &str| {
        let mut got = String::new();
        for _ in 0..50 {
            got = host
                .run(&format!(
                    "curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:8080{path}"
                ))
                .1
                .trim()
                .to_string();
            if got == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        got
    };
    assert_eq!(
        status("/", "403"),
        "403",
        "the rule is not in force on the site at all:\n{}",
        host.sh("cat /etc/nginx/sites-enabled/test-site.conf")
    );
    assert_eq!(
        status("/stop-bots/login", "200"),
        "200",
        "the console is locked out by the site's own rule:\n{}",
        host.sh("cat /etc/nginx/sites-enabled/test-site.conf")
    );
}

/// The host allowlist is the DNS-rebinding guard, and `tests/web.rs`
/// checks it against a `Host:` header it sets itself. This checks it
/// against one a real client sent to a real listener.
#[test]
fn the_console_refuses_a_host_header_it_was_not_told_about() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-hosts");
    host.console();

    let loopback = host.sh("curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8787/login");
    assert!(
        loopback.trim().starts_with('2'),
        "loopback should need no configuration, got {loopback}"
    );

    let forged = host.sh(
        "curl -s -o /dev/null -w '%{http_code}' -H 'Host: evil.example' http://127.0.0.1:8787/login",
    );
    // 421 Misdirected Request, which is what the guard returns: the
    // request arrived at a server that does not answer for that name.
    assert_eq!(
        forged.trim(),
        "421",
        "an unlisted Host header reached the console"
    );
}

// ---- NGINX behaviour that needed a real client ----

/// TODO.md has been asking for this one: `set $limit_rate 1` throttles the
/// response body, but how much NGINX writes before the throttle engages on
/// a body this small had never been measured.
///
/// Asserted as "the client is still waiting after five seconds" rather
/// than as a duration: the useful property is that a tarpitted client is
/// held, and a test that pins the exact number would break on an NGINX
/// upgrade without anything being wrong.
#[test]
fn a_tarpitted_client_is_held_while_everyone_else_is_served_at_once() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-tarpit");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("set-block-response --response tarpit");
    server.apply_and_reload();

    let started = std::time::Instant::now();
    let tarpitted = server.status("/", "--max-time 5 -A 'BadBot/1.0'");
    let held = started.elapsed();
    assert_eq!(
        tarpitted, "000",
        "a tarpitted client got a complete response in {held:?}; the throttle \
         is writing the whole body before it engages"
    );
    assert!(
        held >= std::time::Duration::from_secs(4),
        "the connection closed after {held:?} instead of holding the client"
    );

    // The status is the whole assertion: a tarpitted client cannot produce
    // a 200 inside the same five-second window, it produces the "000"
    // above. So a 200 here *is* "the throttle is scoped to the block".
    //
    // There used to be a wall-clock bound under it as well (under 2s, then
    // under 4s). It measured the machine rather than NGINX: the time
    // includes a `podman exec` and a curl start-up, and on a loaded host
    // it failed with the client served correctly, just slowly.
    assert_eq!(
        server.status("/", "--max-time 5 -A 'Mozilla/5.0'"),
        "200",
        "the tarpit is holding everyone, not just blocked clients"
    );
}

/// Rate limiting is `limit_req`, which NGINX enforces at request time —
/// the one piece of this project that is a runtime component, and the one
/// that cannot be checked by reading generated text.
#[test]
fn rate_limiting_refuses_a_burst_and_then_lets_the_client_back_in() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-ratelimit");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.stop_bots("set-rate-limit --enabled true --rps 1 --burst 2");
    server.apply_and_reload();

    // Well past rps+burst, as fast as curl can send them.
    let codes = server.sh(
        "for i in $(seq 1 12); do curl -s -o /dev/null -w '%{http_code} ' http://127.0.0.1:8080/; done",
    );
    // 429, not 503: the generated config sets `limit_req_status 429;`,
    // which is the status this project chose for "back off and retry".
    assert!(
        codes.contains("429"),
        "a burst well over the configured rate was never refused: {codes}"
    );
    assert!(
        codes.contains("200"),
        "every request was refused, so the limit is not letting anyone through: {codes}"
    );
}

/// The whole detection loop, with nothing faked at any join: a real client
/// fetches the honeypot from a real NGINX, the detector reads the log
/// NGINX actually wrote, the rule it adds is rendered and loaded into a
/// real ruleset, and the client can no longer reach the server.
///
/// Every one of those steps is tested in isolation elsewhere. None of the
/// joins between them was tested anywhere.
#[test]
fn fetching_the_honeypot_gets_a_real_client_blocked_end_to_end() {
    if !enabled() {
        return;
    }
    let net = Network::create("stop-bots-honeypot-net");
    let server = Server::start_on_network("stop-bots-honeypot", &net);
    let client = Client::start("stop-bots-honeypot-client", &net);

    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.stop_bots("set-honeypot-path --path /trap-me");
    server.apply_and_reload();

    assert_eq!(
        client.get(&server.name, ""),
        "200",
        "the client cannot reach the server to begin with"
    );

    // The bait, fetched by the real client through the real server.
    client.get_path(&server.name, "/trap-me", "");

    let found = server.stop_bots("block-honeypot --access-log /var/log/nginx/access.log");
    assert!(
        found.contains(&client.address()),
        "the detector did not find the client in the log NGINX wrote:\n{found}"
    );

    server.stop_bots("render-firewall --backend nftables --out /etc/stop-bots/firewall.nft");
    server.sh("nft -f /etc/stop-bots/firewall.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "the client was detected and blocked but can still reach the server. ruleset:\n{}",
        server.sh("nft list ruleset")
    );
}

/// Each request-shape rule, against a request actually shaped that way.
///
/// The generated `if` conditions have golden files, and a golden file
/// cannot tell you whether `$http_accept = ""` is the variable NGINX
/// populates for a request with no `Accept` header. Only a request with no
/// `Accept` header can.
///
/// `http-1x` and `old-tls` are absent on purpose: both only ever land in a
/// TLS `server` block, and there is no certificate here. `http-1x` would
/// also match curl's own HTTP/1.1, so a plain-HTTP check of it could not
/// tell "the rule works" from "everything is blocked".
#[test]
fn each_request_shape_rule_refuses_a_request_actually_shaped_that_way() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-shape");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");

    // A request that satisfies every rule, so a 403 below is the rule
    // under test and not a header curl happened not to send.
    let ordinary = "-H 'Accept: text/html' -H 'Accept-Language: en' \
                    -A 'Mozilla/5.0' -H 'Host: test.example'";

    for (rule, offending) in [
        (
            "no-accept",
            "-H 'Accept:' -H 'Accept-Language: en' -A 'Mozilla/5.0'",
        ),
        (
            "no-accept-language",
            "-H 'Accept: text/html' -H 'Accept-Language:' -A 'Mozilla/5.0'",
        ),
        (
            "no-user-agent",
            "-H 'Accept: text/html' -H 'Accept-Language: en' -A ''",
        ),
        (
            "ip-literal-host",
            "-H 'Accept: text/html' -H 'Accept-Language: en' -A 'Mozilla/5.0' -H 'Host: 127.0.0.1'",
        ),
    ] {
        server.stop_bots(&format!(
            "set-site-rule --site test.example --rule {rule} --enabled true"
        ));
        server.apply_and_reload();

        assert_eq!(
            server.status("/", offending),
            "403",
            "{rule} was on and a request matching it was served anyway"
        );
        assert_eq!(
            server.status("/", ordinary),
            "200",
            "{rule} refused an ordinary request too"
        );

        server.stop_bots(&format!(
            "set-site-rule --site test.example --rule {rule} --enabled false"
        ));
        server.apply_and_reload();
        assert_eq!(
            server.status("/", offending),
            "200",
            "{rule} kept refusing after it was switched off"
        );
    }
}

/// A per-site setting has to stop at that site. Two `server` blocks, one
/// rule, and a request that is refused by one and served by the other.
///
/// `tests/cli.rs` already checks that the *text* lands in one file and not
/// the other. What it cannot check is that NGINX agrees — an `if` in the
/// wrong block, or a block written to a file NGINX does not include, looks
/// identical on disk.
#[test]
fn a_per_site_rule_applies_to_that_site_and_not_its_neighbour() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-two-sites");
    server.sh(
        "printf 'server {\n    listen 8081;\n    server_name other.example;\n    \
         root /var/www/test;\n    index index.html;\n    location / { try_files $uri $uri/ =404; }\n}\n' \
         > /etc/nginx/sites-enabled/other-site.conf",
    );
    server.sh("nginx -s reload");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");

    let no_ua = "-H 'Accept: text/html' -H 'Accept-Language: en' -A ''";
    assert_eq!(
        server.status("/", no_ua),
        "200",
        "site A serves before the rule"
    );
    assert_eq!(
        server.status_on(8081, "/", no_ua),
        "200",
        "site B serves before the rule"
    );

    server.stop_bots("set-site-rule --site test.example --rule no-user-agent --enabled true");
    server.apply_and_reload();

    assert_eq!(
        server.status("/", no_ua),
        "403",
        "the rule did not take effect on the site it was set for"
    );
    assert_eq!(
        server.status_on(8081, "/", no_ua),
        "200",
        "a rule set for one site is being enforced on its neighbour"
    );
}

/// The probe-path detector, through the same full loop as the honeypot:
/// a real client asks a real NGINX for `/.env`, and the rule that follows
/// really stops it.
///
/// Worth having alongside the honeypot test rather than instead of it —
/// they share the loop but not the matcher, and this one runs against the
/// built-in path list rather than a configured single path.
#[test]
fn probing_for_dotenv_gets_a_real_client_blocked_end_to_end() {
    if !enabled() {
        return;
    }
    let net = Network::create("stop-bots-probe-net");
    let server = Server::start_on_network("stop-bots-probe", &net);
    let client = Client::start("stop-bots-probe-client", &net);

    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.apply_and_reload();
    assert_eq!(
        client.get(&server.name, ""),
        "200",
        "the client cannot reach the server to begin with"
    );

    client.get_path(&server.name, "/.env", "");
    client.get_path(&server.name, "/.git/config", "");

    let found = server.stop_bots("block-probe-paths --access-log /var/log/nginx/access.log");
    assert!(
        found.contains(&client.address()),
        "the detector did not find the probing client in NGINX's own log:\n{found}"
    );

    server.stop_bots("render-firewall --backend nftables --out /etc/stop-bots/firewall.nft");
    server.sh("nft -f /etc/stop-bots/firewall.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "the prober was detected and blocked but can still reach the server. ruleset:\n{}",
        server.sh("nft list ruleset")
    );
}

// ---- is this host actually protected? ----

/// **The check the whole `health` module exists for**, against a host
/// genuinely in the state a real server was found in: a pile of generated
/// rules on disk and an empty ruleset in the kernel.
///
/// No amount of asserting on generated text finds this. The script was
/// correct, the database was correct, and the host was open.
#[test]
fn status_reports_generated_rules_that_never_reached_the_kernel() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-status-unenforced");

    host.stop_bots("add-firewall-rule --address 203.0.113.60");
    host.stop_bots("render-firewall --backend nftables --out /etc/stop-bots/firewall.nft");

    // Written, never applied — exactly what `render-firewall` promises and
    // exactly the gap nothing used to look at.
    let (ok, out, err) = host.run_uncontended(&format!("stop-bots status --db {HOST_DB}"));
    let said = format!("{out}{err}");
    assert!(
        !ok,
        "a host with nothing enforced should exit non-zero:\n{said}"
    );
    assert!(said.contains("CRITICAL"), "was:\n{said}");
    assert!(
        said.contains("none loaded into the kernel"),
        "the report does not name the problem:\n{said}"
    );

    // Load them, and the same command has to change its mind. The rule
    // is a set element now, and the count has to find it there rather
    // than call the ruleset behind.
    host.sh("nft -f /etc/stop-bots/firewall.nft");
    let (ok, out, err) = host.run_uncontended(&format!("stop-bots status --db {HOST_DB}"));
    let said = format!("{out}{err}");
    assert!(
        !said.contains("none loaded into the kernel") && !said.contains("behind"),
        "the report does not see what was loaded:\n{said}"
    );
    assert!(
        ok,
        "with the rules loaded and the console running, nothing should be critical:\n{said}"
    );
}

/// nftables rules live in kernel memory. A host that is protected now and
/// comes back open after a reboot is worth being told about, and the
/// answer comes from systemd rather than from anything this tool wrote.
///
/// The question is whether anything re-applies *our* script. Enabling
/// `nftables.service` used to satisfy this check, although that unit loads
/// /etc/nftables.conf and never ours — so it must not change the answer,
/// and `stop-bots install firewall` must.
#[test]
fn status_notices_that_the_ruleset_will_not_survive_a_reboot() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-status-persist");
    let not_reapplied = "nothing re-applies /etc/stop-bots/firewall.nft at boot";

    let (_, out, err) = host.run_uncontended(&format!("stop-bots status --db {HOST_DB}"));
    let said = format!("{out}{err}");
    assert!(
        said.contains(not_reapplied),
        "a host with nothing re-applying the script was not told:\n{said}"
    );

    host.sh("systemctl enable nftables.service");
    let (_, out, err) = host.run_uncontended(&format!("stop-bots status --db {HOST_DB}"));
    let said = format!("{out}{err}");
    assert!(
        said.contains(not_reapplied),
        "enabling nftables.service, which loads a different file, \
         was taken as re-applying ours:\n{said}"
    );

    host.sh("stop-bots install firewall");
    let (_, out, err) = host.run_uncontended(&format!("stop-bots status --db {HOST_DB}"));
    let said = format!("{out}{err}");
    assert!(
        said.contains("/etc/stop-bots/firewall.nft is re-applied at boot"),
        "`install firewall` did not change the answer:\n{said}"
    );
}

/// **A reboot enforces only what was applied.** The internal cron, `batch`
/// without `--apply` and a bare `render-firewall` all write a script; none
/// of them may change what `stop-bots-firewall.service` loads, or a block
/// nobody applied goes live at the next boot. Checked against the real
/// unit under real systemd: apply one rule, render a second without
/// applying, then drop the table and restart the unit the way a boot
/// would.
#[test]
fn the_boot_unit_loads_only_what_was_applied() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-boot-applied");
    let root = "--root /etc/nginx/sites-enabled";

    host.stop_bots("add-firewall-rule --address 203.0.113.70");
    // `--force`: a container has no SSH log for the lockout check.
    host.stop_bots("render-firewall --apply --force");
    host.sh("stop-bots install firewall");

    host.stop_bots("add-firewall-rule --address 203.0.113.71");
    host.stop_bots("render-firewall");
    host.stop_bots(&format!("batch --no-fetch --force {root}"));
    let rendered = host.sh("cat /etc/stop-bots/firewall.next.nft");
    assert!(
        rendered.contains("203.0.113.71"),
        "the unapplied rule should be in the rendered script:\n{rendered}"
    );

    host.sh("nft delete table inet stop_bots");
    host.sh("systemctl restart stop-bots-firewall.service");

    assert!(
        host.run("nft get element inet stop_bots block_v4 '{ 203.0.113.70 }'")
            .0,
        "the applied rule did not come back with the unit"
    );
    assert!(
        !host
            .run("nft get element inet stop_bots block_v4 '{ 203.0.113.71 }'")
            .0,
        "a rule nobody applied was loaded by the boot unit"
    );
}

/// Run without root, the probe cannot read the ruleset — and must say so
/// rather than reporting a host it could not look at as healthy.
#[test]
fn status_run_without_root_says_it_could_not_look() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-status-unprivileged");
    host.sh("useradd --create-home --shell /bin/sh checker");
    host.sh("install -d -o checker /home/checker/state");

    let (_, out, err) =
        host.run("su checker -c 'stop-bots status --db /home/checker/state/db.sqlite3'");
    let said = format!("{out}{err}");
    assert!(
        said.contains("UNKNOWN"),
        "an unprivileged run must report what it could not check:\n{said}"
    );
    assert!(
        said.contains("needs root"),
        "it should say why it could not check:\n{said}"
    );
}

/// The internal cron takes the probe, and both dashboards read it back.
/// This asserts the half that crosses a process boundary: the console
/// records a probe, and a separate `status --cached` run sees it.
#[test]
fn the_console_records_a_health_probe_for_the_dashboards_to_read() {
    if !enabled() {
        return;
    }
    let host = Host::installed("stop-bots-status-cached");
    host.wait_for_console();

    // The internal cron runs every due job shortly after start-up, and the
    // health check has never run on a fresh database, so it is due.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut said = String::new();
    while std::time::Instant::now() < deadline {
        let (_, out, err) =
            host.run_uncontended(&format!("stop-bots status --cached --db {HOST_DB}"));
        said = format!("{out}{err}");
        if !said.contains("no health probe has been recorded") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    assert!(
        said.contains("check(s)"),
        "the console never recorded a probe:\n{said}"
    );
    assert!(
        said.contains("from a probe taken"),
        "a cached report should say when it was taken:\n{said}"
    );
}

// ---- the firewall backends, against the real thing ----

/// **The backend/path drift bug.**
///
/// `AppState.firewall_out` was a path fixed at start-up
/// (`/etc/stop-bots/firewall.nft`) while the backend came from a form on
/// every render, so a host set to iptables got `#!/bin/sh` and twelve
/// thousand `iptables -A` lines in a file named for the other backend.
/// Worse, the internal cron hardcoded nftables, so every tick quietly
/// replaced the operator's iptables script with an nftables one *at the
/// same path* — and the next apply ran `sh` over nftables syntax.
///
/// Driven through the console, because the console is where the backend
/// is chosen and where the drift was.
#[test]
fn the_console_writes_each_backend_to_its_own_path_in_its_own_syntax() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-drift");
    let console = host.console();

    host.stop_bots("add-firewall-rule --address 203.0.113.23");

    // A decoy from "the other backend", to catch a render that writes to
    // whichever path it happened to be started with.
    host.sh("printf 'DECOY - must not be overwritten\n' > /etc/stop-bots/firewall.next.sh");

    console.post("/render-firewall", &[("backend", "nftables"), ("out", "")]);
    let nft = host.sh("cat /etc/stop-bots/firewall.next.nft");
    assert!(
        nft.contains("203.0.113.23"),
        "the nftables render did not land in firewall.nft:\n{nft}"
    );
    assert!(
        !nft.starts_with("#!/bin/sh"),
        "firewall.nft contains a shell script:\n{nft}"
    );
    // Asserted on syntax rather than on the decoy surviving. The decoy may
    // legitimately be replaced: the console has an internal cron that
    // renders the *stored* backend, every job is due on a fresh database,
    // and since render-on-change it fires again as soon as the rules
    // change — which this test just did. What must never happen is one
    // backend's output landing in the other's file, and a shebang is what
    // tells them apart: the iptables render is a shell script, the
    // nftables one is not.
    let sh_after = host.sh("cat /etc/stop-bots/firewall.next.sh");
    assert!(
        !sh_after.contains("add rule") && !sh_after.contains("nft "),
        "an nftables render overwrote the iptables script:\n{sh_after}"
    );

    // Now the other way round: switching backend must move the *path*
    // too, not just the syntax.
    host.sh("rm -f /etc/stop-bots/firewall.next.sh");
    host.sh("printf 'DECOY - must not be overwritten\n' > /etc/stop-bots/firewall.next.nft");
    console.post("/render-firewall", &[("backend", "iptables"), ("out", "")]);

    let sh = host.sh("cat /etc/stop-bots/firewall.next.sh");
    assert!(
        sh.starts_with("#!/bin/sh"),
        "the iptables render is not a shell script:\n{sh}"
    );
    assert!(
        sh.contains("iptables") && sh.contains("203.0.113.23"),
        "the iptables render has no rule in it:\n{sh}"
    );
    // Same again, and this is the half that caught a real cron race: by
    // now the stored backend is iptables, so the cron renders
    // `firewall.sh` and leaves `firewall.nft` alone — but a tick landing
    // between the decoy and this read would have replaced it with a
    // perfectly legitimate nftables render, and the old assertion called
    // that the bug.
    let nft_after = host.sh("cat /etc/stop-bots/firewall.next.nft");
    assert!(
        !nft_after.starts_with("#!/bin/sh"),
        "an iptables render overwrote the nftables script — the bug exactly:\n{nft_after}"
    );
}

/// `batch` is what a real crontab runs, and it used to carry a `nftables`
/// default that no host setting could override — so an operator who chose
/// iptables in the console got an nftables script from every cron run, at
/// the other path, and ended up with two files in two syntaxes one of
/// which was always stale.
///
/// An explicit `--backend` still wins. Without one, the host's own setting
/// decides.
#[test]
fn batch_follows_the_stored_backend_unless_the_flag_says_otherwise() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-batch-backend");
    let console = host.console();

    host.stop_bots("add-firewall-rule --address 203.0.113.50");

    // The console is where the backend is chosen, so choose it there.
    console.post("/render-firewall", &[("backend", "iptables"), ("out", "")]);
    host.sh("rm -f /etc/stop-bots/firewall.next.sh /etc/stop-bots/firewall.next.nft");

    // No --backend: the stored choice has to decide both syntax and path.
    host.stop_bots("batch --no-fetch --force --root /etc/nginx/sites-enabled");
    assert!(
        !host.run("test -e /etc/stop-bots/firewall.next.nft").0,
        "a crontab-shaped run wrote an nftables script to a host set to iptables"
    );
    let written = host.sh("cat /etc/stop-bots/firewall.next.sh");
    assert!(
        written.starts_with("#!/bin/sh") && written.contains("203.0.113.50"),
        "the stored backend did not decide what batch generated:\n{written}"
    );

    // An explicit flag still overrides it, in both syntax and path.
    host.stop_bots("batch --no-fetch --force --backend nftables --root /etc/nginx/sites-enabled");
    let forced = host.sh("cat /etc/stop-bots/firewall.next.nft");
    assert!(
        !forced.starts_with("#!/bin/sh") && forced.contains("203.0.113.50"),
        "an explicit --backend did not win:\n{forced}"
    );
}

/// The iptables backend has been rendered and golden-tested for a long
/// time and never once executed. `iptables.rs` is at 100% line coverage,
/// which says only that every line ran, not that the script it produces
/// loads.
#[test]
fn generated_iptables_scripts_load_into_real_iptables() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-iptables-load");
    server.stop_bots("add-firewall-rule --address 203.0.113.30");
    server.stop_bots("add-firewall-rule --address 203.0.113.0/24");
    server.stop_bots("render-firewall --backend iptables --out /etc/stop-bots/firewall.sh");

    server.sh("sh /etc/stop-bots/firewall.sh");

    let live = server.sh("iptables -S STOP-BOTS");
    assert!(
        live.contains("203.0.113.30"),
        "single address missing:\n{live}"
    );
    assert!(
        live.contains("203.0.113.0/24"),
        "CIDR range missing:\n{live}"
    );
}

/// Applying the same script twice must leave one chain with one copy of
/// each rule. The generated script flushes `STOP-BOTS` before filling it,
/// and "it flushes" is a claim about a shell script nobody had run twice.
#[test]
fn re_applying_the_iptables_script_does_not_double_the_rules() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-iptables-idempotent");
    server.stop_bots("add-firewall-rule --address 203.0.113.31");
    server.stop_bots("render-firewall --backend iptables --out /etc/stop-bots/firewall.sh");

    server.sh("sh /etc/stop-bots/firewall.sh");
    let once = server.sh("iptables -S STOP-BOTS");
    server.sh("sh /etc/stop-bots/firewall.sh");
    let twice = server.sh("iptables -S STOP-BOTS");

    assert_eq!(
        once, twice,
        "a second run changed the chain, so it is accumulating rules"
    );
    assert_eq!(
        twice.matches("203.0.113.31").count(),
        1,
        "the rule is in the chain more than once:\n{twice}"
    );
}

/// Allowlist mode turns the host into default-deny once the script runs.
/// That is the highest-stakes thing this project can generate, and it had
/// a golden file and no execution.
#[test]
fn allowlist_mode_really_drops_everything_outside_the_selection() {
    if !enabled() {
        return;
    }
    let net = Network::create("stop-bots-allowlist-net");
    let server = Server::start_on_network("stop-bots-allowlist", &net);
    let client = Client::start("stop-bots-allowlist-client", &net);

    assert_eq!(
        client.get(&server.name, ""),
        "200",
        "the client cannot reach the server before any rules exist"
    );

    // A country whose ranges cover nothing the client is in, selected in
    // allowlist mode: everything else must be dropped.
    server.sh("printf '192.0.2.0/24\n' > /tmp/zone.zone");
    server.stop_bots("update-country-ranges --country nl --source /tmp/zone.zone");
    server.stop_bots("add-country --country nl");
    server.stop_bots("set-geo-mode --mode allowlist");
    server.stop_bots("render-firewall --backend nftables --out /etc/stop-bots/firewall.nft");
    server.sh("nft -f /etc/stop-bots/firewall.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "allowlist mode let an address outside the selection through. ruleset:\n{}",
        server.sh("nft list ruleset")
    );
    // And the host has not cut off its own loopback, which is the way
    // default-deny goes wrong.
    assert_eq!(
        server.status("/", ""),
        "200",
        "allowlist mode blocked loopback"
    );
}

/// The bug from a real host: its NGINX container could no longer reach a
/// service on the host. A container's source is a private bridge address
/// arriving on the *input* hook, and the allow-list catch-all (like a feed
/// listing private space as bogons) dropped it before ufw saw a packet.
/// Private sources now get past everything derived — and an operator's own
/// block of a private range still applies, because it comes first.
#[test]
fn a_container_on_a_private_bridge_still_reaches_the_host_in_allowlist_mode() {
    if !enabled() {
        return;
    }
    let net = Network::create_private("stop-bots-private-net");
    let server = Server::start_on_network("stop-bots-private-host", &net);
    let client = Client::start("stop-bots-private-client", &net);
    let client_ip = client.address();

    server.sh("printf '192.0.2.0/24\n' > /tmp/zone.zone");
    server.stop_bots("update-country-ranges --country nl --source /tmp/zone.zone");
    server.stop_bots("add-country --country nl");
    server.stop_bots("set-geo-mode --mode allowlist");
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "200",
        "the allow-list catch-all dropped {client_ip}, a private source. ruleset:\n{}",
        server.sh("nft list ruleset")
    );

    server.stop_bots(&format!(
        "add-firewall-rule --address {client_ip} --action block"
    ));
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "an explicit block of {client_ip} must still apply"
    );
}

/// `nftables`' `inet` family covers IPv4 and IPv6 in one table, and an
/// IPv6 block has to stop IPv6 packets from a real second host.
///
/// **This test used to pass without testing anything.** It added
/// 2001:db8::1 to the server's own `lo` and requested it from inside the
/// server, which failed with "connection refused" before any rule was
/// consulted: the site listens on `0.0.0.0:8080` only, so nothing answered
/// on IPv6 at all — and had anything answered, the ruleset accepts `iif
/// lo` before its sets. Reproduced by running the old request with no
/// ruleset loaded: `000`, curl exit 7. Now the request comes from a client
/// container over a dual-stack network, the site listens on IPv6 too, and
/// the same request is shown to succeed before the rule goes in.
#[test]
fn an_ipv6_rule_really_drops_ipv6_traffic() {
    if !enabled() {
        return;
    }
    let net = Network::create_dual_stack("stop-bots-ipv6-net");
    let server = Server::start_on_network("stop-bots-ipv6", &net);
    let client = Client::start("stop-bots-ipv6-client", &net);
    server.sh(
        "sed -i 's/listen 8080;/listen 8080;\\n    listen [::]:8080;/' \
         /etc/nginx/sites-enabled/test-site.conf && nginx -s reload",
    );
    let server_v6 = format!("[{}]", address_of(&server.name, "GlobalIPv6Address"));
    let client_v6 = client.address_v6();
    assert!(
        client_v6.contains(':'),
        "the client has no IPv6 address: {client_v6:?}"
    );

    // The control: without it, a request that fails for any other reason
    // reads as the rule working.
    let served = (0..20).any(|_| {
        let ok = client.get(&server_v6, "-g --max-time 2") == "200";
        if !ok {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        ok
    });
    assert!(served, "the IPv6 client was not served before any rule");

    server.stop_bots(&format!("add-firewall-rule --address {client_v6}"));
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");

    assert!(
        server.in_set("block_v6", &client_v6),
        "the IPv6 rule is not in the live ruleset:\n{}",
        server.sh("nft list table inet stop_bots")
    );
    assert_eq!(
        client.get(&server_v6, "-g --max-time 5"),
        "000",
        "packets from {client_v6} were still served"
    );
    assert_eq!(
        client.get(&address_of(&server.name, "IPAddress"), "--max-time 5"),
        "200",
        "the same client over IPv4 is not blocked, so it must still be served"
    );
}

// ---- NGINX: does the generated config actually parse and behave? ----

/// The check TODO.md has been asking for: every directive form this
/// project can emit, through a real `nginx -t`.
#[test]
fn every_generated_directive_form_parses() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-parse");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");

    // Everything on at once: patterns, exemptions (the flag form), rate
    // limiting, robots.txt, and every request-shape rule.
    server.stop_bots("set-robots-txt --enabled true");
    server.stop_bots("set-rate-limit --enabled true --rps 5 --burst 10");
    // And trust, which adds a `geo`, two `map`s and a second zone in
    // `conf.d`, and a clear to every block.
    server.stop_bots("trust --address 2001:db8::/48");
    server.stop_bots("trust --user-agent Pingdom.com_bot");
    // And an agent exemption, which sets and reads a variable of its own.
    server.stop_bots(
        "exempt-path --site test.example --path /remote.php/dav/ --user-agent okhttp/4.10",
    );

    server.apply_and_reload();
    let (ok, output) = server.nginx_t();
    assert!(ok, "nginx rejected the generated config:\n{output}");
}

/// The whole point of the tool, checked against a live server rather than
/// against the text we hoped would produce it.
#[test]
fn a_blocked_user_agent_is_refused_and_everyone_else_is_served() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-block");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");

    // Before applying: everyone gets in.
    assert_eq!(server.status("/", "-A 'BadBot/1.0'"), "200");

    server.apply_and_reload();

    assert_eq!(
        server.status("/", "-A 'BadBot/1.0'"),
        "403",
        "a blocked user agent should be refused"
    );
    assert_eq!(
        server.status("/", "-A 'Mozilla/5.0'"),
        "200",
        "an ordinary visitor must still be served"
    );
}

/// An agent exemption lets its client through on its paths and nowhere
/// else: not on another path, not by walking out of the prefix with `..`,
/// and not for another bot on the same path. A client let through reaches
/// the site's own `try_files`, so a missing file is its 404, where a
/// refused one gets the block's 403.
#[test]
fn an_agent_exemption_serves_only_its_client_on_only_its_paths() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-agent-exempt");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("okhttp", "okhttp");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("exempt-path --site test.example --path /dav/ --user-agent okhttp");
    server.apply_and_reload();

    for (path, user_agent, expected, why) in [
        (
            "/dav/file",
            "okhttp/4.10.0",
            "404",
            "the exempt client on its path",
        ),
        (
            "/DAV/file",
            "okhttp/4.10.0",
            "404",
            "the path, matched without case",
        ),
        (
            "/",
            "okhttp/4.10.0",
            "403",
            "the exempt client off its path",
        ),
        (
            "/dav/../index.html",
            "okhttp/4.10.0",
            "403",
            "walking out with ..",
        ),
        (
            "/dav/%2e%2e/index.html",
            "okhttp/4.10.0",
            "403",
            "walking out with %2e%2e",
        ),
        (
            "/dav/file",
            "BadBot/1.0",
            "403",
            "another bot on the exempt path",
        ),
        ("/", "Mozilla/5.0", "200", "an ordinary visitor"),
    ] {
        assert_eq!(
            server.status(path, &format!("--path-as-is -A '{user_agent}'")),
            expected,
            "{why}: {user_agent} on {path}"
        );
    }
}

// ---- trust: what NGINX and the kernel actually let through ----

/// A trusted user agent gets past a bot pattern that matches it and a
/// request-shape rule it trips, while the bot it resembles is still
/// refused. Checked against a live NGINX because every piece of the
/// mechanism — the `map` in `conf.d`, the variable it defines, the clear
/// in the sentinel block — only means anything once NGINX has parsed it.
#[test]
fn a_trusted_user_agent_is_served_past_every_block_it_trips() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-trust-ua");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("set-site-rule --site test.example --rule no-accept-language --enabled true");
    server.stop_bots("trust --user-agent badbot-monitor");
    server.apply_and_reload();

    assert_eq!(
        server.status("/", "-A 'BadBot/1.0' -H 'Accept-Language: en'"),
        "403",
        "trusting one agent must not let the bot it resembles through"
    );
    assert_eq!(
        server.status("/", "-A 'Mozilla/5.0 (compatible; BadBot-Monitor/2.0)'"),
        "200",
        "a trusted agent (matched without case, sending no Accept-Language) was refused"
    );
}

/// A trusted range is served whatever its user agent, and never rate
/// limited — the zone is keyed on a variable that is empty for it.
#[test]
fn a_trusted_address_is_served_past_blocks_and_never_rate_limited() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-trust-addr");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("set-rate-limit --enabled true --rps 1 --burst 2");
    let burst = "for i in $(seq 1 12); do curl -s -o /dev/null -w '%{http_code} ' \
                 http://127.0.0.1:8080/; sleep 0.05; done";

    // Rate limiting live *before* anything is trusted, then trust added,
    // then taken away, reloading each time. That order is the one that
    // broke: `nginx -t` passed every time, and NGINX rejected the reload
    // for re-keying a zone it already had, leaving the old config serving
    // with nothing but a line in the error log to say so.
    server.apply_and_reload();
    assert_eq!(server.status("/", "-A 'BadBot/1.0'"), "403");

    // A range rather than the one address, so the `geo` lookup is doing
    // CIDR matching rather than an exact comparison.
    server.stop_bots("trust --address 127.0.0.0/8");
    server.apply_and_reload();
    assert_eq!(
        server.status("/", "-A 'BadBot/1.0'"),
        "200",
        "a client from a trusted range was refused over its user agent"
    );
    let codes = server.sh(burst);
    assert!(
        !codes.contains("429"),
        "a trusted client was rate limited: {codes}"
    );

    server.stop_bots("trust --remove --address 127.0.0.0/8");
    server.apply_and_reload();
    assert_eq!(server.status("/", "-A 'BadBot/1.0'"), "403");
    let codes = server.sh(burst);
    assert!(
        codes.contains("429"),
        "untrusted again, the burst should be limited: {codes}"
    );

    let (_, log, _) = server.run("cat /var/log/nginx/error.log");
    assert!(
        !log.contains("[emerg]"),
        "NGINX rejected a reload that `nginx -t` passed:\n{log}"
    );
}

/// The firewall half, with real packets: a client inside a blocked /24
/// still reaches the server once its own address is trusted, because the
/// accept is evaluated first.
#[test]
fn a_trusted_client_inside_a_blocked_range_still_reaches_the_server() {
    if !enabled() {
        return;
    }
    let net = Network::create("stop-bots-trust-net");
    let server = Server::start_on_network("stop-bots-trust-target", &net);
    let client = Client::start("stop-bots-trust-client", &net);
    let client_ip = client.address();
    let range = format!("{}.0/24", client_ip.rsplit_once('.').unwrap().0);

    server.stop_bots(&format!(
        "add-firewall-rule --address {range} --action block"
    ));
    server.stop_bots(&format!("trust --address {client_ip}"));
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "200",
        "{client_ip} is trusted but was dropped by the block on {range}"
    );

    server.stop_bots(&format!("trust --remove --address {client_ip}"));
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "without the trust, the block on {range} should drop {client_ip}"
    );
}

#[test]
fn each_block_response_is_what_the_client_actually_receives() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-responses");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");

    for (arg, expected) in [
        ("forbidden", "403"),
        ("not-found", "404"),
        ("gone", "410"),
        ("teapot", "418"),
        ("too-many-requests", "429"),
    ] {
        server.stop_bots(&format!("set-block-response --response {arg}"));
        server.apply_and_reload();
        assert_eq!(
            server.status("/", "-A 'BadBot/1.0'"),
            expected,
            "{arg} should reach the client as {expected}"
        );
    }

    // 444 closes without replying, so curl reports no status at all
    // rather than a code — which is the observable difference and worth
    // asserting rather than assuming.
    server.stop_bots("set-block-response --response close");
    server.apply_and_reload();
    assert_eq!(
        server.status("/", "-A 'BadBot/1.0'"),
        "000",
        "444 should close the connection with no response"
    );
}

/// The exemption mechanism, and the guard that keeps ACME working.
#[test]
fn exempt_paths_and_well_known_stay_reachable_for_a_blocked_client() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-exempt");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("badbot", "BadBot");
    server.sh(
        "mkdir -p /var/www/test/blog /var/www/test/.well-known/acme-challenge && \
         echo ok > /var/www/test/blog/index.html && \
         echo token > /var/www/test/.well-known/acme-challenge/test",
    );

    // An exempt path, plus a request rule that forces /.well-known/ open.
    server.stop_bots("set-site-rule --site test.example --rule no-user-agent --enabled true");
    server.stop_bots("exempt-path --site test.example --path /blog");
    server.apply_and_reload();

    assert_eq!(
        server.status("/", "-A 'BadBot/1.0'"),
        "403",
        "the site itself is still blocked"
    );
    assert_eq!(
        server.status("/blog/", "-A 'BadBot/1.0'"),
        "200",
        "an exempt path must stay reachable"
    );
    assert_eq!(
        server.status("/.well-known/acme-challenge/test", "-A '' -H 'User-Agent;'"),
        "200",
        "ACME validation must survive a request rule that would otherwise catch it"
    );
}

/// `s` in single quotes for `sh -c`, so none of it is shell syntax.
fn sh_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// User agents a site that logs in JSON (`escape=json`) can put in front of
/// an operator, each with a character NGINX's config reader or PCRE treats
/// specially, and for some a near miss: a user agent the rule would also
/// have caught had NGINX read the regex differently from how it was
/// checked. The same list as `HOSTILE_USER_AGENTS` in `src/nginx.rs`.
const HOSTILE_USER_AGENTS: [(&str, Option<&str>); 11] = [
    // Read as `...a\\|`: "a backslash, or nothing" — every visitor.
    (r"Scraper/2.1 a\|", Some("Scraper/2.1 a|")),
    // Read as `x\\*`: an x and any number of backslashes.
    (r"Scraper/2.1 x\*", Some("Scraper/2.1 x")),
    // Read as an unclosed group: `nginx -t` failed, and every apply after.
    (r"Scraper/2.1 Evil\(", None),
    (r#"Scraper/2.1 say "hi""#, None),
    (r#"Scraper/2.1 \"quoted\""#, Some(r#"Scraper/2.1 "quoted""#)),
    (r"Scraper/2.1 trailing\", None),
    // Read as a tab and a line feed.
    (r"Scraper/2.1 tab\t", Some("Scraper/2.1 tab")),
    (r"Scraper/2.1 line\n", Some("Scraper/2.1 line")),
    ("Scraper/2.1 $foo", None),
    ("Scraper/2.1 }", None),
    ("Scraper/2.1 ;", None),
];

/// The finding, end to end: blocked user agents with a backslash, a quote
/// or a brace in them, stored the way a release that took them wrote them,
/// each turn away exactly that client under a real NGINX — and not a
/// browser, not curl, not Googlebot.
///
/// Stored with `sqlite3` rather than through the console, because the
/// console now refuses a backslash in a blocked user agent: these are the
/// rows a host that upgrades already has. Two more such rows ride along:
/// path exemptions `/x\|` (which exempted every path) and `/x\(` (which
/// failed `nginx -t`), and a bot list with patterns PCRE refuses, which
/// failed `nginx -t` at apply time and rolled every apply back.
#[test]
fn hostile_stored_entries_block_exactly_their_client_under_a_real_nginx() {
    if !enabled() {
        return;
    }
    let host = Host::start("stop-bots-hostile");
    let db = "/tmp/db.sqlite3";
    let stop_bots = |args: &str| host.sh(&format!("stop-bots {args} --db {db}"));
    stop_bots("scan-sites --root /etc/nginx/sites-enabled");

    let mut sql = String::new();
    for (user_agent, _) in HOSTILE_USER_AGENTS {
        sql.push_str(&format!(
            "INSERT INTO blocked_user_agents (user_agent, blocked_at) VALUES ('{}', 0);\n",
            user_agent.replace('\'', "''")
        ));
    }
    for path in [r"/x\|", r"/x\("] {
        sql.push_str(&format!(
            "INSERT INTO site_path_exemptions (site_id, path) \
             SELECT id, '{path}' FROM sites WHERE server_name = 'test.example';\n"
        ));
    }
    host.sh(&format!("sqlite3 {db} <<'SQL'\n{sql}SQL"));

    let json = r#"[{"id":"listed","categories":["ai"],"pattern":{"accepted":["ListedBot","Bot[z-a]x","Botx{2,1}","Botx{99999}"],"forbidden":[]}}]"#;
    host.sh(&format!("cat > /tmp/bots.json <<'JSON'\n{json}\nJSON"));
    let fetched = stop_bots("update-bot-lists --source /tmp/bots.json");
    assert!(
        fetched.contains("left out 3 pattern(s)"),
        "the fetch did not say what it left out:\n{fetched}"
    );

    let (applied, warned) = {
        let (ok, stdout, stderr) = host.run(&format!(
            "stop-bots apply-blocks --root /etc/nginx/sites-enabled --no-reload --db {db}"
        ));
        assert!(ok, "apply-blocks failed:\n{stdout}{stderr}");
        (stdout, stderr)
    };
    assert!(
        warned.contains(r#"exempt path "/x\\|""#) && warned.contains(r#"exempt path "/x\\(""#),
        "the unwritten exemptions were not named:\n{applied}{warned}"
    );
    let (ok, stdout, stderr) = host.run("nginx -t");
    assert!(
        ok,
        "nginx rejected the generated config:\n{stdout}{stderr}\n{}",
        host.sh("cat /etc/nginx/sites-enabled/test-site.conf")
    );
    host.sh("systemctl reload nginx");
    std::thread::sleep(std::time::Duration::from_millis(300));

    let status = |user_agent: &str| {
        host.run(&format!(
            "curl -s -o /dev/null -w '%{{http_code}}' -A {} http://127.0.0.1:8080/",
            sh_quoted(user_agent)
        ))
        .1
        .trim()
        .to_string()
    };
    for (user_agent, near_miss) in HOSTILE_USER_AGENTS {
        assert_eq!(status(user_agent), "403", "{user_agent:?} was served");
        if let Some(near_miss) = near_miss {
            assert_eq!(
                status(near_miss),
                "200",
                "{near_miss:?} was refused by the rule for {user_agent:?}"
            );
        }
    }
    assert_eq!(
        status("ListedBot/1.0"),
        "403",
        "the list's good pattern was lost"
    );
    for ordinary in [
        "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
        "curl/8.14.1",
        "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
    ] {
        assert_eq!(status(ordinary), "200", "{ordinary:?} was refused");
    }
}

#[test]
fn the_generated_robots_txt_is_served() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-robots");
    server.stop_bots("scan-sites --root /etc/nginx/sites-enabled");
    server.seed_bot("gptbot", "GPTBot");
    server.stop_bots("set-robots-txt --enabled true");
    server.apply_and_reload();

    let body = server.body("/robots.txt", "");
    assert!(
        body.contains("User-agent: GPTBot"),
        "robots.txt was:\n{body}"
    );
    assert!(body.contains("Disallow:"), "robots.txt was:\n{body}");

    // And a blocked crawler can still read it — otherwise the file names
    // exactly the agents that can never see it.
    let as_bot = server.body("/robots.txt", "-A 'GPTBot/1.0'");
    assert!(
        as_bot.contains("User-agent: GPTBot"),
        "a blocked crawler must still be able to read robots.txt; got:\n{as_bot}"
    );
}

// ---- nftables: does the generated script load, and does it block? ----

/// The other half of the "never validated against the real tool" gap:
/// every firewall script shape, through a real `nft -f`.
#[test]
fn generated_firewall_scripts_load_into_real_nftables() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-nft");
    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");
    server.stop_bots("add-firewall-rule --address 198.51.100.0/24 --action block");
    server.stop_bots("add-firewall-rule --address 2001:db8:1:2::/64 --action block");
    server.stop_bots("add-firewall-rule --address 192.0.2.7 --action allow");

    // No SSH log in the container, so the check can't run — this is the
    // documented `--force` path, and the CLI is where it's allowed.
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");

    // `-c` is a parse-and-check that touches nothing, then the real load.
    let (ok, stdout, stderr) = server.run("nft -c -f /tmp/fw.nft");
    assert!(ok, "nft rejected the generated script:\n{stdout}{stderr}");
    server.sh("nft -f /tmp/fw.nft");

    let ruleset = server.sh("nft list table inet stop_bots");
    for (set, element) in [
        ("block_v4", "203.0.113.9"),
        ("block_v4", "198.51.100.0/24"),
        ("block_v6", "2001:db8:1:2::/64"),
        ("allow_v4", "192.0.2.7"),
    ] {
        assert!(
            server.in_set(set, element),
            "{element} missing from {set} in the loaded ruleset:\n{ruleset}"
        );
    }
    for rule in [
        "ip saddr @block_v4 drop",
        "ip6 saddr @block_v6 drop",
        "ip saddr @allow_v4 accept",
    ] {
        assert!(
            ruleset.contains(rule),
            "{rule:?} missing from the loaded ruleset:\n{ruleset}"
        );
    }

    // The script resets only its own table, so applying twice is a no-op
    // rather than a duplicate — the idiom TODO.md flagged as reasoned-
    // through but unverified.
    server.sh("nft -f /tmp/fw.nft");
    let twice = server.sh("nft list table inet stop_bots");
    assert_eq!(
        twice.matches("203.0.113.9").count(),
        1,
        "re-applying must not duplicate elements:\n{twice}"
    );
    assert_eq!(
        twice.matches("@block_v4").count(),
        1,
        "re-applying must not duplicate rules:\n{twice}"
    );
}

/// Loopback is deliberately exempt, and that is a safety property worth
/// pinning: the generated chain accepts `iif lo` before any drop rule, so
/// a rule an admin adds for their own public address can never sever the
/// machine's own local traffic.
#[test]
fn loopback_is_never_blocked_however_the_rules_are_written() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-loopback");
    server.stop_bots("add-firewall-rule --address 127.0.0.1 --action block");
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");

    assert_eq!(
        server.status("/", "--max-time 5"),
        "200",
        "loopback must survive even an explicit rule against it"
    );
    let ruleset = server.sh("nft list table inet stop_bots");
    assert!(
        ruleset.contains("iif \"lo\" accept"),
        "the lo exemption should come from the generated chain:\n{ruleset}"
    );
}

/// Real packets, from a real second host, across a real network.
///
/// Everything above sends requests from inside the server container, where
/// the source address is loopback and therefore exempt. This is the test
/// that actually proves a firewall rule stops a remote client: a separate
/// container, on a user-defined Docker network, with its own address.
#[test]
fn a_blocked_client_container_cannot_reach_the_server() {
    if !enabled() {
        return;
    }
    let net = Network::create("stop-bots-net");
    let server = Server::start_on_network("stop-bots-target", &net);
    let client = Client::start("stop-bots-client", &net);

    let client_ip = client.address();
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "200",
        "the client should reach the server before anything is blocked"
    );

    server.stop_bots(&format!(
        "add-firewall-rule --address {client_ip} --action block"
    ));
    server.stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --force");
    server.sh("nft -f /tmp/fw.nft");

    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "packets from {client_ip} should not reach the server at all"
    );

    // Removing our table restores service — the recovery an admin needs
    // after locking themselves out, and the reason the generated script
    // never touches any other table.
    server.sh("nft delete table inet stop_bots");
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "200",
        "deleting the table should restore service"
    );
}

// ---- batch mode: does one command really do the lot? ----

/// `batch --apply` is the only thing in this project that enforces
/// anything unattended, and it enforces on two planes at once. Both are
/// checked the only way that means anything: a real `nft` ruleset, and a
/// real HTTP request to a real NGINX that has genuinely been reloaded.
///
/// `--force` because a container has no SSH log, so the lockout guard
/// cannot run — which is exactly the case `--apply` refuses on, and the
/// documented way through. `--no-fetch` to keep the run offline and
/// deterministic.
#[test]
fn batch_apply_enforces_on_both_planes_at_once() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-batch");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");

    // Everything is still inert at this point: no table, and the server
    // serves the bot happily.
    assert_eq!(server.status("/", "-H 'User-Agent: BadBot/1.0'"), "200");

    let output = server.stop_bots(
        "batch --apply --force --no-fetch --verbose \
         --root /etc/nginx/sites-enabled --out /tmp/fw.nft --access-log /dev/null",
    );

    // The firewall plane: the script was written *and* loaded.
    let ruleset = server.sh("nft list table inet stop_bots");
    assert!(
        server.in_set("block_v4", "203.0.113.9") && ruleset.contains("ip saddr @block_v4 drop"),
        "batch --apply should have loaded the rules; batch said:\n{output}\nruleset:\n{ruleset}"
    );

    // The NGINX plane: the config was written *and* reloaded, which only
    // a request can show.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        server.status("/", "-H 'User-Agent: BadBot/1.0'"),
        "403",
        "batch --apply should have reloaded NGINX; batch said:\n{output}"
    );
    assert_eq!(
        server.status("/", "-H 'User-Agent: Mozilla/5.0'"),
        "200",
        "and everyone else must still be served"
    );
}

/// Without `--apply`, the same run writes both and enforces neither. This
/// is the project's default and the reason `--apply` is a flag rather
/// than the behaviour: a script on disk and a config NGINX has not
/// re-read are both inert.
#[test]
fn batch_without_apply_writes_both_and_enforces_neither() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-batch-dry");
    server.seed_bot("badbot", "BadBot");
    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");

    server.stop_bots(
        "batch --no-fetch --root /etc/nginx/sites-enabled \
         --out /tmp/fw.nft --access-log /dev/null",
    );

    // Beside `--out`, which names the applied script: nothing was applied.
    assert!(
        server.run("test -s /tmp/fw.next.nft").0,
        "the script should have been written"
    );
    assert!(
        !server.run("test -e /tmp/fw.nft").0,
        "and not where the applied script goes"
    );
    let (loaded, _, _) = server.run("nft list table inet stop_bots");
    assert!(!loaded, "but nothing should have been loaded into nftables");
    assert_eq!(
        server.status("/", "-H 'User-Agent: BadBot/1.0'"),
        "200",
        "and NGINX should still be serving its old config"
    );
}

// ---- the lockout guard, against a real ruleset ----

/// The guard that failed in production. With a log naming a connected
/// admin, a rule covering that admin must stop the render — before
/// anything is written, let alone applied.
#[test]
fn rendering_refuses_to_lock_out_a_connected_admin() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-lockout");
    server.sh(
        "printf '%s\\n' 'Aug 17 10:00:01 host sshd[100]: Accepted publickey for admin \
         from 198.51.100.42 port 50000 ssh2' > /tmp/auth.log",
    );
    // A rule that contains the admin's address — the shape of what took a
    // real server off the network.
    server.stop_bots("add-firewall-rule --address 198.51.100.0/24 --action block");

    let output = server.stop_bots_expect_failure(
        "render-firewall --backend nftables --out /tmp/fw.nft --ssh-log /tmp/auth.log",
    );
    assert!(
        output.contains("198.51.100.42"),
        "the refusal should name the admin it would cut off:\n{output}"
    );
    let (exists, _, _) = server.run("test -f /tmp/fw.nft");
    assert!(!exists, "nothing may be written when the guard refuses");
}

/// The counterpart: a log with no risk in it lets the render through.
#[test]
fn rendering_proceeds_when_no_connected_admin_is_affected() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-lockout-ok");
    server.sh(
        "printf '%s\\n' 'Aug 17 10:00:01 host sshd[100]: Accepted publickey for admin \
         from 192.0.2.10 port 50000 ssh2' > /tmp/auth.log",
    );
    server.stop_bots("add-firewall-rule --address 198.51.100.0/24 --action block");
    server
        .stop_bots("render-firewall --backend nftables --out /tmp/fw.nft --ssh-log /tmp/auth.log");
    server.sh("nft -c -f /tmp/fw.nft");
}

// ---- sets, restores and upgrades: the firewall at scale ----

/// The /28 a test network put `ip` in: the range to block around a
/// client, so that an Allow for the client alone has something to beat.
fn subnet_28(ip: &str) -> String {
    let (head, last) = ip.rsplit_once('.').expect("an IPv4 address");
    let last: u8 = last.parse().expect("an IPv4 address");
    format!("{head}.{}/28", last / 16 * 16)
}

/// Real packets against both halves of the ordering the sets must keep:
/// an address inside a blocked range is dropped, and an Allow given
/// before that range still lets its one client through.
fn allow_ahead_of_a_blocked_range_wins(backend: &str, prefix: &str) {
    let net = Network::create(&format!("{prefix}-net"));
    let server = Server::start_on_network(&format!("{prefix}-server"), &net);
    let client = Client::start(&format!("{prefix}-client"), &net);
    let client_ip = client.address();
    let range = subnet_28(&client_ip);

    server.stop_bots(&format!(
        "add-firewall-rule --address {client_ip} --action allow"
    ));
    server.stop_bots(&format!(
        "add-firewall-rule --address {range} --action block"
    ));
    server.render_and_load(backend);
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "200",
        "{backend}: the Allow for {client_ip} came first and must beat the block of {range}. \
         script:\n{}",
        server.sh("cat /tmp/fw")
    );

    // The Allow was rule 1. Without it the range decides.
    server.stop_bots("remove-firewall-rule --id 1");
    server.render_and_load(backend);
    assert_eq!(
        client.get(&server.name, "--max-time 5"),
        "000",
        "{backend}: {client_ip} is inside the blocked {range} and must be dropped. script:\n{}",
        server.sh("cat /tmp/fw")
    );
}

#[test]
fn nftables_sets_drop_a_listed_address_and_an_allow_ahead_still_wins() {
    if !enabled() {
        return;
    }
    allow_ahead_of_a_blocked_range_wins("nftables", "stop-bots-sets");
}

#[test]
fn an_iptables_restore_drops_a_listed_address_and_an_allow_ahead_still_wins() {
    if !enabled() {
        return;
    }
    allow_ahead_of_a_blocked_range_wins("iptables", "stop-bots-restore");
}

/// A detector's block expires, and since sets the kernel is what lets it
/// go: the element carries a `timeout`, and nothing has to apply the
/// script again for the block to lift.
#[test]
fn a_timed_block_is_loaded_with_a_timeout_that_the_kernel_honours() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-timeout");
    server.sh(
        "for i in $(seq 25); do echo 'Aug 17 10:00:01 host sshd[100]: Failed password \
         for root from 203.0.113.50 port 4444 ssh2'; done > /tmp/auth.log",
    );
    server.stop_bots("block-scanners --ssh-log /tmp/auth.log --ttl-days 1");
    server.render_and_load("nftables");

    let set = server.sh("nft list set inet stop_bots block_v4");
    assert!(
        set.contains("203.0.113.50 timeout") && set.contains("expires"),
        "the detector's block should be a timed element:\n{set}"
    );

    // A day is too long to wait for, so the same script with the timeout
    // cut to a few seconds: the kernel, not a re-render, has to remove it.
    // Loaded and looked up in one exec, so a slow runner cannot spend the
    // timeout between the two.
    server.sh("sed -E 's/timeout [0-9dhms]+/timeout 4s/' /tmp/fw > /tmp/fw-short");
    let (present, _, stderr) = server
        .run("nft -f /tmp/fw-short && nft get element inet stop_bots block_v4 '{ 203.0.113.50 }'");
    assert!(
        present,
        "the element should be there before its timeout: {stderr}"
    );
    server.sh("sleep 5");
    assert!(
        !server.in_set("block_v4", "203.0.113.50"),
        "the kernel should have dropped the element when its timeout ran out:\n{}",
        server.sh("nft list set inet stop_bots block_v4")
    );
}

/// IPv6 used to be skipped by this backend altogether, including every
/// /64 a detector writes. It goes to ip6tables now, into the same chain
/// shape with the same jump.
#[test]
fn the_iptables_backend_loads_ipv6_rules_into_ip6tables() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-ip6tables");
    server.stop_bots("add-firewall-rule --address 2001:db8:1:2::/64 --action block");
    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");
    server.render_and_load("iptables");

    let v6 = server.sh("ip6tables -S STOP-BOTS");
    assert!(
        v6.contains("-s 2001:db8:1:2::/64 -j DROP"),
        "the IPv6 rule is not in ip6tables:\n{v6}"
    );
    assert!(
        !v6.contains("203.0.113.9"),
        "an IPv4 rule reached ip6tables:\n{v6}"
    );
    let jumps = server.sh("ip6tables -S INPUT");
    assert!(
        jumps.contains("-A INPUT -j STOP-BOTS"),
        "nothing jumps to the IPv6 chain:\n{jumps}"
    );
    assert!(
        server.sh("iptables -S STOP-BOTS").contains("203.0.113.9"),
        "the IPv4 half is missing"
    );
}

/// The reason for the restore. The old script flushed the chain and then
/// ran one process per rule under `set -e`, so a failure part way through
/// left the chain half filled until the next apply. A restore is checked
/// in full before it is committed: a failed apply leaves the previous
/// chain exactly as it was.
#[test]
fn a_failed_iptables_apply_leaves_the_previous_chain_whole() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-restore-atomic");
    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");
    server.stop_bots("add-firewall-rule --address 203.0.113.10 --action block");
    server.render_and_load("iptables");
    let before = server.sh("iptables -S STOP-BOTS");

    // The same script with a new rule, then one iptables-restore refuses.
    server.sh(
        "sed 's|^-A STOP-BOTS -s 203.0.113.10 -j DROP$|&\\n-A STOP-BOTS -s 198.51.100.1 -j DROP\\n-A STOP-BOTS -s not-an-address -j DROP|' \
         /tmp/fw > /tmp/fw-broken",
    );
    assert!(
        server.sh("cat /tmp/fw-broken").contains("not-an-address"),
        "the test did not manage to break the script"
    );
    let (ok, _, stderr) = server.run("sh /tmp/fw-broken");
    assert!(!ok, "the broken script should fail");
    assert!(stderr.contains("not-an-address"), "stderr was:\n{stderr}");

    assert_eq!(
        server.sh("iptables -S STOP-BOTS"),
        before,
        "a failed apply changed the live chain"
    );
}

/// A host upgrading from 0.0.15 has that version's script loaded: one rule
/// per address, no sets. Loading the new one over it has to leave the new
/// shape and nothing of the old.
#[test]
fn a_ruleset_written_by_0_0_15_is_replaced_cleanly_by_sets() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-nft-upgrade");
    let old = include_str!("fixtures/nftables/written-by-0.0.15.nft");
    server.sh(&format!("cat > /tmp/old.nft <<'NFT'\n{old}NFT"));
    server.sh("nft -f /tmp/old.nft");
    assert!(
        server
            .sh("nft list table inet stop_bots")
            .contains("ip saddr 198.51.100.0/24 drop"),
        "the 0.0.15 script did not load as it used to"
    );

    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");
    server.render_and_load("nftables");

    let table = server.sh("nft list table inet stop_bots");
    for gone in ["198.51.100.0/24", "203.0.113.7", "2001:db8::/32"] {
        assert!(
            !table.contains(gone),
            "{gone} from the 0.0.15 script survived the upgrade:\n{table}"
        );
    }
    assert!(
        server.in_set("block_v4", "203.0.113.9"),
        "the new rule is not in its set:\n{table}"
    );
}

/// The same for iptables: 0.0.15's script made the chain and the jumps
/// with plain `iptables` commands. The restore takes the chain over, and
/// the jumps are found rather than added a second time.
#[test]
fn a_chain_written_by_0_0_15_is_taken_over_by_the_restore() {
    if !enabled() {
        return;
    }
    let server = Server::start("stop-bots-ipt-upgrade");
    let old = include_str!("fixtures/iptables/written-by-0.0.15.sh");
    server.sh(&format!("cat > /tmp/old.sh <<'SH'\n{old}SH"));
    server.sh("sh /tmp/old.sh");
    assert!(
        server
            .sh("iptables -S STOP-BOTS")
            .contains("198.51.100.0/24"),
        "the 0.0.15 script did not load as it used to"
    );

    server.stop_bots("add-firewall-rule --address 203.0.113.9 --action block");
    server.render_and_load("iptables");

    let chain = server.sh("iptables -S STOP-BOTS");
    assert_eq!(
        chain
            .lines()
            .filter(|l| l.contains(" -s "))
            .collect::<Vec<_>>(),
        vec!["-A STOP-BOTS -s 203.0.113.9/32 -j DROP"],
        "the chain should hold the new rules and only those:\n{chain}"
    );
    for hook in ["INPUT", "FORWARD"] {
        let jumps = server.sh(&format!("iptables -S {hook}"));
        assert_eq!(
            jumps.matches("-j STOP-BOTS").count(),
            1,
            "{hook} should jump to STOP-BOTS exactly once:\n{jumps}"
        );
    }
}

// ---- the stranger test: the README's quick start, on a fresh host ----

/// The two distributions the README names, each as a fresh server with
/// NGINX and nftables from its own archive and nothing of ours on it. See
/// `Dockerfile.stranger` for what "fresh" includes.
#[derive(Clone, Copy)]
enum Distro {
    Debian12,
    Ubuntu2404,
}

impl Distro {
    fn slug(self) -> &'static str {
        match self {
            Distro::Debian12 => "debian-12",
            Distro::Ubuntu2404 => "ubuntu-24.04",
        }
    }

    /// The build arguments: the base image, and what that distribution's
    /// cloud image has that the other's does not.
    fn build_args(self) -> [&'static str; 4] {
        match self {
            Distro::Debian12 => ["--build-arg", "BASE=debian:12", "--build-arg", "EXTRA="],
            Distro::Ubuntu2404 => [
                "--build-arg",
                "BASE=ubuntu:24.04",
                "--build-arg",
                "EXTRA=rsyslog",
            ],
        }
    }

    fn image(self) -> String {
        tagged(&format!("stop-bots-stranger-{}", self.slug()))
    }

    /// Builds this distribution's image once per run, the way
    /// [`build_host_image`] does the other.
    fn build(self) {
        static DEBIAN: std::sync::Once = std::sync::Once::new();
        static UBUNTU: std::sync::Once = std::sync::Once::new();
        let once = match self {
            Distro::Debian12 => &DEBIAN,
            Distro::Ubuntu2404 => &UBUNTU,
        };
        once.call_once(|| {
            with_build_lock(|| build_unless_current(&self.image(), || self.build_now()))
        });
    }

    fn build_now(self) {
        let ctx = stage_binary();
        let out = Command::new(runtime())
            .args(["build", "-q", "-f", &format!("{ctx}/Dockerfile.stranger")])
            .args(self.build_args())
            .args(["-t", &self.image(), &ctx])
            .output()
            .expect("failed to run the image build");
        assert!(
            out.status.success(),
            "{} build ({}) failed:\n{}",
            runtime(),
            self.slug(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The `.deb` to install, from `STOP_BOTS_STRANGER_DEB`, or `None` with the
/// reason printed.
///
/// A package rather than `CARGO_BIN_EXE_stop-bots`, for two reasons. The
/// quick start installs a package, and the package is part of what a
/// stranger meets: its dependencies, its man pages, where it puts the
/// binary. And the binary this test run compiled links the build machine's
/// glibc, which is newer than Debian 12's, so it would not even start
/// there. The released package is static (musl), and only a package built
/// the same way tests what ships. `make stranger-test` builds one and runs
/// these; CI does the same in its `stranger` job.
fn stranger_deb() -> Option<String> {
    match std::env::var("STOP_BOTS_STRANGER_DEB") {
        Ok(path) if !path.is_empty() => {
            assert!(
                std::path::Path::new(&path).is_file(),
                "STOP_BOTS_STRANGER_DEB names {path}, which is not a file"
            );
            Some(path)
        }
        _ => {
            stop_bots::say_err!(
                "skipping: the stranger test installs a .deb. Set STOP_BOTS_STRANGER_DEB to one \
                 built for x86_64-unknown-linux-musl, or run `make stranger-test`."
            );
            None
        }
    }
}

/// Whether to run `batch --apply` exactly as the quick start writes it,
/// which downloads every bot list and crawler range from the internet.
///
/// Off by default, so the suite stays hermetic: a feed that is down, or a
/// runner without outbound access, would otherwise turn it red for a
/// reason that is nobody's commit. Off, `batch` gets `--no-fetch` and
/// blocks from the list compiled into the binary, which is what a host
/// without outbound access runs anyway. CI's weekly run turns it on, which
/// is where "the world changed" failures belong.
///
/// Not a URL override pointing the downloads at a local server: that would
/// be a switch in the shipped binary that redirects where its blocklists
/// come from, and it would test reqwest rather than this project (see
/// "Coverage" in CONTRIBUTING.md).
fn stranger_fetches() -> bool {
    std::env::var_os("STOP_BOTS_STRANGER_FETCH").is_some_and(|value| !value.is_empty())
}

/// A user agent from the compiled-in scanner list, which the default
/// policy blocks. From the built-in list rather than a downloaded one so
/// that the hermetic run can block it.
const SCANNER_UA: &str = "Mozilla/5.0 (compatible; ModatScanner/1.0)";
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

impl Client {
    /// The status code for `url`, `000` when nothing answered: the
    /// stranger's host serves the distribution's default site, on port 80.
    fn status_of(&self, url: &str, extra: &str) -> String {
        let (_, stdout, _) = exec_in(
            &self.name,
            &format!("curl -s -o /dev/null -w '%{{http_code}}' {extra} {url}"),
        );
        stdout.trim().to_string()
    }

    /// [`Self::status_of`], polled until it is `want` or five seconds have
    /// passed: a reload is asynchronous, and an old worker can still
    /// answer the first request after one.
    fn eventually(&self, url: &str, extra: &str, want: &str) -> String {
        let mut got = String::new();
        for _ in 0..50 {
            got = self.status_of(url, extra);
            if got == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        got
    }
}

impl Host {
    /// Runs one quick-start command as the README gives it, and returns
    /// what it printed. A stranger has no reason to expect any of them to
    /// fail, so a failure is the finding, with everything it said.
    fn quick_start(&self, command: &str) -> String {
        let (ok, stdout, stderr) = self.run(command);
        let said = format!("{stdout}{stderr}");
        assert!(ok, "`{command}` failed on a fresh host:\n{said}");
        said
    }

    /// What `uninstall` promises to put back, read the same way each
    /// time: every file under /etc/nginx by hash, the nft tables, the
    /// units, and /etc/stop-bots.
    fn footprint(&self) -> String {
        self.sh("set -e\n\
             find /etc/nginx -type f | sort | xargs sha256sum\n\
             echo '-- nft'; nft list tables\n\
             echo '-- units'; find /etc/systemd/system -name 'stop-bots*' | sort\n\
             systemctl list-unit-files 'stop-bots*' --no-legend | sort\n\
             echo '-- /etc/stop-bots'; ls -A /etc/stop-bots 2>/dev/null || true")
    }
}

/// **A stranger, following only the README's quick start, from install to
/// blocking to uninstall, on a fresh `distro`.**
///
/// Every command is the quick start's, run as it is written, in its order:
///
/// ```text
/// apt install ./stop-bots_*.deb
/// stop-bots status
/// stop-bots batch --dry-run --diff
/// stop-bots batch --apply
/// stop-bots install firewall
/// stop-bots status
/// stop-bots uninstall all --dry-run
/// stop-bots uninstall all
/// ```
///
/// with one deviation, `--no-fetch` on `batch --apply` unless
/// [`stranger_fetches`], and no other setup: no seeded bot, no site of
/// ours, no `--root`, `--db`, `--ssh-log` or `--force`. Whatever the
/// defaults find on a fresh host is what the test gets. As root, as `sudo`
/// would run it; the container's shell already is.
///
/// "Blocking" is checked from a second container, from its own address:
/// NGINX turns away a scanner's user agent and serves a browser, and a
/// client that asks for `/.env` is blocked by the probe-path detector on
/// the next pass and then dropped by nftables. After `install firewall`
/// the table is deleted and the unit restarted, the way a boot loads it,
/// and only what was applied comes back. After `uninstall all`, the host
/// is as it was before the package was installed.
fn a_stranger_follows_the_quick_start(distro: Distro) {
    if !enabled() {
        return;
    }
    let Some(deb) = stranger_deb() else {
        return;
    };
    distro.build();
    let tag = format!("stop-bots-stranger-{}", distro.slug());
    let net = Network::create(&format!("{tag}-net"));
    let host = Host::boot_image(&tag, &distro.image(), Some(&net));
    let client = Client::start(&format!("{tag}-client"), &net);
    let site = format!("http://{}/", host.name);
    let scanner = format!("-A '{SCANNER_UA}'");
    let fetch = if stranger_fetches() {
        ""
    } else {
        " --no-fetch"
    };

    host.sh("systemctl start nginx.service");
    let pristine = host.footprint();
    assert_eq!(
        client.eventually(&site, &scanner, "200"),
        "200",
        "a fresh NGINX should serve everyone"
    );

    // Installing it: the package, with no network, so a dependency it
    // forgets to declare fails here rather than being fetched.
    host.put(&deb, "/root/stop-bots.deb");
    host.quick_start("DEBIAN_FRONTEND=noninteractive apt-get install -y /root/stop-bots.deb");
    //
    // The documentation is checked in dpkg's own list of what it
    // installed, not on disk: Ubuntu's container and minimal cloud images
    // tell dpkg to skip /usr/share/man, for every package alike.
    assert_eq!(host.sh("command -v stop-bots").trim(), "/usr/bin/stop-bots");
    let files = host.sh("dpkg -L stop-bots");
    for path in [
        "/usr/share/man/man1/stop-bots-batch.1.gz",
        "/usr/share/bash-completion/completions/stop-bots",
    ] {
        assert!(
            files.lines().any(|line| line == path),
            "the package does not install {path}:\n{files}"
        );
    }

    // Before anything: a first look, and the plan.
    host.quick_start("stop-bots status");
    let default_site = "sha256sum /etc/nginx/sites-available/default";
    let untouched = host.sh(default_site);
    let plan = host.quick_start("stop-bots batch --dry-run --diff");
    for (what, needle) in [
        (
            "the SSH log the lockout check reads",
            "lockout check passed",
        ),
        ("the block it would add", "+    # BEGIN stop-bots"),
        ("a scanner the binary knows", "ModatScanner"),
    ] {
        assert!(
            plan.contains(needle),
            "the dry run does not show {what}:\n{plan}"
        );
    }
    assert_eq!(
        host.sh(default_site),
        untouched,
        "the dry run changed the site"
    );

    // Blocking.
    let applied = host.quick_start(&format!("stop-bots batch --apply{fetch}"));
    assert_eq!(
        client.eventually(&site, &scanner, "403"),
        "403",
        "a scanner's user agent is still served. batch said:\n{applied}"
    );
    assert_eq!(
        client.status_of(&site, &format!("-A '{BROWSER_UA}'")),
        "200",
        "a browser is no longer served"
    );

    client.status_of(&format!("{site}.env"), "");
    let pass = host.quick_start(&format!("stop-bots batch --apply{fetch}"));
    let prober = client.address();
    let in_kernel = |address: &str| {
        host.run(&format!(
            "nft get element inet stop_bots block_v4 '{{ {address} }}'"
        ))
        .0
    };
    assert!(
        in_kernel(&prober),
        "the client that asked for /.env is not in the kernel's set. batch said:\n{pass}\n{}",
        host.ruleset()
    );
    assert_eq!(
        client.status_of(&site, "--max-time 5"),
        "000",
        "the prober is blocked and still reaches NGINX"
    );

    // Persistence: the unit loads what was applied, and nothing else.
    let installed = host.quick_start("stop-bots install firewall");
    assert!(
        installed.contains("holds the rules the last apply loaded"),
        "install firewall tells a host that has applied to go and apply:\n{installed}"
    );
    let status = host.quick_start("stop-bots status");
    for level in ["[WARN]", "[CRITICAL]"] {
        assert!(
            !status.contains(level),
            "the quick start ends with a {level}:\n{status}"
        );
    }
    let unapplied = "203.0.113.99";
    host.sh(&format!(
        "stop-bots add-firewall-rule --address {unapplied}"
    ));
    host.sh("stop-bots render-firewall");
    host.sh("nft delete table inet stop_bots");
    host.sh("systemctl restart stop-bots-firewall.service");
    assert!(
        in_kernel(&prober),
        "the applied rules did not come back with the unit:\n{}",
        host.journal("stop-bots-firewall.service")
    );
    assert!(
        !in_kernel(unapplied),
        "the unit loaded a rule that was rendered and never applied"
    );

    // And back out.
    let before = host.footprint();
    let dry = host.quick_start("stop-bots uninstall all --dry-run");
    assert_eq!(
        host.footprint(),
        before,
        "the dry run changed something:\n{dry}"
    );
    let removed = host.quick_start("stop-bots uninstall all");
    // Without `--purge`, the README says, the database and the host
    // settings file stay: what the operator set is not thrown away by a
    // plain uninstall. Everything else is as it was.
    assert_eq!(
        host.footprint(),
        format!("{pristine}host.conf\n"),
        "the host is not as it was before stop-bots, bar host.conf. uninstall said:\n{removed}"
    );
    assert!(
        !removed.contains("did not write"),
        "uninstall called a file of its own somebody else's:\n{removed}"
    );
    let purged = host.quick_start("stop-bots uninstall all --purge");
    assert_eq!(
        host.footprint(),
        pristine,
        "--purge left something behind. It said:\n{purged}"
    );
    let (valid, out, err) = host.run("nginx -t");
    assert!(
        valid,
        "NGINX does not load what uninstall left:\n{out}{err}"
    );
    assert_eq!(
        client.eventually(&site, &scanner, "200"),
        "200",
        "the scanner, and the blocked prober, are still turned away"
    );
}

#[test]
fn a_stranger_on_debian_12_goes_from_install_to_blocking_to_uninstall() {
    a_stranger_follows_the_quick_start(Distro::Debian12);
}

#[test]
fn a_stranger_on_ubuntu_24_04_goes_from_install_to_blocking_to_uninstall() {
    a_stranger_follows_the_quick_start(Distro::Ubuntu2404);
}
