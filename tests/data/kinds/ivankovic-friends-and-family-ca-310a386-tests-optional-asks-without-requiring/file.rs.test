/*  This file is part of Friends and Family CA.
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
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program. If not, see <https://www.gnu.org/licenses/>.
 */
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use time::OffsetDateTime;

use super::*;

/// A site the way a real one looks: a bot filter of `if` blocks full of quoted regular expressions
/// and braces first, client-certificate lines commented out, and a second server for plain HTTP.
const BOOKS: &str = r#"server {
    # BEGIN stop-bots (DO NOT EDIT)
    set $stop_bots_block 0;
    if ($http_user_agent ~* "1h4x\.com|A6-Indexer|Ab{2,}ot|semi;colon|\"quoted\"") {
        set $stop_bots_block 1;
    }
    if ($stop_bots_block) {
        return 444;
    }
    # END stop-bots

    listen 443 ssl;
    http2 on;

    server_name books.example.org;

    ssl_certificate     /etc/letsencrypt/live/example/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/example/privkey.pem;

    # ssl_client_certificate /etc/nginx/certs/ca.cert.pem;
    # ssl_verify_client on;

    include /etc/nginx/conf.d/ssl-params.inc;
    client_max_body_size 500M;                     # uploads

    location / {
        proxy_pass http://books:8083;
        proxy_set_header Host ${host};
    }
}

server {
    listen 80;
    server_name books.example.org;
    return 301 https://$host$request_uri;
}
"#;

/// A site with an older CA's certificate active and verification commented out.
const PHOTOS: &str = "server {
    listen 443 ssl;
    server_name photos.example.org www.photos.example.org;
    ssl_certificate     /etc/letsencrypt/live/example/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/example/privkey.pem;

    ssl_client_certificate /etc/nginx/certs/ca.cert.pem;
    #ssl_verify_client on;

    location / { proxy_pass http://photos:2283; }
}
";

fn settings(dir: &Path) -> config::Nginx {
    config::Nginx {
        sites: dir.join("conf.d"),
        ca_files: dir.join("certs"),
        ca_files_in_nginx: "/etc/nginx/certs".into(),
        test: "true".into(),
        reload: "true".into(),
    }
}

fn folder(files: &[(&str, &str)]) -> (tempfile::TempDir, config::Nginx) {
    let dir = crate::test_dir();
    let nginx = settings(dir.path());
    fs::create_dir(&nginx.sites).unwrap();
    // ffca writes only into a folder nobody else can change, whatever the umask made it.
    fs::set_permissions(&nginx.sites, fs::Permissions::from_mode(0o755)).unwrap();
    for (name, text) in files {
        fs::write(nginx.sites.join(name), text).unwrap();
    }
    (dir, nginx)
}

fn ca(dir: &Path) -> Ca {
    Ca::create(
        &Store::new(dir.join("state")),
        "Test",
        OffsetDateTime::now_utc(),
        ca::DEFAULT_CA_VALIDITY,
    )
    .unwrap()
}

fn only_site(nginx: &config::Nginx) -> Site {
    let survey = survey(nginx).unwrap();
    assert_eq!(survey.sites.len(), 1, "{survey:?}");
    survey.sites[0].clone()
}

#[test]
fn the_survey_finds_https_sites_and_their_modes() {
    let (_dir, nginx) = folder(&[
        ("books.conf", BOOKS),
        ("photos.conf", PHOTOS),
        ("ssl-params.inc", "ssl_protocols TLSv1.3;\n"),
        (
            "00-log-format.conf",
            "log_format main '$remote_addr \"$request\"';\n",
        ),
    ]);
    let survey = survey(&nginx).unwrap();
    assert!(survey.problems.is_empty(), "{:?}", survey.problems);
    let found: Vec<(String, String, usize, Mode, bool)> = survey
        .sites
        .iter()
        .map(|s| {
            (
                s.file_name(),
                s.name().to_owned(),
                s.index,
                s.mode,
                s.uses_ca,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "books.conf".into(),
                "books.example.org".into(),
                0,
                Mode::Off,
                false
            ),
            (
                "photos.conf".into(),
                "photos.example.org".into(),
                0,
                Mode::Off,
                false
            ),
        ],
        "the port-80 server and the files that are not *.conf are not sites"
    );
    assert!(survey.sites[1].serves("WWW.photos.example.org"));
}

#[test]
fn a_file_nginx_could_not_read_is_reported_and_the_others_still_listed() {
    let (_dir, nginx) = folder(&[
        ("books.conf", BOOKS),
        ("broken.conf", "server {\n    listen 443 ssl;\n"),
    ]);
    let survey = survey(&nginx).unwrap();
    assert_eq!(survey.sites.len(), 1);
    assert_eq!(survey.problems.len(), 1);
    assert!(
        survey.problems[0].contains("broken.conf") && survey.problems[0].contains("not closed"),
        "{:?}",
        survey.problems
    );
}

