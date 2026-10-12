//! The web UI's guards, driven through the assembled router.
//!
//! `tower::ServiceExt::oneshot` runs the real middleware stack — host
//! check, security headers, session lookup, CSRF — without binding a port.
//! That is deliberate: the properties worth asserting here are all
//! decisions those layers make, and a real listener would only add a port
//! to collide on and a server to shut down.
//!
//! Every test drives HTTP the way a browser would, because the thing under
//! test *is* the HTTP behaviour. There are no mocks: the database is a
//! real SQLite one on a tempdir, and the password is really hashed with
//! Argon2 and really verified.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use stop_bots::db::Db;
use stop_bots::web::server;
use stop_bots::web::state::AppState;
use tower::ServiceExt;

mod common;

/// A router over a fresh database, plus the password that opens it.
fn app() -> (Router, String, tempfile::TempDir) {
    let (router, password, tmp, _) = app_with_db();
    (router, password, tmp)
}

/// The same, keeping the database path so a test can open a second
/// connection and check what a handler actually wrote. SQLite is perfectly
/// happy with two connections to one file, and reading the result back
/// through the real database is what makes these tests assertions about
/// behaviour rather than about status codes.
fn app_with_db() -> (Router, String, tempfile::TempDir, std::path::PathBuf) {
    app_under("")
}

/// The same, served under a path prefix.
fn app_under(base: &str) -> (Router, String, tempfile::TempDir, std::path::PathBuf) {
    app_configured(base, |_| {})
}

/// The same, with `configure` given the state before the router is built.
fn app_configured(
    base: &str,
    configure: impl FnOnce(&mut AppState),
) -> (Router, String, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db = Db::open(&db_path).unwrap();
    let password = stop_bots::web::auth::generate_password().unwrap();
    stop_bots::web::auth::set_password(&db, &password).unwrap();
    stop_bots::botlist::register_all_sources(&db).unwrap();
    stop_bots::ipranges::reputation::register_all_reputation_sources(&db).unwrap();
    drop(db);

    let nginx_root = tmp.path().join("nginx");
    std::fs::create_dir_all(nginx_root.join("sites-enabled")).unwrap();
    // A `conf.d` of its own, so the generated http-context files resolve
    // here rather than to the host's `/etc/nginx/conf.d` (see
    // `nginx::conf_d_dir`), which an apply would otherwise read and clean.
    std::fs::create_dir_all(nginx_root.join("conf.d")).unwrap();

    // An SSH log that exists and names nobody. Without one the lockout
    // guard goes looking for the host's own, which a test must never read,
    // and on a host where it cannot, every apply is (rightly) refused.
    let ssh_log = tmp.path().join("auth.log");
    std::fs::write(&ssh_log, "").unwrap();

    // `apply_for_real: false` throughout — a test must never reload the
    // developer's NGINX or run a firewall script at them.
    let mut state = AppState::with_base(
        Db::open(&db_path).unwrap(),
        nginx_root,
        Some(ssh_log),
        false,
        stop_bots::web::BasePath::parse(base).unwrap(),
    );
    // An explicit override, not left at `None`: without one the path
    // follows the backend and lands under `/etc/stop-bots/`, which a test
    // must never write to. It doubles as the assertion that an explicit
    // path wins over the backend's own default — the iptables tests below
    // still find their script at this `.nft` name.
    state.firewall_out = Some(tmp.path().join("firewall.nft"));
    // The host settings file is `/etc/stop-bots/host.conf` otherwise; a
    // test reads and writes one of its own.
    state.host_conf = tmp.path().join("host.conf");
    configure(&mut state);
    (server::router(state), password, tmp, db_path)
}

/// Posts a form with the session's CSRF token, and returns the flash
/// message the redirect carries.
///
/// The message is the handler's own account of what it did, so asserting
/// on it is asserting on the outcome the operator is shown — not on a
/// status code that a refusal and a success would share.
async fn act(
    app: &Router,
    cookie: &str,
    csrf: &str,
    path: &str,
    body: &str,
) -> (StatusCode, String) {
    let full = if body.is_empty() {
        format!("csrf={csrf}")
    } else {
        format!("csrf={csrf}&{body}")
    };
    let response = app
        .clone()
        .oneshot(with_cookie(post(path, &full), cookie))
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let flash = flash_shown(app, cookie, &location).await;
    (status, flash)
}

/// The message a redirect leaves for the page it lands on, read the way
/// the operator reads it: by following the redirect. The URL itself
/// carries only an opaque id, never the text.
async fn flash_shown(app: &Router, cookie: &str, location: &str) -> String {
    if !location.contains("flash=") {
        return String::new();
    }
    let page = app
        .clone()
        .oneshot(with_cookie(get(location), cookie))
        .await
        .unwrap();
    let html = body_string(page).await;
    html.split(r#"<div class="flash "#)
        .nth(1)
        .and_then(|rest| rest.split_once('>'))
        .and_then(|(_, rest)| rest.split("</div>").next())
        .map(unescape_html)
        .unwrap_or_default()
}

/// The flash of the redirect `response` is.
async fn flash_of(app: &Router, cookie: &str, response: &Response) -> String {
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    flash_shown(app, cookie, &location).await
}

fn unescape_html(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::HOST, "localhost")
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn with_cookie(mut request: Request<Body>, cookie: &str) -> Request<Body> {
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    request
}

async fn body_string(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn session_cookie_from(response: &Response) -> String {
    let set = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a successful login sets a cookie")
        .to_str()
        .unwrap();
    set.split(';').next().unwrap().to_string()
}

/// Logs in and returns the session cookie plus that session's CSRF token,
/// read back out of a rendered form.
async fn login(app: &Router, password: &str) -> (String, String) {
    let response = app
        .clone()
        .oneshot(post("/login", &format!("password={password}")))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a correct password should redirect into the app"
    );
    let cookie = session_cookie_from(&response);

    // From the meta tag rather than a form field: every authenticated
    // page carries it, whereas a form only appears once there is a row to
    // act on, and these tests start from an empty database.
    let page = app
        .clone()
        .oneshot(with_cookie(get("/firewall"), &cookie))
        .await
        .unwrap();
    let html = body_string(page).await;
    let csrf = html
        .split(r#"<meta name="csrf-token" content=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("every authenticated page carries the token")
        .to_string();

    (cookie, csrf)
}

// ---- reaching it at all ----

#[tokio::test]
async fn an_unauthenticated_request_is_sent_to_the_login_page() {
    let (app, _password, _tmp) = app();

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{path} must not render to a stranger"
        );
        assert_eq!(
            response.headers()[header::LOCATION],
            "/login",
            "{path} should redirect to the login page"
        );
    }
}

#[tokio::test]
async fn the_login_page_and_its_assets_are_reachable_without_a_session() {
    let (app, _password, _tmp) = app();

    for path in ["/login", "/assets/style.css"] {
        let response = app.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} has to load before anyone can log in"
        );
    }
}

#[tokio::test]
async fn a_wrong_password_is_refused_and_sets_no_cookie() {
    let (app, _password, _tmp) = app();

    let response = app
        .clone()
        .oneshot(post("/login", "password=not-the-password"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "a failed login must not hand out a session"
    );
}

#[tokio::test]
async fn a_correct_password_opens_a_session_that_reaches_every_screen() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app
            .clone()
            .oneshot(with_cookie(get(path), &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} should render");
    }
}

#[tokio::test]
async fn a_forged_session_cookie_does_not_work() {
    let (app, _password, _tmp) = app();

    let response = app
        .clone()
        .oneshot(with_cookie(get("/"), "stop_bots_session=made-up"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/login");
}

#[tokio::test]
async fn logging_out_ends_the_session_server_side() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    app.clone()
        .oneshot(with_cookie(post("/logout", ""), &cookie))
        .await
        .unwrap();

    // The same cookie must now be worthless — clearing it in the browser
    // is not what makes logout safe.
    let response = app
        .clone()
        .oneshot(with_cookie(get("/"), &cookie))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a logged-out cookie must not still open the app"
    );
}

/// `stop-bots web --set-password` runs as a separate process and can only
/// change the stored hash. That has to be enough to end a session this
/// server is holding in memory — rotating the password is what an
/// operator does when they think a session has leaked.
#[tokio::test]
async fn a_new_password_ends_the_sessions_opened_under_the_old_one() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let rotated = stop_bots::web::auth::generate_password().unwrap();
    stop_bots::web::auth::set_password(&Db::open(&db_path).unwrap(), &rotated).unwrap();

    let response = app.oneshot(with_cookie(get("/"), &cookie)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/login",
        "a session from before the rotation must not still open the app"
    );
}

/// Under a prefix `/login` is the host site's own page, or a 404 — not
/// this console's.
#[tokio::test]
async fn logging_out_under_a_prefix_lands_on_the_prefixed_login() {
    let (app, password, _tmp, _db) = app_under(PREFIX);
    let response = app
        .clone()
        .oneshot(post(
            &format!("{PREFIX}/login"),
            &format!("password={password}"),
        ))
        .await
        .unwrap();
    let cookie = session_cookie_from(&response);

    let response = app
        .oneshot(with_cookie(post(&format!("{PREFIX}/logout"), ""), &cookie))
        .await
        .unwrap();
    assert_eq!(response.headers()[header::LOCATION], "/stop-bots/login");
}

// ---- DNS rebinding ----

#[tokio::test]
async fn a_request_carrying_an_unlisted_host_is_refused() {
    let (app, _password, _tmp) = app();

    let request = Request::builder()
        .uri("/login")
        .header(header::HOST, "evil.example")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::MISDIRECTED_REQUEST,
        "this is the DNS-rebinding guard; a rebound request arrives under the attacker's name"
    );
}

/// The refusal is the one response an attacker's page can provoke, so it
/// gets the same headers as everything else rather than none.
#[tokio::test]
async fn the_host_refusal_carries_the_security_headers_too() {
    let (app, _password, _tmp) = app();

    let request = Request::builder()
        .uri("/login")
        .header(header::HOST, "evil.example")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    let headers = response.headers();
    for name in [
        "content-security-policy",
        "x-content-type-options",
        "x-frame-options",
    ] {
        assert!(headers.contains_key(name), "{name} missing from the 421");
    }
}

#[tokio::test]
async fn loopback_host_names_are_accepted_without_configuration() {
    let (app, _password, _tmp) = app();

    for host in [
        "localhost",
        "localhost:8787",
        "127.0.0.1:8787",
        "[::1]:8787",
    ] {
        let request = Request::builder()
            .uri("/login")
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "Host: {host}");
    }
}

#[tokio::test]
async fn a_configured_host_becomes_acceptable() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("db.sqlite3")).unwrap();
    db.set_text_setting(stop_bots::web::ALLOWED_HOSTS_KEY, "admin.example.com")
        .unwrap();
    let mut state = AppState::new(db, tmp.path().join("nginx"), None, false);
    state.host_conf = tmp.path().join("host.conf");
    let app = server::router(state);

    let request = Request::builder()
        .uri("/login")
        .header(header::HOST, "admin.example.com")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
}

// ---- CSRF ----

#[tokio::test]
async fn a_post_without_a_csrf_token_is_refused() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(
            post("/firewall/block-address", "address=192.0.2.1"),
            &cookie,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a session cookie alone must not be enough to change anything"
    );
}

