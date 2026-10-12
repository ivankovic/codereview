//! End-to-end happy-path test for the db + nginx + bot-list CLI flow:
//! update-bot-lists -> scan-sites -> apply-blocks, run against a throwaway
//! copy of the NGINX fixtures. No network access is used.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
mod common;
use common::{
    copy_dir_all, path_with, scan_sites, seed_bots, stop_bots, stop_bots_bin, stop_bots_with_host,
};

// Every end-to-end test below drives the real binary against a throwaway
// database and NGINX root. The helpers here wrap the incantations that made
// up 20-40 lines of identical scaffolding in each of them.

/// A throwaway world for one end-to-end test: a database, an NGINX config
/// root, and the two directories the product writes its own generated
/// files into.
///
/// Every command run through this has the managed-directory overrides set,
/// which is a safety property and not just convenience: without them,
/// `apply-blocks` writes generated files under `/etc`. Today only
/// robots.txt and rate limiting do that and both default to off, so the
/// eight tests that never set the overrides happen to be harmless — one
/// setting away from not being. Making the fixture own them removes the
/// footgun rather than documenting it.
struct Fixture {
    // Held for its Drop: the directory is deleted when the fixture is.
    _tmp: tempfile::TempDir,
    db: std::path::PathBuf,
    nginx_root: std::path::PathBuf,
    /// `MANAGED_DIR` — where the generated robots.txt goes.
    managed: std::path::PathBuf,
    /// `conf.d` — where the generated rate-limit zone goes.
    conf_d: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let nginx_root = tmp.path().join("nginx");
        fs::create_dir_all(&nginx_root).unwrap();
        Fixture {
            db: tmp.path().join("db.sqlite3"),
            nginx_root,
            managed: tmp.path().join("managed"),
            conf_d: tmp.path().join("conf.d"),
            _tmp: tmp,
        }
    }

    /// Runs `stop-bots` with `args` plus `--db`, asserting success and
    /// returning the assertion so a caller can check stdout.
    fn run(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        self.cmd(args).assert().success()
    }

    /// The same, without asserting success — for the tests that expect a
    /// failure and check stderr.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = stop_bots_bin();
        cmd.env("STOP_BOTS_NGINX_DIR", &self.managed)
            .env("STOP_BOTS_NGINX_CONF_D", &self.conf_d)
            .env("STOP_BOTS_HOST_CONF", self.host_conf())
            .args(args)
            .args(["--db", self.db.to_str().unwrap()]);
        cmd
    }

    /// Seeds the bot list from the checked-in sample, so tests never touch
    /// the network.
    fn seed_bots(&self) {
        self.run(&[
            "update-bot-lists",
            "--source",
            "tests/fixtures/botlists/well-known-bots-sample.json",
        ]);
    }

    fn scan_sites(&self) -> assert_cmd::assert::Assert {
        self.run(&["scan-sites", "--root", self.nginx_root.to_str().unwrap()])
    }

    /// `--no-reload` throughout: these run on whatever machine hosts the
    /// test suite, and an apply without it shells out to the real
    /// `nginx -t` and `systemctl reload nginx`.
    fn apply_blocks(&self) -> assert_cmd::assert::Assert {
        self.run(&[
            "apply-blocks",
            "--root",
            self.nginx_root.to_str().unwrap(),
            "--no-reload",
        ])
    }

    /// One `server` block named `name`, in this fixture's config root.
    fn write_site(&self, name: &str) -> std::path::PathBuf {
        write_site(&self.nginx_root, name)
    }

    fn robots_txt(&self) -> std::path::PathBuf {
        self.managed.join("robots.txt")
    }

    /// This fixture's host settings file.
    fn host_conf(&self) -> std::path::PathBuf {
        self._tmp.path().join("host.conf")
    }

    fn rate_limit_conf(&self) -> std::path::PathBuf {
        self.conf_d.join("stop-bots-limits.conf")
    }
}

/// The rule in an nftables script that decides `element`, and its line
/// number: the `add rule` matching the set whose `add element` block lists
/// it, e.g. `(31, "ip saddr @allow_v4 accept")`. Lower lines are matched
/// first. `None` if no set lists `element`.
fn nft_rule_for(script: &str, element: &str) -> Option<(usize, String)> {
    let mut set = None;
    for line in script.lines() {
        if let Some(rest) = line.strip_prefix("add element inet stop_bots ") {
            set = rest.split(' ').next();
        } else if line.starts_with('\t') && line.trim().trim_end_matches(',') == element {
            let matcher = format!("@{} ", set?);
            return script.lines().enumerate().find_map(|(at, l)| {
                l.strip_prefix("add rule inet stop_bots bot_rules ")
                    .filter(|rule| rule.contains(&matcher))
                    .map(|rule| (at, rule.to_string()))
            });
        }
    }
    None
}

/// `--no-reload` throughout: these run on whatever machine hosts the test
/// suite, and an apply without it shells out to the real `nginx -t` and
/// `systemctl reload nginx`.
fn apply_blocks(db: &Path, root: &Path) -> assert_cmd::assert::Assert {
    stop_bots(&[
        "apply-blocks",
        "--root",
        root.to_str().unwrap(),
        "--db",
        db.to_str().unwrap(),
        "--no-reload",
    ])
}

/// One `server` block with `server_name`, written to `root/<name>.conf`.
fn write_site(root: &Path, name: &str) -> std::path::PathBuf {
    let path = root.join(format!("{name}.conf"));
    fs::write(
        &path,
        format!("server {{\n    listen 80;\n    server_name {name};\n}}\n"),
    )
    .unwrap();
    path
}

/// One NGINX combined-format access-log line.
fn access_line(ip: &str, path: &str, status: u16, ua: &str) -> String {
    format!(
        "{ip} - - [10/Jul/2026:12:00:00 +0000] \"GET {path} HTTP/1.1\" {status} 0 \"-\" \"{ua}\"\n"
    )
}

/// Creates fake `nginx`, `systemctl` and `nft` executables in a directory
/// meant to be prepended to a child's PATH, so tests can exercise the real
/// reload and apply code paths — the process spawn, the argument building,
/// the exit-code handling, the ordering — on a machine where none of those
/// tools exist.
///
/// This is a fake, not a mock, in the sense that matters: nothing in the
/// product knows it's under test. The product resolves a command name
/// through PATH and interprets an exit code, exactly as in production; only
/// the binary found is ours. Each fake appends its name and arguments to
/// `calls.log` (returned) and exits 0 — unless a file named `fail-<tool>`
/// exists next to it, in which case it prints that file's contents to
/// stderr and exits 1, which is how a test stages e.g. a failing
/// `nginx -t`.
fn fake_tools(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let bin = dir.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    let log = dir.join("calls.log");
    for tool in ["nginx", "systemctl", "nft", "docker"] {
        let path = bin.join(tool);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n\
                 echo \"{tool} $@\" >> \"{log}\"\n\
                 if [ -f \"{dir}/fail-{tool}\" ]; then cat \"{dir}/fail-{tool}\" >&2; exit 1; fi\n\
                 exit 0\n",
                log = log.display(),
                dir = dir.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (bin, log)
}

#[test]
fn update_scan_and_apply_blocks_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let nginx_root = tmp.path().join("nginx");
    copy_dir_all(Path::new("tests/fixtures/nginx"), &nginx_root);
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_bin()
        .args([
            "update-bot-lists",
            "--db",
            db_path.to_str().unwrap(),
            "--source",
            "tests/fixtures/botlists/well-known-bots-sample.json",
        ])
        .assert()
        .success()
        // Five of the sample's seven bots: one has no pattern, and the two
        // quoted patterns (one bot's only one) are left out, and said to be.
        .stdout(predicate::str::contains("Stored 5 bot(s)"))
        .stdout(predicate::str::contains("It left out 2 pattern(s)"));

    scan_sites(&db_path, &nginx_root).stdout(predicate::str::contains("Discovered 2 site(s)"));

    // `--no-reload`: this test's "nginx root" is a throwaway temp dir, not
    // the real system config, so there's nothing for a real `nginx -t` /
    // `systemctl reload nginx` to validate — and the CLI would otherwise
    // reload the machine's actual NGINX (if any) as a side effect of
    // running this test suite.
    stop_bots_bin()
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--no-reload",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 file(s) changed"));

    let example_com = fs::read_to_string(nginx_root.join("sites-enabled/example.com")).unwrap();
    assert!(
        example_com.contains("# BEGIN stop-bots"),
        "example_com was:\n{example_com}"
    );
    assert!(
        example_com.contains("AISearchBot"),
        "example_com was:\n{example_com}"
    );
    // The search-engine and unknown-category bots default to allowed, so
    // only the AI bot's pattern should show up in the generated rule.
    assert!(
        !example_com.contains("Googlebot"),
        "example_com was:\n{example_com}"
    );

    // Re-running apply-blocks should be a no-op.
    stop_bots_bin()
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--no-reload",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) changed"));
}

/// Regression test for a real bug caught in design review: `sites` has
/// `UNIQUE(server_name, config_path)`, so the *same* `server_name` can
/// legitimately appear in two different files (e.g. a stale config left
/// behind after a rename). A per-site override must stay scoped to the
/// specific file its site row came from — building one flat
/// name-to-patterns map for the whole `apply-blocks` run and reusing it
/// across every file would let one site's override leak onto the other's
/// same-named block in a different file.
#[test]
fn apply_blocks_scopes_a_site_override_to_its_own_file_even_with_a_shared_server_name() {
    let tmp = tempfile::tempdir().unwrap();
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    // Two different files, both declaring the same server_name.
    let file_a = nginx_root.join("a.conf");
    let file_b = nginx_root.join("b.conf");
    fs::write(
        &file_a,
        "server {\n    listen 80;\n    server_name shared.example;\n    root /var/www/a;\n}\n",
    )
    .unwrap();
    fs::write(
        &file_b,
        "server {\n    listen 80;\n    server_name shared.example;\n    root /var/www/b;\n}\n",
    )
    .unwrap();

    seed_bots(&db_path);

    scan_sites(&db_path, &nginx_root).stdout(predicate::str::contains("Discovered 2 site(s)"));

    // Override the Search category to Blocked, but only for the site row
    // whose config_path is file_a.
    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        let site_a = db
            .list_sites()
            .unwrap()
            .into_iter()
            .find(|s| Path::new(&s.config_path) == file_a)
            .unwrap();
        db.set_site_category_override(
            site_a.id,
            stop_bots::db::Category::Search,
            Some(stop_bots::db::Policy::Blocked),
        )
        .unwrap();
    }

    apply_blocks(&db_path, &nginx_root);

    let a = fs::read_to_string(&file_a).unwrap();
    let b = fs::read_to_string(&file_b).unwrap();
    // file_a's site overrides Search to Blocked: the search-engine bot's
    // pattern should show up there.
    assert!(a.contains("Googlebot"), "a was:\n{a}");
    // file_b's same-named site has no override of its own and must not
    // pick up file_a's — it follows the global default (Search allowed).
    // It still gets a block, just for the AI bot (blocked by default),
    // not the search-engine one.
    assert!(!b.contains("Googlebot"), "b was:\n{b}");
    assert!(b.contains("AISearchBot"), "b was:\n{b}");
}

/// The block-response setting end to end: change it, apply, and confirm
/// the generated config carries the new code — the whole point being that
/// setting it alone changes nothing on disk until `apply-blocks` runs.
#[test]
fn set_block_response_changes_the_generated_status_code_on_the_next_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    let site = write_site(&nginx_root, "a.example");

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    let apply = |db_path: &Path, root: &Path| {
        apply_blocks(db_path, root);
    };

    apply(&db_path, &nginx_root);
    assert!(fs::read_to_string(&site).unwrap().contains("return 403;"));

    stop_bots_bin()
        .args([
            "set-block-response",
            "--db",
            db_path.to_str().unwrap(),
            "--response",
            "close",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("apply-blocks"));

    // Still 403 on disk: setting it is not applying it.
    assert!(fs::read_to_string(&site).unwrap().contains("return 403;"));

    apply(&db_path, &nginx_root);
    let written = fs::read_to_string(&site).unwrap();
    assert!(written.contains("return 444;"), "written was:\n{written}");
    assert!(!written.contains("return 403;"), "written was:\n{written}");
}

/// Spoofed-crawler detection end to end, including the property that
/// matters most: it does nothing at all until crawler ranges are fetched.
#[test]
fn block_spoofed_crawlers_is_inert_without_ranges_then_blocks_a_forged_googlebot() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let access_log = tmp.path().join("access.log");

    let line = |ip: &str, ua: &str| access_line(ip, "/", 200, ua);
    fs::write(
        &access_log,
        line("203.0.113.9", "Mozilla/5.0 (compatible; Googlebot/2.1)")
            + &line("66.249.66.1", "Mozilla/5.0 (compatible; Googlebot/2.1)"),
    )
    .unwrap();

    let run = |db_path: &Path, log: &Path| {
        stop_bots_bin()
            .args([
                "block-spoofed-crawlers",
                "--db",
                db_path.to_str().unwrap(),
                "--access-log",
                log.to_str().unwrap(),
            ])
            .assert()
            .success()
    };

    // No ranges stored yet: says so explicitly rather than reporting a
    // clean log, and blocks nothing.
    run(&db_path, &access_log).stdout(predicate::str::contains("No crawler IP ranges fetched yet"));
    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        assert!(db.list_firewall_rules().unwrap().is_empty());
    }

    // Seed Googlebot's published ranges the way `update-ip-ranges` would.
    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        db.register_ip_range_source(&stop_bots::db::IpRangeSource {
            id: "googlebot".to_string(),
            name: "Googlebot IP ranges".to_string(),
            url: "https://example.invalid/googlebot.json".to_string(),
            category: stop_bots::db::Category::Search,
            last_fetched_at: None,
            range_count: 0,
        })
        .unwrap();
        db.replace_ip_ranges("googlebot", &["66.249.64.0/19".to_string()])
            .unwrap();
    }

    run(&db_path, &access_log).stdout(predicate::str::contains("203.0.113.9"));

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    let rules = db.list_firewall_rules().unwrap();
    assert_eq!(rules.len(), 1, "rules were: {rules:?}");
    // The forged claim is blocked; the address Google actually publishes
    // is not.
    assert_eq!(rules[0].address, "203.0.113.9");
    assert!(rules[0].expires_at.is_some());
}

/// Probe-path detection end to end, including the property the built-in
/// list is chosen for: a legitimate admin path must survive it.
#[test]
fn block_probe_paths_blocks_a_dotenv_probe_but_not_a_wordpress_login() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let access_log = tmp.path().join("access.log");

    let line = |ip: &str, path: &str| access_line(ip, path, 404, "curl/8");
    fs::write(
        &access_log,
        line("203.0.113.9", "/.env") + &line("198.51.100.2", "/wp-login.php"),
    )
    .unwrap();

    stop_bots_bin()
        .args([
            "block-probe-paths",
            "--db",
            db_path.to_str().unwrap(),
            "--access-log",
            access_log.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("203.0.113.9"));

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    let rules = db.list_firewall_rules().unwrap();
    assert_eq!(rules.len(), 1, "rules were: {rules:?}");
    assert_eq!(rules[0].address, "203.0.113.9");

    // The site's own administrator signing in is not a probe.
    assert!(!rules.iter().any(|r| r.address == "198.51.100.2"));
}