#[test]
fn requiring_a_certificate_adds_ffcas_block_after_the_server_name() {
    let (_dir, nginx) = folder(&[]);
    let planned = plan(BOOKS, 0, Mode::Required, &nginx).unwrap();
    let expected_block = "    server_name books.example.org;

    # BEGIN ffca - client certificates, managed by Friends and Family CA
    ssl_client_certificate /etc/nginx/certs/ffca-ca.crt;
    ssl_crl /etc/nginx/certs/ffca.crl;
    ssl_verify_client on;
    # END ffca

    ssl_certificate ";
    assert!(planned.contains(expected_block), "{planned}");
    assert_eq!(
        planned.len(),
        BOOKS.len() + expected_block.len()
            - "    server_name books.example.org;\n\n    ssl_certificate ".len()
    );
    let site = &sites_in(Path::new("books.conf"), &planned, &nginx).unwrap()[0];
    assert_eq!((site.mode, site.uses_ca), (Mode::Required, true));
}

#[test]
fn turning_it_off_again_gives_back_the_file_as_it_was() {
    let (_dir, nginx) = folder(&[]);
    for mode in [Mode::Optional, Mode::Required] {
        let on = plan(BOOKS, 0, mode, &nginx).unwrap();
        assert_eq!(
            plan(&on, 0, mode, &nginx).unwrap(),
            on,
            "setting {mode} twice changes nothing more"
        );
        assert_eq!(plan(&on, 0, Mode::Off, &nginx).unwrap(), BOOKS);
    }
    assert_eq!(
        plan(BOOKS, 0, Mode::Off, &nginx).unwrap(),
        BOOKS,
        "off on an untouched site changes nothing"
    );
}

#[test]
fn optional_asks_without_requiring() {
    let (_dir, nginx) = folder(&[]);
    let planned = plan(BOOKS, 0, Mode::Optional, &nginx).unwrap();
    assert!(
        planned.contains("    ssl_verify_client optional;\n"),
        "{planned}"
    );
    assert_eq!(
        sites_in(Path::new("x.conf"), &planned, &nginx).unwrap()[0].mode,
        Mode::Optional
    );
}

#[test]
fn another_cas_active_lines_are_commented_out_not_deleted() {
    let (_dir, nginx) = folder(&[]);
    let planned = plan(PHOTOS, 0, Mode::Required, &nginx).unwrap();
    assert!(
        planned.contains("    # ffca: ssl_client_certificate /etc/nginx/certs/ca.cert.pem;\n"),
        "{planned}"
    );
    assert!(
        planned.contains("    #ssl_verify_client on;\n"),
        "already commented lines stay as they are"
    );
    let site = &sites_in(Path::new("photos.conf"), &planned, &nginx).unwrap()[0];
    assert!(site.uses_ca);
    let off = plan(&planned, 0, Mode::Off, &nginx).unwrap();
    assert!(off.contains("# ffca: ssl_client_certificate"), "{off}");
    assert_eq!(
        sites_in(Path::new("photos.conf"), &off, &nginx).unwrap()[0].mode,
        Mode::Off
    );
}

#[test]
fn a_line_holding_more_than_the_directive_is_left_to_a_person() {
    let (_dir, nginx) = folder(&[]);
    let crowded = "server {\n    listen 443 ssl; server_name a.example.org;\n    ssl_verify_client on; ssl_verify_depth 2;\n}\n";
    let error = plan(crowded, 0, Mode::Required, &nginx).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("line 3 holds more than `ssl_verify_client`"),
        "{error}"
    );
}

#[test]
fn the_right_server_block_is_changed_when_a_file_has_several() {
    let (_dir, nginx) = folder(&[]);
    let two = "server {\n    listen 443 ssl;\n    server_name a.example.org;\n}\nserver {\n    listen 443 ssl;\n    server_name b.example.org;\n}\n";
    let planned = plan(two, 1, Mode::Required, &nginx).unwrap();
    let sites = sites_in(Path::new("two.conf"), &planned, &nginx).unwrap();
    assert_eq!(
        (sites[0].name(), sites[0].mode),
        ("a.example.org", Mode::Off)
    );
    assert_eq!(
        (sites[1].name(), sites[1].mode),
        ("b.example.org", Mode::Required)
    );
    assert!(plan(two, 2, Mode::Required, &nginx).is_err());
}

#[test]
fn a_server_without_a_name_gets_the_block_at_its_top() {
    let (_dir, nginx) = folder(&[]);
    let planned = plan(
        "server {\n    listen 443 ssl;\n}\n",
        0,
        Mode::Required,
        &nginx,
    )
    .unwrap();
    assert!(
        planned.starts_with("server {\n\n    # BEGIN ffca"),
        "{planned}"
    );
    assert_eq!(
        plan(&planned, 0, Mode::Off, &nginx).unwrap(),
        "server {\n    listen 443 ssl;\n}\n"
    );
}