#[tokio::test]
async fn a_post_with_another_sessions_csrf_token_is_refused() {
    let (app, password, _tmp) = app();
    let (first_cookie, _first_csrf) = login(&app, &password).await;
    let (_second_cookie, second_csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(
            post(
                "/firewall/block-address",
                &format!("csrf={second_csrf}&address=192.0.2.1"),
            ),
            &first_cookie,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "tokens are per-session, so one session's token must not authorise another's request"
    );
}

#[tokio::test]
async fn a_post_with_the_right_csrf_token_is_accepted() {
    let (app, password, _tmp) = app();
    let (cookie, csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(
            post(
                "/firewall/block-address",
                &format!("csrf={csrf}&address=192.0.2.1"),
            ),
            &cookie,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "an action redirects back to the page it came from"
    );
}

// ---- headers ----

#[tokio::test]
async fn every_response_carries_the_headers_that_lock_the_page_down() {
    let (app, _password, _tmp) = app();

    // Including a 404: an error page is as framable as any other if the
    // headers only go on the successful paths.
    for path in ["/login", "/no-such-page"] {
        let response = app.clone().oneshot(get(path)).await.unwrap();
        let headers = response.headers();

        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(
            csp.contains("frame-ancestors 'none'"),
            "a console that rewrites firewall rules must not be framable; {path} CSP was: {csp}"
        );
        assert!(csp.contains("default-src 'none'"), "{path} CSP was: {csp}");
        assert!(
            !csp.contains("unsafe-inline"),
            "{path} CSP must not allow inline script: {csp}"
        );
        assert_eq!(headers["x-frame-options"], "DENY", "{path}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(headers["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(headers["cache-control"], "no-store", "{path}");
    }
}

#[tokio::test]
async fn the_csp_names_the_hash_of_the_one_inline_script() {
    let (app, _password, _tmp) = app();
    let response = app.oneshot(get("/login")).await.unwrap();
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();

    let expected = stop_bots::web::layout::theme_script_hash();
    assert!(
        csp.contains(&expected),
        "the theme toggle is inline and must be allowed by hash, not by 'unsafe-inline'.\nCSP: {csp}\nexpected to contain: {expected}"
    );
}

/// htmx was loaded on every page and used by none, and `<body hx-headers>`
/// handed the CSRF token to any request it made: an injected `hx-post`
/// would have been an authorised action. No page loads a script file now,
/// nothing attaches the token outside a form, and the CSP allows the one
/// inline script and no file at all, this origin's included.
#[tokio::test]
async fn no_page_loads_a_script_file_or_hands_out_the_token_outside_a_form() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    for path in [
        "/login",
        "/",
        "/bots",
        "/nginx",
        "/firewall",
        "/blocks",
        "/help",
    ] {
        let response = app
            .clone()
            .oneshot(with_cookie(get(path), &cookie))
            .await
            .unwrap();
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_string();
        let script_src = csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("script-src"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            script_src,
            format!("script-src {}", stop_bots::web::layout::theme_script_hash()),
            "{path}: CSP was {csp}"
        );

        let html = body_string(response).await;
        for needle in ["<script src", "hx-", "htmx", "x-csrf-token"] {
            assert!(!html.contains(needle), "{path} carries {needle}:\n{html}");
        }
    }
    let asset = app
        .oneshot(get("/assets/htmx.min.js"))
        .await
        .unwrap()
        .status();
    assert_ne!(asset, StatusCode::OK, "the htmx asset is still served");
}

#[tokio::test]
async fn the_session_cookie_is_not_readable_from_script() {
    let (app, password, _tmp) = app();

    let response = app
        .oneshot(post("/login", &format!("password={password}")))
        .await
        .unwrap();
    let set = response.headers()[header::SET_COOKIE].to_str().unwrap();

    assert!(set.contains("HttpOnly"), "was: {set}");
    assert!(
        set.contains("SameSite=Strict"),
        "SameSite is the second lock on CSRF; was: {set}"
    );
}

/// The CSP allows inline script only by hash, and a hash does not cover an
/// inline event handler — that needs `unsafe-hashes`, which is the hole
/// hashing was meant to avoid. So a page with an `onclick` is a page with
/// a control that silently does nothing in any browser that enforces CSP.
#[tokio::test]
async fn no_page_uses_an_inline_event_handler() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app
            .clone()
            .oneshot(with_cookie(get(path), &cookie))
            .await
            .unwrap();
        let html = body_string(response).await;

        for handler in ["onclick=", "onchange=", "onsubmit=", "onload=", "onerror="] {
            assert!(
                !html.contains(handler),
                "{path} carries {handler}, which this server's own CSP blocks"
            );
        }
    }
}

/// Every screen lays its panels out in the responsive two-column grid, so
/// that a wide window puts tables side by side instead of stacking them a
/// screen apart. The breakpoint itself is CSS; what a test can hold is
/// that no screen was left out of the container that applies it, and that
/// no panel escaped it — one stray panel outside the grid becomes a
/// full-width band across an otherwise two-column page.
#[tokio::test]
async fn every_screen_lays_its_panels_out_in_the_column_grid() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app
            .clone()
            .oneshot(with_cookie(get(path), &cookie))
            .await
            .unwrap();
        let html = body_string(response).await;

        assert_eq!(
            html.matches(r#"<div class="cols">"#).count(),
            1,
            "{path} has no single column grid"
        );
        let (before, inside) = html.split_once(r#"<div class="cols">"#).unwrap();
        assert!(
            !before.contains(r#"<section class="panel">"#),
            "{path} has a panel above its grid"
        );
        assert!(
            inside.contains(r#"<section class="panel">"#),
            "{path} has a grid with no panels in it"
        );
    }
}

/// The login page too — it is the one page an operator sees before
/// anything else works.
#[tokio::test]
async fn the_login_page_uses_no_inline_event_handler() {
    let (app, _password, _tmp) = app();
    let html = body_string(app.oneshot(get("/login")).await.unwrap()).await;
    assert!(!html.contains("onclick="), "was: {html}");
}

// ---- actions: what the handlers actually write ----
//
// Every one of these reads the result back out of the database through a
// second connection, rather than trusting a 303. A refusal and a success
// are both redirects; the difference is what changed and what the operator
// is told.

use stop_bots::db::{BotStatus, Category, GeoMode, Policy};

#[tokio::test]
async fn setting_a_category_default_is_stored_and_reported() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (status, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/category",
        "category=ai&policy=allowed",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(
        flash.contains("AI bots"),
        "the message names the category: {flash}"
    );

    let db = Db::open(&db_path).unwrap();
    assert_eq!(
        db.get_category_default(Category::Ai).unwrap(),
        Policy::Allowed
    );
}

#[tokio::test]
async fn an_unknown_category_changes_nothing() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let before = Db::open(&db_path)
        .unwrap()
        .get_category_default(Category::Ai)
        .unwrap();

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/category",
        "category=wat&policy=allowed",
    )
    .await;
    assert!(flash.contains("not a category"), "was: {flash}");

    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .get_category_default(Category::Ai)
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn the_geo_mode_and_country_selection_round_trip() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    act(&app, &cookie, &csrf, "/geo-mode", "mode=allowlist").await;
    act(&app, &cookie, &csrf, "/geo-add", "country=cn").await;

    let db = Db::open(&db_path).unwrap();
    assert_eq!(db.get_geo_mode().unwrap(), GeoMode::Allowlist);
    assert_eq!(
        db.list_selected_countries().unwrap(),
        ["CN"],
        "a lowercase code is normalised, not rejected"
    );
    drop(db);

    act(&app, &cookie, &csrf, "/geo-remove", "country=CN").await;
    assert!(Db::open(&db_path)
        .unwrap()
        .list_selected_countries()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_country_that_is_not_a_two_letter_code_is_refused() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    for bad in ["england", "c", "1"] {
        let (_, flash) = act(&app, &cookie, &csrf, "/geo-add", &format!("country={bad}")).await;
        assert!(flash.contains("two-letter"), "{bad} gave: {flash}");
    }
    assert!(Db::open(&db_path)
        .unwrap()
        .list_selected_countries()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_detector_can_be_switched_on_and_given_a_ttl() {
    use stop_bots::protection::Detector;

    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let detector = Detector::ALL[0];

    act(
        &app,
        &cookie,
        &csrf,
        "/detector",
        &format!("detector={}&enabled=1", detector.id()),
    )
    .await;
    act(
        &app,
        &cookie,
        &csrf,
        "/detector-ttl",
        &format!("detector={}&days=9", detector.id()),
    )
    .await;

    let db = Db::open(&db_path).unwrap();
    assert!(detector.is_enabled(&db).unwrap());
    assert_eq!(detector.ttl_days(&db).unwrap(), 9);
}

#[tokio::test]
async fn a_zero_day_ttl_is_refused() {
    use stop_bots::protection::Detector;

    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let detector = Detector::ALL[0];
    let before = detector.ttl_days(&Db::open(&db_path).unwrap()).unwrap();

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/detector-ttl",
        &format!("detector={}&days=0", detector.id()),
    )
    .await;
    assert!(flash.contains("at least a day"), "was: {flash}");
    assert_eq!(
        detector.ttl_days(&Db::open(&db_path).unwrap()).unwrap(),
        before
    );
}

/// Days times 86,400 plus now overflowed for a large enough number, and a
/// block born already expired is a detector that silently does nothing.
#[tokio::test]
async fn a_ttl_past_ten_years_is_refused() {
    use stop_bots::protection::Detector;

    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let detector = Detector::ALL[0];
    let before = detector.ttl_days(&Db::open(&db_path).unwrap()).unwrap();

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/detector-ttl",
        &format!("detector={}&days=106751991167301", detector.id()),
    )
    .await;
    assert!(flash.contains("at most"), "was: {flash}");
    assert_eq!(
        detector.ttl_days(&Db::open(&db_path).unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn a_reputation_feed_can_be_switched_on() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let id = Db::open(&db_path)
        .unwrap()
        .list_reputation_sources()
        .unwrap()[0]
        .id
        .clone();
    act(
        &app,
        &cookie,
        &csrf,
        "/feed",
        &format!("feed={id}&enabled=1"),
    )
    .await;

    let enabled = Db::open(&db_path)
        .unwrap()
        .list_reputation_sources()
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap()
        .enabled;
    assert!(enabled);
}

#[tokio::test]
async fn writing_the_firewall_script_produces_a_file_and_records_the_signature() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    Db::open(&db_path)
        .unwrap()
        .block_address_permanently("192.0.2.10", stop_bots::db::RuleSource::Tui, None)
        .unwrap();

    // Beside the configured script, which is the applied one: writing
    // alone must never reach what a boot unit loads.
    let out = tmp.path().join("firewall.next.nft");
    let (_, flash) = act(&app, &cookie, &csrf, "/render-firewall", "backend=nftables").await;

    assert!(flash.contains("Wrote"), "was: {flash}");
    let script = std::fs::read_to_string(&out).expect("the script must exist on disk");
    assert!(script.contains("192.0.2.10"), "script was:\n{script}");
    assert!(
        Db::open(&db_path)
            .unwrap()
            .get_firewall_rendered_signature()
            .unwrap()
            .is_some(),
        "recording the signature is what stops the Dashboard calling it stale straight away"
    );
}

/// The destination used to come from a free-text form field, which made
/// this the one handler that could create a root-owned file anywhere on the
/// host — and the file it creates is an executable script. The console is
/// deliberately the restricted front-end, so it writes where it was
/// configured to and nowhere else.
#[tokio::test]
async fn the_firewall_script_goes_where_the_server_was_configured_to_put_it() {
    let (app, password, tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let elsewhere = tmp.path().join("elsewhere.sh");
    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/render-firewall",
        &format!("out={}&backend=nftables", elsewhere.display()),
    )
    .await;

    assert!(flash.contains("Wrote"), "was: {flash}");
    assert!(
        !elsewhere.exists(),
        "the form steered the write to {}",
        elsewhere.display()
    );
    assert!(
        tmp.path().join("firewall.next.nft").exists(),
        "the configured destination was not written"
    );
}

#[tokio::test]
async fn a_bot_override_is_stored() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let db = Db::open(&db_path).unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "gptbot".into(),
        name: "GPTBot".into(),
        is_ai: true,
        is_search_engine: false,
        is_scanner: false,
        user_agent_pattern: "gptbot".into(),
        source_id: "well-known-bots".into(),
    })
    .unwrap();
    drop(db);

    act(
        &app,
        &cookie,
        &csrf,
        "/bots/status",
        "slug=gptbot&status=blocked",
    )
    .await;

    let status = Db::open(&db_path)
        .unwrap()
        .list_bots()
        .unwrap()
        .into_iter()
        .find(|b| b.slug == "gptbot")
        .unwrap()
        .status;
    assert_eq!(status, BotStatus::Blocked);
}

// ---- sites ----

/// Writes a site config under the router's NGINX root and returns its path.
fn write_site(tmp: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    let path = tmp.path().join("nginx/sites-enabled").join(name);
    std::fs::write(
        &path,
        format!("server {{\n    listen 80;\n    server_name {name};\n}}\n"),
    )
    .unwrap();
    path
}

