//! The browser UI behind nginx, as `docs/deploy.md` describes it: TLS on a name, the plain
//! port redirecting to it, and everything else proxied to codereview on loopback.
//!
//! This is the only test that needs something other than a Rust toolchain, so it is ignored
//! by default and run by `make integration-test`, which needs docker, curl and openssl. It
//! runs nginx in a container sharing the host's network, so that `proxy_pass` to
//! `127.0.0.1` reaches the server this test starts.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Fixed, and well above the privileged range: the container shares the host's network, so
/// these are host ports.
const HTTP_PORT: u16 = 18080;
const HTTPS_PORT: u16 = 18443;
const UPSTREAM_PORT: u16 = 18765;
const CONTAINER: &str = "codereview-nginx-test";
const IMAGE: &str = "nginx:1.27-alpine";
const HOST: &str = "review.test";
const PASSWORD: &str = "correct horse battery staple";

/// Everything the test starts, stopped again however the test ends.
struct Harness {
    server: Child,
    _work: tempfile::TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        remove_container();
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

fn remove_container() {
    let _ = Command::new("docker")
        .args(["rm", "-f", CONTAINER])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether `program` is there to be run. `probe` is how it is asked, which is not the same
/// argument everywhere.
fn have(program: &str, probe: &str) -> bool {
    Command::new(program)
        .arg(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {program}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn wait_for_port(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what} never started listening on {port}");
}

/// A request through the proxy. Returns the status, the headers in lower case, and the body.
fn curl(ca: &Path, args: &[&str]) -> (u16, String, String) {
    let resolve_https = format!("{HOST}:{HTTPS_PORT}:127.0.0.1");
    let resolve_http = format!("{HOST}:{HTTP_PORT}:127.0.0.1");
    let mut all: Vec<String> = vec![
        "-sS".into(),
        "-i".into(),
        "--max-time".into(),
        "30".into(),
        "--cacert".into(),
        ca.display().to_string(),
        "--resolve".into(),
        resolve_https,
        "--resolve".into(),
        resolve_http,
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    let out = Command::new("curl").args(&all).output().expect("curl");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !text.is_empty(),
        "curl {args:?} said nothing: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status in {head:?}"));
    (status, head.to_lowercase(), body.to_string())
}

fn url(path: &str) -> String {
    format!("https://{HOST}:{HTTPS_PORT}{path}")
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
}

/// A repository with something in it to serve.
fn scratch_repo(root: &Path) {
    run("git", &["init", "-q", root.to_str().unwrap()]);
    let git = |args: &[&str]| {
        let mut all = vec!["-C", root.to_str().unwrap()];
        all.extend_from_slice(args);
        run("git", &all);
    };
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("REVIEW.md"), "# Pending\n\n# Completed\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "first"]);
}

#[test]
#[ignore = "needs docker, curl and openssl; run with `make integration-test`"]
fn the_browser_ui_works_behind_nginx() {
    for (program, probe) in [
        ("docker", "--version"),
        ("curl", "--version"),
        ("git", "--version"),
        ("openssl", "version"),
    ] {
        assert!(have(program, probe), "{program} is needed for this test");
    }
    for port in [HTTP_PORT, HTTPS_PORT, UPSTREAM_PORT] {
        assert!(port_is_free(port), "port {port} is already in use");
    }
    // A container left behind by a run that crashed would hold the ports.
    remove_container();

    let work = tempfile::tempdir().unwrap();
    let repo = work.path().join("repo");
    let certs = work.path().join("certs");
    let conf = work.path().join("conf");
    std::fs::create_dir_all(&certs).unwrap();
    std::fs::create_dir_all(&conf).unwrap();
    scratch_repo(&repo);

    // The site under test is the one in docs/deploy.md; the fixture is what that document
    // says, with this test's ports. If the document loses a directive the test exercises,
    // the two have drifted and one of them is wrong.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/nginx/site.conf");
    let site = std::fs::read_to_string(&fixture).unwrap();
    let doc =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/deploy.md"))
            .unwrap();
    for directive in [
        "log_format noquery",
        "limit_req_zone",
        "limit_req_status 429",
        "server_tokens off",
        "ssl_protocols TLSv1.2 TLSv1.3",
        "add_header Strict-Transport-Security",
        "proxy_set_header X-Forwarded-Proto",
        "client_max_body_size 1m",
        "proxy_pass http://127.0.0.1:",
    ] {
        assert!(site.contains(directive), "the fixture lost {directive:?}");
        assert!(
            doc.contains(directive),
            "docs/deploy.md no longer has {directive:?}, which this test exercises"
        );
    }
    std::fs::copy(&fixture, conf.join("site.conf")).unwrap();

    run(
        "openssl",
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            certs.join("key.pem").to_str().unwrap(),
            "-out",
            certs.join("cert.pem").to_str().unwrap(),
            "-subj",
            &format!("/CN={HOST}"),
            "-addext",
            &format!("subjectAltName=DNS:{HOST}"),
        ],
    );

    // The server, signed in to with a password, with no agent: what the document recommends
    // for anything reachable from another machine.
    let binary = env!("CARGO_BIN_EXE_codereview");
    let mut hashing = Command::new(binary)
        .arg("hash-password")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("hash-password");
    hashing
        .stdin
        .take()
        .unwrap()
        .write_all(PASSWORD.as_bytes())
        .unwrap();
    let hashed = hashing.wait_with_output().unwrap();
    assert!(hashed.status.success(), "hash-password failed");
    let hash = String::from_utf8_lossy(&hashed.stdout).trim().to_string();
    assert!(hash.starts_with("$argon2id$"), "{hash}");

    let server = Command::new(binary)
        .args([
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            &UPSTREAM_PORT.to_string(),
            "--no-open",
            "--no-agent",
            "--public-url",
            &format!("https://{HOST}:{HTTPS_PORT}"),
            repo.to_str().unwrap(),
        ])
        .env("CODEREVIEW_PASSWORD_HASH", &hash)
        .env("CODEREVIEW_CONFIG", work.path().join("config.toml"))
        .env_remove("CODEREVIEW_TOKEN")
        .stdout(Stdio::null())
        .spawn()
        .expect("codereview web");
    let harness = Harness {
        server,
        _work: work,
    };
    wait_for_port(UPSTREAM_PORT, "codereview");

    // nginx shares the host's network, so proxy_pass to 127.0.0.1 reaches the server above
    // and the listen ports are the host's.
    run(
        "docker",
        &[
            "run",
            "-d",
            "--name",
            CONTAINER,
            "--network",
            "host",
            "-v",
            &format!("{}:/etc/nginx/conf.d:ro", conf.display()),
            "-v",
            &format!("{}:/etc/nginx/certs:ro", certs.display()),
            IMAGE,
        ],
    );
    wait_for_port(HTTPS_PORT, "nginx");
    let ca = certs.join("cert.pem");

    // The plain port sends the browser to the encrypted one.
    let (status, head, _) = curl(&ca, &[&format!("http://{HOST}:{HTTP_PORT}/api/state")]);
    assert_eq!(status, 301, "{head}");
    assert_eq!(
        header(&head, "location"),
        Some(format!("https://{HOST}:{HTTPS_PORT}/api/state").as_str())
    );

    // The page is the sign-in form, and the headers survive the proxy.
    let (status, head, body) = curl(&ca, &[&url("/")]);
    assert_eq!(status, 200, "{head}");
    assert!(body.contains("name=\"password\""), "not the sign-in form");
    assert!(!body.contains("const TOKEN"), "a secret before signing in");
    assert_eq!(header(&head, "cache-control"), Some("no-store"));
    assert_eq!(header(&head, "referrer-policy"), Some("no-referrer"));
    assert!(head.contains("content-security-policy: default-src 'none'"));
    assert!(head.contains("strict-transport-security: max-age=31536000"));
    assert_eq!(
        header(&head, "server"),
        Some("nginx"),
        "server_tokens off should hide the version"
    );

    // A wrong password opens nothing.
    let (status, head, _) = curl(&ca, &["-X", "POST", "-d", "password=wrong", &url("/login")]);
    assert_eq!(status, 401, "{head}");
    assert!(header(&head, "set-cookie").is_none(), "{head}");

    // The right one does, and the cookie is marked Secure because the site is HTTPS.
    let form = format!("password={}", PASSWORD.replace(' ', "+"));
    let (status, head, _) = curl(&ca, &["-X", "POST", "-d", &form, &url("/login")]);
    assert_eq!(status, 303, "{head}");
    let cookie = header(&head, "set-cookie")
        .expect("a session cookie")
        .to_string();
    assert!(cookie.contains("secure"), "{cookie}");
    assert!(cookie.contains("httponly"), "{cookie}");
    let cookie = cookie.split(';').next().unwrap().to_string();

    // The page then carries the API token, and the API answers for it.
    let (status, _, page) = curl(&ca, &["-H", &format!("Cookie: {cookie}"), &url("/")]);
    assert_eq!(status, 200);
    let start = page.find("const TOKEN = ").expect("a token in the page") + 14;
    let rest = &page[start..];
    let token: String =
        serde_json::from_str(rest[..rest.find(';').unwrap()].trim()).expect("a JSON string");
    let bearer = format!("Authorization: Bearer {token}");

    let (status, _, body) = curl(&ca, &["-H", &bearer, &url("/api/state")]);
    assert_eq!(status, 200, "{body}");
    let state: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(state["agent"], false, "started with --no-agent");
    assert!(
        state["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "a.rs")
    );

    // Without the token, nothing; and the agent is not there to be reached.
    assert_eq!(curl(&ca, &[&url("/api/state")]).0, 403);
    assert_eq!(
        curl(&ca, &["-H", &bearer, &url("/api/agent/events")]).0,
        404
    );
    // The cookie alone does not drive the API, whatever the proxy forwards.
    assert_eq!(
        curl(
            &ca,
            &["-H", &format!("Cookie: {cookie}"), &url("/api/state")]
        )
        .0,
        403
    );

    // Writing a comment through the proxy reaches the file on disk.
    let comment = r#"{"path":"a.rs","line":1,"text":"through the proxy"}"#;
    let (status, _, body) = curl(
        &ca,
        &[
            "-H",
            &bearer,
            "-H",
            "Content-Type: application/json",
            "-d",
            comment,
            &url("/api/comments"),
        ],
    );
    assert_eq!(status, 200, "{body}");
    let review = std::fs::read_to_string(repo.join("REVIEW.md")).unwrap();
    assert!(review.contains("through the proxy"), "{review}");

    // The access log keeps neither the password nor the token.
    let log = run(
        "docker",
        &["exec", CONTAINER, "cat", "/var/log/nginx/review.access.log"],
    );
    assert!(log.contains("POST /login"), "the log is empty: {log}");
    assert!(!log.contains("password="), "the log kept a password: {log}");
    assert!(!log.contains(&token), "the log kept the token");

    // Guessing is stopped by the proxy before it reaches the server.
    let mut stopped_by_nginx = false;
    for _ in 0..12 {
        let (status, _, body) = curl(&ca, &["-X", "POST", "-d", "password=wrong", &url("/login")]);
        if status == 429 && body.contains("nginx") {
            stopped_by_nginx = true;
            break;
        }
    }
    assert!(
        stopped_by_nginx,
        "nginx never rate-limited the sign-in form"
    );

    drop(harness);
}