/// Extra probe paths are configurable, and entries that could never match
/// are reported rather than silently dropped.
#[test]
fn set_probe_paths_adds_extras_and_reports_unanchored_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let access_log = tmp.path().join("access.log");
    fs::write(
        &access_log,
        access_line("203.0.113.9", "/internal/dump", 200, "curl/8"),
    )
    .unwrap();

    stop_bots_bin()
        .args([
            "set-probe-paths",
            "--db",
            db_path.to_str().unwrap(),
            "--paths",
            "/internal\nnot-anchored",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 accepted"))
        .stdout(predicate::str::contains("Ignored 1"));

    stop_bots_bin()
        .args(["list-probe-paths", "--db", db_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("/.env"))
        .stdout(predicate::str::contains("/internal"));

    stop_bots_bin()
        .args([
            "block-probe-paths",
            "--db",
            db_path.to_str().unwrap(),
            "--access-log",
            access_log.to_str().unwrap(),
        ])
        .assert()
        .success();

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    assert_eq!(db.list_firewall_rules().unwrap()[0].address, "203.0.113.9");
}

/// The honeypot end to end, including its guard against a path that could
/// never match.
#[test]
fn honeypot_path_is_configurable_and_blocks_whatever_fetches_it() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let access_log = tmp.path().join("access.log");
    fs::write(
        &access_log,
        access_line("203.0.113.9", "/trap-me/", 404, "curl/8"),
    )
    .unwrap();

    // A path with no leading slash can never match, so it's rejected
    // rather than stored.
    stop_bots_bin()
        .args([
            "set-honeypot-path",
            "--db",
            db_path.to_str().unwrap(),
            "--path",
            "trap-me",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must start with"));

    stop_bots_bin()
        .args([
            "set-honeypot-path",
            "--db",
            db_path.to_str().unwrap(),
            "--path",
            "/trap-me/",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("published"));

    stop_bots_bin()
        .args([
            "block-honeypot",
            "--db",
            db_path.to_str().unwrap(),
            "--access-log",
            access_log.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("203.0.113.9"));

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    let rules = db.list_firewall_rules().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].address, "203.0.113.9");
}

/// Reputation feeds end to end, without touching the network: seed ranges
/// directly, then confirm the on/off switch is what decides whether they
/// reach the rendered firewall script.
#[test]
fn a_reputation_feed_only_reaches_the_firewall_script_once_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let out = tmp.path().join("firewall.nft");

    stop_bots_bin()
        .args(["list-reputation-sources", "--db", db_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("firehol-level1"))
        .stdout(predicate::str::contains("[OFF]"));

    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        db.replace_reputation_ranges("tor-exits", &["198.51.100.7".to_string()])
            .unwrap();
    }

    let render = |db_path: &Path, out: &Path| {
        stop_bots_bin()
            .args([
                "render-firewall",
                "--backend",
                "nftables",
                "--out",
                out.to_str().unwrap(),
                "--db",
                db_path.to_str().unwrap(),
                "--ssh-log",
                "/nonexistent/auth.log",
            ])
            .assert()
            .success();
    };

    // Fetched but off: the range must not be in the script.
    render(&db_path, &out);
    assert!(!fs::read_to_string(&out).unwrap().contains("198.51.100.7"));

    stop_bots_bin()
        .args([
            "set-reputation-source",
            "--db",
            db_path.to_str().unwrap(),
            "--source-id",
            "tor-exits",
            "--enabled",
            "true",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("render-firewall"));

    render(&db_path, &out);
    assert!(fs::read_to_string(&out).unwrap().contains("198.51.100.7"));

    // And switching it back off removes it again, without losing the data.
    stop_bots_bin()
        .args([
            "set-reputation-source",
            "--db",
            db_path.to_str().unwrap(),
            "--source-id",
            "tor-exits",
            "--enabled",
            "false",
        ])
        .assert()
        .success();
    render(&db_path, &out);
    assert!(!fs::read_to_string(&out).unwrap().contains("198.51.100.7"));

    stop_bots_bin()
        .args(["list-reputation-sources", "--db", db_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 range(s)"));
}

/// Enabling a cloud-provider feed must say what it actually does.
#[test]
fn enabling_a_provider_feed_warns_and_flags_that_nothing_is_fetched_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_bin()
        .args([
            "set-reputation-source",
            "--db",
            db_path.to_str().unwrap(),
            "--source-id",
            "aws",
            "--enabled",
            "true",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("not just bots"))
        .stdout(predicate::str::contains("No ranges stored yet"));
}

#[test]
fn an_unknown_reputation_source_is_rejected_with_the_known_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_bin()
        .args([
            "set-reputation-source",
            "--db",
            db_path.to_str().unwrap(),
            "--source-id",
            "not-a-feed",
            "--enabled",
            "true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("firehol-level1"));
}

/// What the detectors will make allowances for is said when the site is
/// found, before it matters.
#[test]
fn scan_sites_says_which_application_runs_behind_a_site() {
    let fx = Fixture::new();
    fs::write(
        fx.nginx_root.join("cloud.example"),
        "server {\n    listen 443 ssl;\n    server_name cloud.example;\n    \
         location / {\n        proxy_pass http://nextcloud:80;\n    }\n}\n",
    )
    .unwrap();
    fx.write_site("blog.example");

    let out = String::from_utf8(fx.scan_sites().get_output().stdout.clone()).unwrap();

    assert!(
        out.contains("cloud.example runs Nextcloud"),
        "out was:\n{out}"
    );
    assert!(!out.contains("blog.example runs"), "out was:\n{out}");
}

/// robots.txt generation end to end: the file is written, the site config
/// aliases it, and disabling removes both.
#[test]
fn robots_txt_is_generated_aliased_and_then_removed_again() {
    let fx = Fixture::new();
    let site = fx.write_site("a.example");
    fx.seed_bots();
    fx.scan_sites();

    // Off by default: no location block, no file.
    fx.apply_blocks();
    assert!(!fs::read_to_string(&site).unwrap().contains("/robots.txt"));
    assert!(!fx.robots_txt().exists());

    fx.run(&["set-robots-txt", "--enabled", "true"])
        .stdout(predicate::str::contains("replaces"));
    fx.run(&["show-robots-txt"])
        .stdout(predicate::str::contains("User-agent:"))
        // The honeypot trap path is always published.
        .stdout(predicate::str::contains("stop-bots-trap"));

    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert!(
        written.contains("location = /robots.txt"),
        "written was:\n{written}"
    );
    // The alias points at the file that was actually written.
    let robots = fx.robots_txt();
    assert!(robots.exists(), "robots.txt should have been written");
    assert!(
        written.contains(robots.to_str().unwrap()),
        "written was:\n{written}"
    );
    assert!(fs::read_to_string(&robots).unwrap().contains("User-agent:"));

    // Disabling removes the directive *and* the generated file — a stale
    // generated artifact left on disk invites being wired back up by hand.
    fx.run(&["set-robots-txt", "--enabled", "false"]);
    fx.apply_blocks();
    assert!(!fs::read_to_string(&site).unwrap().contains("/robots.txt"));
    assert!(!robots.exists(), "the generated file should be removed");
}

/// Rate limiting end to end, and specifically the ordering that keeps the
/// config valid: the http-context zone file must exist while any server
/// block references it, and must only be deleted once none does.
#[test]
fn rate_limit_writes_the_zone_file_and_removes_it_only_after_the_directive_goes() {
    let fx = Fixture::new();
    let site = fx.write_site("a.example");
    fx.scan_sites();

    fx.run(&[
        "set-rate-limit",
        "--enabled",
        "true",
        "--rps",
        "7",
        "--burst",
        "14",
    ])
    .stdout(predicate::str::contains("7 req/s"));

    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert!(
        written.contains("limit_req zone=stop_bots burst=14 nodelay;"),
        "written was:\n{written}"
    );
    assert!(
        written.contains("limit_req_status 429;"),
        "written was:\n{written}"
    );

    let zone = fs::read_to_string(fx.rate_limit_conf()).expect("zone file should exist");
    assert!(zone.contains("rate=7r/s"), "zone was:\n{zone}");
    // The zone the server block references is the zone that was defined.
    assert!(zone.contains("zone=stop_bots:"), "zone was:\n{zone}");

    fx.run(&["set-rate-limit", "--enabled", "false"]);

    // Still present until an apply actually rewrites the config — deleting
    // it while the directive is live would make nginx -t fail outright.
    assert!(fx.rate_limit_conf().exists());

    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert!(!written.contains("limit_req"), "written was:\n{written}");
    assert!(
        !fx.rate_limit_conf().exists(),
        "zone file should be removed after apply"
    );

    // Parameters persist across a disable, so re-enabling doesn't silently
    // revert to defaults.
    fx.run(&["set-rate-limit", "--enabled", "true"])
        .stdout(predicate::str::contains("7 req/s"));
}

/// Trust end to end: an address reaches the firewall script as an accept
/// ahead of every drop and the NGINX trust file as a `geo` entry; a user
/// agent reaches the trust file only; the block clears for both; and
/// taking both away removes the file once nothing references it.
#[test]
fn trust_reaches_the_firewall_script_first_and_nginx_through_the_trust_file() {
    let fx = Fixture::new();
    fx.seed_bots();
    let site = fx.write_site("a.example");
    fx.scan_sites();
    let trust_conf = fx.conf_d.join("stop-bots-trusted.conf");

    // Typed with host bits set; stored, and echoed, as the network.
    fx.run(&["trust", "--address", "203.0.113.77/24"])
        .stdout(predicate::str::contains("Trusting 203.0.113.0/24"));
    fx.run(&["trust", "--user-agent", "UptimeRobot"])
        .stdout(predicate::str::contains("still judge it by its address"));
    fx.run(&["add-firewall-rule", "--address", "203.0.113.9"]);
    fx.run(&["list-trusted"])
        .stdout(predicate::str::contains("address     203.0.113.0/24"))
        .stdout(predicate::str::contains("user agent  UptimeRobot"));

    let script_path = fx._tmp.path().join("firewall.nft");
    let empty_log = fx._tmp.path().join("auth.log");
    fs::write(&empty_log, "").unwrap();
    fx.run(&[
        "render-firewall",
        "--backend",
        "nftables",
        "--out",
        script_path.to_str().unwrap(),
        "--ssh-log",
        empty_log.to_str().unwrap(),
    ]);
    let script = fs::read_to_string(&script_path).unwrap();
    let (accept, rule) = nft_rule_for(&script, "203.0.113.0/24")
        .unwrap_or_else(|| panic!("no accept for the trusted range:\n{script}"));
    assert!(
        rule.ends_with(" accept"),
        "the trusted range is in {rule:?}"
    );
    let (drop, rule) = nft_rule_for(&script, "203.0.113.9")
        .unwrap_or_else(|| panic!("the block rule went missing:\n{script}"));
    assert!(rule.ends_with(" drop"), "the block is in {rule:?}");
    assert!(accept < drop, "the accept must come first:\n{script}");

    fx.apply_blocks();
    let conf = fs::read_to_string(&trust_conf).expect("the trust file should exist");
    assert!(conf.contains("    203.0.113.0/24 1;"), "conf was:\n{conf}");
    assert!(conf.contains("\"~*UptimeRobot\" 1;"), "conf was:\n{conf}");
    let written = fs::read_to_string(&site).unwrap();
    assert!(
        written.contains("if ($stop_bots_trusted) {"),
        "the site block does not clear for trusted clients:\n{written}"
    );

    fx.run(&["trust", "--remove", "--address", "203.0.113.0/24"]);
    fx.run(&["trust", "--remove", "--user-agent", "UptimeRobot"]);
    // Still there until the block that reads it is rewritten: deleting it
    // first would make `nginx -t` fail on an unknown variable.
    assert!(trust_conf.exists());
    fx.apply_blocks();
    assert!(
        !trust_conf.exists(),
        "the trust file outlived its last entry"
    );
    assert!(!fs::read_to_string(&site)
        .unwrap()
        .contains("stop_bots_trusted"));
}

/// Trusting a second user agent changes the trust file and no site's
/// block. An apply that counted only site files decided nothing changed
/// and never reloaded NGINX, so the new entry never took effect.
/// A config the test rejects is put back exactly as it was, and a
/// generated file the apply created is removed again. Left in place, the
/// running NGINX would carry on serving the old config from memory and the
/// broken one would surface at the next restart — every site at once.
#[test]
fn a_config_that_fails_the_test_is_put_back_byte_for_byte() {
    let fx = Fixture::new();
    fx.seed_bots();
    let site = fx.write_site("a.example");
    fx.scan_sites();
    fx.run(&["set-robots-txt", "--enabled", "true"]);
    fx.run(&["set-rate-limit", "--enabled", "true"]);
    let before = fs::read(&site).unwrap();
    let (bin, calls) = fake_tools(fx._tmp.path());
    fs::write(
        fx._tmp.path().join("fail-nginx"),
        "nginx: [emerg] unknown directive\n",
    )
    .unwrap();

    fx.cmd(&["apply-blocks", "--root", fx.nginx_root.to_str().unwrap()])
        .env("PATH", path_with(&bin))
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown directive"))
        .stderr(predicate::str::contains("put back"));

    assert_eq!(fs::read(&site).unwrap(), before, "the site file changed");
    assert!(!fx.robots_txt().exists(), "the new robots.txt was left");
    assert!(!fx.rate_limit_conf().exists(), "the new zone was left");
    let log = fs::read_to_string(&calls).unwrap();
    assert!(!log.contains("systemctl"), "log was: {log}");
}

/// The first thing a host with NGINX in a container meets: no `nginx` on
/// PATH. The error names the verb that fixes it.
#[test]
fn nginx_missing_from_path_points_at_set_nginx_commands() {
    let fx = Fixture::new();
    fx.seed_bots();
    fx.write_site("a.example");
    fx.scan_sites();
    let empty = fx._tmp.path().join("empty-bin");
    fs::create_dir_all(&empty).unwrap();

    fx.cmd(&["apply-blocks", "--root", fx.nginx_root.to_str().unwrap()])
        .env("PATH", &empty)
        .assert()
        .failure()
        .stderr(predicate::str::contains("`nginx` isn't on PATH"))
        .stderr(predicate::str::contains(
            "see `stop-bots set-nginx-commands --help`",
        ));
}

/// A config root that is not there names both ways to point at the
/// right one.
#[test]
fn a_missing_nginx_root_says_how_to_set_it() {
    let fx = Fixture::new();
    let missing = fx._tmp.path().join("no-such-nginx");

    fx.cmd(&["scan-sites", "--root", missing.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"))
        .stderr(predicate::str::contains(
            "stop-bots set-nginx-commands --root <dir>",
        ));
}

/// A user without write access to the site files is told to use sudo,
/// after the error that says which file it was.
#[test]
fn a_site_file_this_user_cannot_write_says_to_use_sudo() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return; // root is never denied, so there is nothing to see
    }
    let fx = Fixture::new();
    fx.seed_bots();
    let site = fx.write_site("a.example");
    fx.scan_sites();
    fs::set_permissions(&site, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&fx.nginx_root, fs::Permissions::from_mode(0o555)).unwrap();

    let assert = fx
        .cmd(&[
            "apply-blocks",
            "--root",
            fx.nginx_root.to_str().unwrap(),
            "--no-reload",
        ])
        .assert();
    fs::set_permissions(&fx.nginx_root, fs::Permissions::from_mode(0o755)).unwrap();

    assert
        .failure()
        .stderr(predicate::str::contains("a.example.conf"))
        .stderr(predicate::str::contains(
            "This needs root. Run it with sudo.",
        ));
}

#[test]
fn a_change_to_the_trust_file_alone_still_reloads_nginx() {
    let fx = Fixture::new();
    fx.seed_bots();
    fx.write_site("a.example");
    fx.scan_sites();
    let (bin, calls) = fake_tools(fx._tmp.path());
    let apply = |fx: &Fixture| {
        fx.cmd(&["apply-blocks", "--root", fx.nginx_root.to_str().unwrap()])
            .env("PATH", path_with(&bin))
            .assert()
            .success()
    };

    fx.run(&["trust", "--user-agent", "UptimeRobot"]);
    apply(&fx);
    fx.run(&["trust", "--user-agent", "Pingdom"]);
    apply(&fx).stdout(predicate::str::contains("Reloaded NGINX"));

    let log = fs::read_to_string(&calls).unwrap();
    assert_eq!(
        log.lines()
            .filter(|l| l.starts_with("systemctl reload"))
            .count(),
        2,
        "the second apply did not reload; calls were:\n{log}"
    );
}

#[test]
fn trust_refuses_what_would_trust_more_than_it_says() {
    let fx = Fixture::new();
    for (args, why) in [
        (vec!["trust", "--address", "0.0.0.0/0"], "every address"),
        (vec!["trust", "--user-agent", " "], "every client"),
        (
            vec!["trust", "--remove", "--address", "203.0.113.7"],
            "is not trusted",
        ),
    ] {
        fx.cmd(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(why));
    }
    fx.run(&["list-trusted"])
        .stdout(predicate::str::contains("Nothing is trusted."));
}

/// The mirror of trust's `/0` refusal: blocking every address is the host
/// off the network, not a rule.
#[test]
fn add_firewall_rule_refuses_a_block_of_every_address() {
    let fx = Fixture::new();
    for address in ["0.0.0.0/0", "::/0", " 0.0.0.0/0"] {
        fx.cmd(&["add-firewall-rule", "--address", address])
            .assert()
            .failure()
            .stderr(predicate::str::contains("every address"));
    }
    fx.run(&["list-firewall-rules"])
        .stdout(predicate::str::contains("No firewall rules stored."));

    // Closing one port to everyone is an ordinary rule.
    fx.run(&[
        "add-firewall-rule",
        "--address",
        "0.0.0.0/0",
        "--port",
        "23",
    ]);
}

/// Per-site path exemptions end to end: the generated block switches to
/// the flag form, and only the exempted path escapes the rule.
#[test]
fn a_site_path_exemption_switches_the_block_to_the_flag_form() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();

    let site_file = write_site(&nginx_root, "a.example");

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    let apply = || {
        apply_blocks(&db_path, &nginx_root);
    };

    // Without exemptions: the original direct-return shape.
    apply();
    let plain = fs::read_to_string(&site_file).unwrap();
    assert!(plain.contains("return 403;"), "plain was:\n{plain}");
    assert!(!plain.contains("$stop_bots_block"), "plain was:\n{plain}");

    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        let site = db.list_sites().unwrap().into_iter().next().unwrap();
        db.add_site_path_exemption(site.id, "/blog").unwrap();
    }

    apply();
    let exempted = fs::read_to_string(&site_file).unwrap();
    assert!(
        exempted.contains("set $stop_bots_block 0;"),
        "exempted was:\n{exempted}"
    );
    assert!(
        exempted.contains("set $stop_bots_block 1;"),
        "exempted was:\n{exempted}"
    );
    assert!(
        exempted.contains("if ($uri ~* \"^(/blog)\")"),
        "exempted was:\n{exempted}"
    );
    assert!(
        exempted.contains("if ($stop_bots_block) {"),
        "exempted was:\n{exempted}"
    );
    // Exactly one sentinel block still, not a second appended.
    assert_eq!(exempted.matches("# BEGIN stop-bots").count(), 1);
}

/// An exemption scoped to one user agent, end to end through the CLI: it
/// reaches the site's block as a clear of its own, beside the bot rule it
/// narrows, and `--remove` takes it back out.
#[test]
fn exempt_path_with_a_user_agent_scopes_the_clear_to_that_client() {
    let fx = Fixture::new();
    let site_file = fx.write_site("a.example");
    fx.seed_bots();
    fx.scan_sites();
    let exempt = |extra: &[&str]| {
        let mut args = vec![
            "exempt-path",
            "--site",
            "a.example",
            "--path",
            "/remote.php/dav/",
            "--user-agent",
            "okhttp",
        ];
        args.extend_from_slice(extra);
        fx.run(&args)
    };

    exempt(&[]).stdout(predicate::str::contains("contains \"okhttp\""));
    fx.apply_blocks();
    let exempted = fs::read_to_string(&site_file).unwrap();
    for expected in [
        "if ($http_user_agent ~* \"okhttp\") {",
        "set $stop_bots_exempt $uri;",
        "if ($stop_bots_exempt ~* \"^(/remote\\.php/dav/)\") {",
    ] {
        assert!(
            exempted.contains(expected),
            "missing {expected}; the site was:\n{exempted}"
        );
    }

    exempt(&["--remove"]);
    fx.apply_blocks();
    let removed = fs::read_to_string(&site_file).unwrap();
    assert!(
        !removed.contains("stop_bots_exempt"),
        "the site was:\n{removed}"
    );
}

#[test]
fn exempt_path_refuses_a_user_agent_that_would_match_more_than_it_says() {
    let fx = Fixture::new();
    fx.write_site("a.example");
    fx.scan_sites();
    let base = ["exempt-path", "--site", "a.example", "--path", "/dav/"];
    for (extra, why) in [
        (vec!["--user-agent", " "], "every client"),
        (vec!["--user-agent", "ok\"http"], "cannot contain"),
        (
            vec!["--user-agent", "okhttp", "--remove"],
            "was not exempt for",
        ),
    ] {
        let args: Vec<&str> = base.iter().copied().chain(extra).collect();
        fx.cmd(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(why));
    }
}

/// The reload path end to end, with no real NGINX anywhere: apply-blocks
/// without --no-reload must run `nginx -t` and then `systemctl reload
/// nginx`, in that order — validation before reload is the property, since
/// reloading an invalid config is how every site goes down at once.
#[test]
fn apply_blocks_reloads_nginx_after_validating_the_config() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    write_site(&nginx_root, "a.example");
    let (bin, calls) = fake_tools(tmp.path());

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    stop_bots_bin()
        .env("PATH", path_with(&bin))
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Reloaded NGINX"));

    let log = fs::read_to_string(&calls).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], "nginx -t", "validate first; log was: {log}");
    assert_eq!(lines[1], "systemctl reload nginx");
    assert_eq!(lines.len(), 2, "no extra tool invocations; log was: {log}");
}

/// A config that fails validation stops the reload cold: `systemctl` must
/// never run, and the error the admin sees is nginx's own message rather
/// than a pointer at systemctl status.
#[test]
fn a_failing_nginx_config_check_stops_the_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    write_site(&nginx_root, "a.example");
    let (bin, calls) = fake_tools(tmp.path());
    fs::write(
        tmp.path().join("fail-nginx"),
        "nginx: [emerg] unexpected end of file\n",
    )
    .unwrap();

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    stop_bots_bin()
        .env("PATH", path_with(&bin))
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected end of file"));

    let log = fs::read_to_string(&calls).unwrap();
    assert!(
        !log.contains("systemctl"),
        "an invalid config must never be reloaded; log was: {log}"
    );
}

