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
//! The CA's certificates are checked the way a TLS server checks a client: by `rustls-webpki`,
//! with the CRL, and - because nginx is built on OpenSSL - by `openssl verify` as well.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use rustls_pki_types::{CertificateDer, UnixTime};
use time::macros::datetime;
use webpki::{
    CertRevocationList, EndEntityCert, ExpirationPolicy, KeyUsage, OwnedCertRevocationList,
    RevocationOptionsBuilder,
};

use super::*;

mod requests;

const NOW: OffsetDateTime = datetime!(2026-10-03 12:00 UTC);

fn device<'a>(person: &'a str, device: &'a str) -> Holder<'a> {
    Holder::Device { person, device }
}

fn new_ca(dir: &Path) -> Ca {
    Ca::create(&Store::new(dir), "Test family", NOW, DEFAULT_CA_VALIDITY).unwrap()
}

fn der(pem: &str) -> Vec<u8> {
    x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .unwrap()
        .1
        .contents
}

/// What a TLS server decides about `certificate` at `at`, given the CA and, if any, its CRL.
fn verify(
    ca: &Ca,
    certificate: &str,
    crl: Option<&str>,
    at: OffsetDateTime,
) -> Result<(), webpki::Error> {
    verify_for(ca, certificate, crl, at, KeyUsage::client_auth())
}

fn verify_for(
    ca: &Ca,
    certificate: &str,
    crl: Option<&str>,
    at: OffsetDateTime,
    usage: KeyUsage,
) -> Result<(), webpki::Error> {
    let anchor_der = CertificateDer::from(der(ca.certificate_pem()));
    let anchor = webpki::anchor_from_trusted_cert(&anchor_der).unwrap();
    let certificate_der = CertificateDer::from(der(certificate));
    let certificate = EndEntityCert::try_from(&certificate_der).unwrap();
    let crl = crl
        .map(|pem| CertRevocationList::from(OwnedCertRevocationList::from_der(&der(pem)).unwrap()));
    let crls: Vec<&CertRevocationList> = crl.iter().collect();
    let revocation = (!crls.is_empty()).then(|| {
        RevocationOptionsBuilder::new(&crls)
            .unwrap()
            .with_expiration_policy(ExpirationPolicy::Enforce)
            .build()
    });
    let time = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
        at.unix_timestamp().try_into().unwrap(),
    ));
    certificate
        .verify_for_usage(
            &[webpki::ring::ECDSA_P256_SHA256],
            &[anchor],
            &[],
            time,
            usage,
            revocation,
            None,
        )
        .map(|_| ())
}

fn crl(ca: &Ca) -> String {
    ca.store().read(CRL_FILE).unwrap()
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn an_issued_certificate_verifies_for_client_authentication() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), NOW).unwrap();
}

#[test]
fn an_issued_certificate_cannot_act_as_a_server() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let result = verify_for(
        &ca,
        &issued.certificate_pem,
        None,
        NOW,
        KeyUsage::server_auth(),
    );
    assert!(
        result.is_err(),
        "a client certificate must not pass as a server's"
    );
}

#[test]
fn an_issued_certificate_names_its_holder_as_the_ledger_spells_it() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    ca.issue(
        device("Anna", "phone"),
        Key::Generate,
        NOW,
        DEFAULT_CLIENT_VALIDITY,
    )
    .unwrap();
    let issued = ca
        .issue(
            device("  anna ", "PHONE"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    assert_eq!(issued.holder, "Anna (phone)");
    let der = der(&issued.certificate_pem);
    let (_, certificate) = x509_parser::parse_x509_certificate(&der).unwrap();
    assert_eq!(
        certificate.subject().to_string(),
        "OU=people, CN=Anna (phone)"
    );
    assert_eq!(certificate.issuer().to_string(), "CN=Test family");
    assert_eq!(
        certificate.raw_serial(),
        issued.certificate.serial.0,
        "the ledger's serial must be the certificate's, byte for byte"
    );
}

#[test]
fn a_certificate_stops_verifying_when_it_expires() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            Duration::days(10),
        )
        .unwrap();
    verify(&ca, &issued.certificate_pem, None, NOW + Duration::days(9)).unwrap();
    assert_eq!(
        verify(&ca, &issued.certificate_pem, None, NOW + Duration::days(11)),
        Err(webpki::Error::CertExpired {
            time: UnixTime::since_unix_epoch(std::time::Duration::from_secs(
                (NOW + Duration::days(11)).unix_timestamp() as u64
            )),
            not_after: UnixTime::since_unix_epoch(std::time::Duration::from_secs(
                (NOW + Duration::days(10)).unix_timestamp() as u64
            )),
        })
    );
}