#[test]
fn the_parser_reads_what_nginx_reads() {
    for (text, problem) in [
        ("server {\n listen 443 ssl;\n", "not closed"),
        ("server {\n listen 443 ssl\n}\n", "unexpected `}`"),
        ("listen 443 ssl", "is not ended with `;`"),
        ("}\n", "closes nothing"),
        ("set $a \"unclosed;\n", "not closed"),
    ] {
        let error = Parsed::new(text)
            .err()
            .unwrap_or_else(|| panic!("{text:?} parsed"));
        assert!(
            format!("{error:#}").contains(problem),
            "{text:?}: {error:#}"
        );
    }
    let parsed =
        Parsed::new("set $x \"a { b ; c\"; # } not a brace\nset ${y}z 'q\\'s';\n").unwrap();
    let args: Vec<_> = parsed.statements.iter().map(|s| s.args.clone()).collect();
    assert_eq!(
        args,
        [
            vec!["$x".to_owned(), "a { b ; c".into()],
            vec!["${y}z".into(), "q's".into()]
        ]
    );
}

#[test]
fn an_ffca_block_without_its_end_is_refused() {
    let (_dir, nginx) = folder(&[]);
    let damaged = "server {\n    listen 443 ssl;\n    # BEGIN ffca\n    ssl_verify_client on;\n}\n";
    assert!(
        plan(damaged, 0, Mode::Off, &nginx)
            .unwrap_err()
            .to_string()
            .contains("without its # END ffca")
    );
}

#[test]
fn set_mode_writes_tests_and_reloads_and_keeps_the_files_permissions() {
    let (dir, nginx) = folder(&[("books.conf", BOOKS)]);
    let file = nginx.sites.join("books.conf");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    let ca = ca(dir.path());
    let site = only_site(&nginx);
    assert!(set_mode(&nginx, &ca, &site, Mode::Required).unwrap());
    assert_eq!(only_site(&nginx).mode, Mode::Required);
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(
        !set_mode(&nginx, &ca, &only_site(&nginx), Mode::Required).unwrap(),
        "nothing to change"
    );
    assert_eq!(
        fs::read_to_string(nginx.ca_files.join(CA_FILE)).unwrap(),
        ca.certificate_pem()
    );
    assert_eq!(
        fs::read_to_string(nginx.ca_files.join(CRL_FILE)).unwrap(),
        ca.store().read(ca::CRL_FILE).unwrap()
    );
}