#[tokio::test]
async fn scanning_finds_the_sites_on_disk() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    write_site(&tmp, "shop.example.com");

    let (_, flash) = act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    assert!(flash.contains("2 site"), "was: {flash}");

    let names: Vec<String> = Db::open(&db_path)
        .unwrap()
        .list_sites()
        .unwrap()
        .into_iter()
        .map(|s| s.server_name)
        .collect();
    assert!(names.contains(&"example.com".to_string()), "got: {names:?}");
}

#[tokio::test]
async fn the_nginx_settings_round_trip() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    act(
        &app,
        &cookie,
        &csrf,
        "/nginx/block-response",
        "response=444",
    )
    .await;
    act(&app, &cookie, &csrf, "/nginx/robots", "enabled=1").await;
    act(
        &app,
        &cookie,
        &csrf,
        "/nginx/rate-limit",
        "enabled=1&rps=7&burst=21",
    )
    .await;

    let db = Db::open(&db_path).unwrap();
    assert_eq!(db.get_block_response().unwrap().stored(), "444");
    assert!(db.get_serve_robots_txt().unwrap());
    assert!(db.get_rate_limit_enabled().unwrap());
    assert_eq!(db.get_rate_limit_rps().unwrap(), 7);
    assert_eq!(db.get_rate_limit_burst().unwrap(), 21);
}

#[tokio::test]
async fn a_rate_limit_of_zero_is_refused() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/nginx/rate-limit",
        "enabled=1&rps=0&burst=5",
    )
    .await;
    assert!(flash.contains("at least 1"), "was: {flash}");
    assert!(
        !Db::open(&db_path)
            .unwrap()
            .get_rate_limit_enabled()
            .unwrap(),
        "a refused save must not switch it on"
    );
}

#[tokio::test]
async fn applying_writes_the_rule_into_the_site_config() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let path = write_site(&tmp, "example.com");

    // Something to block, or an apply writes nothing.
    let db = Db::open(&db_path).unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "gptbot".into(),
        name: "GPTBot".into(),
        is_ai: true,
        is_search_engine: false,
        is_scanner: false,
        user_agent_pattern: "GPTBot".into(),
        source_id: "well-known-bots".into(),
    })
    .unwrap();
    db.set_bot_status("gptbot", BotStatus::Blocked).unwrap();
    drop(db);

    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;
    let (_, flash) = act(&app, &cookie, &csrf, "/nginx/apply", &format!("id={id}")).await;

    assert!(flash.contains("Applied"), "was: {flash}");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("GPTBot"),
        "the blocking rule should be in the config now:\n{written}"
    );
    assert!(
        flash.contains("--no-apply"),
        "the message must say NGINX was not reloaded: {flash}"
    );
}

#[tokio::test]
async fn applying_to_every_site_touches_all_of_them() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let first = write_site(&tmp, "a.example");
    let second = write_site(&tmp, "b.example");

    let db = Db::open(&db_path).unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "gptbot".into(),
        name: "GPTBot".into(),
        is_ai: true,
        is_search_engine: false,
        is_scanner: false,
        user_agent_pattern: "GPTBot".into(),
        source_id: "well-known-bots".into(),
    })
    .unwrap();
    db.set_bot_status("gptbot", BotStatus::Blocked).unwrap();
    drop(db);

    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    act(&app, &cookie, &csrf, "/nginx/apply-all", "").await;

    for path in [first, second] {
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("GPTBot"),
            "{} was:\n{written}",
            path.display()
        );
    }
}

#[tokio::test]
async fn a_site_detail_page_renders_and_its_overrides_stick() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;

    let page = app
        .clone()
        .oneshot(with_cookie(get(&format!("/nginx/{id}")), &cookie))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(body_string(page).await.contains("example.com"));

    act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/category"),
        "category=ai&policy=blocked",
    )
    .await;
    act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-add"),
        "path=/blog",
    )
    .await;

    let db = Db::open(&db_path).unwrap();
    assert_eq!(
        db.get_site_category_override(id, Category::Ai).unwrap(),
        Some(Policy::Blocked)
    );
    assert_eq!(db.site_path_exemptions(id).unwrap(), ["/blog"]);
    drop(db);

    act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-remove"),
        "path=/blog",
    )
    .await;
    assert!(Db::open(&db_path)
        .unwrap()
        .site_path_exemptions(id)
        .unwrap()
        .is_empty());
}

/// A site id nobody has is a page that does not exist, not a server fault.
#[tokio::test]
async fn a_site_that_does_not_exist_is_a_404() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .oneshot(with_cookie(get("/nginx/999"), &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_exemption_that_is_not_a_path_is_refused() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-add"),
        "path=blog",
    )
    .await;
    assert!(flash.contains("start with"), "was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .site_path_exemptions(id)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_exemption_with_a_user_agent_is_scoped_to_that_client() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;
    let form = "path=/remote.php/dav/&user_agent=+okhttp+";

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-add"),
        form,
    )
    .await;
    assert!(flash.contains("exempt for okhttp"), "was: {flash}");
    let db = Db::open(&db_path).unwrap();
    let stored: Vec<(String, String)> = db
        .site_agent_exemptions(id)
        .unwrap()
        .into_iter()
        .map(|e| (e.path, e.user_agent))
        .collect();
    assert_eq!(
        stored,
        [("/remote.php/dav/".to_string(), "okhttp".to_string())]
    );
    assert!(db.site_path_exemptions(id).unwrap().is_empty());
    drop(db);

    act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-remove"),
        "path=/remote.php/dav/&user_agent=okhttp",
    )
    .await;
    assert!(Db::open(&db_path)
        .unwrap()
        .site_agent_exemptions(id)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_exemption_user_agent_that_cannot_be_stored_is_refused() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/exempt-add"),
        "path=/dav/&user_agent=ok%22http",
    )
    .await;
    assert!(flash.contains("cannot contain"), "was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .site_agent_exemptions(id)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_request_shape_rule_can_be_switched_on_for_one_site() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;

    let rule = stop_bots::nginx::RequestRule::ALL[1];
    act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{id}/rule"),
        &format!("rule={}&enabled=1", rule.id()),
    )
    .await;

    assert_eq!(
        Db::open(&db_path).unwrap().site_request_rules(id).unwrap(),
        [rule.id()]
    );
}

/// Seeds GPTBot — an AI crawler, blocked by the AI category's default —
/// and returns its id.
fn seed_gptbot(db_path: &std::path::Path) -> i64 {
    let db = Db::open(db_path).unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "gptbot".into(),
        name: "GPTBot".into(),
        is_ai: true,
        is_search_engine: false,
        is_scanner: false,
        user_agent_pattern: "gptbot".into(),
        source_id: "well-known-bots".into(),
    })
    .unwrap();
    db.list_bots()
        .unwrap()
        .into_iter()
        .find(|b| b.slug == "gptbot")
        .unwrap()
        .id
}

async fn page_text(app: &Router, cookie: &str, path: &str) -> String {
    body_string(
        app.clone()
            .oneshot(with_cookie(get(path), cookie))
            .await
            .unwrap(),
    )
    .await
}

/// A bot can be allowed or blocked on one site from that site's page,
/// the way the TUI's site detail does it: search, pick, and the override
/// is stored for that site only, leaving the bot's global status alone.
#[tokio::test]
async fn a_bot_can_be_overridden_for_one_site_from_its_page() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let site = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;
    let bot = seed_gptbot(&db_path);

    let found = page_text(&app, &cookie, &format!("/nginx/{site}?q=gpt")).await;
    assert!(
        found.contains("GPTBot") && found.contains(r#"name="bot""#),
        "the search did not offer GPTBot:\n{found}"
    );

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        &format!("/nginx/{site}/bot?q=gpt"),
        &format!("bot={bot}&policy=allowed"),
    )
    .await;

    assert_eq!(
        flash,
        "GPTBot on example.com: allowed. Apply to write it out."
    );
    let db = Db::open(&db_path).unwrap();
    let stored: Vec<(i64, Policy)> = db
        .site_bot_overrides(site)
        .unwrap()
        .into_iter()
        .map(|o| (o.bot_id, o.policy))
        .collect();
    assert_eq!(stored, [(bot, Policy::Allowed)]);
    assert_eq!(
        db.list_bots().unwrap()[0].status,
        BotStatus::Default,
        "the global status must not move"
    );
}

/// The redirect comes back to the search the form was posted from, so
/// setting several bots in a row does not mean searching again each time.
#[tokio::test]
async fn setting_a_site_bot_override_returns_to_the_same_search() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let site = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;
    let bot = seed_gptbot(&db_path);

    let response = app
        .clone()
        .oneshot(with_cookie(
            post(
                &format!("/nginx/{site}/bot?q=gpt%20bot"),
                &format!("csrf={csrf}&bot={bot}&policy=blocked"),
            ),
            &cookie,
        ))
        .await
        .unwrap();

    let location = response.headers()[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with(&format!("/nginx/{site}?q=gpt%20bot&flash=")),
        "was: {location}"
    );
}

/// With no search, the panel lists the bots this site already overrides,
/// each tagged as a site override — and "follow" removes one again.
#[tokio::test]
async fn a_site_s_bot_overrides_are_listed_until_cleared() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    write_site(&tmp, "example.com");
    act(&app, &cookie, &csrf, "/nginx/scan", "").await;
    let site = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;
    let bot = seed_gptbot(&db_path);
    let path = format!("/nginx/{site}");

    let before = page_text(&app, &cookie, &path).await;
    assert!(
        !before.contains("GPTBot"),
        "listed before any override:\n{before}"
    );

    let set = format!("{path}/bot");
    act(
        &app,
        &cookie,
        &csrf,
        &set,
        &format!("bot={bot}&policy=allowed"),
    )
    .await;
    let listed = page_text(&app, &cookie, &path).await;
    assert!(
        listed.contains("GPTBot") && listed.contains("site override"),
        "the override is not listed:\n{listed}"
    );

    act(
        &app,
        &cookie,
        &csrf,
        &set,
        &format!("bot={bot}&policy=default"),
    )
    .await;
    assert!(Db::open(&db_path)
        .unwrap()
        .site_bot_overrides(site)
        .unwrap()
        .is_empty());
}

// ---- firewall ----

#[tokio::test]
async fn blocking_and_unblocking_an_address_round_trips() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/block-address",
        "address=192.0.2.55",
    )
    .await;
    assert!(
        Db::open(&db_path)
            .unwrap()
            .list_firewall_rules()
            .unwrap()
            .iter()
            .any(|r| r.address == "192.0.2.55"),
        "the rule should be stored"
    );

    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/unblock-address",
        "address=192.0.2.55",
    )
    .await;
    assert!(Db::open(&db_path)
        .unwrap()
        .list_firewall_rules()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn blocking_and_unblocking_a_user_agent_round_trips() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/block-ua",
        "user_agent=curl%2F8.5.0",
    )
    .await;
    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .list_blocked_user_agents()
            .unwrap(),
        ["curl/8.5.0"]
    );

    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/unblock-ua",
        "user_agent=curl%2F8.5.0",
    )
    .await;
    assert!(Db::open(&db_path)
        .unwrap()
        .list_blocked_user_agents()
        .unwrap()
        .is_empty());
}

/// The table's buttons name the agent by reference. Pressing one acts on
/// the exact string the row shows; pressing one for an agent pruned since
/// the page was drawn changes nothing, and says so.
#[tokio::test]
async fn a_user_agent_row_s_buttons_act_on_the_agent_by_reference() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let agent = "sqlmap/1.7 (' UNION SELECT 1--)";
    let mut counts = std::collections::HashMap::new();
    counts.insert(agent.to_string(), 5);
    Db::open(&db_path)
        .unwrap()
        .record_user_agent_hits(&counts, 1_700_000_000)
        .unwrap();

    let page = page_text(&app, &cookie, "/firewall").await;
    let reference = page
        .split(r#"name="ua" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the row's buttons carry a reference")
        .to_string();

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/firewall/block-ua",
        &format!("ua={reference}"),
    )
    .await;
    assert!(flash.contains("Blocked"), "flash was: {flash}");
    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .list_blocked_user_agents()
            .unwrap(),
        [agent]
    );

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/firewall/trust",
        "kind=user_agent&ua=999-0123456789abcdef",
    )
    .await;
    assert!(flash.contains("no longer counted"), "flash was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .list_trusted_user_agents()
        .unwrap()
        .is_empty());
}