/// The containerised case, end to end: with the test and reload commands
/// pointed at `docker exec`, an apply must drive *those* and must never
/// fall back to the host's `nginx` or `systemctl` — a host with NGINX in a
/// container generally has neither, and silently reloading the wrong one
/// is worse than failing.
#[test]
fn a_configured_reload_command_replaces_systemctl_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    let host = tmp.path().join("host.conf");
    let db_path = tmp.path().join("db.sqlite3");
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    write_site(&nginx_root, "a.example");
    let (bin, calls) = fake_tools(tmp.path());

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    stop_bots_with_host(
        &host,
        &[
            "set-nginx-commands",
            "--db",
            db_path.to_str().unwrap(),
            "--test",
            "docker exec web nginx -t",
            "--reload",
            "docker exec web nginx -s reload",
        ],
    );

    stop_bots_bin()
        .env("STOP_BOTS_HOST_CONF", &host)
        .env("PATH", path_with(&bin))
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let log = fs::read_to_string(&calls).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines,
        [
            "docker exec web nginx -t",
            "docker exec web nginx -s reload"
        ],
        "validate first, then reload, both through docker; log was: {log}"
    );
}

/// A configured test command that fails still stops the reload. The guard
/// is the property, not the binary it happens to run.
#[test]
fn a_configured_test_command_that_fails_stops_the_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let host = tmp.path().join("host.conf");
    let db_path = tmp.path().join("db.sqlite3");
    let nginx_root = tmp.path().join("nginx");
    fs::create_dir_all(&nginx_root).unwrap();
    write_site(&nginx_root, "a.example");
    let (bin, calls) = fake_tools(tmp.path());
    fs::write(
        tmp.path().join("fail-docker"),
        "nginx: [emerg] unknown directive\n",
    )
    .unwrap();

    seed_bots(&db_path);
    scan_sites(&db_path, &nginx_root);

    stop_bots_with_host(
        &host,
        &[
            "set-nginx-commands",
            "--db",
            db_path.to_str().unwrap(),
            "--test",
            "docker exec web nginx -t",
            "--reload",
            "docker exec web nginx -s reload",
        ],
    );

    stop_bots_bin()
        .env("STOP_BOTS_HOST_CONF", &host)
        .env("PATH", path_with(&bin))
        .args([
            "apply-blocks",
            "--root",
            nginx_root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown directive"));

    let log = fs::read_to_string(&calls).unwrap();
    assert_eq!(
        log.lines().count(),
        1,
        "the reload must not run after a failed check; log was: {log}"
    );
}

/// Setting only one of the two leaves the other at its default, and the
/// command is echoed back so `set-nginx-commands` with no flags is a way
/// to ask what is in effect.
#[test]
fn set_nginx_commands_reports_both_and_defaults_the_unset_one() {
    let tmp = tempfile::tempdir().unwrap();
    let host = tmp.path().join("host.conf");
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_with_host(
        &host,
        &[
            "set-nginx-commands",
            "--db",
            db_path.to_str().unwrap(),
            "--reload",
            "docker exec web nginx -s reload",
        ],
    )
    .stdout(predicate::str::contains("Test command:   nginx -t"))
    .stdout(predicate::str::contains(
        "Reload command: docker exec web nginx -s reload",
    ));

    stop_bots_with_host(
        &host,
        &["set-nginx-commands", "--db", db_path.to_str().unwrap()],
    )
    .stdout(predicate::str::contains(
        "Reload command: docker exec web nginx -s reload",
    ));

    stop_bots_with_host(
        &host,
        &[
            "set-nginx-commands",
            "--db",
            db_path.to_str().unwrap(),
            "--reset",
        ],
    )
    .stdout(predicate::str::contains(
        "Reload command: systemctl reload nginx",
    ));
}

/// An unusable command is rejected when it is stored, not at the next
/// reload — which could be a cron run hours later with nobody watching.
#[test]
fn set_nginx_commands_rejects_an_unparsable_command() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_bin()
        .args([
            "set-nginx-commands",
            "--db",
            db_path.to_str().unwrap(),
            "--reload",
            "docker exec \"web nginx -s reload",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unbalanced quote"));
}

/// Per-site HTTP/1.x rejection end to end, including the guard that keeps
/// it from taking a site offline: a site's port-80 and port-443 blocks
/// share one `server_name`, so the setting reaches both, and only the TLS
/// one may carry the rule.
#[test]
fn rejecting_http_1x_applies_only_to_the_tls_server_block() {
    let fx = Fixture::new();
    let site = fx.nginx_root.join("a.example.conf");
    fs::write(
        &site,
        concat!(
            "server {\n    listen 80;\n    server_name a.example;\n}\n",
            "server {\n    listen 443 ssl;\n    server_name a.example;\n}\n"
        ),
    )
    .unwrap();
    fx.seed_bots();
    fx.scan_sites();

    // Off by default.
    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert!(
        !written.contains("$server_protocol"),
        "written was:\n{written}"
    );

    {
        let db = stop_bots::db::Db::open(&fx.db).unwrap();
        let s = db.list_sites().unwrap().into_iter().next().unwrap();
        db.set_site_request_rule(s.id, "http_1x", true).unwrap();
        // A header-shape rule alongside it: those are not TLS-dependent
        // and must reach *both* blocks.
        db.set_site_request_rule(s.id, "no_user_agent", true)
            .unwrap();
    }

    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert_eq!(
        written.matches("$server_protocol").count(),
        1,
        "only the TLS block may reject HTTP/1.x; written was:\n{written}"
    );
    assert_eq!(
        written.matches(r#"$http_user_agent = """#).count(),
        2,
        "a header-shape rule works over plain HTTP too, so both blocks \
         get it; written was:\n{written}"
    );
    // Certificate renewal has to keep working: ACME fetches
    // /.well-known/ over HTTP/1.1.
    assert!(
        written.contains(r"/\.well-known/"),
        "written was:\n{written}"
    );
}

/// Every block-response option end to end: each has to reach the config
/// as its own status code, and tarpit has to bring its throttle with it.
#[test]
fn each_block_response_reaches_the_generated_config() {
    let fx = Fixture::new();
    let site = fx.write_site("a.example");
    fx.seed_bots();
    fx.scan_sites();

    for (arg, expected) in [
        ("forbidden", "return 403;"),
        ("not-found", "return 404;"),
        ("gone", "return 410;"),
        ("too-many-requests", "return 429;"),
        ("teapot", "return 418;"),
        ("close", "return 444;"),
    ] {
        fx.run(&["set-block-response", "--response", arg]);
        fx.apply_blocks();
        let written = fs::read_to_string(&site).unwrap();
        assert!(
            written.contains(expected),
            "{arg} should render {expected}; written was:\n{written}"
        );
        assert!(
            !written.contains("set $limit_rate"),
            "{arg} must not throttle; written was:\n{written}"
        );
    }

    fx.run(&["set-block-response", "--response", "tarpit"]);
    fx.apply_blocks();
    let written = fs::read_to_string(&site).unwrap();
    assert!(
        written.contains("set $limit_rate 1;"),
        "written was:\n{written}"
    );
    assert!(written.contains("return 403;"), "written was:\n{written}");
}

#[test]
fn firewall_add_list_render_remove_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "1.2.3.4",
            "--action",
            "block",
        ])
        .assert()
        .success();

    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "66.249.64.0/19",
            "--action",
            "allow",
        ])
        .assert()
        .success();

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("1.2.3.4"))
        .stdout(predicate::str::contains("66.249.64.0/19"));

    let script_path = tmp.path().join("stop-bots.sh");
    stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "iptables",
            "--out",
            script_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Wrote 2 rule(s)"));

    let script = fs::read_to_string(&script_path).unwrap();
    assert!(
        script.contains("-A STOP-BOTS -s 1.2.3.4 -j DROP"),
        "script was:\n{script}"
    );
    assert!(
        script.contains("-A STOP-BOTS -s 66.249.64.0/19 -j ACCEPT"),
        "script was:\n{script}"
    );
    // This is the safety property that matters most: the script must never
    // touch chains/policies outside our own dedicated STOP-BOTS chain. The
    // restore it feeds declares that one chain and sets no policy.
    let declared: Vec<&str> = script.lines().filter(|l| l.starts_with(':')).collect();
    assert_eq!(
        declared,
        vec![":STOP-BOTS - [0:0]", ":STOP-BOTS - [0:0]"],
        "one declaration per family, of our chain only:\n{script}"
    );
    assert!(!script.contains(" -P "), "script was:\n{script}");

    stop_bots_bin()
        .args(["remove-firewall-rule", "--db", db_path, "--id", "1"])
        .assert()
        .success();

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("1.2.3.4").not());
}

/// The lockout safety net: `render-firewall` must refuse to write a script
/// that would block an IP address with a recent successful SSH login,
/// unless `--force` is passed. Uses `--ssh-log` to point at a fixture
/// instead of the real system logs, so this is deterministic regardless of
/// what's actually in `/var/log/auth.log` on whatever machine runs the test.
#[test]
fn render_firewall_refuses_to_lock_out_a_connected_ssh_client() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "4.5.6.0/24",
            "--action",
            "block",
        ])
        .assert()
        .success();

    let log_path = tmp.path().join("auth.log");
    fs::write(
        &log_path,
        "Jun 12 01:02:03 host sshd[111]: Accepted publickey for admin from 4.5.6.7 port 54321 ssh2: ED25519 SHA256:abc\n",
    )
    .unwrap();

    let script_path = tmp.path().join("stop-bots.sh");

    // Without --force: refuses, warns once, writes nothing.
    let output = stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
            "--ssh-log",
            log_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("WARNING").count(), 1);
    assert!(stderr.contains("4.5.6.7"), "stderr was:\n{stderr}");
    assert!(
        stderr.contains("Refusing to write"),
        "stderr was:\n{stderr}"
    );
    assert!(!script_path.exists());

    // With --force: still warns (once), but writes the script.
    let output = stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("WARNING").count(), 1);
    assert!(stderr.contains("4.5.6.7"), "stderr was:\n{stderr}");
    assert!(fs::read_to_string(&script_path)
        .unwrap()
        .contains("4.5.6.0/24"));
}

/// `set-log-paths --ssh-log` is where a host that keeps its SSH log
/// somewhere else says so, and the lockout guard used to ignore it: with no
/// flag it searched the defaults, found nothing, and could not run. Now the
/// stored path is what it reads.
#[test]
fn the_lockout_guard_reads_the_stored_ssh_log() {
    let tmp = tempfile::tempdir().unwrap();
    let host = tmp.path().join("host.conf");
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();
    let log_path = tmp.path().join("elsewhere-auth.log");
    fs::write(
        &log_path,
        "Jun 12 01:02:03 host sshd[111]: Accepted publickey for admin from 4.5.6.7 port 54321 ssh2\n",
    )
    .unwrap();
    for args in [
        vec![
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "4.5.6.0/24",
            "--action",
            "block",
        ],
        vec![
            "set-log-paths",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
        ],
    ] {
        stop_bots_bin()
            .env("STOP_BOTS_HOST_CONF", &host)
            .args(args)
            .assert()
            .success();
    }

    let script_path = tmp.path().join("stop-bots.nft");
    let output = stop_bots_bin()
        .env("STOP_BOTS_HOST_CONF", &host)
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was:\n{stderr}");
    assert!(stderr.contains("4.5.6.7"), "stderr was:\n{stderr}");
    assert!(!script_path.exists());
}

/// The flip side: a log with logins from unrelated IPs must not trip the
/// safety net at all — `render-firewall` should behave exactly as if
/// `--ssh-log` were never passed.
#[test]
fn render_firewall_proceeds_normally_when_no_connected_ip_is_at_risk() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "4.5.6.0/24",
            "--action",
            "block",
        ])
        .assert()
        .success();

    let log_path = tmp.path().join("auth.log");
    fs::write(
        &log_path,
        "Accepted publickey for admin from 9.9.9.9 port 54321 ssh2\n",
    )
    .unwrap();

    let script_path = tmp.path().join("stop-bots.sh");
    stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
            "--ssh-log",
            log_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARNING").not());
    assert!(script_path.exists());
}

/// A crawler IP-range / country-block feeding into the *same* rendered
/// output must trip the same safety net as an admin-added rule — the check
/// runs against everything about to be written, not just `firewall_rules`.
#[test]
fn render_firewall_lockout_check_covers_derived_country_ranges_too() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    {
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        db.replace_country_ranges("xx", &["4.5.6.0/24".to_string()])
            .unwrap();
        db.set_country_selected("xx", true).unwrap();
    }

    let log_path = tmp.path().join("auth.log");
    fs::write(
        &log_path,
        "Accepted publickey for admin from 4.5.6.7 port 54321 ssh2\n",
    )
    .unwrap();

    let script_path = tmp.path().join("stop-bots.sh");
    stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path.to_str().unwrap(),
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
            "--ssh-log",
            log_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("4.5.6.7"));
    assert!(!script_path.exists());
}

/// `render-firewall --apply`, end to end against a fake `nft`: the
/// rendered script is what runs, and only once it has does it become the
/// script a boot unit loads.
#[test]
fn render_firewall_apply_runs_the_render_and_then_makes_it_the_boot_script() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, calls) = fake_tools(tmp.path());
    let db_path = tmp.path().join("db.sqlite3");
    let applied = tmp.path().join("firewall.nft");
    let rendered = tmp.path().join("firewall.next.nft");
    stop_bots_bin()
        .args(["add-firewall-rule", "--db", db_path.to_str().unwrap()])
        .args(["--address", "203.0.113.9"])
        .assert()
        .success();

    stop_bots_bin()
        .env("PATH", path_with(&bin))
        .args([
            "render-firewall",
            "--db",
            db_path.to_str().unwrap(),
            "--apply",
        ])
        .args(["--out", applied.to_str().unwrap()])
        .args(["--ssh-log", "tests/fixtures/logs/auth.log"])
        .assert()
        .success()
        .stdout(predicate::str::contains("loads at boot"));

    let log = fs::read_to_string(&calls).unwrap();
    assert_eq!(log.trim(), format!("nft -f {}", rendered.display()));
    let boot = fs::read_to_string(&applied).expect("the applied script should be in place");
    assert!(boot.contains("203.0.113.9"), "applied script was:\n{boot}");
    assert_eq!(fs::read_to_string(&rendered).unwrap(), boot);
}

/// With no SSH log to check against, the script is written for review and
/// not run, and the script a boot unit loads is left alone.
#[test]
fn render_firewall_apply_refuses_when_the_lockout_check_cannot_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, calls) = fake_tools(tmp.path());
    let db_path = tmp.path().join("db.sqlite3");
    let applied = tmp.path().join("firewall.nft");

    stop_bots_bin()
        .env("PATH", path_with(&bin))
        .args([
            "render-firewall",
            "--db",
            db_path.to_str().unwrap(),
            "--apply",
        ])
        .args(["--out", applied.to_str().unwrap()])
        .args(["--ssh-log", "/nonexistent/auth.log"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not applied"))
        .stderr(predicate::str::contains("--force overrides this"));

    assert!(!calls.exists(), "nft must not have run");
    assert!(!applied.exists());
    assert!(tmp.path().join("firewall.next.nft").exists());
}

/// A script `nft` rejects must not become what the host loads at boot:
/// the previous applied script stays exactly as it was.
#[test]
fn a_failed_apply_leaves_the_boot_script_as_it_was() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, _calls) = fake_tools(tmp.path());
    fs::write(tmp.path().join("fail-nft"), "Error: syntax error\n").unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let applied = tmp.path().join("firewall.nft");
    fs::write(&applied, "# what was applied last week\n").unwrap();

    stop_bots_bin()
        .env("PATH", path_with(&bin))
        .args([
            "render-firewall",
            "--db",
            db_path.to_str().unwrap(),
            "--apply",
        ])
        .args(["--out", applied.to_str().unwrap()])
        .args(["--ssh-log", "tests/fixtures/logs/auth.log"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("applying failed"));

    assert_eq!(
        fs::read_to_string(&applied).unwrap(),
        "# what was applied last week\n"
    );
}