#[test]
fn a_certificate_is_valid_from_a_little_before_now() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    verify(
        &ca,
        &issued.certificate_pem,
        None,
        NOW - Duration::minutes(30),
    )
    .unwrap();
}

#[test]
fn a_certificate_never_outlives_the_ca() {
    let dir = crate::test_dir();
    let ca = Ca::create(
        &Store::new(dir.path()),
        "Short-lived",
        NOW,
        Duration::days(100),
    )
    .unwrap();
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    assert_eq!(issued.certificate.not_after, ca.not_after());
}

#[test]
fn an_expired_ca_issues_nothing() {
    let dir = crate::test_dir();
    let ca = Ca::create(
        &Store::new(dir.path()),
        "Short-lived",
        NOW,
        Duration::days(1),
    )
    .unwrap();
    let error = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW + Duration::days(2),
            DEFAULT_CLIENT_VALIDITY,
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("the CA expired"), "{error}");
}

#[test]
fn a_revoked_certificate_fails_and_the_others_still_pass() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let lost = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let kept = ca
        .issue(
            device("Anna", "laptop"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let later = NOW + Duration::days(1);
    ca.revoke(
        Target::Certificate(lost.certificate.serial),
        Reason::Lost,
        later,
    )
    .unwrap();
    let crl = crl(&ca);
    assert_eq!(
        verify(&ca, &lost.certificate_pem, Some(&crl), later),
        Err(webpki::Error::CertRevoked)
    );
    verify(&ca, &kept.certificate_pem, Some(&crl), later).unwrap();
}

#[test]
fn a_revocation_survives_every_later_crl() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let lost = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    ca.revoke(
        Target::Certificate(lost.certificate.serial),
        Reason::Lost,
        NOW,
    )
    .unwrap();
    let later = NOW + Duration::days(20);
    ca.refresh_crl(later).unwrap();
    ca.issue(
        device("Ben", "tablet"),
        Key::Generate,
        later,
        DEFAULT_CLIENT_VALIDITY,
    )
    .unwrap();
    ca.refresh_crl(later).unwrap();
    assert_eq!(
        verify(&ca, &lost.certificate_pem, Some(&crl(&ca)), later),
        Err(webpki::Error::CertRevoked)
    );
}

#[test]
fn the_crl_expires_unless_it_is_refreshed() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let after_expiry = NOW + CRL_VALIDITY + Duration::days(1);
    assert!(matches!(
        verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), after_expiry),
        Err(webpki::Error::CrlExpired { .. })
    ));
    ca.refresh_crl(NOW + Duration::days(29)).unwrap();
    verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), after_expiry).unwrap();
}

#[test]
fn every_crl_has_a_higher_number() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    assert_eq!(
        ca.ledger().unwrap().crl_number,
        1,
        "create signs the first CRL"
    );
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    ca.revoke(
        Target::Certificate(issued.certificate.serial),
        Reason::Retired,
        NOW,
    )
    .unwrap();
    ca.refresh_crl(NOW).unwrap();
    assert_eq!(ca.ledger().unwrap().crl_number, 3);
    let der = der(&crl(&ca));
    let (_, parsed) = x509_parser::parse_x509_crl(&der).unwrap();
    assert_eq!(parsed.crl_number().unwrap().to_u64_digits(), [3]);
}