/// An empty user agent is a substring of every one: stored, it blocked
/// every visitor of every site at the next apply.
#[tokio::test]
async fn blocking_an_empty_user_agent_is_refused_with_a_reason() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/firewall/block-ua", "user_agent=").await;

    assert!(flash.contains("Could not block"), "flash was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .list_blocked_user_agents()
        .unwrap()
        .is_empty());
}

/// Trusting a user agent echoes the agent in the flash, and the agent is
/// whatever a client sent. That text used to ride in the redirect's query
/// string, so the operator's own next request carried an attacker's
/// payload into NGINX's log, where the injection detector blocked them.
/// The redirect names the message by id; the page still shows it.
#[tokio::test]
async fn a_message_about_client_text_never_puts_that_text_in_a_url() {
    let (app, password, _tmp) = app();
    let (cookie, csrf) = login(&app, &password).await;

    let payload = "sqlmap' UNION SELECT password FROM users--";
    let response = app
        .clone()
        .oneshot(with_cookie(
            post(
                "/firewall/trust",
                &format!(
                    "csrf={csrf}&kind=user_agent&value={}",
                    server::percent_encode(payload)
                ),
            ),
            &cookie,
        ))
        .await
        .unwrap();
    let location = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();

    for needle in ["UNION", "sqlmap", "SELECT", "%27", "%20"] {
        assert!(!location.contains(needle), "location was {location}");
    }
    let flash = flash_of(&app, &cookie, &response).await;
    assert!(flash.contains(payload), "flash was: {flash}");
}

/// A link anyone can craft used to render its own text in the console's
/// chrome, coloured as an error: `?flash=<text>&kind=err`.
#[tokio::test]
async fn a_crafted_flash_link_shows_nothing() {
    let (app, password, _tmp) = app();
    let (cookie, _csrf) = login(&app, &password).await;

    let html = page_text(
        &app,
        &cookie,
        "/?flash=Your%20session%20expired.%20Log%20in%20at%20evil.example&kind=err",
    )
    .await;

    assert!(!html.contains("evil.example"), "page was:\n{html}");
    assert!(!html.contains(r#"class="flash"#), "page was:\n{html}");
}

/// The free-text field decides what it was given; the row buttons say.
/// Either way the database ends up with the stored form, and the flash
/// echoes it — a /24 typed with host bits set is not what was stored.
#[tokio::test]
async fn trusting_and_untrusting_round_trips_through_the_console() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/firewall/trust",
        "value=203.0.113.77%2F24",
    )
    .await;
    assert!(flash.contains("203.0.113.0/24"), "flash was: {flash}");
    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/trust",
        "kind=user_agent&value=UptimeRobot%2F2.0",
    )
    .await;
    let db = Db::open(&db_path).unwrap();
    assert_eq!(db.list_trusted_addresses().unwrap(), ["203.0.113.0/24"]);
    assert_eq!(db.list_trusted_user_agents().unwrap(), ["UptimeRobot/2.0"]);

    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/untrust",
        "kind=address&value=203.0.113.0%2F24",
    )
    .await;
    act(
        &app,
        &cookie,
        &csrf,
        "/firewall/untrust",
        "kind=user_agent&value=UptimeRobot%2F2.0",
    )
    .await;
    assert!(db.list_trusted_addresses().unwrap().is_empty());
    assert!(db.list_trusted_user_agents().unwrap().is_empty());
}

#[tokio::test]
async fn a_mistyped_address_is_refused_with_the_reason() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/firewall/trust",
        "value=10.0.0.1%2F33",
    )
    .await;

    assert!(flash.contains("Could not trust"), "flash was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .list_trusted_user_agents()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_dynamic_screen_filters_are_all_reachable() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    for filter in ["all", "pending", "blocked", "nonsense"] {
        let response = app
            .clone()
            .oneshot(with_cookie(
                get(&format!("/firewall?filter={filter}")),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "filter={filter}");
    }
}

#[tokio::test]
async fn searching_the_bot_list_is_reachable_from_the_url() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .oneshot(with_cookie(get("/bots?q=gpt"), &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// The anti-lockout guard, through the router.
///
/// `ConnectInfo` is normally supplied by the listener, which these tests do
/// not use — so it is inserted into the request's extensions directly,
/// which is exactly where the middleware reads it from. That keeps the
/// guard under test rather than the plumbing that feeds it.
fn from_peer(mut request: Request<Body>, peer: &str) -> Request<Body> {
    let addr: std::net::SocketAddr = peer.parse().unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    request
}

#[tokio::test]
async fn blocking_the_address_you_are_connected_from_is_refused() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let request = from_peer(
        with_cookie(
            post(
                "/firewall/block-address",
                &format!("csrf={csrf}&address=203.0.113.5"),
            ),
            &cookie,
        ),
        "203.0.113.5:44321",
    );
    let response = app.clone().oneshot(request).await.unwrap();
    let flash = flash_of(&app, &cookie, &response).await;

    assert!(flash.contains("lock you out"), "flash was: {flash}");
    assert!(
        Db::open(&db_path)
            .unwrap()
            .list_firewall_rules()
            .unwrap()
            .is_empty(),
        "the refusal has to actually prevent the write, not just word it differently"
    );
}

#[tokio::test]
async fn blocking_a_different_address_from_the_same_peer_still_works() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let request = from_peer(
        with_cookie(
            post(
                "/firewall/block-address",
                &format!("csrf={csrf}&address=198.51.100.9"),
            ),
            &cookie,
        ),
        "203.0.113.5:44321",
    );
    app.clone().oneshot(request).await.unwrap();

    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .list_firewall_rules()
            .unwrap()
            .len(),
        1,
        "the guard must only refuse the caller's own address"
    );
}

/// Posts a block from `peer` and returns the flash plus whether anything
/// was stored.
async fn block_from(peer: &str, address: &str) -> (String, usize) {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let body = format!("csrf={csrf}&address={}", address.replace('/', "%2F"));
    let request = from_peer(
        with_cookie(post("/firewall/block-address", &body), &cookie),
        peer,
    );
    let response = app.clone().oneshot(request).await.unwrap();
    let flash = flash_of(&app, &cookie, &response).await;
    let stored = Db::open(&db_path)
        .unwrap()
        .list_firewall_rules()
        .unwrap()
        .len();
    (flash, stored)
}

/// The guard used to be `client == address`, text against text. Every
/// other way of writing a block that covers the caller walked past it.
#[tokio::test]
async fn any_block_that_covers_the_caller_is_refused() {
    for target in [
        "203.0.113.0/24",
        "203.0.113.5/32",
        "%20203.0.113.5",
        "203.0.0.0/8",
    ] {
        let (flash, stored) = block_from("203.0.113.5:44321", target).await;
        assert!(
            flash.contains("lock you out"),
            "{target}: flash was {flash}"
        );
        assert_eq!(stored, 0, "{target} was stored");
    }
}

#[tokio::test]
async fn an_ipv6_block_is_compared_as_an_address_not_as_text() {
    for target in ["2001:DB8::1", "2001:db8:0:0::1", "2001:db8::/32"] {
        let (flash, stored) = block_from("[2001:db8::1]:44321", target).await;
        assert!(
            flash.contains("lock you out"),
            "{target}: flash was {flash}"
        );
        assert_eq!(stored, 0, "{target} was stored");
    }
}

/// A dual-stack bind reports an IPv4 client as `::ffff:a.b.c.d`.
#[tokio::test]
async fn an_ipv4_client_on_a_dual_stack_bind_is_still_recognised() {
    let (flash, stored) = block_from("[::ffff:203.0.113.5]:44321", "203.0.113.5").await;
    assert!(flash.contains("lock you out"), "flash was {flash}");
    assert_eq!(stored, 0);
}

/// Every address, whoever is asking — and whether or not this server can
/// tell who that is.
#[tokio::test]
async fn a_manual_block_of_every_address_is_refused() {
    for target in ["0.0.0.0/0", "::/0"] {
        let (app, password, _tmp, db_path) = app_with_db();
        let (cookie, csrf) = login(&app, &password).await;

        let (_, flash) = act(
            &app,
            &cookie,
            &csrf,
            "/firewall/block-address",
            &format!("address={}", target.replace('/', "%2F")),
        )
        .await;

        assert!(
            flash.contains("every address"),
            "{target}: flash was {flash}"
        );
        assert!(
            Db::open(&db_path)
                .unwrap()
                .list_firewall_rules()
                .unwrap()
                .is_empty(),
            "{target} was stored"
        );
    }
}

/// With a proxy in front, the peer is the proxy. The guard follows
/// `X-Forwarded-For` — but only once the operator has said a proxy exists,
/// because otherwise the header is just text anyone can send.
#[tokio::test]
async fn a_forwarded_address_is_only_believed_when_configured() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let attempt = |app: Router, cookie: String, csrf: String| async move {
        let mut request = from_peer(
            with_cookie(
                post(
                    "/firewall/block-address",
                    &format!("csrf={csrf}&address=203.0.113.77"),
                ),
                &cookie,
            ),
            "127.0.0.1:5000",
        );
        request
            .headers_mut()
            .insert("x-forwarded-for", "203.0.113.77".parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        flash_of(&app, &cookie, &response).await
    };

    // Untrusted: the header is ignored, so the block goes through.
    let flash = attempt(app.clone(), cookie.clone(), csrf.clone()).await;
    assert!(
        flash.contains("Blocked"),
        "an untrusted forwarded header must not be able to veto a block: {flash}"
    );

    let db = Db::open(&db_path).unwrap();
    db.unblock_address("203.0.113.77").unwrap();
    db.set_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, true)
        .unwrap();
    drop(db);

    // Trusted: now it names the client, and the guard fires.
    let flash = attempt(app, cookie, csrf).await;
    assert!(flash.contains("lock you out"), "was: {flash}");
    assert!(Db::open(&db_path)
        .unwrap()
        .list_firewall_rules()
        .unwrap()
        .is_empty());
}

/// NGINX's `$proxy_add_x_forwarded_for` appends the address it saw to
/// whatever the client sent. Only that last entry is the proxy's word;
/// anything before it is the client's, and must not name the client.
#[tokio::test]
async fn only_the_address_the_proxy_appended_is_believed() {
    let (app, password, _tmp, db_path) = app_with_db();
    Db::open(&db_path)
        .unwrap()
        .set_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, true)
        .unwrap();
    let (cookie, csrf) = login(&app, &password).await;

    let attempt = |app: Router, forwarded: &'static str| {
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        async move {
            let mut request = from_peer(
                with_cookie(
                    post(
                        "/firewall/block-address",
                        &format!("csrf={csrf}&address=203.0.113.77"),
                    ),
                    &cookie,
                ),
                "127.0.0.1:5000",
            );
            request
                .headers_mut()
                .insert("x-forwarded-for", forwarded.parse().unwrap());
            let response = app.clone().oneshot(request).await.unwrap();
            flash_of(&app, &cookie, &response).await
        }
    };

    // The client typed the victim's address in; the proxy appended theirs.
    let flash = attempt(app.clone(), "203.0.113.77, 198.51.100.4").await;
    assert!(
        flash.contains("Blocked"),
        "a client-supplied entry must not be taken for the client: {flash}"
    );

    Db::open(&db_path)
        .unwrap()
        .unblock_address("203.0.113.77")
        .unwrap();

    // The proxy saw 203.0.113.77 itself, whatever came before it.
    let flash = attempt(app, "198.51.100.4, 203.0.113.77").await;
    assert!(flash.contains("lock you out"), "was: {flash}");
}

// ---- served under a path prefix ----
//
// The whole point of `--base-path` is that *nothing* the browser is told
// to fetch escapes the prefix. A single absolute `/bots` left in a
// template is a broken link that only shows up in a proxied deployment,
// so the check here is exhaustive over the emitted markup rather than a
// spot check on a few known URLs.

const PREFIX: &str = "/stop-bots";

fn get_under(path: &str) -> Request<Body> {
    get(&format!("{PREFIX}{path}"))
}