#[test]
fn geo_mode_and_country_selection_cli_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    // Defaults to Blocklist with nothing selected.
    stop_bots_bin()
        .args(["list-selected-countries", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Blocklist"))
        .stdout(predicate::str::contains("No countries selected"));

    stop_bots_bin()
        .args(["add-country", "--db", db_path, "--country", "nl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added country nl"));

    stop_bots_bin()
        .args(["list-selected-countries", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("nl"));

    stop_bots_bin()
        .args(["set-geo-mode", "--db", db_path, "--mode", "allowlist"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Allowlist"));

    stop_bots_bin()
        .args(["list-selected-countries", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Allowlist"))
        .stdout(predicate::str::contains("nl"));

    stop_bots_bin()
        .args(["remove-country", "--db", db_path, "--country", "nl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed country nl"));

    stop_bots_bin()
        .args(["list-selected-countries", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("No countries selected"));
}

/// The nftables-only guard, end to end through the real CLI: Allowlist
/// mode's trailing default-deny catch-all is unsafe on iptables (no
/// loopback/established allowance, silently permits all IPv6), so
/// render-firewall must refuse before ever writing a script.
#[test]
fn render_firewall_rejects_allowlist_mode_on_iptables_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args(["set-geo-mode", "--db", db_path, "--mode", "allowlist"])
        .assert()
        .success();

    let script_path = tmp.path().join("stop-bots.sh");
    stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "iptables",
            "--out",
            script_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nftables"));
    assert!(!script_path.exists());
}

/// The scenario the whole first-match-wins lockout fix exists for: in
/// Allowlist mode, an admin whose connected IP is covered by an *earlier*
/// Allow rule (here, their own admin-added `firewall_rules` entry) must not
/// be flagged as at-risk just because the trailing catch-all's CIDR also
/// technically contains their IP — the catch-all never actually applies to
/// them, since the Allow rule matches first.
#[test]
fn render_firewall_allowlist_mode_does_not_warn_when_an_earlier_allow_rule_covers_the_admin() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args(["set-geo-mode", "--db", db_path, "--mode", "allowlist"])
        .assert()
        .success();
    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "4.5.6.7",
            "--action",
            "allow",
        ])
        .assert()
        .success();

    let log_path = tmp.path().join("auth.log");
    fs::write(
        &log_path,
        "Accepted publickey for admin from 4.5.6.7 port 54321 ssh2\n",
    )
    .unwrap();

    let script_path = tmp.path().join("stop-bots.sh");
    stop_bots_bin()
        .args([
            "render-firewall",
            "--db",
            db_path,
            "--backend",
            "nftables",
            "--out",
            script_path.to_str().unwrap(),
            "--ssh-log",
            log_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARNING").not());

    let script = fs::read_to_string(&script_path).unwrap();
    for (element, verdict) in [
        ("4.5.6.7", " accept"),
        ("0.0.0.0/0", " drop"),
        ("::/0", " drop"),
    ] {
        let (_, rule) = nft_rule_for(&script, element)
            .unwrap_or_else(|| panic!("{element} is in no set:\n{script}"));
        assert!(rule.ends_with(verdict), "{element} is decided by {rule:?}");
    }
    assert!(
        nft_rule_for(&script, "4.5.6.7").unwrap().0 < nft_rule_for(&script, "0.0.0.0/0").unwrap().0,
        "the admin's Allow must come before the catch-all:\n{script}"
    );
}

fn repeat_failed_attempt(ip: &str, times: usize) -> String {
    format!("Failed password for root from {ip} port 4444 ssh2\n").repeat(times)
}

/// Every detector subcommand's `--ttl-days` has the ceiling the console
/// and the TUI have; past it the arithmetic that dates the block wraps.
/// `=` so clap does not read the negative one as a flag.
#[test]
fn a_ttl_past_ten_years_is_refused_on_the_command_line() {
    let fx = Fixture::new();
    for command in [
        "block-scanners",
        "block-web-scanners",
        "block-spoofed-crawlers",
        "block-probe-paths",
        "block-honeypot",
    ] {
        for days in ["3651", "106751991167301", "-106751991167301"] {
            let flag = format!("--ttl-days={days}");
            fx.cmd(&[command, "--dry-run", &flag])
                .assert()
                .failure()
                .stderr(predicate::str::contains("--ttl-days"));
        }
    }
}

#[test]
fn block_scanners_adds_a_rule_for_an_ip_over_the_threshold_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 25)).unwrap();

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 new block rule"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9"));

    // Re-running against the same log must not add a duplicate rule.
    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "all already covered by an existing firewall rule",
        ));
}

#[test]
fn block_scanners_ignores_an_ip_below_the_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 5)).unwrap();

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No scanning IPs found"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

/// `--threshold 0` used to be accepted, and every detector compares
/// `count >= threshold`, so it blocked every address in the log whatever
/// it had done — five single failed logins became five block rules. One
/// flag, or one shell variable that expanded to nothing, away from mass
/// blocking in a tool whose whole premise is not blocking people by
/// mistake.
///
/// Rejected at parse time, so nothing is written and no database is even
/// created; 1 is still allowed, because "one failed attempt is a scanner"
/// is an aggressive policy rather than a mistake.
#[test]
fn block_scanners_refuses_a_threshold_of_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 1)).unwrap();

    for command in ["block-scanners", "block-web-scanners"] {
        stop_bots_bin()
            .args([command, "--db", db_path, "--threshold", "0"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("matches every address in the log"));
    }

    assert!(
        !std::path::Path::new(db_path).exists(),
        "a rejected threshold still created a database"
    );

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "1",
        ])
        .assert()
        .success();
}

/// The safety property that matters most: an IP that eventually logs in
/// successfully must never be auto-blocked, even with a mountain of failed
/// attempts before it.
#[test]
fn block_scanners_never_blocks_an_ip_that_eventually_logged_in() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    let mut log = repeat_failed_attempt("198.51.100.9", 25);
    log.push_str("Accepted publickey for admin from 198.51.100.9 port 5555 ssh2\n");
    fs::write(&log_path, log).unwrap();

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No scanning IPs found"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

#[test]
fn block_scanners_dry_run_reports_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 25)).unwrap();

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Would add 1 new block rule"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

fn not_found_line(ip: &str, path: &str) -> String {
    format!(
        "{ip} - - [10/Jul/2026:12:00:00 +0000] \"GET {path} HTTP/1.1\" 404 162 \"-\" \"Mozilla/5.0\"\n"
    )
}

#[test]
fn block_web_scanners_adds_a_rule_for_distinct_not_found_paths_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("access.log");
    let log: String = (0..20)
        .map(|i| not_found_line("198.51.100.9", &format!("/missing-{i}")))
        .collect();
    fs::write(&log_path, log).unwrap();

    stop_bots_bin()
        .args([
            "block-web-scanners",
            "--db",
            db_path,
            "--access-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "15",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 new block rule"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9"));

    // Re-running against the same log must not add a duplicate rule.
    stop_bots_bin()
        .args([
            "block-web-scanners",
            "--db",
            db_path,
            "--access-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "15",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "all already covered by an existing firewall rule",
        ));
}

/// The core discriminator this feature exists to get right, verified again
/// at the CLI/DB-state level (already unit-tested in `accesslog.rs`):
/// hitting the *same* dead path repeatedly must never look like scanning.
#[test]
fn block_web_scanners_ignores_repeated_hits_on_a_single_dead_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("access.log");
    let log: String = (0..50)
        .map(|_| not_found_line("198.51.100.9", "/missing"))
        .collect();
    fs::write(&log_path, log).unwrap();

    stop_bots_bin()
        .args([
            "block-web-scanners",
            "--db",
            db_path,
            "--access-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "15",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No scanning IPs found"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

#[test]
fn block_web_scanners_dry_run_reports_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("access.log");
    let log: String = (0..20)
        .map(|i| not_found_line("198.51.100.9", &format!("/missing-{i}")))
        .collect();
    fs::write(&log_path, log).unwrap();

    stop_bots_bin()
        .args([
            "block-web-scanners",
            "--db",
            db_path,
            "--access-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "15",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Would add 1 new block rule"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

/// End-to-end through the real binary: a rule added with an already-past
/// TTL (`--ttl-days=-1`, using `=` so clap doesn't mistake the negative
/// number for a flag) must be gone by the very next `list-firewall-rules`
/// call — proving the CLI wiring actually reaches `Db::list_firewall_rules`'s
/// pruning, not just the unit-tested `Db` layer in isolation.
#[test]
fn block_scanners_ttl_days_expires_the_rule_by_the_next_list() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 25)).unwrap();

    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
            "--ttl-days=-1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("expiring in -1 day"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9").not());
}

/// The default (positive) TTL path — a freshly added rule must still show
/// up normally, with its expiry reflected in the confirmation message.
#[test]
fn block_web_scanners_ttl_days_defaults_to_one_day() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    let log_path = tmp.path().join("access.log");
    let log: String = (0..20)
        .map(|i| not_found_line("198.51.100.9", &format!("/missing-{i}")))
        .collect();
    fs::write(&log_path, log).unwrap();

    stop_bots_bin()
        .args([
            "block-web-scanners",
            "--db",
            db_path,
            "--access-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "15",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("expiring after 1 day"));

    stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("198.51.100.9"));
}

/// `list-firewall-rules` must show a temporary rule's expiry (so an admin
/// scanning the list can tell an auto-added block from a permanent
/// hand-added one), but never show one for a permanent rule.
#[test]
fn list_firewall_rules_shows_expiry_only_for_temporary_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let db_path = db_path.to_str().unwrap();

    stop_bots_bin()
        .args([
            "add-firewall-rule",
            "--db",
            db_path,
            "--address",
            "9.9.9.9",
            "--action",
            "block",
        ])
        .assert()
        .success();

    let log_path = tmp.path().join("auth.log");
    fs::write(&log_path, repeat_failed_attempt("198.51.100.9", 25)).unwrap();
    stop_bots_bin()
        .args([
            "block-scanners",
            "--db",
            db_path,
            "--ssh-log",
            log_path.to_str().unwrap(),
            "--threshold",
            "20",
        ])
        .assert()
        .success();

    let output = stop_bots_bin()
        .args(["list-firewall-rules", "--db", db_path])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let permanent_line = stdout.lines().find(|l| l.contains("9.9.9.9")).unwrap();
    let temporary_line = stdout.lines().find(|l| l.contains("198.51.100.9")).unwrap();
    assert!(
        !permanent_line.contains("expires"),
        "permanent_line was:\n{permanent_line}"
    );
    assert!(
        temporary_line.contains("expires in"),
        "temporary_line was:\n{temporary_line}"
    );
}

// ---- batch mode ----

impl Fixture {
    /// One batch pass over this fixture, offline. `--no-fetch` throughout:
    /// every test in this suite must run without a network, and the
    /// download steps are the only part of batch that needs one.
    ///
    /// A fixture SSH log rather than auto-detection, for the usual reason
    /// — and because the lockout guard reads it, so leaving it to
    /// auto-detection would make these tests depend on whatever log the
    /// machine running them happens to have.
    fn batch(&self, extra: &[&str]) -> Command {
        let out = self.firewall_script();
        let mut args = vec![
            "batch",
            "--no-fetch",
            "--root",
            self.nginx_root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--ssh-log",
            "tests/fixtures/logs/auth.log",
            "--access-log",
            "/dev/null",
        ];
        args.extend_from_slice(extra);
        self.cmd(&args)
    }

    /// The applied script: what `--out` names, and what the boot unit
    /// loads. Only an apply writes it.
    fn firewall_script(&self) -> std::path::PathBuf {
        self.managed.join("firewall.nft")
    }

    /// The rendered script beside it, which every run writes.
    fn rendered_script(&self) -> std::path::PathBuf {
        self.managed.join("firewall.next.nft")
    }
}

/// The whole point of batch mode: one command does the lot. It has to
/// discover the site, write the NGINX block, and write the firewall
/// script, from a database that starts empty.
#[test]
fn batch_scans_applies_and_renders_in_one_pass() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");

    fixture.batch(&[]).assert().success();

    let config = fs::read_to_string(&site).unwrap();
    assert!(
        config.contains("stop-bots"),
        "the site config should carry the generated block:\n{config}"
    );
    assert!(
        fixture.rendered_script().exists(),
        "the firewall script should have been written"
    );
    assert!(
        !fixture.firewall_script().exists(),
        "without --apply, nothing reaches the script the boot unit loads"
    );
}

/// Quiet on success, because `cron` mails the owner anything a job
/// prints and a nightly run that says nothing is one nobody has to read.
#[test]
fn a_successful_batch_run_says_nothing() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");

    fixture
        .batch(&[])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

/// `--verbose` is what a first run by hand wants: every step, named.
#[test]
fn verbose_batch_reports_every_step() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");

    fixture
        .batch(&["--verbose"])
        .assert()
        .success()
        .stdout(predicate::str::contains("scan sites"))
        .stdout(predicate::str::contains("nginx blocks"))
        .stdout(predicate::str::contains("firewall"));
}

/// **The safety property this whole feature turns on.** Under `--apply`,
/// a lockout guard that could not run at all counts as a refusal.
///
/// Interactive `render-firewall` prints a note and carries on here,
/// which is defensible when a human is watching the terminal. From
/// crontab nobody is — and this project has already taken a server off
/// the network once by letting exactly this case fall through.
#[test]
fn batch_apply_refuses_when_the_lockout_check_cannot_run() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");

    fixture
        .cmd(&[
            "batch",
            "--no-fetch",
            "--apply",
            "--root",
            fixture.nginx_root.to_str().unwrap(),
            "--out",
            fixture.firewall_script().to_str().unwrap(),
            "--ssh-log",
            "/nonexistent/auth.log",
            "--access-log",
            "/dev/null",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not applied"))
        .stderr(predicate::str::contains("lockout check could not run"))
        .stderr(predicate::str::contains("--ssh-log"));

    assert!(
        !fixture.firewall_script().exists(),
        "refusing must mean nothing reached the script the boot unit loads"
    );
    assert!(
        fixture.rendered_script().exists(),
        "the rendered script is inert, so it is written for review"
    );
}

/// The other half of the guard: rules that would cut off the admin who is
/// connected right now. The fixture log's Accepted line is for 192.0.2.10,
/// which the rule below covers.
///
/// Batch records that login before rendering, as the internal cron does,
/// so the admin gets an Allow rule ahead of the block rather than a
/// refusal. (This used to be the refusal case, because batch never fed the
/// login window.) No `--apply`: with the admin protected nothing would stop
/// it running the real `nft`.
#[test]
fn batch_protects_the_connected_admin_ahead_of_a_block_covering_them() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");
    fixture.run(&[
        "add-firewall-rule",
        "--address",
        "192.0.2.0/24",
        "--action",
        "block",
    ]);

    fixture.batch(&[]).assert().success();

    let script = fs::read_to_string(fixture.rendered_script()).unwrap();
    let allow = nft_rule_for(&script, "192.0.2.10");
    let block = nft_rule_for(&script, "192.0.2.0/24");
    assert!(
        matches!((&allow, &block), (Some((a, allow)), Some((b, block)))
            if a < b && allow.ends_with(" accept") && block.ends_with(" drop")),
        "the admin's address should be allowed ahead of the block:\n{script}"
    );
}

/// Nothing is enforced without `--apply`, so the same rule set that is
/// refused above is written without complaint — the admin still gets to
/// read the script before running it, which is this project's default
/// everywhere else.
#[test]
fn batch_without_apply_writes_rules_the_guard_would_refuse_to_apply() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");
    fixture.run(&[
        "add-firewall-rule",
        "--address",
        "192.0.2.0/24",
        "--action",
        "block",
    ]);

    fixture.batch(&[]).assert().success();

    assert!(fixture.rendered_script().exists());
}

/// `batch --dry-run` prints what the two planes would do and changes
/// nothing: no site file, no script, and not even the rendered signature
/// a Dashboard reads.
#[test]
fn a_batch_dry_run_says_what_would_change_and_changes_nothing() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");
    let before = fs::read_to_string(&site).unwrap();
    fixture.run(&["add-firewall-rule", "--address", "203.0.113.9"]);

    let output = fixture.batch(&["--apply", "--dry-run"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout:\n{stdout}");
    for (what, expected) in [
        ("that it is a dry run", "Dry run"),
        ("the site file", "example.com"),
        ("the rules against the applied script", "all of it is new"),
        ("the lockout check", "lockout check passed"),
        ("the verdict", "would be applied"),
    ] {
        assert!(stdout.contains(expected), "no {what} in:\n{stdout}");
    }
    assert_eq!(fs::read_to_string(&site).unwrap(), before);
    assert!(!fixture.rendered_script().exists());
    assert!(!fixture.firewall_script().exists());
    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert_eq!(db.get_firewall_rendered_signature().unwrap(), None);
}

/// And `--diff` shows how: the site's new block and the script's rules.
#[test]
fn a_batch_dry_run_with_diff_prints_every_change() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");
    fixture.run(&["add-firewall-rule", "--address", "203.0.113.9"]);

    fixture
        .batch(&["--dry-run", "--diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+++ "))
        .stdout(predicate::str::contains("+\t203.0.113.9"))
        .stdout(predicate::str::contains("+    # BEGIN stop-bots"));
}