#[test]
fn a_reopened_ca_signs_with_the_same_key() {
    let dir = crate::test_dir();
    let created = new_ca(dir.path());
    let reopened = Ca::open(&Store::new(dir.path())).unwrap();
    assert_eq!(reopened.not_after(), created.not_after());
    let issued = reopened
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    verify(&created, &issued.certificate_pem, Some(&crl(&created)), NOW).unwrap();
}

#[test]
fn create_refuses_to_replace_a_ca() {
    let dir = crate::test_dir();
    new_ca(dir.path());
    let result = Ca::create(&Store::new(dir.path()), "Again", NOW, DEFAULT_CA_VALIDITY);
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("already holds a CA")
    );
}

#[test]
fn open_without_a_ca_says_how_to_make_one() {
    let dir = crate::test_dir();
    let result = Ca::open(&Store::new(dir.path()));
    assert!(result.err().unwrap().to_string().contains("ffca init"));
}

#[test]
fn open_refuses_a_key_that_is_not_the_cas() {
    let first = crate::test_dir();
    let second = crate::test_dir();
    new_ca(first.path());
    new_ca(second.path());
    std::fs::copy(second.path().join(KEY_FILE), first.path().join(KEY_FILE)).unwrap();
    let result = Ca::open(&Store::new(first.path()));
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("does not belong to")
    );
}

#[test]
fn the_key_is_private_and_the_certificates_are_public() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let store = ca.store();
    assert_eq!(mode_of(&store.path(KEY_FILE)), 0o600);
    assert_eq!(mode_of(&store.path(CERTIFICATE_FILE)), 0o644);
    assert_eq!(mode_of(&store.path(CRL_FILE)), 0o644);
    assert_eq!(mode_of(&store.path(&issued.certificate.file())), 0o644);
}

#[test]
fn the_client_key_is_never_stored() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let body = issued.key_pem.unwrap().lines().nth(1).unwrap().to_owned();
    for entry in walk(dir.path()) {
        let contents = std::fs::read_to_string(&entry).unwrap_or_default();
        assert!(
            !contents.contains(&body),
            "{} holds the client key",
            entry.display()
        );
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flat_map(|e| {
            let path = e.unwrap().path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

#[test]
fn concurrent_issues_all_reach_the_ledger() {
    let dir = crate::test_dir();
    new_ca(dir.path());
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = dir.path().to_owned();
            std::thread::spawn(move || {
                let ca = Ca::open(&Store::new(path)).unwrap();
                ca.issue(
                    device("Anna", &format!("device {i}")),
                    Key::Generate,
                    NOW,
                    DEFAULT_CLIENT_VALIDITY,
                )
                .unwrap();
            })
        })
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    let ca = Ca::open(&Store::new(dir.path())).unwrap();
    assert_eq!(ca.ledger().unwrap().certificates().count(), 8);
}

/// nginx verifies client certificates with OpenSSL, so OpenSSL's verdict is the one that counts.
#[test]
fn openssl_accepts_a_valid_certificate_and_rejects_a_revoked_one() {
    // OpenSSL checks against the real clock, so this test runs on it too.
    let now = OffsetDateTime::now_utc();
    let dir = crate::test_dir();
    let ca = Ca::create(
        &Store::new(dir.path()),
        "Test family",
        now,
        DEFAULT_CA_VALIDITY,
    )
    .unwrap();
    let valid = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            now,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let lost = ca
        .issue(
            device("Anna", "tablet"),
            Key::Generate,
            now,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    ca.revoke(
        Target::Certificate(lost.certificate.serial),
        Reason::Lost,
        now,
    )
    .unwrap();

    let openssl_verify = |certificate: &str| {
        let path = dir.path().join("client.crt");
        std::fs::write(&path, certificate).unwrap();
        let output = Command::new("openssl")
            .arg("verify")
            .arg("-CAfile")
            .arg(ca.store().path(CERTIFICATE_FILE))
            .arg("-crl_check")
            .arg("-CRLfile")
            .arg(ca.store().path(CRL_FILE))
            .args(["-purpose", "sslclient"])
            .arg(&path)
            .output()
            .expect("these tests need the `openssl` command");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        )
    };
    let (ok, output) = openssl_verify(&valid.certificate_pem);
    assert!(ok, "{output}");
    let (ok, output) = openssl_verify(&lost.certificate_pem);
    assert!(!ok && output.contains("certificate revoked"), "{output}");
}

#[test]
fn the_ledger_records_the_certificates_own_times() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let now = NOW + Duration::nanoseconds(123_456_789);
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            now,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let der = der(&issued.certificate_pem);
    let (_, certificate) = x509_parser::parse_x509_certificate(&der).unwrap();
    assert_eq!(
        issued.certificate.not_before,
        certificate.validity().not_before.to_datetime()
    );
    assert_eq!(
        issued.certificate.not_after,
        certificate.validity().not_after.to_datetime()
    );
    ca.revoke(
        Target::Certificate(issued.certificate.serial),
        Reason::Lost,
        now,
    )
    .unwrap();
    let ledger = ca.ledger().unwrap();
    let (_, revoked) = ledger.find(&issued.certificate.serial.to_string()).unwrap();
    assert_eq!(revoked.revoked, Some(NOW));
}