/// Every URL the page emits, from `href`, `src` and `action` attributes.
fn emitted_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for attribute in ["href=\"", "src=\"", "action=\""] {
        for (_, rest) in html
            .match_indices(attribute)
            .map(|(i, _)| (i, &html[i + attribute.len()..]))
        {
            if let Some(url) = rest.split('"').next() {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

#[tokio::test]
async fn under_a_prefix_the_screens_are_reachable_at_the_prefixed_paths() {
    let (app, password, _tmp, _db) = app_under(PREFIX);

    // Login first, at the prefixed path.
    let response = app
        .clone()
        .oneshot(post(
            &format!("{PREFIX}/login"),
            &format!("password={password}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/stop-bots/",
        "the post-login redirect must land inside the prefix, with its trailing slash"
    );
    let cookie = session_cookie_from(&response);

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app
            .clone()
            .oneshot(with_cookie(get_under(path), &cookie))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{PREFIX}{path} should render"
        );
    }
}

#[tokio::test]
async fn under_a_prefix_the_unprefixed_paths_are_not_served() {
    let (app, _password, _tmp, _db) = app_under(PREFIX);

    // This is what makes the "proxy must not strip the prefix" rule
    // enforceable rather than advice: a stripped request does not
    // accidentally half-work.
    for path in ["/login", "/", "/assets/style.css"] {
        let response = app.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} must not be served when a prefix is configured"
        );
    }
}

#[tokio::test]
async fn no_url_on_any_page_escapes_the_prefix() {
    let (app, password, tmp, db_path) = app_under(PREFIX);

    // Seed enough that every table renders rows, so their action URLs are
    // in the markup being checked rather than behind an empty state.
    let db = Db::open(&db_path).unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "gptbot".into(),
        name: "GPTBot".into(),
        is_ai: true,
        is_search_engine: false,
        is_scanner: false,
        user_agent_pattern: "GPTBot".into(),
        source_id: "well-known-bots".into(),
    })
    .unwrap();
    db.block_address_permanently("192.0.2.9", stop_bots::db::RuleSource::Tui, None)
        .unwrap();
    db.set_country_selected("CN", true).unwrap();
    drop(db);
    write_site(&tmp, "example.com");

    let response = app
        .clone()
        .oneshot(post(
            &format!("{PREFIX}/login"),
            &format!("password={password}"),
        ))
        .await
        .unwrap();
    let cookie = session_cookie_from(&response);

    let csrf_page = app
        .clone()
        .oneshot(with_cookie(get_under("/nginx"), &cookie))
        .await
        .unwrap();
    let csrf = body_string(csrf_page)
        .await
        .split(r#"<meta name="csrf-token" content=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();

    let scan = format!("{PREFIX}/nginx/scan");
    app.clone()
        .oneshot(with_cookie(post(&scan, &format!("csrf={csrf}")), &cookie))
        .await
        .unwrap();
    let site_id = Db::open(&db_path).unwrap().list_sites().unwrap()[0].id;

    let pages = [
        "/".to_string(),
        "/bots".to_string(),
        "/bots?q=gpt".to_string(),
        "/nginx".to_string(),
        format!("/nginx/{site_id}"),
        "/firewall".to_string(),
        "/help".to_string(),
    ];

    for path in pages {
        let response = app
            .clone()
            .oneshot(with_cookie(get_under(&path), &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let html = body_string(response).await;

        let urls = emitted_urls(&html);
        assert!(!urls.is_empty(), "{path} emitted no URLs at all to check");

        for url in urls {
            // Off-site links (the README's, say) and fragments are none of
            // this check's business; a root-relative one is.
            if !url.starts_with('/') {
                continue;
            }
            assert!(
                url.starts_with("/stop-bots/"),
                "{path} emits {url}, which points outside the prefix and would 404 behind the proxy"
            );
        }
    }
}

#[tokio::test]
async fn under_a_prefix_the_session_cookie_is_scoped_to_it() {
    let (app, password, _tmp, _db) = app_under(PREFIX);

    let response = app
        .oneshot(post(
            &format!("{PREFIX}/login"),
            &format!("password={password}"),
        ))
        .await
        .unwrap();
    let set = response.headers()[header::SET_COOKIE].to_str().unwrap();

    assert!(
        set.contains("Path=/stop-bots/"),
        "a session for this console has no business being sent to every other app on the \
         domain; was: {set}"
    );
}

#[tokio::test]
async fn under_a_prefix_a_redirect_after_an_action_stays_inside_it() {
    let (app, password, _tmp, _db) = app_under(PREFIX);
    let response = app
        .clone()
        .oneshot(post(
            &format!("{PREFIX}/login"),
            &format!("password={password}"),
        ))
        .await
        .unwrap();
    let cookie = session_cookie_from(&response);

    let page = app
        .clone()
        .oneshot(with_cookie(get_under("/"), &cookie))
        .await
        .unwrap();
    let csrf = body_string(page)
        .await
        .split(r#"<meta name="csrf-token" content=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();

    let response = app
        .oneshot(with_cookie(
            post(
                &format!("{PREFIX}/category"),
                &format!("csrf={csrf}&category=ai&policy=allowed"),
            ),
            &cookie,
        ))
        .await
        .unwrap();
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with("/stop-bots/?flash="),
        "an action's redirect must come back inside the prefix; was: {location}"
    );
}

#[tokio::test]
async fn an_unauthenticated_request_under_a_prefix_is_sent_to_the_prefixed_login() {
    let (app, _password, _tmp, _db) = app_under(PREFIX);

    let response = app.oneshot(get_under("/bots")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/stop-bots/login",
        "redirecting to /login would send the browser outside the location block"
    );
}

// ---- login throttling ----

/// A login post from one identifiable client. The throttle keys on the
/// address, and a request with none at all shares the key everyone
/// without one shares, which has no backoff of its own.
fn login_attempt(body: &str) -> Request<Body> {
    from_peer(post("/login", body), "198.51.100.20:5000")
}

/// A router whose login throttle runs on `config`, and a way to set up
/// the database before it starts, for the tests that need the global
/// bucket drained without running twenty Argon2 verifications to do it.
fn app_throttled(
    config: stop_bots::web::auth::ThrottleConfig,
    setup: impl FnOnce(&Db),
) -> (Router, String, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db = Db::open(&db_path).unwrap();
    let password = stop_bots::web::auth::generate_password().unwrap();
    stop_bots::web::auth::set_password(&db, &password).unwrap();
    setup(&db);
    drop(db);
    let mut state = AppState::new(
        Db::open(&db_path).unwrap(),
        tmp.path().join("nginx"),
        None,
        false,
    );
    state.login_throttle = std::sync::Arc::new(stop_bots::web::auth::LoginThrottle::new(config));
    // The test's own, never the host's: on a machine that runs stop-bots,
    // /etc/stop-bots/host.conf would otherwise decide what the console shows.
    state.host_conf = tmp.path().join("host.conf");
    (server::router(state), password, tmp, db_path)
}

/// Every `Set-Cookie` on a response, name=value only.
fn cookies_set(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
        .collect()
}

/// The console behind the Web Access proxy, with `trust_forwarded_for`
/// off: every request arrives from 127.0.0.1, attacker and operator alike.
/// One attacker posting a wrong password each time `Retry-After` lapsed
/// used to hold that shared key at the thirty-second ceiling, and the
/// operator got 0 of 40 logins through with the right password.
///
/// A browser that has logged in before is remembered, and gets in with
/// the right password whatever the shared limits say.
#[tokio::test]
async fn a_remembered_browser_logs_in_while_an_attacker_holds_the_shared_limits() {
    let (app, password, _tmp, _db) = app_throttled(
        stop_bots::web::auth::ThrottleConfig {
            burst: 2.0,
            refill_per_second: 0.001,
            ..Default::default()
        },
        |db| {
            db.set_text_setting(stop_bots::web::ALLOWED_HOSTS_KEY, "example.com")
                .unwrap()
        },
    );
    let through_proxy = |body: String, cookie: Option<&str>| {
        let mut request = from_peer(post("/login", &body), "127.0.0.1:40000");
        if let Some(cookie) = cookie {
            request = with_cookie(request, cookie);
        }
        request
    };

    // The operator logged in last week, from this browser.
    let first = app
        .clone()
        .oneshot(through_proxy(format!("password={password}"), None))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let remembered = cookies_set(&first)
        .into_iter()
        .find(|c| c.starts_with("stop_bots_remember="))
        .expect("a successful login remembers the browser");

    // The attacker, through the same proxy, until refused.
    let mut refused = false;
    for i in 0..5 {
        let status = app
            .clone()
            .oneshot(through_proxy(format!("password=wrong{i}"), None))
            .await
            .unwrap()
            .status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            refused = true;
            break;
        }
    }
    assert!(refused, "the shared limit should have tripped");

    // A browser the console has not seen waits with the attacker...
    let stranger = app
        .clone()
        .oneshot(through_proxy(format!("password={password}"), None))
        .await
        .unwrap();
    assert_eq!(stranger.status(), StatusCode::TOO_MANY_REQUESTS);

    // ...and the operator's does not.
    let operator = app
        .clone()
        .oneshot(through_proxy(
            format!("password={password}"),
            Some(&remembered),
        ))
        .await
        .unwrap();
    assert_eq!(
        operator.status(),
        StatusCode::SEE_OTHER,
        "the operator's own browser must get in whatever an attacker does"
    );
}

/// The remembered-browser token only ever skips the throttle; it is not a
/// credential. And `--set-password` forgets every one of them.
#[tokio::test]
async fn a_remembered_browser_still_needs_the_password_and_a_new_password_forgets_it() {
    let (app, password, _tmp, db_path) = app_throttled(
        stop_bots::web::auth::ThrottleConfig {
            burst: 1.0,
            refill_per_second: 0.001,
            ..Default::default()
        },
        |_| {},
    );
    let first = app
        .clone()
        .oneshot(login_attempt(&format!("password={password}")))
        .await
        .unwrap();
    let remembered = cookies_set(&first)
        .into_iter()
        .find(|c| c.starts_with("stop_bots_remember="))
        .unwrap();

    let wrong = app
        .clone()
        .oneshot(with_cookie(login_attempt("password=wrong"), &remembered))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert!(
        cookies_set(&wrong).is_empty(),
        "a wrong password opens nothing"
    );

    let new_password = stop_bots::web::auth::generate_password().unwrap();
    stop_bots::web::auth::set_password(&Db::open(&db_path).unwrap(), &new_password).unwrap();

    // The bucket was spent by the first login; only a remembered browser
    // could get past it, and this one is no longer remembered.
    let after = app
        .oneshot(with_cookie(
            login_attempt(&format!("password={new_password}")),
            &remembered,
        ))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// A run of wrong passwords eventually gets a 429 rather than another
/// Argon2 verification.
///
/// The production burst is 20, so this posts past it. That is the point:
/// before this existed, anyone who could reach `/login` could make the
/// server run unbounded Argon2 — ~50ms of CPU and 19MB each — as fast as
/// they could post, against the very host this tool protects.
#[tokio::test]
async fn a_flood_of_login_attempts_is_refused_before_it_costs_a_hash() {
    let (app, _password, _tmp, _db) = app_with_db();

    let mut refused = None;
    for attempt in 0..40 {
        let response = app
            .clone()
            .oneshot(login_attempt("password=wrong"))
            .await
            .unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some((attempt, response));
            break;
        }
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should be a plain refusal until the limiter trips"
        );
    }

    let (attempt, response) = refused.expect("the limiter must trip within 40 attempts");
    assert!(
        attempt >= 5,
        "tripping after {attempt} attempts would punish an ordinary typo"
    );
    assert!(
        response.headers().contains_key(header::RETRY_AFTER),
        "a 429 without Retry-After tells the caller nothing about when to come back"
    );

    let retry: u64 = response.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After is a number of seconds");
    assert!(retry >= 1, "Retry-After was {retry}");
}