#[test]
fn a_change_nginx_refuses_is_put_back() {
    let (dir, mut nginx) = folder(&[("books.conf", BOOKS)]);
    let ca = ca(dir.path());
    let site = only_site(&nginx);
    nginx.test = "false".into();
    let error = set_mode(&nginx, &ca, &site, Mode::Required).unwrap_err();
    assert!(
        error.to_string().contains("nginx refused the change, so"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("`false` failed"), "{error:#}");
    assert_eq!(
        fs::read_to_string(nginx.sites.join("books.conf")).unwrap(),
        BOOKS
    );

    nginx.test = "true".into();
    nginx.reload = "false".into();
    let error = set_mode(&nginx, &ca, &site, Mode::Required).unwrap_err();
    assert!(
        error.to_string().contains("nginx did not reload"),
        "{error:#}"
    );
    assert_eq!(
        fs::read_to_string(nginx.sites.join("books.conf")).unwrap(),
        BOOKS
    );
}

#[test]
fn a_file_changed_since_it_was_read_is_not_touched() {
    let (dir, nginx) = folder(&[("books.conf", BOOKS)]);
    let ca = ca(dir.path());
    let site = only_site(&nginx);
    let edited = BOOKS.replace(
        "server_name books.example.org;\n\n    ssl",
        "server_name library.example.org;\n\n    ssl",
    );
    fs::write(nginx.sites.join("books.conf"), &edited).unwrap();
    let error = set_mode(&nginx, &ca, &site, Mode::Required).unwrap_err();
    assert!(
        error.to_string().contains("changed since it was read"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(nginx.sites.join("books.conf")).unwrap(),
        edited
    );
}

#[test]
fn commands_report_what_they_printed() {
    assert_eq!(
        run("echo nginx: syntax is ok").unwrap(),
        "nginx: syntax is ok\n"
    );
    let error = run("sh -c exit_3").unwrap_err().to_string();
    assert!(error.contains("`sh -c exit_3` failed"), "{error}");
    assert!(run("").unwrap_err().to_string().contains("empty"));
    assert!(
        run("no-such-program-ffca")
            .unwrap_err()
            .to_string()
            .contains("cannot run")
    );
}

#[test]
fn publish_and_reload_tests_before_it_reloads() {
    let dir = crate::test_dir();
    let mut nginx = settings(dir.path());
    let ca = ca(dir.path());
    let marker = dir.path().join("reloaded");
    nginx.reload = format!("touch {}", marker.display());
    nginx.test = "false".into();
    assert!(publish_and_reload(&nginx, &ca).is_err());
    assert!(
        !marker.exists(),
        "a configuration nginx refuses is not loaded"
    );
    nginx.test = "true".into();
    publish_and_reload(&nginx, &ca).unwrap();
    assert!(marker.exists());
    let mode = fs::metadata(nginx.ca_files.join(CRL_FILE))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "nginx's workers read it");
}

/// Off on a site that is off changes nothing, even with another CA's certificate line active: only
/// `ssl_verify_client` makes nginx ask.
#[test]
fn off_comments_out_only_what_asks_for_a_certificate() {
    let (_dir, nginx) = folder(&[]);
    assert_eq!(plan(PHOTOS, 0, Mode::Off, &nginx).unwrap(), PHOTOS);
    let asking = PHOTOS.replace("    #ssl_verify_client on;", "    ssl_verify_client on;");
    let off = plan(&asking, 0, Mode::Off, &nginx).unwrap();
    assert!(off.contains("    # ffca: ssl_verify_client on;\n"), "{off}");
    assert!(
        off.contains("    ssl_client_certificate /etc/nginx/certs/ca.cert.pem;\n"),
        "{off}"
    );
    assert_eq!(
        sites_in(Path::new("photos.conf"), &off, &nginx).unwrap()[0].mode,
        Mode::Off
    );
}

/// nginx keeps a CRL across a reload when its file looks unchanged; a new inode every time is what
/// makes it read the new one.
#[test]
fn every_publish_is_a_new_file_for_nginx_to_notice() {
    use std::os::unix::fs::MetadataExt;
    let dir = crate::test_dir();
    let nginx = settings(dir.path());
    let ca = ca(dir.path());
    publish(&nginx, &ca).unwrap();
    let first = fs::metadata(nginx.ca_files.join(CRL_FILE)).unwrap().ino();
    publish(&nginx, &ca).unwrap();
    assert_ne!(
        fs::metadata(nginx.ca_files.join(CRL_FILE)).unwrap().ino(),
        first
    );
}

/// A site on the same domain as the enrollment site, for it to copy from.
const CLOUD: &str = "server {
    listen 443 ssl default_server;
    http2 on;
    server_name cloud.example.org;
    ssl_certificate /etc/le/example/fullchain.pem;
    ssl_certificate_key /etc/le/example/privkey.pem;
    include /etc/nginx/snippets/logging.conf;
    location / { return 200; }
}
";

fn enrollment(upstream: &str) -> config::Enrollment {
    config::Enrollment {
        host: "k.example.org".into(),
        upstream: upstream.into(),
        user: config::default_page_user(),
    }
}