#[test]
fn an_agent_is_named_as_an_agent_and_verifies() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            Holder::Agent("backup"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    assert_eq!(issued.holder, "backup (agent)");
    let der = der(&issued.certificate_pem);
    let (_, certificate) = x509_parser::parse_x509_certificate(&der).unwrap();
    assert_eq!(certificate.subject().to_string(), "OU=agents, CN=backup");
    verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), NOW).unwrap();
}

#[test]
fn a_signing_request_keeps_the_agents_key_and_the_cas_terms() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    // The request asks for things the CA does not give: a server name, another subject, CA powers.
    let mut asked = CertificateParams::new(vec!["evil.example".to_owned()]).unwrap();
    asked.distinguished_name.push(DnType::CommonName, "admin");
    asked.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let request = asked.serialize_request(&key).unwrap().pem().unwrap();

    let issued = ca
        .issue(
            Holder::Agent("backup"),
            Key::Request(&request),
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    assert!(issued.key_pem.is_none());
    let der = der(&issued.certificate_pem);
    let (_, certificate) = x509_parser::parse_x509_certificate(&der).unwrap();
    assert_eq!(certificate.public_key().raw, key.subject_public_key_info());
    assert_eq!(certificate.subject().to_string(), "OU=agents, CN=backup");
    assert!(!certificate.is_ca());
    assert!(certificate.subject_alternative_name().unwrap().is_none());
    verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), NOW).unwrap();
}

#[test]
fn a_tampered_signing_request_is_refused_and_nothing_is_issued() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let pem = CertificateParams::default()
        .serialize_request(&key)
        .unwrap()
        .pem()
        .unwrap();
    // The last line of base64 encodes the end of the signature; change its first character.
    let mut lines: Vec<String> = pem.lines().map(str::to_owned).collect();
    let last = lines.len() - 2;
    let first = lines[last].remove(0);
    lines[last].insert(0, if first == 'A' { 'B' } else { 'A' });
    let request = lines.join("\n");
    let error = ca
        .issue(
            Holder::Agent("backup"),
            Key::Request(&request),
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("signing request"), "{error}");
    assert_eq!(ca.ledger().unwrap().certificates().count(), 0);
}

/// What an administrator would run on the agent's machine.
#[test]
fn a_signing_request_from_openssl_is_accepted() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let key = dir.path().join("agent.key");
    let request = dir.path().join("agent.csr");
    let status = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
        ])
        .args(["-subj", "/CN=backup"])
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&request)
        .output()
        .expect("these tests need the `openssl` command");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let request = std::fs::read_to_string(&request).unwrap();
    let issued = ca
        .issue(
            Holder::Agent("backup"),
            Key::Request(&request),
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    verify(&ca, &issued.certificate_pem, Some(&crl(&ca)), NOW).unwrap();
}