/// The throttle must not stand between the operator and a console they
/// have the password for.
#[tokio::test]
async fn a_correct_password_still_works_after_a_few_typos() {
    let (app, password, _tmp, _db) = app_with_db();

    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(login_attempt("password=wrong"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .oneshot(login_attempt(&format!("password={password}")))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a handful of typos must not lock the operator out"
    );
}

/// And a success clears the history, so the next typo starts from zero
/// rather than from wherever the previous run left off.
#[tokio::test]
async fn logging_in_successfully_resets_the_throttle() {
    let (app, password, _tmp, _db) = app_with_db();

    // Every free attempt, so the next failure without a reset would start
    // the backoff. Fewer than that would pass with no reset at all.
    let free = stop_bots::web::auth::ThrottleConfig::default().free_attempts;
    for _ in 0..free {
        app.clone()
            .oneshot(login_attempt("password=wrong"))
            .await
            .unwrap();
    }
    app.clone()
        .oneshot(login_attempt(&format!("password={password}")))
        .await
        .unwrap();

    // Two more typos: the first would set a delay if the history had
    // survived, and the second would be refused by it.
    for attempt in 0..2 {
        let response = app
            .clone()
            .oneshot(login_attempt("password=wrong"))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "typo {attempt} after a good login should be a plain refusal, not a carried-over 429"
        );
    }
}

/// The 429 page is still the login page, so the operator sees why and can
/// try again from it.
#[tokio::test]
async fn a_throttled_login_still_renders_the_login_form() {
    let (app, _password, _tmp, _db) = app_with_db();

    let mut throttled = None;
    for _ in 0..40 {
        let response = app
            .clone()
            .oneshot(login_attempt("password=wrong"))
            .await
            .unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            throttled = Some(response);
            break;
        }
    }

    let html = body_string(throttled.expect("the limiter must trip")).await;
    assert!(html.contains("Too many login attempts"), "was: {html}");
    assert!(
        html.contains(r#"name="password""#),
        "the operator should be able to try again from this page"
    );
}

/// The mitigation for the shared-bucket problem, and the reason
/// `web:trust_forwarded_for` earns its keep beyond the anti-lockout guard.
///
/// Behind a proxy every request arrives from 127.0.0.1, so a flood shares
/// the operator's bucket and delays their login too. Once the console is
/// told it may believe the forwarded header, the attacker gets their own
/// bucket and the operator is unaffected — which is what this asserts.
#[tokio::test]
async fn a_trusted_forwarded_address_keeps_one_clients_flood_off_another() {
    let (app, password, _tmp, db_path) = app_with_db();
    Db::open(&db_path)
        .unwrap()
        .set_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, true)
        .unwrap();

    let attempt = |app: Router, forwarded: &'static str, body: String| async move {
        let mut request = post("/login", &body);
        request
            .headers_mut()
            .insert("x-forwarded-for", forwarded.parse().unwrap());
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:5000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        app.oneshot(request).await.unwrap().status()
    };

    // An attacker, from one address, until it is refused.
    let mut throttled = false;
    for i in 0..20 {
        let status = attempt(app.clone(), "203.0.113.9", format!("password=wrong{i}")).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            throttled = true;
            break;
        }
    }
    assert!(
        throttled,
        "the attacker's address should have been throttled"
    );

    // The operator, from another, with the right password.
    let status = attempt(app, "198.51.100.4", format!("password={password}")).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "one address's flood must not stand between the operator and their own console"
    );
}

/// The detail panel is reached by a query parameter rather than a
/// script-loaded fragment, so it has to survive an ordinary GET — including one typed
/// into the URL bar with an address that is not in the table.
#[tokio::test]
async fn inspecting_an_address_renders_its_detail_panel() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let _ = csrf;

    let db = Db::open(&db_path).unwrap();
    db.register_reputation_source(&stop_bots::db::ReputationSource {
        id: "tor-exits".into(),
        name: "Tor exit nodes".into(),
        url: "https://example.invalid/tor".into(),
        enabled: true,
        last_fetched_at: None,
        range_count: 0,
    })
    .unwrap();
    db.set_reputation_source_enabled("tor-exits", true).unwrap();
    db.replace_reputation_ranges("tor-exits", &["185.220.101.0/24".to_string()])
        .unwrap();
    drop(db);

    let response = app
        .clone()
        .oneshot(with_cookie(
            get("/firewall?filter=all&inspect=185.220.101.7"),
            &cookie,
        ))
        .await
        .unwrap();
    let body = body_string(response).await;

    assert!(body.contains("About 185.220.101.7"), "body was:\n{body}");
    assert!(body.contains("Tor exit nodes"), "body was:\n{body}");
}

/// The parameter is whatever is in the URL bar. A value that is not an
/// address at all must render a page saying so, not a 500.
#[tokio::test]
async fn inspecting_something_that_is_not_an_address_still_renders() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(
            get("/firewall?inspect=not-an-address"),
            &cookie,
        ))
        .await
        .unwrap();
    let body = body_string(response).await;

    assert!(body.contains("Not a valid IP address"), "body was:\n{body}");
}

/// The user-agent panel's own route, which the unit tests cannot reach:
/// the string arrives percent-encoded in a query parameter, and a user
/// agent is full of characters that have to survive that trip.
#[tokio::test]
async fn inspecting_a_user_agent_renders_its_detail_panel() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let ua = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
    let db = Db::open(&db_path).unwrap();
    // A source of its own rather than one of the built-in ids, which the
    // server re-registers under its own name at startup.
    db.register_source(&stop_bots::db::Source {
        id: "a-second-opinion".into(),
        name: "A second opinion".into(),
        url: "https://example.invalid/list".into(),
        last_fetched_at: None,
        bot_count: 0,
    })
    .unwrap();
    db.upsert_bot(&stop_bots::db::NewBot {
        slug: "googlebot".into(),
        name: "Googlebot".into(),
        is_ai: false,
        is_search_engine: true,
        is_scanner: false,
        user_agent_pattern: "Googlebot".into(),
        source_id: "a-second-opinion".into(),
    })
    .unwrap();
    let mut counts = std::collections::HashMap::new();
    counts.insert(ua.to_string(), 412);
    db.record_user_agent_hits(&counts, 1_700_000_000).unwrap();
    drop(db);

    // Followed from the table, the way the operator gets there: the link
    // names the row, and carries nothing of the string.
    let table = page_text(&app, &cookie, "/firewall?filter=all").await;
    let link = table
        .split(r#"href=""#)
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .find(|href| href.contains("inspect_ua="))
        .expect("the user agent's cell links to its detail")
        .replace("&amp;", "&");
    for needle in ["Mozilla", "Googlebot", "google", "%2F"] {
        assert!(!link.contains(needle), "the link carries {needle}: {link}");
    }
    let body = page_text(&app, &cookie, &link).await;

    assert!(body.contains("About this user agent"), "body was:\n{body}");
    assert!(body.contains("A second opinion"), "body was:\n{body}");
    assert!(body.contains("412"), "body was:\n{body}");
}

/// The parameter is whatever is in the URL bar. A reference to nothing —
/// a stale one, or a string typed in — renders a page saying so, not a
/// 500 and not a blank panel, and is never looked up as a user agent.
#[tokio::test]
async fn inspecting_a_user_agent_that_is_not_there_still_renders() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    for parameter in [
        "%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "123-0123456789abcdef",
        "Googlebot",
    ] {
        let response = app
            .clone()
            .oneshot(with_cookie(
                get(&format!("/firewall?inspect_ua={parameter}")),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{parameter}");
        let body = body_string(response).await;

        assert!(body.contains("no longer counted"), "body was:\n{body}");
        assert!(!body.contains("<script>alert"), "body was:\n{body}");
    }
}

/// Twenty thousand distinct user agents made this page 418 MB and held
/// the database for nine seconds. It shows one page of the most-seen,
/// however many there are, and says how many there are.
#[tokio::test]
async fn the_firewall_page_shows_one_page_of_a_flood_of_user_agents() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;
    let counts: std::collections::HashMap<String, u64> = (0..450u64)
        .map(|i| (format!("flood-{i:04}/{}", "x".repeat(2_000)), 1 + i % 7))
        .collect();
    Db::open(&db_path)
        .unwrap()
        .record_user_agent_hits(&counts, 1_700_000_000)
        .unwrap();

    let html = page_text(&app, &cookie, "/firewall").await;

    let rows = html.matches("inspect_ua=").count();
    assert_eq!(rows, stop_bots::dynamic::UA_PAGE_ROWS, "rows shown");
    assert!(html.contains(" of 450"), "no total shown");
    assert!(
        html.len() < 500_000,
        "a page of agents came to {} bytes",
        html.len()
    );

    let next = page_text(&app, &cookie, "/firewall?filter=all&ua_page=3").await;
    assert!(next.contains("User agents 401"), "page three:\n{next}");
}

/// The complaint that started this: selecting a country told the operator
/// to go and run a CLI command. A console that knows what needs doing
/// should offer to do it.
#[tokio::test]
async fn selecting_a_country_names_a_button_rather_than_a_cli_command() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/geo-add", "country=ru").await;

    assert!(
        !flash.contains("stop-bots update-country-ranges"),
        "the message still sends the operator to the CLI: {flash}"
    );
    assert!(flash.contains("Update everything"), "was: {flash}");
}

/// Both buttons have to be on the page and carry the token, or they are
/// 403s waiting to happen — and they belong in the header, on every
/// screen, because they act on the whole host rather than on one list
/// and are the TUI's `u` and `a`, which work from anywhere.
/// "Update everything" while another download is out — the console's own
/// weekly job, the TUI's, or a `batch` from cron — fetches nothing and
/// says so, rather than asking the same feeds for the same files twice.
#[tokio::test]
async fn update_everything_waits_for_a_download_already_running() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let other = Db::open(&db_path).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    stop_bots::refresh::claim(&other, now).unwrap();

    let (_, flash) = act(&app, &cookie, &csrf, "/update-all", "").await;

    assert!(flash.contains("Another update"), "was: {flash}");
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Refreshing one crawler's ranges waits for a download already out, the
/// same as "Update everything" does — one download at a time, whichever
/// button started it.
#[tokio::test]
async fn a_crawler_range_refresh_waits_for_a_download_already_running() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;
    let other = Db::open(&db_path).unwrap();
    stop_bots::refresh::claim(&other, now_secs()).unwrap();

    let (_, flash) = act(&app, &cookie, &csrf, "/crawler-ranges", "source=googlebot").await;

    assert!(flash.contains("Another update"), "was: {flash}");
    assert!(
        other.list_ip_range_sources().unwrap().is_empty(),
        "nothing should have been stored"
    );
}

#[tokio::test]
async fn an_unknown_crawler_is_refused_before_anything_is_fetched() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/crawler-ranges", "source=yandexbot").await;

    assert_eq!(flash, "Unknown crawler: yandexbot");
}

/// The Dashboard lists every crawler's published ranges — fetched or not,
/// since the row is where the first fetch is offered — with how many
/// ranges each holds, when it was fetched, and a refresh button for it.
#[tokio::test]
async fn the_dashboard_shows_each_crawler_range_with_its_count_and_age() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;
    stop_bots::ipranges::store(
        &Db::open(&db_path).unwrap(),
        stop_bots::ipranges::IpRangeSourceKind::GoogleBot,
        &["66.249.64.0/19".to_string(), "66.249.96.0/20".to_string()],
    )
    .unwrap();

    let body = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;

    let panel = body
        .split_once("Crawler IP ranges")
        .map(|(_, rest)| rest.split("</section>").next().unwrap_or(rest))
        .unwrap_or_else(|| panic!("no crawler panel:\n{body}"));
    let row = |name: &str| {
        panel
            .split("<tr>")
            .find(|row| row.contains(&format!("<td>{name}</td>")))
            .unwrap_or_else(|| panic!("no {name} row:\n{panel}"))
            .to_string()
    };
    for (name, expected) in [
        ("Googlebot", r#"<td class="num">2</td>"#),
        ("Googlebot", "just now"),
        ("Bingbot", "NEVER"),
        ("GPTBot", "NEVER"),
        ("GPTBot", r#"name="source" value="gptbot""#),
    ] {
        assert!(
            row(name).contains(expected),
            "{name} lacks {expected}:\n{}",
            row(name)
        );
    }
}

#[tokio::test]
async fn the_dashboard_offers_update_everything_and_apply_everything() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(get("/"), &cookie))
        .await
        .unwrap();
    let body = body_string(response).await;

    for action in ["/update-all", "/apply-all"] {
        assert!(body.contains(action), "no form posts to {action}:\n{body}");
    }
    assert!(
        body.matches(r#"name="csrf""#).count() >= 2,
        "the new forms must carry the token too"
    );

    let (header, _) = body.split_once("</header>").expect("a page with a header");
    for action in ["/update-all", "/apply-all"] {
        assert!(
            header.contains(action),
            "{action} is on the page but not in the header:\n{header}"
        );
    }
}