/// The whole file, so that its shape cannot drift unseen: what is copied, what is forwarded, and
/// the access log turned off for the whole site.
#[test]
fn the_enrollment_site_is_written_out_in_full() {
    let (_dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let (file, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    assert_eq!(file, nginx.sites.join(ENROLLMENT_FILE));
    assert_eq!(
        text,
        "# Written by Friends and Family CA: the enrollment page, where invites are collected.
# It never asks for a client certificate: whoever opens an invite has none yet. Its listen,
# TLS certificate and includes are copied from cloud.conf.
server {
    listen 443 ssl;
    http2 on;
    ssl_certificate /etc/le/example/fullchain.pem;
    ssl_certificate_key /etc/le/example/privkey.pem;
    include /etc/nginx/snippets/logging.conf;
    server_name k.example.org;
    ssl_verify_client off;

    # An invite's link carries its secret, and a request turned away before it reaches a
    # location - by a bot filter, say - is logged here: nothing on this site is.
    access_log off;

    # Looked up when a request comes (Docker's DNS), so that nginx starts while the page is down.
    resolver 127.0.0.11 valid=30s ipv6=off;
    set $ffca_enrollment http://ffca:8080;

    location / {
        proxy_pass $ffca_enrollment;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }

    # nginx names the request in an error too - the page being down, say.
    location ~ ^/(i|d)/ {
        error_log /dev/null crit;
        proxy_pass $ffca_enrollment;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
"
    );
    Parsed::new(&text).unwrap();
}

#[test]
fn an_address_needs_no_resolver() {
    let (_dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let (_, text) = enrollment_site(&nginx, &enrollment("http://127.0.0.1:8080")).unwrap();
    assert!(
        !text.contains("resolver") && !text.contains("$ffca_enrollment"),
        "{text}"
    );
    assert_eq!(
        text.matches("        proxy_pass http://127.0.0.1:8080;\n")
            .count(),
        2,
        "{text}"
    );
}

/// stop-bots writes its bot filter into every site it finds, ffca's included; rewriting the site
/// keeps it, as stop-bots left it.
#[test]
fn another_tools_block_survives_a_rewrite() {
    let (_dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let (file, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    let filter = "    # BEGIN stop-bots (DO NOT EDIT)
    if ($http_user_agent ~* \"BadBot\") {
        return 444;
    }
    # END stop-bots
";
    let filtered = text.replacen("server {\n", &format!("server {{\n{filter}\n"), 1);
    fs::write(&file, &filtered).unwrap();
    let (_, rewritten) = enrollment_site(&nginx, &enrollment("http://ffca:9090")).unwrap();
    assert!(
        rewritten.contains(&format!("server {{\n{filter}\n    listen 443 ssl;")),
        "{rewritten}"
    );
    assert!(
        rewritten.contains("set $ffca_enrollment http://ffca:9090;"),
        "{rewritten}"
    );
    assert_eq!(
        rewritten.matches("# BEGIN").count(),
        1,
        "nothing doubled:\n{rewritten}"
    );
    Parsed::new(&rewritten).unwrap();
}

#[test]
fn a_file_that_is_not_ffcas_is_neither_read_for_blocks_nor_written() {
    let (dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let theirs = "# BEGIN stop-bots (DO NOT EDIT)\n# END stop-bots\nserver { listen 443 ssl; server_name k.example.org; }\n";
    fs::write(nginx.sites.join(ENROLLMENT_FILE), theirs).unwrap();
    let (file, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    assert!(!text.contains("stop-bots"), "{text}");
    let error = write_enrollment_site(&nginx, &ca(dir.path()), &file, &text).unwrap_err();
    assert!(error.to_string().contains("is not ffca's"), "{error}");
    assert_eq!(fs::read_to_string(&file).unwrap(), theirs);
}

/// A site linked into the folder stays linked, and its file keeps its permissions.
#[test]
fn a_linked_site_is_changed_where_the_link_points() {
    let (dir, nginx) = folder(&[]);
    let available = dir.path().join("available");
    fs::create_dir(&available).unwrap();
    fs::set_permissions(&available, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(available.join("books.conf"), BOOKS).unwrap();
    fs::set_permissions(
        available.join("books.conf"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    std::os::unix::fs::symlink(available.join("books.conf"), nginx.sites.join("books.conf"))
        .unwrap();
    let ca = ca(dir.path());
    assert!(set_mode(&nginx, &ca, &only_site(&nginx), Mode::Required).unwrap());
    let link = fs::symlink_metadata(nginx.sites.join("books.conf")).unwrap();
    assert!(link.file_type().is_symlink(), "still a link");
    let real = fs::read_to_string(available.join("books.conf")).unwrap();
    assert!(real.contains("ssl_verify_client on;"), "{real}");
    assert_eq!(
        fs::metadata(available.join("books.conf"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}

#[test]
fn optional_no_ca_is_marked_for_what_it_lets_in() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {\n    listen 443 ssl;\n    server_name a.example.org;\n    ssl_verify_client optional_no_ca;\n}\n";
    let site = &sites_in(Path::new("a.conf"), text, &nginx).unwrap()[0];
    assert_eq!((site.mode, site.any_issuer), (Mode::Optional, true));
}

/// The site the read-back checks below change, between two others it must not touch.
fn requiring(text: &str, index: usize) -> Result<String> {
    let (_dir, nginx) = folder(&[]);
    plan(text, index, Mode::Required, &nginx)
}

/// A server written on one line has nowhere inside it for ffca's lines to go: after its
/// `server_name`, they would land after its `}`, at the level of every site.
#[test]
fn a_server_on_one_line_is_left_to_a_person() {
    let one_line = "server { listen 443 ssl; server_name a.example.test; ssl_certificate a.crt; }
server {
    listen 443 ssl;
    server_name b.example.test;
}
";
    let error = requiring(one_line, 0).unwrap_err();
    assert!(
        error.to_string().contains("written on one line"),
        "{error:#}"
    );
}

#[test]
fn a_server_name_sharing_its_line_with_the_close_is_not_an_anchor() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    server_name a.example.test; }
server {
    listen 443 ssl;
    server_name b.example.test;
}
";
    let planned = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert!(
        planned.starts_with("server {\n\n    # BEGIN ffca"),
        "after the server's own {{:\n{planned}"
    );
    let sites = sites_in(Path::new("x.conf"), &planned, &nginx).unwrap();
    assert_eq!(
        sites.iter().map(|s| s.mode).collect::<Vec<_>>(),
        [Mode::Required, Mode::Off]
    );
    assert_eq!(plan(&planned, 0, Mode::Off, &nginx).unwrap(), text);
}

#[test]
fn a_server_whose_brace_is_on_the_next_line_gets_the_block_inside() {
    let (_dir, nginx) = folder(&[]);
    let text = "server\n{\n    listen 443 ssl;\n}\n";
    let planned = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert!(
        planned.starts_with("server\n{\n\n    # BEGIN ffca"),
        "{planned}"
    );
    assert_eq!(plan(&planned, 0, Mode::Off, &nginx).unwrap(), text);
}

#[test]
fn a_server_open_and_named_on_one_line_gets_the_block_before_its_close() {
    let (_dir, nginx) = folder(&[]);
    let text = "server { listen 443 ssl; server_name a.example.test; ssl_certificate a.crt;\n}\n";
    let planned = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert!(planned.ends_with("    # END ffca\n}\n"), "{planned}");
    assert_eq!(
        sites_in(Path::new("x.conf"), &planned, &nginx).unwrap()[0].mode,
        Mode::Required
    );
    assert_eq!(plan(&planned, 0, Mode::Off, &nginx).unwrap(), text);
}

/// An apostrophe inside a word is part of the word, as nginx reads it: the quoted string after
/// it holds text that only looks like directives.
#[test]
fn quotes_inside_a_word_are_letters() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    server_name a.example.test;
    add_header X-A don't;
    add_header X-B 'x; ssl_verify_client on; add_header X-C y';
    add_header X-D it's;
    default_type a}b;
}
";
    let site = &sites_in(Path::new("x.conf"), text, &nginx).unwrap()[0];
    assert_eq!(site.mode, Mode::Off, "nginx does not ask here");
    let parsed = Parsed::new(text).unwrap();
    let args: Vec<Vec<String>> = parsed.servers()[0]
        .block
        .as_ref()
        .unwrap()
        .iter()
        .map(|s| s.args.clone())
        .collect();
    assert_eq!(args[2], ["X-A", "don't"]);
    assert_eq!(
        args[3],
        ["X-B", "x; ssl_verify_client on; add_header X-C y"]
    );
    assert_eq!(args[5], ["a}b"]);
    let planned = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert!(
        planned.contains("    add_header X-B 'x; ssl_verify_client on; add_header X-C y';\n"),
        "{planned}"
    );
}

/// `${` keeps the `{` in its word, and the word ends where nginx ends it: lines after it are
/// counted right, and the line ffca comments out is the one that asks.
#[test]
fn a_dollar_brace_does_not_swallow_lines() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    ssl_client_certificate /etc/nginx/certs/ffca-ca.crt;
    default_type ${x;
    if ($host) {}
    charset utf-8;
    server_name a.example.test;
    ssl_verify_client optional;
    add_header X-Keep keep;
}
";
    let off = plan(text, 0, Mode::Off, &nginx).unwrap();
    assert!(
        off.contains("    # ffca: ssl_verify_client optional;\n"),
        "{off}"
    );
    assert!(off.contains("\n    server_name a.example.test;\n"), "{off}");
}

#[test]
fn the_parser_undoes_escapes_as_nginx_does() {
    let parsed = Parsed::new(
        "set $a \"x\\\"y\\\\z\\q\";\nset $b a\\;b;\nset $c 'tab\\there';\nreturn 200 \"ok\")\n;\n",
    )
    .unwrap();
    let args: Vec<_> = parsed.statements.iter().map(|s| s.args.clone()).collect();
    assert_eq!(
        args,
        [
            vec!["$a".to_owned(), "x\"y\\z\\q".into()],
            // `\;` is not an escape: the `\` stays, and the `;` is in the word.
            vec!["$b".into(), "a\\;b".into()],
            vec!["$c".into(), "tab\there".into()],
            vec!["200".into(), "ok".into(), ")".into()],
        ]
    );
    for (text, problem) in [
        ("set $a \"x\"y;\n", "after a quoted string"),
        ("set $a \"x\"#c\n;\n", "after a quoted string"),
        ("; listen 80;\n", "unexpected `;`"),
    ] {
        let error = Parsed::new(text).err().unwrap();
        assert!(
            format!("{error:#}").contains(problem),
            "{text:?}: {error:#}"
        );
    }
    // Only space, tab and line ends separate words.
    let parsed = Parsed::new("server_name a.example.test\u{a0}b.example.test;\n").unwrap();
    assert_eq!(
        parsed.statements[0].args,
        ["a.example.test\u{a0}b.example.test"]
    );
}

/// Another tool's block whose name starts with ffca's is not ffca's: turning the site off
/// leaves it, and a lone `# END ffca` is refused rather than taken as the end of something.
#[test]
fn only_ffcas_exact_markers_mark_its_block() {
    let (_dir, nginx) = folder(&[]);
    let theirs = "server {
    listen 443 ssl;
    server_name a.example.test;
    # BEGIN ffca-admin-acl (mine)
    location /admin { deny all; }
    # END ffca-admin-acl
}
";
    assert_eq!(plan(theirs, 0, Mode::Off, &nginx).unwrap(), theirs);
    let on = plan(theirs, 0, Mode::Required, &nginx).unwrap();
    assert_eq!(plan(&on, 0, Mode::Off, &nginx).unwrap(), theirs);

    let stray = theirs.replace("# END ffca-admin-acl", "# END ffca");
    let error = plan(&stray, 0, Mode::Off, &nginx).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("# END ffca without its # BEGIN ffca"),
        "{error:#}"
    );
}

/// Markers inside a quoted string are text, not comments.
#[test]
fn markers_inside_strings_are_not_markers() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    server_name a.example.test;
    add_header X-Note \"
    # BEGIN ffca
    \";
    ssl_verify_client off;
    add_header X-E \"
    # END ffca
\";
}
";
    let planned = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert!(
        planned.contains("    # ffca: ssl_verify_client off;\n"),
        "{planned}"
    );
    assert!(planned.contains("add_header X-Note \"\n    # BEGIN ffca\n    \";\n"));
    assert_eq!(
        sites_in(Path::new("x.conf"), &planned, &nginx).unwrap()[0].mode,
        Mode::Required
    );
}

#[test]
fn an_ffca_block_holding_more_than_its_lines_is_left_to_a_person() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    server_name a.example.test;

    # BEGIN ffca - client certificates, managed by Friends and Family CA
    ssl_verify_client on;
    location /x { return 403; }
    # END ffca
}
";
    let error = plan(text, 0, Mode::Off, &nginx).unwrap_err();
    assert!(
        error.to_string().contains("holds more than its own lines"),
        "{error:#}"
    );
}

#[test]
fn every_ffca_block_goes_when_the_site_is_turned_off() {
    let (_dir, nginx) = folder(&[]);
    let text = "server {
    listen 443 ssl;
    server_name a.example.test;

    # BEGIN ffca - x
    ssl_verify_client optional;
    # END ffca

    # BEGIN ffca - y
    ssl_client_certificate /etc/nginx/certs/ffca-ca.crt;
    ssl_crl /etc/nginx/certs/ffca.crl;
    ssl_verify_client off;
    # END ffca
}
";
    let off = plan(text, 0, Mode::Off, &nginx).unwrap();
    assert_eq!(
        off,
        "server {\n    listen 443 ssl;\n    server_name a.example.test;\n}\n"
    );
    let on = plan(text, 0, Mode::Required, &nginx).unwrap();
    assert_eq!(on.matches("# BEGIN ffca").count(), 1, "{on}");
}

#[test]
fn the_folder_in_nginx_is_checked_where_it_is_written() {
    let (_dir, mut nginx) = folder(&[]);
    nginx.ca_files_in_nginx = "/etc/x;#".into();
    assert!(plan(BOOKS, 0, Mode::Required, &nginx).is_err());
}

#[test]
fn listen_addresses_are_grouped_as_nginx_groups_them() {
    assert_eq!(listen_address("443"), "*:443");
    assert_eq!(listen_address("*:443"), "*:443");
    assert_eq!(listen_address("0.0.0.0:443"), "*:443");
    assert_eq!(listen_address("[::]:443"), "[::]:443");
    assert_eq!(listen_address("127.0.0.1"), "127.0.0.1:80");
    assert_eq!(listen_address("[::1]"), "[::1]:80");
}

const SECOND_BOOKS: &str = "server {
    listen *:443 ssl;
    server_name Books.example.org;
}
";