#[test]
fn revoking_a_person_fails_every_device_and_spares_everyone_else() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issue = |holder| {
        ca.issue(holder, Key::Generate, NOW, DEFAULT_CLIENT_VALIDITY)
            .unwrap()
            .certificate_pem
    };
    let phone = issue(device("Anna", "phone"));
    let laptop = issue(device("Anna", "laptop"));
    let ben = issue(device("Ben", "phone"));
    let agent = issue(Holder::Agent("backup"));
    let revoked = ca
        .revoke(Target::Person("Anna"), Reason::Retired, NOW)
        .unwrap();
    assert_eq!(revoked.len(), 2);
    let crl = crl(&ca);
    for gone in [&phone, &laptop] {
        assert_eq!(
            verify(&ca, gone, Some(&crl), NOW),
            Err(webpki::Error::CertRevoked)
        );
    }
    for kept in [&ben, &agent] {
        verify(&ca, kept, Some(&crl), NOW).unwrap();
    }
}

#[test]
fn a_rename_reaches_the_next_certificate_only() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let before = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    ca.rename(Target::Person("Anna"), "Ana").unwrap();
    let after = ca
        .issue(
            device("Ana", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let subject = |pem: &str| {
        let der = der(pem);
        x509_parser::parse_x509_certificate(&der)
            .unwrap()
            .1
            .subject()
            .to_string()
    };
    assert_eq!(
        subject(&before.certificate_pem),
        "OU=people, CN=Anna (phone)"
    );
    assert_eq!(subject(&after.certificate_pem), "OU=people, CN=Ana (phone)");
    let ledger = ca.ledger().unwrap();
    assert_eq!(ledger.people.len(), 1);
    assert_eq!(ledger.people[0].devices[0].certificates.len(), 2);
}

#[test]
fn the_ca_reports_its_name_and_its_crls_state() {
    let dir = crate::test_dir();
    let created = new_ca(dir.path());
    let ca = Ca::open(&Store::new(dir.path())).unwrap();
    assert_eq!((created.name(), ca.name()), ("Test family", "Test family"));
    assert_eq!(
        ca.crl_status().unwrap(),
        CrlStatus {
            number: 1,
            next_update: NOW + CRL_VALIDITY
        }
    );
    ca.refresh_crl(NOW + Duration::days(3)).unwrap();
    assert_eq!(
        ca.crl_status().unwrap(),
        CrlStatus {
            number: 2,
            next_update: NOW + Duration::days(3) + CRL_VALIDITY
        }
    );
}

#[test]
fn a_certificates_subject_is_read_as_issued() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    ca.rename(Target::Person("Anna"), "Ana").unwrap();
    assert_eq!(
        ca.subject(issued.certificate.serial).unwrap(),
        "CN=Anna (phone),OU=people"
    );
}

/// A comma in a name must not read as the end of the field: nginx escapes it, and so does the UI.
#[test]
fn a_subject_escapes_what_rfc_4514_reserves() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Anna, the elder", "#1 phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    assert_eq!(
        ca.subject(issued.certificate.serial).unwrap(),
        "CN=Anna\\, the elder (#1 phone),OU=people"
    );
    assert_eq!(rfc4514_escape("#a b "), "\\#a b\\ ");
}

/// nginx's `$ssl_client_s_dn` is OpenSSL's RFC 2253 form, which writes each byte beyond ASCII as
/// `\XX`: the UI shows the same text, so a `map` copied from it matches.
#[test]
fn a_subject_reads_as_nginx_shows_it() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let issued = ca
        .issue(
            device("Željko", "telefon, \"novi\""),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let path = dir.path().join("client.crt");
    std::fs::write(&path, &issued.certificate_pem).unwrap();
    let output = Command::new("openssl")
        .args(["x509", "-noout", "-subject", "-nameopt", "RFC2253", "-in"])
        .arg(&path)
        .output()
        .expect("these tests need the `openssl` command");
    let printed = String::from_utf8(output.stdout).unwrap();
    let subject = ca.subject(issued.certificate.serial).unwrap();
    assert_eq!(printed.trim_end(), format!("subject={subject}"));
    assert_eq!(
        subject,
        "CN=\\C5\\BDeljko (telefon\\, \\\"novi\\\"),OU=people"
    );
}