/// The panel that answers "is this host actually protected", as opposed to
/// the rest of the Dashboard, which answers "is what this tool would write
/// up to date".
#[tokio::test]
async fn the_dashboard_reports_system_status_once_a_probe_exists() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    // Before any probe, the panel says so rather than rendering seven
    // rows of UNKNOWN.
    let body = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    let panel = panel_html(&body, "System status");
    assert!(
        panel.contains("No check has run yet"),
        "an unprobed host should say so:\n{panel}"
    );

    // The state a real host was found in: rules generated, none loaded.
    {
        // A second connection to the same file, which is also a small
        // exercise of the busy timeout `Db::open` now sets.
        let db = Db::open(&db_path).unwrap();
        db.add_firewall_rule(&stop_bots::db::NewFirewallRule {
            address: "198.51.100.7".to_string(),
            port: None,
            action: stop_bots::db::FirewallAction::Block,
            source: stop_bots::db::RuleSource::Cli,
            evidence: None,
        })
        .unwrap();
        stop_bots::health::store_probe(
            &db,
            &stop_bots::health::Probe {
                live_rules: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    }

    let body = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    let panel = panel_html(&body, "System status");

    assert!(panel.contains("CRITICAL"), "was:\n{panel}");
    assert!(
        panel.contains("none loaded into the kernel"),
        "the panel does not say what is wrong:\n{panel}"
    );
    assert!(
        panel.contains("Apply everything"),
        "the panel does not say what to do about it:\n{panel}"
    );
}

/// A prefix that has been recorded but not picked up is the state a
/// console ends up in the moment Web Access writes one: the panel shows
/// the new prefix, the process is still serving the old one, and every
/// link 404s with nothing on the page to explain it.
#[tokio::test]
async fn a_recorded_prefix_that_needs_a_restart_says_so() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    // Nothing recorded: this console serves the root, nothing is pending.
    let body = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !panel_html(&body, "Web Access").contains("Restart needed"),
        "a console with no recorded prefix should not claim a restart is pending"
    );

    // Recorded after start-up, exactly as the Web Access form does it.
    {
        let db = Db::open(&db_path).unwrap();
        db.set_text_setting(stop_bots::web::BASE_PATH_KEY, "/stop-bots")
            .unwrap();
    }

    let body = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    let panel = panel_html(&body, "Web Access");

    assert!(panel.contains("Restart needed"), "was:\n{panel}");
    assert!(
        panel.contains("systemctl restart"),
        "the notice must say how to fix it:\n{panel}"
    );
    // And the prefix renders as a path, not with a doubled slash.
    assert!(
        panel.contains("/stop-bots") && !panel.contains("//stop-bots"),
        "the prefix is rendered with a doubled slash:\n{panel}"
    );
}

/// Loose content — anything that is not the full-bleed table — has to sit
/// in a `.panel-body`, because that is the only thing carrying the side
/// padding. Web Access shipped without it and its dropdowns sat flush
/// against the panel edge.
#[tokio::test]
async fn every_panel_puts_its_loose_content_in_a_padded_body() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(get("/"), &cookie))
        .await
        .unwrap();
    let body = body_string(response).await;

    for title in ["Web Access", "Policy"] {
        let panel = panel_html(&body, title);
        // A table is full-bleed by design and provides its own cell
        // padding; everything after it is the loose content in question.
        let loose = match panel.rfind("</table>") {
            Some(at) => &panel[at + "</table>".len()..],
            None => {
                panel
                    .split_once("</h2>")
                    .expect("a panel with no heading")
                    .1
            }
        };
        assert!(
            loose.starts_with(r#"<div class="panel-body""#),
            "{title} puts loose content outside a .panel-body:\n{loose}"
        );
    }
}

/// The markup of one `section.panel`, picked out by its heading.
fn panel_html<'a>(body: &'a str, title: &str) -> &'a str {
    let heading = body
        .find(title)
        .unwrap_or_else(|| panic!("no panel titled {title}:\n{body}"));
    let start = body[..heading]
        .rfind("<section")
        .expect("a heading outside any section");
    let end = body[start..]
        .find("</section>")
        .expect("an unclosed section");
    &body[start..start + end]
}