/// **A fresh host run by `batch --no-fetch` alone blocks the scanners the
/// binary already knows.** The built-in list used to be stored only by
/// the TUI and the web console, or as one of the downloads, so on a host
/// without outbound access every step reported success and no user agent
/// was blocked. Found by the stranger test in the container suite.
#[test]
fn batch_without_fetching_still_blocks_the_built_in_list() {
    let fixture = Fixture::new();
    let site = fixture.write_site("example.com");

    fixture.batch(&[]).assert().success();

    let config = fs::read_to_string(&site).unwrap();
    assert!(
        config.contains("ModatScanner"),
        "a built-in scanner should be in the generated block:\n{config}"
    );
}

/// And the dry run a stranger reads before that first run shows the same
/// block, rather than "no file would change".
#[test]
fn a_batch_dry_run_on_a_fresh_host_shows_the_built_in_list() {
    let fixture = Fixture::new();
    let site = fixture.write_site("example.com");
    let before = fs::read_to_string(&site).unwrap();

    fixture
        .batch(&["--dry-run", "--diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+    # BEGIN stop-bots"))
        .stdout(predicate::str::contains("ModatScanner"));

    assert_eq!(fs::read_to_string(&site).unwrap(), before);
}

/// `apply-blocks` is the other way a CLI-only host writes NGINX config.
#[test]
fn apply_blocks_on_a_fresh_database_writes_the_built_in_list() {
    let fixture = Fixture::new();
    let site = fixture.write_site("example.com");
    let root = fixture.nginx_root.to_str().unwrap();

    fixture.run(&["scan-sites", "--root", root]);
    fixture.run(&["apply-blocks", "--root", root, "--no-reload"]);

    let config = fs::read_to_string(&site).unwrap();
    assert!(config.contains("ModatScanner"), "config was:\n{config}");
}

#[test]
fn apply_blocks_dry_run_lists_and_diffs_without_writing() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");
    let before = fs::read_to_string(&site).unwrap();
    let root = fixture.nginx_root.to_str().unwrap();

    fixture
        .run(&["apply-blocks", "--root", root, "--dry-run", "--diff"])
        .stdout(predicate::str::contains("1 file(s) would change"))
        .stdout(predicate::str::contains(site.to_str().unwrap()))
        .stdout(predicate::str::contains("+    # BEGIN stop-bots"));

    assert_eq!(fs::read_to_string(&site).unwrap(), before);
}

/// One step failing must not stop the others, and must still be visible:
/// `cron` only tells anyone about a job that exits non-zero.
#[test]
fn a_failed_step_is_reported_and_sets_a_failing_exit_status() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");

    // An unwritable output path fails the firewall step and nothing else.
    fixture
        .cmd(&[
            "batch",
            "--no-fetch",
            "--root",
            fixture.nginx_root.to_str().unwrap(),
            "--out",
            "/nonexistent/directory/firewall.nft",
            "--ssh-log",
            "tests/fixtures/logs/auth.log",
            "--access-log",
            "/dev/null",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("FAIL"))
        .stderr(predicate::str::contains("firewall"));

    // The NGINX plane is independent and ran anyway.
    let config = fs::read_to_string(&site).unwrap();
    assert!(config.contains("stop-bots"), "config was:\n{config}");
}

/// Batch records each step under the same key the TUI's internal cron
/// uses, so the two schedulers agree about what has already run rather
/// than each doing the work again — and the Dashboard's "Scheduled
/// tasks" panel shows what the real cron did.
#[test]
fn batch_records_its_run_against_the_internal_crons_schedule() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");

    fixture.batch(&[]).assert().success();

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    for job in [
        stop_bots::cron::CronJob::RecordAccessStats,
        stop_bots::cron::CronJob::RenderFirewall,
        stop_bots::cron::CronJob::Detect(stop_bots::protection::Detector::SshScanners),
    ] {
        assert!(
            db.get_cron_last_run(job.id()).unwrap().is_some(),
            "{} should have been stamped",
            job.label()
        );
    }
}

/// The report exists because nothing else in this tool answered "what is
/// my own policy turning away" — which is how three first-party apps were
/// blocked on a real host for days. Reading the log is the whole feature,
/// so the end-to-end test reads a log.
#[test]
fn list_turned_away_names_the_refused_client_and_what_still_got_through() {
    let fixture = Fixture::new();
    let log = fixture.nginx_root.join("access.log");
    let client = "Jellyfin Android TV/0.19.10 via jellyfin-sdk-kotlin (OkHttp/4.12.0)";
    fs::write(
        &log,
        [
            access_line("203.0.113.5", "/", 444, client),
            access_line("203.0.113.5", "/", 444, client),
            access_line("203.0.113.6", "/", 200, "Mozilla/5.0"),
        ]
        .concat(),
    )
    .unwrap();
    fixture.run(&["set-block-response", "--response", "close"]);

    let out = fixture.run(&["list-turned-away", "--access-log", log.to_str().unwrap()]);
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();

    assert!(
        stdout.contains("Jellyfin Android TV"),
        "the refused client must be named:\n{stdout}"
    );
    assert!(
        !stdout.contains("Mozilla/5.0"),
        "a client that was never refused has nothing to report:\n{stdout}"
    );
}

/// A host turning nobody away says so, rather than printing an empty
/// table and leaving the reader to wonder whether it ran.
#[test]
fn list_turned_away_says_so_when_nothing_was_refused() {
    let fixture = Fixture::new();
    let log = fixture.nginx_root.join("access.log");
    fs::write(&log, access_line("203.0.113.6", "/", 200, "Mozilla/5.0")).unwrap();

    fixture
        .run(&["list-turned-away", "--access-log", log.to_str().unwrap()])
        .stdout(predicate::str::contains(
            "Nothing in this log was turned away",
        ));
}

/// A user agent as `escape=json` writes one carrying an OSC 52 sequence,
/// which would set the operator's clipboard. serde decodes the `\u001b`
/// into a real ESC, so it reaches stdout unless the printing replaces it.
fn json_line_with_hostile_agent(status: u16) -> String {
    format!(
        r#"{{"remote_addr":"203.0.113.5","request_uri":"/","status":"{status}","http_user_agent":"evil\u001b]52;c;aGk=\u0007"}}"#
    ) + "\n"
}

/// Asserts `stdout` names the hostile agent from
/// [`json_line_with_hostile_agent`] with its control characters replaced.
fn assert_printed_defanged(stdout: &str) {
    assert!(
        !stdout.contains(['\u{1b}', '\u{7}']),
        "a control character reached the terminal: {stdout:?}"
    );
    assert!(
        stdout.contains("evil\u{fffd}]52;c;aGk=\u{fffd}"),
        "the agent should still be listed, defanged: {stdout:?}"
    );
}

#[test]
fn list_turned_away_replaces_control_characters_in_a_user_agent() {
    let fixture = Fixture::new();
    let log = fixture.nginx_root.join("access.log");
    fs::write(&log, json_line_with_hostile_agent(444)).unwrap();
    fixture.run(&["set-block-response", "--response", "close"]);

    let out = fixture.run(&["list-turned-away", "--access-log", log.to_str().unwrap()]);

    assert_printed_defanged(&String::from_utf8(out.get_output().stdout.clone()).unwrap());
}

#[test]
fn list_access_stats_replaces_control_characters_in_a_user_agent() {
    let fixture = Fixture::new();
    let log = fixture.nginx_root.join("access.log");
    fs::write(&log, json_line_with_hostile_agent(200)).unwrap();
    fixture.run(&["record-access-stats", "--access-log", log.to_str().unwrap()]);

    let out = fixture.run(&["list-access-stats"]);

    assert_printed_defanged(&String::from_utf8(out.get_output().stdout.clone()).unwrap());
}

/// Batch must share the access-log read offset with everything else that
/// reads the same log, not keep one of its own.
///
/// That offset is how `Db` remembers what has already been tallied. With a
/// key of its own, batch would re-count the whole log on its first run and
/// then double-count every line for as long as anything else read it too —
/// and the number it inflates is the hit count Firewall shows an
/// admin deciding whether to block a user agent.
#[test]
fn batch_shares_the_access_log_offset_with_record_access_stats() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    fixture.write_site("example.com");
    let log = fixture.nginx_root.join("access.log");
    fs::write(&log, access_line("203.0.113.5", "/", 200, "Mozilla/5.0")).unwrap();

    fixture.run(&["record-access-stats", "--access-log", log.to_str().unwrap()]);
    let after_cli = hit_count(&fixture, "Mozilla/5.0");
    assert_eq!(after_cli, 1, "one line, counted once");

    // Same log, nothing appended: batch has nothing new to count.
    let out = fixture.firewall_script();
    fixture
        .cmd(&[
            "batch",
            "--no-fetch",
            "--root",
            fixture.nginx_root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--ssh-log",
            "tests/fixtures/logs/auth.log",
            "--access-log",
            log.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert_eq!(
        hit_count(&fixture, "Mozilla/5.0"),
        after_cli,
        "batch re-counted a log line that had already been tallied"
    );
}

fn hit_count(fixture: &Fixture, user_agent: &str) -> i64 {
    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    db.list_user_agent_stats()
        .unwrap()
        .into_iter()
        .find(|s| s.user_agent == user_agent)
        .map(|s| s.hit_count)
        .unwrap_or(0)
}

// ---- `--source`: every download has a local-file path through it ----
//
// Every fetch in this project now takes a `--source <file>` override that
// parses the same format the server would have sent. It is a real feature
// for a host with no outbound access — and it is what lets these tests
// cover the parse-and-store half of a download without a network, which
// was the single largest gap in this suite's coverage.

/// Crawler ranges, from Google's published `prefixes` JSON. Both families
/// come out of one file, and both have to reach the database.
#[test]
fn update_ip_ranges_reads_a_local_file_instead_of_downloading() {
    let fixture = Fixture::new();

    fixture
        .run(&[
            "update-ip-ranges",
            "--source-id",
            "googlebot",
            "--source",
            "tests/fixtures/ipranges/googlebot-sample.json",
        ])
        .stdout(predicate::str::contains("Stored 3 CIDR range(s)"));

    // Visible where it matters: a rendered firewall script, not just a row.
    let out = fixture.managed.join("fw.nft");
    fixture.run(&[
        "render-firewall",
        "--backend",
        "nftables",
        "--out",
        out.to_str().unwrap(),
        "--force",
    ]);
    let script = fs::read_to_string(&out).unwrap();
    // Googlebot is a Search bot, allowed by default, so its ranges are
    // fetched but inert — which is the point of storing them: they are
    // what the spoofed-crawler detector checks *against*.
    assert!(!script.contains("192.178.4.0/27"), "script was:\n{script}");
}

/// A country's IPdeny zone file, which is bare CIDRs with blank lines.
#[test]
fn update_country_ranges_reads_a_local_file_instead_of_downloading() {
    let fixture = Fixture::new();

    fixture
        .run(&[
            "update-country-ranges",
            "--country",
            "nl",
            "--source",
            "tests/fixtures/ipranges/country-sample.zone",
        ])
        .stdout(predicate::str::contains("Stored 3 CIDR range(s)"));

    fixture.run(&["add-country", "--country", "nl"]);
    let out = fixture.managed.join("fw.nft");
    fixture.run(&[
        "render-firewall",
        "--backend",
        "nftables",
        "--out",
        out.to_str().unwrap(),
        "--force",
    ]);
    let script = fs::read_to_string(&out).unwrap();
    assert!(script.contains("86.48.240.0/20"), "script was:\n{script}");
}

/// The two shapes a reputation feed arrives in: a plain `.netset` list and
/// a provider's JSON. Both go through `--source`, so both parsers are
/// exercised offline.
#[test]
fn update_reputation_source_reads_a_local_file_in_either_format() {
    for (source_id, path, expected) in [
        (
            "firehol-level1",
            "tests/fixtures/ipranges/firehol-sample.netset",
            "198.51.100.0/24",
        ),
        (
            "aws",
            "tests/fixtures/ipranges/aws-sample.json",
            "13.34.37.64/27",
        ),
    ] {
        let fixture = Fixture::new();
        fixture
            .run(&[
                "update-reputation-source",
                "--source-id",
                source_id,
                "--source",
                path,
            ])
            .stdout(predicate::str::contains("Stored 2 range(s)"));
        // Fetching does not switch a feed on, so the output says so —
        // otherwise a fetch that changes nothing reads as a bug.
        fixture.run(&[
            "set-reputation-source",
            "--source-id",
            source_id,
            "--enabled",
            "true",
        ]);

        let out = fixture.managed.join("fw.nft");
        fixture.run(&[
            "render-firewall",
            "--backend",
            "nftables",
            "--out",
            out.to_str().unwrap(),
            "--force",
        ]);
        let script = fs::read_to_string(&out).unwrap();
        assert!(
            script.contains(expected),
            "{source_id}: script was:\n{script}"
        );
    }
}

/// The comments and blank lines a real `.netset` carries must not become
/// CIDRs. The fixture has one of each, and "Stored 2" above is only
/// meaningful because of it.
#[test]
fn a_feeds_comments_and_blank_lines_are_not_stored_as_ranges() {
    let fixture = Fixture::new();

    fixture.run(&[
        "update-reputation-source",
        "--source-id",
        "firehol-level1",
        "--source",
        "tests/fixtures/ipranges/firehol-sample.netset",
    ]);

    fixture
        .run(&["list-reputation-sources"])
        .stdout(predicate::str::contains("2 range"));
}

/// A `--source` that isn't there says which file, rather than leaving a
/// bare "No such file or directory" to be matched against the three paths
/// a command line can carry.
#[test]
fn a_missing_source_file_names_the_file() {
    let fixture = Fixture::new();

    fixture
        .cmd(&[
            "update-ip-ranges",
            "--source-id",
            "googlebot",
            "--source",
            "/nonexistent/ranges.json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("/nonexistent/ranges.json"));
}

// ---- `stop-bots web` startup checks ----

/// Exposing the console is opt-in, and the refusal has to be a refusal:
/// nothing may be persisted, or a later plain `stop-bots web` comes up on
/// every interface having never passed this check.
#[test]
fn web_refuses_a_non_loopback_bind_without_expose_and_saves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_bin()
        .args([
            "web",
            "--db",
            db_path.to_str().unwrap(),
            "--bind",
            "0.0.0.0:8787",
            "--save",
            "--set-password",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to bind"))
        .stderr(predicate::str::contains("ssh -L"));

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    assert_eq!(
        db.get_text_setting(stop_bots::web::BIND_KEY).unwrap(),
        None,
        "a refused bind must not be persisted"
    );
    assert!(
        !db.get_bool_setting(stop_bots::web::EXPOSE_KEY, false)
            .unwrap(),
        "nor may the exposure flag be"
    );
}

/// The deliberate path does persist, so a later plain run starts the same
/// way.
#[test]
fn web_with_expose_and_save_persists_the_bind() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    stop_bots_cmd(&[
        "web",
        "--db",
        db_path.to_str().unwrap(),
        "--bind",
        "0.0.0.0:8788",
        "--expose",
        "--allowed-hosts",
        "admin.example.com",
        "--save",
        "--set-password",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("New password:"));

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    assert_eq!(
        db.get_text_setting(stop_bots::web::BIND_KEY)
            .unwrap()
            .as_deref(),
        Some("0.0.0.0:8788")
    );
    assert!(db
        .get_bool_setting(stop_bots::web::EXPOSE_KEY, false)
        .unwrap());
    assert_eq!(
        db.get_text_setting(stop_bots::web::ALLOWED_HOSTS_KEY)
            .unwrap()
            .as_deref(),
        Some("admin.example.com")
    );
}

/// The two proxy settings are read from the database on every request, so
/// like `--allowed-hosts` they are stored with or without `--save` — and
/// `false` has to switch them back off, not just leave them alone.
#[test]
fn web_stores_the_proxy_settings_and_can_turn_them_off() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");
    let run = |value: &str| {
        stop_bots_cmd(&[
            "web",
            "--db",
            db_path.to_str().unwrap(),
            "--trust-forwarded-for",
            value,
            "--secure-cookie",
            value,
            "--set-password",
        ])
        .assert()
        .success();
        let db = stop_bots::db::Db::open(&db_path).unwrap();
        (
            db.get_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, false)
                .unwrap(),
            db.get_bool_setting(stop_bots::web::SECURE_COOKIE_KEY, false)
                .unwrap(),
        )
    };

    assert_eq!(run("true"), (true, true));
    assert_eq!(run("false"), (false, false));
}

/// `--set-password` stores a hash and prints the password once. The
/// password itself must not survive anywhere this program can read it
/// back.
#[test]
fn web_set_password_stores_only_a_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("db.sqlite3");

    let output = stop_bots_cmd(&["web", "--db", db_path.to_str().unwrap(), "--set-password"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let password = stdout
        .lines()
        .next()
        .unwrap()
        .trim_start_matches("New password: ")
        .to_string();
    assert!(!password.is_empty(), "stdout was:\n{stdout}");

    let db = stop_bots::db::Db::open(&db_path).unwrap();
    let stored = db
        .get_text_setting(stop_bots::web::auth::PASSWORD_HASH_KEY)
        .unwrap()
        .unwrap();
    assert!(stored.starts_with("$argon2id$"), "was: {stored}");
    assert!(!stored.contains(&password));
    assert!(stop_bots::web::auth::verify_password(&db, &password).unwrap());
}

/// An unparsable bind address is rejected by name rather than defaulted.
#[test]
fn web_rejects_a_bind_that_is_not_an_address_and_port() {
    let tmp = tempfile::tempdir().unwrap();
    stop_bots_bin()
        .args([
            "web",
            "--db",
            tmp.path().join("db.sqlite3").to_str().unwrap(),
            "--bind",
            "8787",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("8787"));
}

/// Stages a fake Debian-with-systemd root that `install web` will accept,
/// and a binary for its ExecStart to point at.
fn fake_debian_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("run/systemd/system")).unwrap();
    fs::create_dir_all(tmp.path().join("etc")).unwrap();
    fs::write(tmp.path().join("etc/debian_version"), "13.1\n").unwrap();
    let binary = tmp.path().join("stop-bots");
    fs::write(&binary, b"#!/bin/true\n").unwrap();
    (tmp, binary)
}

/// The whole installer, end to end, into a prefix: the three units, the
/// two directories, a generated password, and no `systemctl` and no
/// `useradd` — a unit under a prefix is not a path systemd reads, nor the
/// tree a user database, and saying so beats implying either took effect.
#[test]
fn install_web_writes_a_unit_and_a_password_under_a_prefix() {
    let (tmp, binary) = fake_debian_root();

    let output = stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("Console password"), "was: {stdout}");
    assert!(stdout.contains("skipping systemctl"), "was: {stdout}");

    assert!(
        stdout.contains("not this host's user database"),
        "was: {stdout}"
    );

    let unit_file = |name: &str| {
        fs::read_to_string(tmp.path().join("etc/systemd/system").join(name))
            .unwrap_or_else(|_| panic!("no {name} written"))
    };
    // Directive lines only: the units' own comments name directives to
    // explain them, so a plain substring search finds them in the prose.
    let directives = |unit: &str| -> Vec<String> {
        unit.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(String::from)
            .collect()
    };
    let console = directives(&unit_file("stop-bots-web.service"));
    let socket = tmp.path().join("run/stop-bots/helper.sock");
    let exec = format!(
        "ExecStart={} web --db {} --helper {}",
        binary.display(),
        tmp.path().join("var/lib/stop-bots/db.sqlite3").display(),
        socket.display()
    );
    for line in [exec.as_str(), "User=stop-bots", "ProtectSystem=strict"] {
        assert!(
            console.contains(&line.to_string()),
            "no {line}: {console:?}"
        );
    }
    assert!(
        directives(&unit_file("stop-bots-helper.socket"))
            .contains(&format!("ListenStream={}", socket.display())),
        "the socket listens somewhere else"
    );
    // The helper's: strict, with the NGINX config given back. Without that
    // line the first NGINX apply fails on a read-only /etc. Under the
    // prefix, like every other path in the unit.
    let helper = directives(&unit_file("stop-bots-helper.service"));
    let nginx = format!("ReadWritePaths=-{}", tmp.path().join("etc/nginx").display());
    for line in ["ProtectSystem=strict", nginx.as_str()] {
        assert!(helper.contains(&line.to_string()), "no {line}: {helper:?}");
    }
    assert!(tmp.path().join("var/lib/stop-bots/db.sqlite3").exists());
    assert!(tmp.path().join("etc/stop-bots").is_dir());
}

/// Re-running must not issue a second password: the first one was printed
/// once and written down, and silently replacing it locks the operator out
/// of their own console.
#[test]
fn install_web_run_twice_keeps_the_first_password() {
    let (tmp, binary) = fake_debian_root();
    let args = [
        "install",
        "web",
        "--prefix",
        tmp.path().to_str().unwrap(),
        "--binary",
        binary.to_str().unwrap(),
    ];

    let first = stop_bots_bin().args(args).assert().success();
    let first = String::from_utf8(first.get_output().stdout.clone()).unwrap();
    let second = stop_bots_bin().args(args).assert().success();
    let second = String::from_utf8(second.get_output().stdout.clone()).unwrap();

    assert!(first.contains("Console password"));
    assert!(
        !second.contains("Console password"),
        "a second password was issued: {second}"
    );
    assert!(second.contains("already set"), "was: {second}");
    assert!(second.contains("already up to date"), "was: {second}");
}

/// A service is where the proxy settings are needed, and a second
/// `stop-bots web` to set them would collide with the one systemd runs.
#[test]
fn install_web_stores_the_proxy_settings() {
    let (tmp, binary) = fake_debian_root();
    stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
            "--trust-forwarded-for",
            "true",
            "--secure-cookie",
            "true",
        ])
        .assert()
        .success();

    let db = stop_bots::db::Db::open(tmp.path().join("var/lib/stop-bots/db.sqlite3")).unwrap();
    assert!(db
        .get_bool_setting(stop_bots::web::TRUST_FORWARDED_KEY, false)
        .unwrap());
    assert!(db
        .get_bool_setting(stop_bots::web::SECURE_COOKIE_KEY, false)
        .unwrap());
}