/// nginx serves the first of two blocks with a name on an address and ignores the other, so
/// whatever the other says about certificates does not count.
#[test]
fn a_name_two_blocks_claim_is_marked_and_not_set() {
    let (dir, mut nginx) = folder(&[("books.conf", BOOKS), ("books-old.conf", SECOND_BOOKS)]);
    let sites = survey(&nginx).unwrap().sites;
    assert!(sites.iter().all(|s| s.duplicate), "{sites:?}");
    let (_dir2, alone) = folder(&[("books.conf", BOOKS), ("photos.conf", PHOTOS)]);
    assert!(survey(&alone).unwrap().sites.iter().all(|s| !s.duplicate));

    nginx.test =
        "echo nginx: [warn] conflicting server name \"books.example.org\" on 0.0.0.0:443, ignored"
            .into();
    let ca = ca(dir.path());
    let site = sites
        .iter()
        .find(|s| s.file_name() == "books.conf")
        .unwrap();
    let error = set_mode(&nginx, &ca, site, Mode::Required).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("two server blocks for books.example.org"),
        "{error:#}"
    );
    assert_eq!(
        fs::read_to_string(nginx.sites.join("books.conf")).unwrap(),
        BOOKS
    );
    assert!(
        set_mode(&nginx, &ca, site, Mode::Off).is_ok(),
        "turning it off is never what lets anyone in"
    );
}