/// `--no-apply` is the flag that keeps a test — or a cautious operator —
/// from having their real firewall rewritten under them. The fixture sets
/// it, so this is also the assertion that the suite cannot run `nft`.
#[tokio::test]
async fn apply_everything_writes_but_does_not_enforce_under_no_apply() {
    let (app, password, tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/apply-all", "").await;

    assert!(flash.contains("not applied: --no-apply"), "was: {flash}");
    assert!(
        tmp.path().join("firewall.next.nft").exists(),
        "the script should still have been written"
    );
    assert!(
        !tmp.path().join("firewall.nft").exists(),
        "nothing was applied, so nothing may reach the script a boot unit loads"
    );
}

/// One click on "Apply everything" changes nothing: the button is a `GET`
/// to a page that says what would change — the files, the rules, the
/// lockout check — and carries the `POST` that does it.
#[tokio::test]
async fn apply_everything_asks_first_and_asking_changes_nothing() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;
    Db::open(&db_path)
        .unwrap()
        .block_address_permanently("192.0.2.10", stop_bots::db::RuleSource::Web, None)
        .unwrap();

    let home = body_string(
        app.clone()
            .oneshot(with_cookie(get("/"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        home.contains(r#"method="get" action="/apply-all""#),
        "the header button must not post:\n{home}"
    );

    let page = body_string(
        app.clone()
            .oneshot(with_cookie(get("/apply-all"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    for expected in [
        "Apply everything?",
        "all of it is new",
        "lockout check",
        r#"method="post" action="/apply-all""#,
        "Show the diff",
    ] {
        assert!(page.contains(expected), "no {expected:?} on:\n{page}");
    }
    assert!(
        !tmp.path().join("firewall.next.nft").exists(),
        "a preview wrote the script"
    );
    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .get_firewall_rendered_signature()
            .unwrap(),
        None,
        "a preview recorded a render"
    );

    let diff = body_string(
        app.clone()
            .oneshot(with_cookie(get("/apply-all?diff=1"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        diff.contains("+\t192.0.2.10"),
        "the diff should show the rule it adds:\n{diff}"
    );
}

/// The render form's new "run it after writing" box, under the same flag.
#[tokio::test]
async fn the_render_form_can_ask_for_an_apply_and_still_respects_no_apply() {
    let (app, password, tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/render-firewall",
        "backend=iptables&apply=1",
    )
    .await;

    assert!(flash.contains("not applied: --no-apply"), "was: {flash}");
    assert!(
        tmp.path().join("firewall.next.nft").exists(),
        "was: {flash}"
    );
}

/// The backend choice is remembered, so a one-click "Apply everything" has
/// an answer and an operator who chose iptables is not handed an nftables
/// script next time.
#[tokio::test]
async fn the_chosen_firewall_backend_is_remembered() {
    let (app, password, _tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    act(
        &app,
        &cookie,
        &csrf,
        "/render-firewall",
        "backend=iptables&apply=1",
    )
    .await;

    let db = Db::open(&db_path).unwrap();
    assert_eq!(
        stop_bots::firewall::stored_backend(&db).unwrap(),
        stop_bots::firewall::FirewallBackend::Iptables
    );
}

/// Path mode end to end: the `location` block, the base path and the host
/// allowlist all have to land, because any one of them missing produces a
/// console that looks broken rather than misconfigured — a 404 for the
/// prefix, a 403 for the host.
#[tokio::test]
async fn setting_up_path_access_writes_the_config_and_both_settings() {
    let (app, password, tmp, db_path) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let site = tmp.path().join("nginx/sites-enabled/example.com");
    std::fs::write(
        &site,
        "server {\n    listen 443 ssl;\n    server_name example.com;\n}\n",
    )
    .unwrap();

    // `nginx -t` is what validates the generated config, and a test must
    // never run the developer's real one — this is the same
    // `set-nginx-commands` escape hatch the container tests use.
    stop_bots::hostconf::HostConf {
        nginx_test_command: Some("true".into()),
        ..Default::default()
    }
    .save_to(&tmp.path().join("host.conf"))
    .unwrap();
    let db = Db::open(&db_path).unwrap();
    let scanned = stop_bots::nginx::discover_sites(&tmp.path().join("nginx")).unwrap();
    for found in &scanned {
        db.upsert_site(&found.server_name, &found.config_path.to_string_lossy())
            .unwrap();
    }
    drop(db);

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/web-access",
        "mode=path&site=example.com&prefix=/stop-bots/&host=",
    )
    .await;

    assert!(flash.contains("Wrote"), "was: {flash}");
    let written = std::fs::read_to_string(&site).unwrap();
    assert!(
        written.contains(r#"location "/stop-bots/" {"#),
        "config was:\n{written}"
    );
    assert!(
        written.contains("proxy_pass http://127.0.0.1:8787;"),
        "config was:\n{written}"
    );

    let db = Db::open(&db_path).unwrap();
    assert_eq!(
        stop_bots::web::BasePath::from_db(&db).unwrap().as_str(),
        "/stop-bots",
        "without this every link the console generates leaves the location block"
    );
    assert!(
        stop_bots::web::configured_hosts(&db)
            .unwrap()
            .contains(&"example.com".to_string()),
        "without this the proxied request's Host is refused"
    );
    // The location sets X-Forwarded-For; without believing it, every
    // proxied client is 127.0.0.1 and shares one login throttle.
    assert!(db
        .get_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, false)
        .unwrap());
    // The site's block is `listen 443 ssl`, so the console is behind TLS.
    assert!(db
        .get_bool_setting(stop_bots::web::SECURE_COOKIE_KEY, false)
        .unwrap());
    assert!(flash.contains("X-Forwarded-For"), "was: {flash}");
}

/// Mounting the console at the site root would take over the whole site.
#[tokio::test]
async fn path_access_refuses_the_site_root() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/web-access",
        "mode=path&site=example.com&prefix=/&host=",
    )
    .await;

    assert!(flash.contains("needs a prefix"), "was: {flash}");
}

#[tokio::test]
async fn subdomain_access_needs_a_host_name() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/web-access",
        "mode=subdomain&site=&prefix=/stop-bots/&host=not+a+host",
    )
    .await;

    assert!(flash.contains("host name"), "was: {flash}");
}

/// The panel has to be on the page with both modes offered.
#[tokio::test]
async fn the_dashboard_offers_a_web_access_panel() {
    let (app, password, _tmp, _db) = app_with_db();
    let (cookie, _csrf) = login(&app, &password).await;

    let response = app
        .clone()
        .oneshot(with_cookie(get("/"), &cookie))
        .await
        .unwrap();
    let body = body_string(response).await;

    assert!(body.contains("Web Access"), "no panel:\n{body}");
    assert!(body.contains("/web-access"), "no form action");
    for mode in ["Path on an existing site", "Its own subdomain"] {
        assert!(body.contains(mode), "missing mode {mode}");
    }
}

// ---- Blocks ----

/// Seeds `count` detector blocks, half from each of two detectors, with
/// evidence, in one transaction: the shape of a busy host's table.
fn seed_blocks(db_path: &std::path::Path, count: u32) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch("PRAGMA synchronous = OFF; BEGIN")
        .unwrap();
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO firewall_rules
                     (address, action, enabled, created_at, expires_at, source, evidence)
                 VALUES (?1, 'block', 1, 1790000000, 4102444800, ?2, '\"GET /.env HTTP/1.1\" 404')",
            )
            .unwrap();
        for i in 0..count {
            let address = std::net::Ipv4Addr::from(0x0a00_0000 + i).to_string();
            let source = if i % 2 == 0 {
                "block_probe_paths"
            } else {
                "block_web_scanners"
            };
            stmt.execute(rusqlite::params![address, source]).unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
}

async fn page_html(app: &Router, cookie: &str, path: &str) -> String {
    let response = app
        .clone()
        .oneshot(with_cookie(get(path), cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    body_string(response).await
}

/// A detector's block, which no other page shows, with the request that
/// earned it; and filtering, searching and paging all reach it.
#[tokio::test]
async fn the_blocks_page_shows_a_detector_s_block_and_why() {
    let (app, password, _tmp, db_path) = app_with_db();
    seed_blocks(&db_path, 3);
    let (cookie, _csrf) = login(&app, &password).await;

    let html = page_html(&app, &cookie, "/blocks?source=probe-paths").await;
    for needle in ["10.0.0.0", "10.0.0.2", "Probe paths", "GET /.env HTTP/1.1"] {
        assert!(html.contains(needle), "{needle:?} missing:\n{html}");
    }
    assert!(
        !html.contains("10.0.0.1<"),
        "a web scanner's block, filtered out"
    );

    let searched = page_html(&app, &cookie, "/blocks?q=10.0.0.1").await;
    assert!(searched.contains("Rules 1\u{2013}1 of 1"), "{searched}");
}

/// "Unblock all" is asked with the count, removes exactly that source,
/// and refuses if the count changed after it was asked.
#[tokio::test]
async fn unblocking_all_from_a_source_removes_that_source_only() {
    let (app, password, _tmp, db_path) = app_with_db();
    seed_blocks(&db_path, 4);
    let (cookie, csrf) = login(&app, &password).await;

    let asked = page_html(&app, &cookie, "/blocks?source=web-scanners&confirm=1").await;
    assert!(
        asked.contains("Unblock all 2 rule(s) from Web scanners?"),
        "{asked}"
    );

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/blocks/unblock-all",
        "source=web-scanners&expected=3",
    )
    .await;
    assert!(flash.contains("Nothing was removed"), "{flash}");

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/blocks/unblock-all",
        "source=web-scanners&expected=2",
    )
    .await;
    assert!(
        flash.contains("Removed 2 rule(s) from Web scanners"),
        "{flash}"
    );
    let db = Db::open(&db_path).unwrap();
    let left: Vec<String> = db
        .list_firewall_rules()
        .unwrap()
        .into_iter()
        .map(|r| r.address)
        .collect();
    assert_eq!(left, ["10.0.0.0", "10.0.0.2"]);
    assert_eq!(db.unblocked_addresses().unwrap().len(), 2, "remembered");
}

#[tokio::test]
async fn unblocking_one_rule_returns_to_the_same_view() {
    let (app, password, _tmp, db_path) = app_with_db();
    seed_blocks(&db_path, 2);
    let (cookie, csrf) = login(&app, &password).await;
    let id = Db::open(&db_path).unwrap().list_firewall_rules().unwrap()[0].id;

    let response = app
        .clone()
        .oneshot(with_cookie(
            post(
                "/blocks/unblock",
                &format!("csrf={csrf}&id={id}&source=probe-paths&page=1"),
            ),
            &cookie,
        ))
        .await
        .unwrap();
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with("/blocks?page=1&source=probe-paths&flash="),
        "{location}"
    );
    assert_eq!(
        Db::open(&db_path)
            .unwrap()
            .list_firewall_rules()
            .unwrap()
            .len(),
        1
    );
}

/// Fifty thousand rules is what a busy host with every detector on can
/// hold. A page of them, the last page, a filtered page and a search all
/// read one page's worth, not the table.
#[tokio::test]
async fn the_blocks_page_stays_fast_with_fifty_thousand_rules() {
    let (app, password, _tmp, db_path) = app_with_db();
    seed_blocks(&db_path, 50_000);
    let (cookie, _csrf) = login(&app, &password).await;

    for path in [
        "/blocks",
        "/blocks?page=500",
        "/blocks?source=web-scanners&page=250",
        "/blocks?q=10.0.195.80",
    ] {
        let started = std::time::Instant::now();
        let html = page_html(&app, &cookie, path).await;
        let took = started.elapsed();
        assert!(took.as_millis() < 400, "{path} took {took:?}");
        assert!(
            html.matches("<tr>").count() <= 101,
            "{path} rendered more than a page"
        );
    }
    let last = page_html(&app, &cookie, "/blocks?page=500").await;
    assert!(
        last.contains("Rules 49901\u{2013}50000 of 50000"),
        "the last page"
    );
}

// ---- privilege: through the helper, and read-only ----

/// A console whose every privileged operation goes through a real helper
/// on a socket in this process, serving `HelperHost`'s tree. The console's
/// own idea of the NGINX root names nothing, and its own firewall path is
/// the default under `/etc`: whatever lands in the host's tree got there
/// through the helper, because nothing else knew where it is.
fn app_through_helper() -> (Router, String, common::HelperHost) {
    let host = common::HelperHost::new();
    let socket = host.serve();
    let password = stop_bots::web::auth::generate_password().unwrap();
    {
        let db = host.open_db();
        stop_bots::web::auth::set_password(&db, &password).unwrap();
        common::block_a_bot(&db, "BadBot");
    }
    let mut state = AppState::new(
        host.open_db(),
        std::path::PathBuf::from("/nonexistent/console-root"),
        Some(host.dir.path().join("auth.log")),
        false,
    );
    state.privilege = stop_bots::web::state::Privilege::Helper(socket);
    state.host_conf = host.host_conf.clone();
    (server::router(state), password, host)
}

#[tokio::test]
async fn through_the_helper_a_scan_finds_the_host_s_sites() {
    let (app, password, host) = app_through_helper();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/nginx/scan", "").await;

    assert!(flash.contains("Found 1 site(s)"), "was: {flash}");
    let sites = host.open_db().list_sites().unwrap();
    assert_eq!(sites[0].config_path, host.site.display().to_string());
}

#[tokio::test]
async fn through_the_helper_the_nginx_page_shows_each_site_s_status() {
    let (app, password, host) = app_through_helper();
    let (cookie, _) = login(&app, &password).await;
    host.open_db()
        .upsert_site("example.com", host.site.to_str().unwrap())
        .unwrap();

    let page = body_string(
        app.clone()
            .oneshot(with_cookie(get("/nginx"), &cookie))
            .await
            .unwrap(),
    )
    .await;

    assert!(page.contains("STALE"), "page was:\n{page}");
}

#[tokio::test]
async fn through_the_helper_applying_writes_the_host_s_sites() {
    let (app, password, host) = app_through_helper();
    let (cookie, csrf) = login(&app, &password).await;
    let id = {
        let db = host.open_db();
        db.upsert_site("example.com", host.site.to_str().unwrap())
            .unwrap();
        db.list_sites().unwrap()[0].id
    };

    let (_, flash) = act(&app, &cookie, &csrf, "/nginx/apply", &format!("id={id}")).await;
    assert!(flash.contains("Applied to example.com"), "was: {flash}");
    assert!(std::fs::read_to_string(&host.site)
        .unwrap()
        .contains("BadBot"));

    std::fs::write(&host.site, common::HELPER_SITE).unwrap();
    let (_, flash) = act(&app, &cookie, &csrf, "/nginx/apply-all", "").await;
    assert!(flash.contains("1 file(s) changed"), "was: {flash}");
    assert!(std::fs::read_to_string(&host.site)
        .unwrap()
        .contains("BadBot"));
}

#[tokio::test]
async fn through_the_helper_the_firewall_script_is_written_where_the_helper_says() {
    let (app, password, host) = app_through_helper();
    let (cookie, csrf) = login(&app, &password).await;

    let (_, flash) = act(&app, &cookie, &csrf, "/render-firewall", "backend=nftables").await;

    assert!(flash.contains("Wrote"), "was: {flash}");
    assert!(
        stop_bots::firewall::rendered_path(&host.firewall).exists(),
        "the helper's script was not written"
    );
}

#[tokio::test]
async fn through_the_helper_apply_everything_confirms_then_applies() {
    let (app, password, host) = app_through_helper();
    let (cookie, csrf) = login(&app, &password).await;
    host.open_db()
        .upsert_site("example.com", host.site.to_str().unwrap())
        .unwrap();

    let confirm = body_string(
        app.clone()
            .oneshot(with_cookie(get("/apply-all?diff=1"), &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        confirm.contains("NGINX: 1 file(s) would change"),
        "page was:\n{confirm}"
    );
    assert!(
        confirm.contains("BadBot"),
        "the diff is missing:\n{confirm}"
    );

    let (_, flash) = act(&app, &cookie, &csrf, "/apply-all", "").await;
    assert!(flash.contains("NGINX: 1 file(s) changed"), "was: {flash}");
    assert!(std::fs::read_to_string(&host.site)
        .unwrap()
        .contains("BadBot"));
    assert!(stop_bots::firewall::rendered_path(&host.firewall).exists());
}

#[tokio::test]
async fn through_the_helper_web_access_writes_the_host_s_site() {
    let (app, password, host) = app_through_helper();
    let (cookie, csrf) = login(&app, &password).await;
    host.open_db()
        .upsert_site("example.com", host.site.to_str().unwrap())
        .unwrap();

    let (_, flash) = act(
        &app,
        &cookie,
        &csrf,
        "/web-access",
        "mode=path&site=example.com&prefix=/stop-bots/&host=",
    )
    .await;

    assert!(flash.contains("Wrote"), "was: {flash}");
    assert!(std::fs::read_to_string(&host.site)
        .unwrap()
        .contains(r#"location "/stop-bots/""#));
}

/// The Help page says how this console changes the host.
#[tokio::test]
async fn the_help_page_names_the_helper() {
    let (app, password, _host) = app_through_helper();
    let (cookie, _) = login(&app, &password).await;

    let page = body_string(
        app.clone()
            .oneshot(with_cookie(get("/help"), &cookie))
            .await
            .unwrap(),
    )
    .await;

    assert!(
        page.contains("through the root helper at"),
        "page was:\n{page}"
    );
}

/// A console that is neither root nor given a helper: everything it shows,
/// it still shows, and every action that would change the host says what
/// it needs instead — and changes nothing.
#[tokio::test]
async fn without_root_or_a_helper_every_action_says_it_needs_the_helper() {
    let (app, password, tmp, db_path) = app_configured("", |state| {
        state.privilege = stop_bots::web::state::Privilege::ReadOnly;
    });
    let (cookie, csrf) = login(&app, &password).await;
    let site = tmp.path().join("nginx/sites-enabled/example.com");
    std::fs::write(&site, common::HELPER_SITE).unwrap();
    let id = {
        let db = Db::open(&db_path).unwrap();
        common::block_a_bot(&db, "BadBot");
        db.upsert_site("example.com", site.to_str().unwrap())
            .unwrap();
        db.list_sites().unwrap()[0].id
    };

    for (path, body) in [
        ("/nginx/scan", String::new()),
        ("/nginx/apply", format!("id={id}")),
        ("/nginx/apply-all", String::new()),
        ("/apply-all", String::new()),
        ("/render-firewall", "backend=nftables&apply=1".to_string()),
        (
            "/web-access",
            "mode=path&site=example.com&prefix=/stop-bots/&host=".to_string(),
        ),
    ] {
        let (_, flash) = act(&app, &cookie, &csrf, path, &body).await;
        assert!(flash.contains("needs the helper"), "{path} said: {flash}");
    }
    let confirm = app
        .clone()
        .oneshot(with_cookie(get("/apply-all"), &cookie))
        .await
        .unwrap();
    let location = confirm
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let flash = flash_shown(&app, &cookie, &location).await;
    assert!(
        flash.contains("needs the helper"),
        "the confirm page said: {flash}"
    );

    assert_eq!(
        std::fs::read_to_string(&site).unwrap(),
        common::HELPER_SITE,
        "a read-only console wrote a site"
    );
    assert!(!tmp.path().join("firewall.next.nft").exists());
}

/// And its views still work, each saying it is read-only.
#[tokio::test]
async fn without_root_or_a_helper_every_view_still_works_and_says_so() {
    let (app, password, _tmp, _db) = app_configured("", |state| {
        state.privilege = stop_bots::web::state::Privilege::ReadOnly;
    });
    let (cookie, _) = login(&app, &password).await;

    for path in ["/", "/bots", "/nginx", "/firewall", "/blocks", "/help"] {
        let response = app
            .clone()
            .oneshot(with_cookie(get(path), &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let page = body_string(response).await;
        assert!(page.contains("needs the helper"), "{path} did not say so");
    }
}