/// A crash after the ledger is saved and before the CRL is written must not lose the revocation,
/// nor sign two different CRLs with one number.
#[test]
fn a_revocation_whose_crl_was_never_written_reaches_the_next_one() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let lost = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            NOW,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let before = crl(&ca);
    // A folder in the CRL's place: the CRL cannot be written.
    let crl_path = ca.store().path(CRL_FILE);
    std::fs::remove_file(&crl_path).unwrap();
    std::fs::create_dir_all(crl_path.join("in-the-way")).unwrap();
    assert!(
        ca.revoke(
            Target::Certificate(lost.certificate.serial),
            Reason::Lost,
            NOW
        )
        .is_err()
    );
    std::fs::remove_dir_all(&crl_path).unwrap();
    std::fs::write(&crl_path, &before).unwrap();
    assert_eq!(ca.ledger().unwrap().crl_number, 2, "the number is spent");

    ca.refresh_crl(NOW).unwrap();
    assert_eq!(ca.crl_status().unwrap().number, 3);
    assert_eq!(
        verify(&ca, &lost.certificate_pem, Some(&crl(&ca)), NOW),
        Err(webpki::Error::CertRevoked)
    );
}

/// A ledger behind the CRL on disk - restored from a backup, or edited by hand - still never
/// signs a number that CRL used.
#[test]
fn the_next_crl_number_passes_the_one_on_disk() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    for _ in 0..4 {
        ca.refresh_crl(NOW).unwrap();
    }
    assert_eq!(ca.crl_status().unwrap().number, 5);
    ca.change(|ledger| {
        ledger.crl_number = 2;
        Ok(())
    })
    .unwrap();
    ca.refresh_crl(NOW).unwrap();
    assert_eq!(ca.crl_status().unwrap().number, 6);
    assert_eq!(ca.ledger().unwrap().crl_number, 6);
}

#[test]
fn the_crl_is_valid_from_a_little_before_now() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let der = der(&crl(&ca));
    let (_, parsed) = x509_parser::parse_x509_crl(&der).unwrap();
    assert_eq!(parsed.last_update().to_datetime(), NOW - BACKDATE);
    assert_eq!(
        parsed.next_update().unwrap().to_datetime(),
        NOW + CRL_VALIDITY
    );
}

/// Should the CA's certificate ever land among a system's trusted authorities, it vouches for
/// no server.
#[test]
fn the_ca_vouches_for_clients_only() {
    let now = OffsetDateTime::now_utc();
    let dir = crate::test_dir();
    let ca = Ca::create(
        &Store::new(dir.path()),
        "Test family",
        now,
        DEFAULT_CA_VALIDITY,
    )
    .unwrap();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(vec!["bank.example".to_owned()]).unwrap();
    params.not_before = now - BACKDATE;
    params.not_after = now + Duration::days(30);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server = params.signed_by(&key, &ca.issuer().unwrap()).unwrap();
    let client = ca
        .issue(
            device("Anna", "phone"),
            Key::Generate,
            now,
            DEFAULT_CLIENT_VALIDITY,
        )
        .unwrap();
    let openssl_verify = |certificate: &str, purpose: &str| {
        let path = dir.path().join("leaf.crt");
        std::fs::write(&path, certificate).unwrap();
        let output = Command::new("openssl")
            .arg("verify")
            .arg("-CAfile")
            .arg(ca.store().path(CERTIFICATE_FILE))
            .args(["-purpose", purpose])
            .arg(&path)
            .output()
            .expect("these tests need the `openssl` command");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        )
    };
    let (ok, output) = openssl_verify(&server.pem(), "sslserver");
    // Depth 1 is the CA: its own purpose is what refuses the server.
    assert!(
        !ok && output.contains("at 1 depth lookup: unsuitable certificate purpose"),
        "{output}"
    );
    let (ok, output) = openssl_verify(&client.certificate_pem, "sslclient");
    assert!(ok, "{output}");
}