/// What another tool writes into the file while nginx tests ffca's change is not overwritten by
/// putting the file back.
#[test]
fn a_file_changed_during_the_test_is_left_as_it_is() {
    let (dir, mut nginx) = folder(&[("books.conf", BOOKS)]);
    let file = nginx.sites.join("books.conf");
    let script = dir.path().join("test.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\necho '# theirs' >> {}\nexit 1\n", file.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    nginx.test = script.display().to_string();
    let ca = ca(dir.path());
    let error = set_mode(&nginx, &ca, &only_site(&nginx), Mode::Required).unwrap_err();
    assert!(
        format!("{error:#}").contains("changed while ffca was changing it"),
        "{error:#}"
    );
    assert!(fs::read_to_string(&file).unwrap().ends_with("# theirs\n"));
}

/// A folder someone else could write is one where they could swap a site for a link, or put
/// their own CA where nginx reads this one's: ffca writes into neither.
#[test]
fn folders_others_could_change_are_refused() {
    let (dir, nginx) = folder(&[("books.conf", BOOKS)]);
    let ca = ca(dir.path());
    let site = only_site(&nginx);
    fs::set_permissions(&nginx.sites, fs::Permissions::from_mode(0o775)).unwrap();
    let error = set_mode(&nginx, &ca, &site, Mode::Required).unwrap_err();
    assert!(error.to_string().contains("chmod go-w"), "{error:#}");
    assert_eq!(fs::read_to_string(&site.file).unwrap(), BOOKS);
    fs::set_permissions(&nginx.sites, fs::Permissions::from_mode(0o755)).unwrap();

    fs::set_permissions(&nginx.ca_files, fs::Permissions::from_mode(0o777)).unwrap();
    let error = publish(&nginx, &ca).unwrap_err();
    assert!(error.to_string().contains("chmod go-w"), "{error:#}");
}

#[test]
fn the_ca_files_folder_is_made_readable_and_writable_by_its_owner_only() {
    let dir = crate::test_dir();
    let nginx = settings(dir.path());
    publish(&nginx, &ca(dir.path())).unwrap();
    assert_eq!(
        fs::metadata(&nginx.ca_files).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn an_enrollment_file_ffca_cannot_read_is_not_written_over() {
    let (dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let (file, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    fs::write(&file, b"\xff\xfe not text").unwrap();
    let error = write_enrollment_site(&nginx, &ca(dir.path()), &file, &text).unwrap_err();
    assert!(
        error.to_string().contains("move it away first"),
        "{error:#}"
    );
    assert_eq!(fs::read(&file).unwrap(), b"\xff\xfe not text");
}

/// A template's file name goes into a comment: nothing in it can start a line of its own.
#[test]
fn the_template_file_name_cannot_add_lines() {
    let (_dir, nginx) = folder(&[("a\nserver { return 302 http:evil; }\n#.conf", CLOUD)]);
    let (_, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    assert!(
        text.contains("# TLS certificate and includes are copied from a?server { return 302"),
        "{text}"
    );
    assert_eq!(Parsed::new(&text).unwrap().servers().len(), 1);
}

/// Copied arguments are written back so that nginx reads what it read in the template.
#[test]
fn copied_arguments_read_back_as_they_were() {
    let template = CLOUD.replace(
        "include /etc/nginx/snippets/logging.conf;",
        "include \"/etc/a\\\"b c.conf\" /etc/x\\ty.conf;",
    );
    let (_dir, nginx) = folder(&[("cloud.conf", &template)]);
    let (_, text) = enrollment_site(&nginx, &enrollment("http://ffca:8080")).unwrap();
    let original = Parsed::new(&template).unwrap();
    let copied = Parsed::new(&text).unwrap();
    let include = |p: &Parsed| {
        directives(p.servers()[0].block.as_deref().unwrap(), "include")
            .next()
            .unwrap()
            .args
            .clone()
    };
    assert_eq!(include(&copied), include(&original));
    assert_eq!(include(&original), ["/etc/a\"b c.conf", "/etc/x\ty.conf"]);
}

#[test]
fn the_enrollment_host_is_checked_where_it_is_written() {
    let (_dir, nginx) = folder(&[("cloud.conf", CLOUD)]);
    let mut bad = enrollment("http://ffca:8080");
    bad.host = "k.example.org; return 302 x".into();
    assert!(enrollment_site(&nginx, &bad).is_err());
}

/// A block another tool left without its end is not kept, and does not hide the blocks after it.
#[test]
fn a_block_without_its_end_does_not_swallow_the_rest() {
    let text = "# BEGIN broken\nx\n# BEGIN stop-bots (DO NOT EDIT)\nif ($a) { return 444; }\n# END stop-bots\n";
    assert_eq!(
        foreign_blocks(text),
        [vec![
            "# BEGIN stop-bots (DO NOT EDIT)".to_owned(),
            "if ($a) { return 444; }".into(),
            "# END stop-bots".into(),
        ]]
    );
}