/// `install web --root` is stored, as `set-nginx-commands --root` would
/// store it, in the host settings file the helper reads the root from —
/// and the helper's unit lets the helper, which writes NGINX config for the
/// console, write there. It used to be accepted and then ignored.
#[test]
fn install_web_stores_the_nginx_root_and_lets_the_helper_write_it() {
    let (tmp, binary) = fake_debian_root();
    stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
            "--root",
            "/srv/nginx",
        ])
        .assert()
        .success();

    let host =
        stop_bots::hostconf::HostConf::load_from(&tmp.path().join("etc/stop-bots/host.conf"))
            .unwrap();
    assert_eq!(host.root(None), std::path::PathBuf::from("/srv/nginx"));
    let unit = fs::read_to_string(
        tmp.path()
            .join("etc/systemd/system/stop-bots-helper.service"),
    )
    .unwrap();
    assert!(
        unit.lines()
            .any(|line| line == "ReadWritePaths=-/srv/nginx"),
        "unit was:\n{unit}"
    );
}

/// `install web --ssh-log` is stored where the helper reads the SSH log
/// from, as `set-log-paths --ssh-log` would store it, and not handed to
/// the console, which reads no log itself.
#[test]
fn install_web_stores_the_ssh_log_for_the_helper_not_the_console() {
    let (tmp, binary) = fake_debian_root();
    stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
            "--ssh-log",
            "/srv/logs/auth.log",
        ])
        .assert()
        .success();

    let host =
        stop_bots::hostconf::HostConf::load_from(&tmp.path().join("etc/stop-bots/host.conf"))
            .unwrap();
    assert_eq!(
        host.ssh_log.as_deref(),
        Some(std::path::Path::new("/srv/logs/auth.log"))
    );
    let unit =
        fs::read_to_string(tmp.path().join("etc/systemd/system/stop-bots-web.service")).unwrap();
    assert!(!unit.contains("--ssh-log"), "unit was:\n{unit}");
}

/// A console given the helper reads its logs through it, from the host
/// settings; a log named on its command line would be ignored, so it is
/// refused.
#[test]
fn a_console_with_a_helper_refuses_an_ssh_log_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    stop_bots_bin()
        .args(["web", "--db"])
        .arg(tmp.path().join("db.sqlite3"))
        .args(["--helper"])
        .arg(tmp.path().join("helper.sock"))
        .args(["--ssh-log", "/var/log/auth.log"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--ssh-log cannot be combined with --helper",
        ));
}

/// `--dry-run` has to be trustworthy or nobody will use it on the one
/// command in this project that starts a daemon.
#[test]
fn install_web_dry_run_changes_nothing() {
    let (tmp, binary) = fake_debian_root();

    stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing was changed"));

    assert!(!tmp
        .path()
        .join("etc/systemd/system/stop-bots-web.service")
        .exists());
    assert!(!tmp.path().join("var/lib/stop-bots").exists());
    assert!(!tmp.path().join("etc/stop-bots").exists());
}