/// A request made by `openssl req` with `newkey` and signed with `digest`, if any.
fn openssl_request(dir: &Path, newkey: &[&str], digest: Option<&str>) -> String {
    let key = dir.join("agent.key");
    let request = dir.join("agent.csr");
    let output = Command::new("openssl")
        .args(["req", "-new", "-nodes", "-subj", "/CN=backup", "-newkey"])
        .args(newkey)
        .args(digest)
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&request)
        .output()
        .expect("these tests need the `openssl` command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(&request).unwrap()
}

/// A request given as text, or made by `openssl req` with `newkey` and signed with `digest`.
enum Request {
    /// RSA keys take long to generate, so these are made once and kept with the tests.
    Kept(&'static str),
    Made(&'static [&'static str], Option<&'static str>),
}

#[test]
fn a_signing_request_must_carry_a_strong_key_nginx_takes() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    let request = |request: &Request| match request {
        Request::Kept(text) => (*text).to_owned(),
        Request::Made(newkey, digest) => openssl_request(dir.path(), newkey, *digest),
    };
    let accepted = [
        (
            "P-256",
            Request::Made(&["ec", "-pkeyopt", "ec_paramgen_curve:P-256"], None),
        ),
        (
            "P-384",
            Request::Made(
                &["ec", "-pkeyopt", "ec_paramgen_curve:P-384"],
                Some("-sha384"),
            ),
        ),
        ("RSA-2048", Request::Kept(requests::RSA_2048)),
        ("Ed25519", Request::Made(&["ed25519"], None)),
    ];
    for (i, (kind, made)) in accepted.iter().enumerate() {
        let name = format!("agent{i}");
        let issued = ca
            .issue(
                Holder::Agent(&name),
                Key::Request(&request(made)),
                NOW,
                DEFAULT_CLIENT_VALIDITY,
            )
            .unwrap_or_else(|e| panic!("{kind}: {e:#}"));
        let der = der(&issued.certificate_pem);
        let (_, certificate) = x509_parser::parse_x509_certificate(&der).unwrap();
        assert!(certificate.public_key().parsed().is_ok(), "{kind}");
    }
    let refused = [
        ("RSA-1024", Request::Kept(requests::RSA_1024)),
        (
            "P-521",
            Request::Made(&["ec", "-pkeyopt", "ec_paramgen_curve:P-521"], None),
        ),
        (
            "secp256k1",
            Request::Made(&["ec", "-pkeyopt", "ec_paramgen_curve:secp256k1"], None),
        ),
    ];
    for (kind, made) in &refused {
        let error = ca
            .issue(
                Holder::Agent("refused"),
                Key::Request(&request(made)),
                NOW,
                DEFAULT_CLIENT_VALIDITY,
            )
            .err()
            .unwrap_or_else(|| panic!("{kind} was signed"));
        assert!(
            format!("{error:#}").contains("key is not one the CA signs"),
            "{kind}: {error:#}"
        );
    }
    assert_eq!(ca.ledger().unwrap().certificates().count(), 4);
}

/// rcgen takes a request's key algorithm from its signature: a P-256 key signed with SHA-384
/// would come out as a certificate with a key nobody can read.
#[test]
fn a_signing_request_whose_signature_does_not_match_its_key_is_refused() {
    let dir = crate::test_dir();
    let ca = new_ca(dir.path());
    for (newkey, digest) in [
        (["ec", "-pkeyopt", "ec_paramgen_curve:P-256"], "-sha384"),
        (["ec", "-pkeyopt", "ec_paramgen_curve:P-384"], "-sha256"),
    ] {
        let request = openssl_request(dir.path(), &newkey, Some(digest));
        let error = ca
            .issue(
                Holder::Agent("backup"),
                Key::Request(&request),
                NOW,
                DEFAULT_CLIENT_VALIDITY,
            )
            .err()
            .unwrap_or_else(|| panic!("{newkey:?} {digest} was signed"));
        assert!(
            format!("{error:#}").contains("does not match its key"),
            "{error:#}"
        );
    }
    assert_eq!(ca.ledger().unwrap().certificates().count(), 0);
}