/// A host that is not Debian is told so rather than given a unit nobody
/// has checked.
#[test]
fn install_web_refuses_a_host_that_is_not_debian() {
    let (tmp, binary) = fake_debian_root();
    fs::remove_file(tmp.path().join("etc/debian_version")).unwrap();

    stop_bots_bin()
        .args([
            "install",
            "web",
            "--prefix",
            tmp.path().to_str().unwrap(),
            "--binary",
            binary.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not look like Debian"));
}

// ---- uninstall ----

/// The host back as it was, end to end through the binary: an applied
/// site, a generated `conf.d` file, both units and a firewall script go;
/// the site file is byte for byte what it was before; the database stays,
/// and the output says where.
///
/// Under `--prefix`, which runs no system program; the unit tests in
/// `uninstall` cover the host's path with fakes, and the container suite
/// with the real ones.
#[test]
fn uninstall_takes_a_prefix_back_to_how_it_was() {
    let (tmp, _) = fake_debian_root();
    let prefix = tmp.path();
    let run = |args: &[&str]| {
        stop_bots_bin()
            .env("STOP_BOTS_NGINX_DIR", prefix.join("etc/stop-bots/nginx"))
            .env("STOP_BOTS_NGINX_CONF_D", prefix.join("etc/nginx/conf.d"))
            .args(args)
            .args([
                "--db",
                prefix
                    .join("var/lib/stop-bots/db.sqlite3")
                    .to_str()
                    .unwrap(),
            ])
            .assert()
            .success()
    };
    let root = prefix.join("etc/nginx");
    fs::create_dir_all(root.join("conf.d")).unwrap();
    fs::create_dir_all(root.join("sites-enabled")).unwrap();
    let site = write_site(&root.join("sites-enabled"), "example.com");
    let before = fs::read_to_string(&site).unwrap();
    // First, while there is no database for it to read the backend from.
    run(&["install", "firewall", "--prefix", prefix.to_str().unwrap()]);
    // Rate limiting alone is a block and a generated `conf.d` file.
    run(&["set-rate-limit", "--enabled", "true"]);
    run(&[
        "apply-blocks",
        "--root",
        root.to_str().unwrap(),
        "--no-reload",
    ]);
    fs::create_dir_all(prefix.join("etc/stop-bots")).unwrap();
    let units = prefix.join("etc/systemd/system");
    fs::write(
        units.join("stop-bots-web.service"),
        include_str!("fixtures/units/stop-bots-web-0.0.15.service"),
    )
    .unwrap();
    fs::write(prefix.join("etc/stop-bots/firewall.nft"), "table\n").unwrap();
    assert_ne!(
        fs::read_to_string(&site).unwrap(),
        before,
        "nothing was applied"
    );

    let out = run(&["uninstall", "--prefix", prefix.to_str().unwrap()]);

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert_eq!(fs::read_to_string(&site).unwrap(), before, "{stdout}");
    for gone in [
        root.join("conf.d/stop-bots-limits.conf"),
        units.join("stop-bots-web.service"),
        units.join("stop-bots-firewall.service"),
        prefix.join("etc/stop-bots"),
    ] {
        assert!(
            !gone.exists(),
            "{} is still there:\n{stdout}",
            gone.display()
        );
    }
    let db = prefix.join("var/lib/stop-bots/db.sqlite3");
    assert!(db.exists(), "the database went without --purge");
    assert!(
        stdout.contains(&db.display().to_string()),
        "the output must say where the database is:\n{stdout}"
    );
}

/// `--purge` deletes the database, which every part still installed reads.
#[test]
fn uninstall_refuses_purge_on_a_part() {
    let tmp = tempfile::tempdir().unwrap();
    stop_bots_bin()
        .args([
            "uninstall",
            "nginx",
            "--purge",
            "--prefix",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("only with `all`"));
}

/// Half an uninstall is worse than none, and permissions are where one
/// would stop half way. Skipped when the suite runs as root, where this
/// would uninstall the machine running it.
#[test]
fn uninstall_needs_root_without_a_prefix() {
    if unsafe { libc::geteuid() } == 0 {
        stop_bots::say_err!(
            "skipped: running as root, where this would uninstall the test machine"
        );
        return;
    }
    stop_bots_bin()
        .args(["uninstall", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs root"));
}

// ---- auto-apply ----

/// The switch itself: off by default, settable, and readable back.
///
/// Off by default is the part worth pinning. Every other default in this
/// project decides what gets *written*; this one decides whether a machine
/// reloads a live web server unattended, and an upgrade must not start
/// doing that on an existing install's behalf.
#[test]
fn auto_apply_is_off_until_it_is_turned_on() {
    let fixture = Fixture::new();

    fixture.run(&["set-auto-apply", "--enabled", "false"]);
    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert!(!db.get_auto_apply().unwrap(), "off is the default");
    drop(db);

    fixture
        .run(&["set-auto-apply", "--enabled", "true"])
        .stdout(predicate::str::contains("enabled"))
        // Says where the work happens, because it is not in this command:
        // setting this on a host with no console running turns on a switch
        // nothing will ever read.
        .stdout(predicate::str::contains("internal cron"));

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert!(db.get_auto_apply().unwrap());
}

/// Turning the switch on must not, by itself, apply anything: the CLI sets
/// a flag, and the internal cron inside the console or the TUI is what acts
/// on it. A `set-` command that quietly reloaded NGINX would be the worst
/// possible surprise from a subcommand whose name says "set".
#[test]
fn setting_auto_apply_does_not_itself_touch_any_config() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");
    let before = fs::read_to_string(&site).unwrap();

    fixture.run(&["set-auto-apply", "--enabled", "true"]);

    assert_eq!(
        fs::read_to_string(&site).unwrap(),
        before,
        "setting the switch rewrote a site config"
    );
}

/// The switched-on path, end to end: a config that has fallen behind the
/// database is rewritten, and a second pass finds nothing left to do.
///
/// Lives here rather than beside the code because `apply_all_sites`
/// resolves its managed directory from `STOP_BOTS_NGINX_DIR`, and with
/// robots.txt and rate limiting both off it *deletes* the files that
/// variable points at — `/etc/stop-bots/nginx/robots.txt` on a machine
/// that has one. So the variable has to be pointed somewhere safe, and it
/// is process-global: setting it in the library's own test binary breaks
/// `nginx::tests::kitchen_sink_block_matches_the_golden`, whose golden
/// holds the default path. Nothing in this binary reads that default —
/// every other test here passes the directory to a spawned command — so
/// this is the one place it can be set without breaking something else.
///
/// `reload` is false: a test that let the reload through would run
/// `systemctl reload nginx` on whatever machine hosts the suite.
#[test]
fn auto_apply_rewrites_a_stale_site_config_and_then_leaves_it_alone() {
    let fixture = Fixture::new();
    fixture.seed_bots();
    let site = fixture.write_site("example.com");
    // Both, for the same reason: this test drives `cron::apply_nginx`
    // in-process, so there is no child to pass them to.
    unsafe { std::env::set_var("STOP_BOTS_NGINX_DIR", &fixture.managed) };
    unsafe { std::env::set_var("STOP_BOTS_NGINX_CONF_D", &fixture.conf_d) };

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    db.set_auto_apply(true).unwrap();

    let summary =
        stop_bots::cron::apply_nginx(&db, &Default::default(), &fixture.nginx_root, false);
    assert!(summary.contains("applied 1 site(s)"), "{summary}");
    assert!(summary.contains("1 file(s) changed"), "{summary}");
    // The reload is the step with a side effect outside this project's
    // files, so "did not reload" is said rather than assumed.
    assert!(summary.contains("not reloaded"), "{summary}");
    assert!(
        fs::read_to_string(&site).unwrap().contains("stop-bots"),
        "the generated block never reached the config"
    );

    // Nothing changed on disk the second time, so there is nothing to
    // reload — what makes an hourly job cheap on a quiet host.
    let again = stop_bots::cron::apply_nginx(&db, &Default::default(), &fixture.nginx_root, false);
    assert_eq!(again, "1 site(s) already up to date");
}

/// The firewall switch is separate from the NGINX one, and separately off.
/// Two switches because the risks are not comparable — a bad NGINX config
/// costs a failed `nginx -t`, a bad firewall ruleset costs the host.
#[test]
fn the_firewall_auto_apply_switch_is_independent_of_the_nginx_one() {
    let fixture = Fixture::new();

    fixture.run(&["set-auto-apply", "--enabled", "true"]);

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert!(db.get_auto_apply().unwrap());
    assert!(
        !db.get_auto_apply_firewall().unwrap(),
        "the NGINX switch must not turn the firewall one on"
    );
    drop(db);

    fixture
        .run(&["set-auto-apply-firewall", "--enabled", "true"])
        .stdout(predicate::str::contains("enabled"))
        // The condition that most often stops it doing anything, said when
        // it is switched on rather than discovered in a summary a day on.
        .stdout(predicate::str::contains("anti-lockout"));

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert!(db.get_auto_apply_firewall().unwrap());
}

// ---- maintenance ----

/// The flow an admin reaches for after looking at `du`: prune what has
/// accumulated, hand the free pages back, and say what moved.
///
/// Seeds a user agent last seen well outside the 90-day window, which is
/// the row the scheduled job exists to remove.
#[test]
fn maintain_prunes_stale_user_agents_and_reports_the_size() {
    let fixture = Fixture::new();
    let stale = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - (120 * 24 * 60 * 60);
    {
        let db = stop_bots::db::Db::open(&fixture.db).unwrap();
        let mut counts = std::collections::HashMap::new();
        counts.insert("long-gone-crawler".to_string(), 7);
        db.record_user_agent_hits(&counts, stale).unwrap();
    }

    fixture
        .run(&["maintain"])
        .stdout(predicate::str::contains("pruned 1 stale user agents"))
        .stdout(predicate::str::contains("Database is"));

    let db = stop_bots::db::Db::open(&fixture.db).unwrap();
    assert!(
        db.list_user_agent_stats().unwrap().is_empty(),
        "the stale row should be gone"
    );
    assert!(
        db.get_cron_last_run(stop_bots::cron::CronJob::Maintenance.id())
            .unwrap()
            .is_some(),
        "running it by hand should satisfy the internal cron's schedule too"
    );
}

/// `--force-compact` rewrites the file whatever the thresholds say. The
/// claim under test is that the flag reaches `VACUUM` at all — a fixture
/// database has nothing worth reclaiming, so the scheduled job would
/// (correctly) leave it alone and the line would never appear.
#[test]
fn maintain_can_be_told_to_compact_regardless() {
    let fixture = Fixture::new();

    fixture
        .run(&["maintain", "--force-compact"])
        .stdout(predicate::str::contains("Compacted anyway"));
}

// ---- command-line conventions ----

/// `--db` is global: it works before the subcommand as well as after it,
/// which is where every test above puts it.
#[test]
fn db_may_come_before_the_subcommand() {
    let fx = Fixture::new();
    stop_bots_bin()
        .args([
            "--db",
            fx.db.to_str().unwrap(),
            "trust",
            "--address",
            "203.0.113.7",
        ])
        .assert()
        .success();
    fx.run(&["list-trusted"])
        .stdout(predicate::str::contains("203.0.113.7"));
}

/// `STOP_BOTS_DB` names the database when no `--db` does, and an explicit
/// `--db` still wins over it.
#[test]
fn stop_bots_db_names_the_database_and_db_overrides_it() {
    let fx = Fixture::new();
    let other = fx.db.with_file_name("other.sqlite3");

    stop_bots_bin()
        .env("STOP_BOTS_DB", &fx.db)
        .args(["trust", "--address", "203.0.113.7"])
        .assert()
        .success();
    fx.run(&["list-trusted"])
        .stdout(predicate::str::contains("203.0.113.7"));

    stop_bots_bin()
        .env("STOP_BOTS_DB", &fx.db)
        .args(["list-trusted", "--db", other.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing is trusted."));
}

/// Renamed for 0.1; the old spelling works for one release.
#[test]
fn block_scanners_still_works_under_its_old_name() {
    let fx = Fixture::new();
    let log = fx.db.with_file_name("auth.log");
    fs::write(&log, repeat_failed_attempt("198.51.100.9", 25)).unwrap();

    for command in ["block-ssh-scanners", "block-scanners"] {
        fx.run(&[command, "--ssh-log", log.to_str().unwrap(), "--dry-run"])
            .stdout(predicate::str::contains("Would block 198.51.100.9"));
    }
}

/// `web`'s old setting flags still store what they used to, and say what
/// replaces them.
#[test]
fn web_s_deprecated_setting_flags_still_store_and_say_so() {
    let fx = Fixture::new();
    fx.cmd(&[
        "web",
        "--allowed-hosts",
        "admin.example.com",
        "--set-password",
    ])
    .assert()
    .success()
    .stderr(predicate::str::contains("deprecated"))
    .stderr(predicate::str::contains("set-web"));

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert_eq!(
        db.get_text_setting(stop_bots::web::ALLOWED_HOSTS_KEY)
            .unwrap()
            .as_deref(),
        Some("admin.example.com")
    );
}

// ---- detectors ----

/// The switch, TTL and threshold `set-detector` stores are the ones the
/// scheduled passes read, and `list-detectors` shows them.
#[test]
fn set_detector_stores_the_switch_ttl_and_threshold() {
    use stop_bots::protection::Detector;
    let fx = Fixture::new();

    fx.run(&[
        "set-detector",
        "injection",
        "--enabled",
        "false",
        "--ttl-days",
        "3",
    ]);
    fx.run(&[
        "set-detector",
        "ssh-scanners",
        "--threshold",
        "40",
        "--window-hours",
        "6",
    ]);

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert_eq!(Detector::SshScanners.window_hours(&db).unwrap(), 6);
    assert!(!Detector::Injection.is_enabled(&db).unwrap());
    assert_eq!(Detector::Injection.ttl_days(&db).unwrap(), 3);
    assert_eq!(Detector::SshScanners.threshold(&db).unwrap(), Some(40));

    let listing = fx.run(&["list-detectors"]);
    let out = String::from_utf8(listing.get_output().stdout.clone()).unwrap();
    let line = |name: &str| {
        out.lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} line in:\n{out}"))
            .split_whitespace()
            .take(5)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        line("injection"),
        ["injection", "off", "3d", "-", "24h"],
        "{out}"
    );
    assert_eq!(
        line("ssh-scanners"),
        ["ssh-scanners", "on", "5d", "40", "6h"],
        "{out}"
    );
}

/// Running a detector by hand uses the stored threshold unless told
/// otherwise, so it finds what the scheduled pass would.
#[test]
fn a_detector_run_by_hand_uses_the_stored_threshold() {
    let fx = Fixture::new();
    let log = fx.db.with_file_name("access.log");
    let lines: String = (0..4)
        .map(|i| access_line("203.0.113.9", &format!("/missing-{i}"), 404, "UA"))
        .collect();
    fs::write(&log, lines).unwrap();
    let log = log.to_str().unwrap();

    fx.run(&["block-web-scanners", "--access-log", log, "--dry-run"])
        .stdout(predicate::str::contains("No scanning IPs found"));

    fx.run(&["set-detector", "web-scanners", "--threshold", "4"]);
    fx.run(&["block-web-scanners", "--access-log", log, "--dry-run"])
        .stdout(predicate::str::contains("Would block 203.0.113.9"));
}

/// Each refusal names why, and a command with one bad flag changes
/// nothing — not even the flags beside it that were fine.
#[test]
fn set_detector_refuses_what_it_cannot_store_and_stores_nothing() {
    use stop_bots::protection::Detector;
    let fx = Fixture::new();

    for (args, why) in [
        (
            vec![
                "set-detector",
                "probe-paths",
                "--threshold",
                "5",
                "--ttl-days",
                "9",
            ],
            "no threshold",
        ),
        (
            vec![
                "set-detector",
                "web-scanners",
                "--threshold",
                "1",
                "--ttl-days",
                "9",
            ],
            "threshold under 2",
        ),
        (
            vec!["set-detector", "web-scanners", "--ttl-days", "0"],
            "from 1 to",
        ),
        (
            vec![
                "set-detector",
                "web-scanners",
                "--window-hours",
                "0",
                "--ttl-days",
                "9",
            ],
            "from 1 to 720 hours",
        ),
        (
            vec!["set-detector", "robots-txt", "--enabled", "true"],
            "set-humans-only",
        ),
    ] {
        fx.cmd(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(why));
    }

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    for d in [Detector::ProbePaths, Detector::WebScanners] {
        assert_eq!(d.ttl_days(&db).unwrap(), d.spec().ttl_days_default);
    }
    assert_eq!(Detector::WebScanners.threshold(&db).unwrap(), Some(7));
    assert!(!Detector::RobotsTxt.is_enabled(&db).unwrap());
}

#[test]
fn set_subnet_escalation_switches_it_on_with_a_threshold() {
    let fx = Fixture::new();
    fx.run(&["set-subnet-escalation", "--enabled", "true", "--min", "4"])
        .stdout(predicate::str::contains("escalation is on, at 4"));

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert_eq!(
        stop_bots::protection::subnet_escalation(&db).unwrap(),
        Some(4)
    );
    fx.run(&["list-detectors"])
        .stdout(predicate::str::contains("escalation: on, at 4"));

    fx.run(&["set-subnet-escalation", "--enabled", "false"]);
    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert_eq!(stop_bots::protection::subnet_escalation(&db).unwrap(), None);
}

// ---- categories and bots ----

/// The host-wide category policy reaches the generated NGINX block: an
/// allowed category's bots are no longer in it.
#[test]
fn set_category_changes_what_apply_blocks_writes() {
    let fx = Fixture::new();
    fx.seed_bots();
    let site = fx.write_site("example.com");
    fx.scan_sites();

    fx.run(&["set-category", "--category", "ai", "--policy", "blocked"]);
    fx.apply_blocks();
    let config = fs::read_to_string(&site).unwrap();
    assert!(config.contains("AISearchBot"), "config was:\n{config}");

    fx.run(&["set-category", "--category", "ai", "--policy", "allowed"])
        .stdout(predicate::str::contains(
            "Category ai is now allowed host-wide.",
        ));
    fx.apply_blocks();
    let config = fs::read_to_string(&site).unwrap();
    assert!(!config.contains("AISearchBot"), "config was:\n{config}");
}

/// A site override is stored per site, shown by `list-categories
/// --site`, and removed again by `default` — which on its own, host-wide,
/// is refused, since there is nothing above the host to follow.
#[test]
fn set_category_with_a_site_overrides_it_for_that_site_only() {
    use stop_bots::db::{Category, Policy};
    let fx = Fixture::new();
    fx.write_site("example.com");
    fx.scan_sites();

    fx.run(&[
        "set-category",
        "--category",
        "search",
        "--policy",
        "blocked",
        "--site",
        "example.com",
    ]);
    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    let site = db.list_sites().unwrap()[0].id;
    assert_eq!(
        db.get_site_category_override(site, Category::Search)
            .unwrap(),
        Some(Policy::Blocked)
    );
    fx.run(&["list-categories", "--site", "example.com"])
        .stdout(predicate::str::is_match(r"search\s+\w+\s+blocked").unwrap());

    fx.run(&[
        "set-category",
        "--category",
        "search",
        "--policy",
        "default",
        "--site",
        "example.com",
    ]);
    assert_eq!(
        db.get_site_category_override(site, Category::Search)
            .unwrap(),
        None
    );

    fx.cmd(&[
        "set-category",
        "--category",
        "search",
        "--policy",
        "default",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("nothing above it"));
}

#[test]
fn set_bot_overrides_one_bot_host_wide_and_per_site() {
    use stop_bots::db::{BotStatus, Policy};
    let fx = Fixture::new();
    fx.seed_bots();
    fx.write_site("example.com");
    fx.scan_sites();

    fx.run(&["list-bots", "--search", "search-bot"])
        .stdout(predicate::str::contains("ai-search-bot"))
        .stdout(predicate::str::contains("google-crawler").not());

    fx.run(&["set-bot", "--bot", "ai-search-bot", "--policy", "allowed"]);
    fx.run(&[
        "set-bot",
        "--bot",
        "google-crawler",
        "--policy",
        "blocked",
        "--site",
        "example.com",
    ]);

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    let bots = db.list_bots().unwrap();
    let bot = |slug: &str| bots.iter().find(|b| b.slug == slug).unwrap().clone();
    assert_eq!(bot("ai-search-bot").status, BotStatus::Allowed);
    let site = db.list_sites().unwrap()[0].id;
    let overrides = db.site_bot_overrides(site).unwrap();
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].bot_id, bot("google-crawler").id);
    assert_eq!(overrides[0].policy, Policy::Blocked);
    fx.run(&["list-categories", "--site", "example.com"])
        .stdout(predicate::str::is_match(r"google-crawler\s+blocked").unwrap());

    fx.run(&["set-bot", "--bot", "ai-search-bot", "--policy", "default"]);
    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    let back = db.list_bots().unwrap();
    assert_eq!(
        back.iter()
            .find(|b| b.slug == "ai-search-bot")
            .unwrap()
            .status,
        BotStatus::Default
    );

    fx.cmd(&["set-bot", "--bot", "no-such-bot", "--policy", "blocked"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("list-bots"));
}

// ---- firewall ----

/// A disabled rule stays listed and is left out of the script; enabling
/// it puts it back.
#[test]
fn set_firewall_rule_takes_a_rule_out_of_the_script_without_deleting_it() {
    let fx = Fixture::new();
    let script = fx.db.with_file_name("fw.nft");
    let render = || {
        fx.run(&[
            "render-firewall",
            "--out",
            script.to_str().unwrap(),
            "--ssh-log",
            "tests/fixtures/logs/auth.log",
        ]);
        fs::read_to_string(&script).unwrap()
    };
    fx.run(&["add-firewall-rule", "--address", "198.51.100.7"]);

    fx.run(&["set-firewall-rule", "--id", "1", "--enabled", "false"]);
    fx.run(&["list-firewall-rules"])
        .stdout(predicate::str::contains("198.51.100.7 (disabled)"));
    let written = render();
    assert!(!written.contains("198.51.100.7"), "script was:\n{written}");

    fx.run(&["set-firewall-rule", "--id", "1", "--enabled", "true"]);
    let written = render();
    assert!(written.contains("198.51.100.7"), "script was:\n{written}");

    fx.cmd(&["set-firewall-rule", "--id", "99", "--enabled", "false"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no firewall rule with id: 99"));
}

/// Without `--backend`, `render-firewall` writes for the backend this
/// host is set to, rather than refusing to run.
#[test]
fn render_firewall_follows_the_stored_backend() {
    let fx = Fixture::new();
    let script = fx.db.with_file_name("fw.out");
    fx.run(&["add-firewall-rule", "--address", "198.51.100.7"]);

    fx.run(&["set-firewall-backend", "--backend", "iptables"])
        .stdout(predicate::str::contains("firewall.sh"));
    fx.run(&[
        "render-firewall",
        "--out",
        script.to_str().unwrap(),
        "--ssh-log",
        "tests/fixtures/logs/auth.log",
    ])
    .stdout(predicate::str::contains("firewall.sh"));
    let written = fs::read_to_string(&script).unwrap();
    assert!(
        written.contains("-A STOP-BOTS -s 198.51.100.7 -j DROP"),
        "script was:\n{written}"
    );
}

/// A script written with `--out` is not the one the boot unit loads, so
/// running it by hand would last until the next reboot. The hint names
/// the apply that does persist, not `nft -f` on the file.
#[test]
fn render_firewall_out_points_at_apply_not_a_hand_run() {
    let fx = Fixture::new();
    let script = fx.db.with_file_name("fw.out");
    fx.run(&["add-firewall-rule", "--address", "198.51.100.7"]);
    fx.run(&["set-firewall-backend", "--backend", "nftables"]);

    let output = fx.run(&[
        "render-firewall",
        "--out",
        script.to_str().unwrap(),
        "--ssh-log",
        "tests/fixtures/logs/auth.log",
    ]);
    let stdout = String::from_utf8_lossy(&output.get_output().stdout).to_string();
    assert!(
        stdout.contains("stop-bots render-firewall --apply"),
        "the hint does not name the apply:\n{stdout}"
    );
    assert!(
        !stdout.contains("nft -f"),
        "the hint still suggests running the file by hand:\n{stdout}"
    );
}

// ---- the web console's settings ----

#[test]
fn set_web_stores_every_console_setting() {
    let fx = Fixture::new();
    fx.run(&[
        "set-web",
        "--bind",
        "0.0.0.0:8788",
        "--expose",
        "true",
        "--base-path",
        "/stop-bots/",
        "--allowed-hosts",
        "admin.example.com",
        "--trust-forwarded-for",
        "true",
        "--secure-cookie",
        "true",
    ]);

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    let text = |key| db.get_text_setting(key).unwrap();
    let flag = |key| db.get_bool_setting(key, false).unwrap();
    use stop_bots::web::*;
    assert_eq!(text(BIND_KEY).as_deref(), Some("0.0.0.0:8788"));
    assert_eq!(text(BASE_PATH_KEY).as_deref(), Some("/stop-bots"));
    assert_eq!(
        text(ALLOWED_HOSTS_KEY).as_deref(),
        Some("admin.example.com")
    );
    assert!(flag(EXPOSE_KEY));
    assert!(flag(TRUST_FORWARDED_KEY));
    assert!(flag(SECURE_COOKIE_KEY));

    // `false` switches them back off rather than being ignored.
    fx.run(&[
        "set-web",
        "--trust-forwarded-for",
        "false",
        "--secure-cookie",
        "false",
    ]);
    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert!(!db.get_bool_setting(TRUST_FORWARDED_KEY, true).unwrap());
    assert!(!db.get_bool_setting(SECURE_COOKIE_KEY, true).unwrap());

    fx.run(&["set-web"])
        .stdout(predicate::str::contains("0.0.0.0:8788"))
        .stdout(predicate::str::contains("admin.example.com"));
}

/// The same check `web` makes before it binds, made before anything is
/// stored — and a refusal stores none of the other flags either.
#[test]
fn set_web_refuses_a_network_bind_without_exposure_and_stores_nothing() {
    let fx = Fixture::new();
    fx.cmd(&[
        "set-web",
        "--bind",
        "0.0.0.0:8788",
        "--allowed-hosts",
        "admin.example.com",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("--expose true"));

    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    assert_eq!(db.get_text_setting(stop_bots::web::BIND_KEY).unwrap(), None);
    assert_eq!(
        db.get_text_setting(stop_bots::web::ALLOWED_HOSTS_KEY)
            .unwrap(),
        None
    );
}

/// A `Command` carrying `args`, for the tests above that need one without
/// the `Fixture`'s own NGINX directories. Still not the host's: see
/// [`common::stop_bots_bin`].
fn stop_bots_cmd(args: &[&str]) -> Command {
    let mut cmd = stop_bots_bin();
    cmd.args(args);
    cmd
}

// ---- upgrading from 0.0.x ----

/// A database as 0.0.15 left it, restored from its SQL dump into `db`.
fn restore_0_0_15(db: &Path) {
    let sql = fs::read_to_string("tests/fixtures/db/db-0.0.15.sql").unwrap();
    // No fsyncs for the setup: it is not what is under test.
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch("PRAGMA synchronous = OFF").unwrap();
    conn.execute_batch(&sql).unwrap();
}

/// The upgrade an operator actually does: install the new package, run
/// any command. The rules are all still there, and the database as it
/// was sits next to it.
#[test]
fn the_first_command_after_an_upgrade_keeps_the_rules_and_leaves_a_copy() {
    let fixture = Fixture::new();
    restore_0_0_15(&fixture.db);

    fixture.run(&["list-firewall-rules"]).stdout(
        predicate::str::contains("203.0.113.7")
            .and(predicate::str::contains("198.51.100.0/24"))
            .and(predicate::str::contains("203.0.113.99")),
    );

    let mut backup = fixture.db.as_os_str().to_owned();
    backup.push(".bak-v0");
    assert!(
        Path::new(&backup).exists(),
        "no pre-upgrade copy at {backup:?}"
    );
}

/// Going back a version must not quietly misread what the newer one
/// wrote: it stops, and says which two versions disagree.
#[test]
fn a_database_from_a_newer_release_is_refused() {
    let fixture = Fixture::new();
    fixture.run(&["list-firewall-rules"]);
    rusqlite::Connection::open(&fixture.db)
        .unwrap()
        .pragma_update(None, "user_version", 999)
        .unwrap();

    fixture
        .cmd(&["list-firewall-rules"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("schema version 999").and(predicate::str::contains(format!(
                "versions up to {}",
                stop_bots::db::schema::CURRENT_VERSION
            ))),
        );
}

/// The CLI answer to "why is this blocked, and undo all of it": a
/// detector's block names the detector and the request, can be listed and
/// removed by source, and stays removed when the detector runs again over
/// the same log.
#[test]
fn a_detector_s_blocks_are_listed_and_removed_by_source_and_stay_removed() {
    let fx = Fixture::new();
    let log = fx.db.with_file_name("access.log");
    fs::write(
        &log,
        "203.0.113.5 - - [28/Sep/2026:10:00:00 +0000] \"GET /.env HTTP/1.1\" 404 0 \"-\" \"curl/8\"\n\
         203.0.113.6 - - [28/Sep/2026:10:00:01 +0000] \"GET /.git/config HTTP/1.1\" 404 0 \"-\" \"curl/8\"\n",
    )
    .unwrap();
    let log = log.to_str().unwrap();
    fx.run(&["block-probe-paths", "--access-log", log]);
    fx.run(&["add-firewall-rule", "--address", "192.0.2.1"]);

    fx.run(&["list-firewall-rules", "--source", "probe-paths"])
        .stdout(predicate::str::contains(
            "[probe-paths, added 1m ago]: \"GET /.env HTTP/1.1\" 404",
        ))
        .stdout(predicate::str::contains("192.0.2.1").not());

    fx.run(&[
        "remove-firewall-rule",
        "--source",
        "probe-paths",
        "--dry-run",
    ])
    .stdout(predicate::str::contains("Would remove 2 firewall rule(s)"));
    fx.run(&["remove-firewall-rule", "--source", "probe-paths"])
        .stdout(predicate::str::contains(
            "Removed 2 firewall rule(s) from probe-paths.",
        ));

    fx.run(&["block-probe-paths", "--access-log", log])
        .stdout(predicate::str::contains("Left 2 alone: unblocked by hand"));
    fx.run(&["list-firewall-rules"])
        .stdout(predicate::str::contains("#3 Block 192.0.2.1 [cli"))
        .stdout(predicate::str::contains("203.0.113").not());
}

/// A 0.0.x rule has no source, and says so rather than guessing one.
#[test]
fn a_rule_from_before_0_1_is_listed_and_removed_as_before_0_1() {
    let fx = Fixture::new();
    let conn = rusqlite::Connection::open(&fx.db).unwrap();
    conn.execute_batch(include_str!("fixtures/db/db-0.0.15.sql"))
        .unwrap();
    drop(conn);

    fx.run(&["list-firewall-rules", "--source", "before-0.1"])
        .stdout(predicate::str::contains("#1 Block 203.0.113.7 [before-0.1"));
    fx.run(&["remove-firewall-rule", "--source", "before-0.1"])
        .stdout(predicate::str::contains("Removed 5 firewall rule(s)"));
}

#[test]
fn remove_firewall_rule_needs_an_id_or_a_source_and_names_the_sources() {
    let fx = Fixture::new();
    fx.cmd(&["remove-firewall-rule"]).assert().failure();
    fx.cmd(&["remove-firewall-rule", "--source", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ssh-scanners"));
}

/// `stop-bots list-bots | head -1` — a reader that stops early — used to
/// end in a panic ("failed printing to stdout: Broken pipe", exit 101).
/// A one-shot command now dies quietly of SIGPIPE, as every other
/// command-line tool does.
#[test]
fn output_piped_into_a_reader_that_stops_early_is_not_a_panic() {
    use std::io::{BufRead, BufReader, Read};
    use std::os::unix::process::ExitStatusExt;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite3");
    let root = dir.path().join("nginx");
    fs::create_dir_all(&root).unwrap();
    // A dry run stores the built-in bot list — hundreds of rows, far more
    // than a pipe holds, so the command is still writing when the reader
    // goes away.
    stop_bots(&[
        "apply-blocks",
        "--dry-run",
        "--db",
        db.to_str().unwrap(),
        "--root",
        root.to_str().unwrap(),
    ]);
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("stop-bots"))
        .args(["list-bots", "--db", db.to_str().unwrap()])
        .env_remove("STOP_BOTS_DB")
        .env("STOP_BOTS_HOST_CONF", dir.path().join("host.conf"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    // The reader is gone; everything the command writes from here on hits
    // a closed pipe.
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();

    assert!(first.contains("NAME"), "first line was: {first:?}");
    assert!(!stderr.contains("panicked"), "stderr was:\n{stderr}");
    assert_ne!(status.code(), Some(101), "stderr was:\n{stderr}");
    assert!(
        status.success() || status.signal() == Some(libc::SIGPIPE),
        "status was {status:?}, stderr:\n{stderr}"
    );
}

/// A `--root` outside the stock NGINX tree is a tree of its own: the
/// http-context files go into its own `conf.d`, never the host's
/// `/etc/nginx/conf.d`. `batch --root <scratch>` used to read the host's
/// trust file from there, and an apply would have rewritten it.
///
/// No `STOP_BOTS_NGINX_CONF_D` here, unlike everywhere else: the override
/// is what hid this.
#[test]
fn a_scratch_root_keeps_its_generated_files_inside_itself() {
    let fx = Fixture::new();
    fx.write_site("scratch.example");
    let root = fx.nginx_root.to_str().unwrap();
    let run = |args: &[&str]| {
        fx.cmd(args)
            .env_remove("STOP_BOTS_NGINX_CONF_D")
            .assert()
            .success()
    };

    run(&["scan-sites", "--root", root]);
    run(&["trust", "--user-agent", "Pingdom.com_bot"]);
    run(&["apply-blocks", "--root", root, "--no-reload"]);

    let trust_file = fx.nginx_root.join("conf.d/stop-bots-trusted.conf");
    assert!(
        fs::read_to_string(&trust_file).is_ok_and(|body| body.contains("Pingdom")),
        "the trust file is not inside the root, at {}",
        trust_file.display()
    );
}

/// A blocked user agent or an exempt path stored by an older release
/// without today's checks is left out of the config, and `apply-blocks`
/// says which. Here, an exemption with a backslash in it: `/x\|` was a
/// match-all while exemptions were written verbatim.
#[test]
fn apply_blocks_names_what_it_leaves_out() {
    let fx = Fixture::new();
    fx.seed_bots();
    fx.write_site("old.example");
    fx.scan_sites();
    let db = rusqlite::Connection::open(&fx.db).unwrap();
    db.execute(
        "INSERT INTO site_path_exemptions (site_id, path) \
         SELECT id, '/x\\|' FROM sites WHERE server_name = 'old.example'",
        [],
    )
    .unwrap();
    drop(db);

    fx.apply_blocks().stderr(predicate::str::contains(
        r#"warning: left out of the NGINX config: old.example: exempt path "/x\\|""#,
    ));
    let site = fs::read_to_string(fx.nginx_root.join("old.example.conf")).unwrap();
    assert!(!site.contains("/x"), "the exemption was written:\n{site}");
}

// ---- nothing a database row or a log line holds drives the terminal ----

/// What a value would carry to take over the terminal of the root operator
/// who reads it, or to make what is printed read as something else. The
/// console runs unprivileged and writes the database these commands read,
/// and a client writes the logs, so each of these can arrive in a row.
const HOSTILE: &[(&str, &str)] = &[
    ("an OSC title change", "\u{1b}]0;PWNED\u{7}"),
    ("a CSI as the C1 U+009B", "\u{9b}31mred"),
    ("a bare ESC", "\u{1b}[2J"),
    ("a BEL", "bell\u{7}"),
    ("a DEL", "del\u{7f}"),
    ("a CR that prints over the start", "real\rfake"),
    ("a right-to-left override", "\u{202e}lmth.tob"),
    ("a zero-width space", "Google\u{200b}bot"),
];

/// Each of [`HOSTILE`], as one value inside some ordinary text, so that
/// every one of them reaches every field it is planted in.
fn hostile_values() -> impl Iterator<Item = String> {
    HOSTILE
        .iter()
        .enumerate()
        .map(|(n, (_, chars))| format!("h{n}{chars}x"))
}

/// Fails, naming the character and where, if `bytes` holds anything a
/// terminal acts on other than the program's own newlines and tabs: a C0
/// control, DEL, a C1 control (as its UTF-8 or as a lone byte, which is
/// not UTF-8 at all), or a bidirectional or zero-width character.
fn assert_terminal_safe(command: &str, stream: &str, bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).unwrap_or_else(|err| {
        panic!(
            "`{command}` wrote bytes that are not UTF-8 to {stream} ({err}):\n{}",
            String::from_utf8_lossy(bytes)
        )
    });
    for (at, c) in text.char_indices() {
        let drives_the_terminal = c.is_control() && !matches!(c, '\n' | '\t');
        let reorders_or_hides = stop_bots::present::is_invisible_format(c);
        assert!(
            !drives_the_terminal && !reorders_or_hides,
            "`{command}` wrote {c:?} to {stream} at byte {at}:\n{text}"
        );
    }
}

/// [`assert_terminal_safe`] on both streams of one run.
fn assert_output_terminal_safe(command: &str, output: &std::process::Output) {
    assert_terminal_safe(command, "stdout", &output.stdout);
    assert_terminal_safe(command, "stderr", &output.stderr);
}

impl Fixture {
    /// This fixture with every [`HOSTILE`] value planted where a `list-*`
    /// or `status` command prints it: in the rows themselves, written with
    /// plain SQL past every check, as the console could; in a site's
    /// server name; and in an access log that `host.conf` points at.
    fn hostile() -> Fixture {
        let fx = Fixture::new();
        let log = fx.nginx_root.join("access.log");
        let mut lines = String::new();
        for value in hostile_values() {
            lines += &serde_json::json!({
                "remote_addr": "203.0.113.5",
                "request_uri": format!("/{value}"),
                "status": "444",
                "http_user_agent": value,
            })
            .to_string();
            lines += "\n";
        }
        fs::write(&log, lines).unwrap();
        fs::write(
            fx.host_conf(),
            format!(
                "access_log = {}\nssh_log = {}/tests/fixtures/logs/auth.log\n",
                log.display(),
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .unwrap();
        // Through the library rather than the CLI: a run of the binary
        // costs a tenth of a second on a loaded machine, and this fixture
        // is set up five times.
        let db = stop_bots::db::Db::open(&fx.db).unwrap();
        db.set_block_response(stop_bots::db::BlockResponse::Close)
            .unwrap();
        let hits = hostile_values().map(|value| (value, 1)).collect();
        db.record_user_agent_hits(&hits, 0).unwrap();
        drop(db);

        let mut conn = rusqlite::Connection::open(&fx.db).unwrap();
        // One transaction: a commit per row is a sync per row, seconds.
        let db = conn.transaction().unwrap();
        db.execute(
            "INSERT OR IGNORE INTO sources (id, name, url) VALUES ('evil', 'evil', 'x')",
            [],
        )
        .unwrap();
        for (n, value) in hostile_values().enumerate() {
            let site_id: i64 = db
                .query_row(
                    "INSERT INTO sites (server_name, config_path, discovered_at) \
                     VALUES (?1, ?2, 0) RETURNING id",
                    rusqlite::params![value, format!("/nowhere/{n}.conf")],
                    |row| row.get(0),
                )
                .unwrap();
            for sql in [
                "INSERT INTO firewall_rules (address, action, enabled, created_at, source, evidence) \
                 VALUES (?1, 'block', 1, 0, 'manual', ?1)",
                "INSERT INTO trusted_addresses (address, trusted_at) VALUES (?1, 0)",
                "INSERT INTO trusted_user_agents (user_agent, trusted_at) VALUES (?1, 0)",
                "INSERT INTO blocked_user_agents (user_agent, blocked_at) VALUES (?1, 0)",
                "INSERT INTO reputation_sources (id, name, url) VALUES (?1, ?1, ?1)",
                "INSERT INTO selected_countries (country_code, added_at) VALUES (?1, 0)",
                "INSERT INTO bots (slug, name, user_agent_pattern, status, source_id, updated_at) \
                 VALUES (?1, ?1, ?1, 'blocked', 'evil', 0)",
            ] {
                db.execute(sql, [&value]).unwrap();
            }
            // An exempt path NGINX cannot take, so that the site's name
            // is quoted in the warning that says it was left out.
            db.execute(
                "INSERT INTO site_path_exemptions (site_id, path) VALUES (?1, ?2)",
                rusqlite::params![site_id, format!("/x\\|{value}")],
            )
            .unwrap();
        }
        let paths: Vec<String> = hostile_values().map(|v| format!("/{v}")).collect();
        db.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('detect_probe_paths_extra', ?1)",
            [paths.join("\n")],
        )
        .unwrap();
        db.commit().unwrap();
        fx
    }
}

/// The confirmed attack: a title-set OSC and a C1 CSI in a firewall rule
/// the console wrote reached root's terminal through `list-firewall-rules`.
/// The rule is still listed, one line each, with the characters replaced.
#[test]
fn list_firewall_rules_prints_a_hostile_rule_defanged_on_one_line() {
    let fx = Fixture::hostile();

    let output = fx.cmd(&["list-firewall-rules"]).output().unwrap();

    assert_output_terminal_safe("list-firewall-rules", &output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("h0\u{fffd}]0;PWNED\u{fffd}x") && stdout.contains("h6\\u{202E}lmth.tobx"),
        "the rule should still be listed, defanged:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().count(),
        HOSTILE.len(),
        "one line per rule:\n{stdout}"
    );
}

/// Every `list-*` and `show-*` command `--help` lists, so that one added
/// later is walked too, against a database and a log holding every
/// [`HOSTILE`] value. And `status --cached`, which re-assesses the stored
/// probe against the same database.
#[test]
fn every_list_and_show_command_prints_hostile_rows_terminal_safe() {
    let fx = Fixture::hostile();
    let help = fx.cmd(&["--help"]).output().unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    let commands: Vec<&str> = help
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|word| word.starts_with("list-") || word.starts_with("show-"))
        .collect();
    assert!(
        commands.len() >= 10,
        "the walk found too few commands, so --help changed shape: {commands:?}"
    );

    // All at once: they only read, and one after another is seconds.
    let outputs: Vec<_> = std::thread::scope(|scope| {
        let runs: Vec<_> = commands
            .iter()
            .map(|&command| {
                let fx = &fx;
                scope.spawn(move || (command, fx.cmd(&[command]).output().unwrap()))
            })
            .collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    });
    for (command, output) in outputs {
        assert!(
            output.status.code() != Some(2),
            "`{command}` needs an argument this walk does not give it:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_output_terminal_safe(command, &output);
    }
}

/// `status --cached` prints the strings of a probe the console stored,
/// as stored, and the checks it derives from the database: what is
/// trusted by hand, and what the NGINX config leaves out.
#[test]
fn status_cached_prints_a_hostile_probe_and_database_terminal_safe() {
    let fx = Fixture::hostile();
    let value = hostile_values().collect::<Vec<_>>().join(" ");
    let probe = stop_bots::health::Probe {
        access_log_readable: Some(false),
        access_log_path: Some(value.clone()),
        access_log_unparsed_sample: Some(value.clone()),
        stray_generated_files: vec![value.clone()],
        console_unreadable_logs: vec![value.clone()],
        conf_d_path: Some(value),
        ..Default::default()
    };
    let db = stop_bots::db::Db::open(&fx.db).unwrap();
    stop_bots::health::store_probe(&db, &probe).unwrap();
    drop(db);

    let output = fx.cmd(&["status", "--cached"]).output().unwrap();

    assert_output_terminal_safe("status --cached", &output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("h0\u{fffd}]0;PWNED\u{fffd}x"),
        "the stored path should still be named, defanged:\n{stdout}"
    );
}

/// A site's server name, printed by `list-categories --site`, and quoted
/// by the error that lists the known sites when the name matches none,
/// which goes out through the top-level error printer.
#[test]
fn list_categories_and_its_unknown_site_error_print_server_names_terminal_safe() {
    let fx = Fixture::hostile();
    let site = hostile_values().next().unwrap();

    let found = fx
        .cmd(&["list-categories", "--site", &site])
        .output()
        .unwrap();
    assert!(found.status.success(), "{found:?}");
    assert_output_terminal_safe("list-categories --site", &found);

    let unknown = fx
        .cmd(&["list-categories", "--site", "nope.example"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert_output_terminal_safe("list-categories --site nope.example", &unknown);
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        stderr.starts_with("Error: unknown site: nope.example (known: ")
            && stderr.contains("h5real\u{fffd}fakex"),
        "the error should still list the sites, defanged:\n{stderr}"
    );
}

/// `apply-blocks --dry-run` warns about every entry it would leave out of
/// the config, naming the site and quoting the value.
#[test]
fn apply_blocks_dry_run_names_hostile_skipped_entries_terminal_safe() {
    let fx = Fixture::hostile();

    let output = fx
        .cmd(&[
            "apply-blocks",
            "--root",
            fx.nginx_root.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();

    assert_output_terminal_safe("apply-blocks --dry-run", &output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning: left out of the NGINX config: h0\u{fffd}]0;PWNED\u{fffd}x"),
        "the skipped entries should still be named, defanged:\n{stderr}"
    );
}

/// `batch` prints every step's error with its whole chain, and a step's
/// error quotes what it failed on: here, an NGINX root whose name is
/// [`HOSTILE`], which the site scan cannot find.
#[test]
fn batch_prints_hostile_step_errors_terminal_safe() {
    let fx = Fixture::hostile();
    let root = fx.nginx_root.join(hostile_values().collect::<String>());
    let out = fx.firewall_script();

    let output = fx
        .cmd(&[
            "batch",
            "--no-fetch",
            "--root",
            root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--ssh-log",
            "tests/fixtures/logs/auth.log",
            "--access-log",
            "/dev/null",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success(), "the site scan should fail");
    assert_output_terminal_safe("batch", &output);
    let printed = [output.stdout, output.stderr].concat();
    let printed = String::from_utf8_lossy(&printed);
    assert!(
        printed.contains("FAIL") && printed.contains("h0\u{fffd}]0;PWNED\u{fffd}x"),
        "the failing step should still name the root, defanged:\n{printed}"
    );
}
