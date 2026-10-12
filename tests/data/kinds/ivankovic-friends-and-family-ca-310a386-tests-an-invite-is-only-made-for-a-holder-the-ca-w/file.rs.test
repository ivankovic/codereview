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
use time::macros::datetime;

use std::os::unix::fs::MetadataExt;

use super::*;
use crate::ledger::Ledger;

const NOW: OffsetDateTime = datetime!(2026-10-03 12:00 UTC);
const ANNA_PHONE: Holder = Holder::Device {
    person: "Anna",
    device: "phone",
};

fn ca(dir: &Path) -> Ca {
    Ca::create(
        &Store::new(dir.join("state")),
        "Test family",
        NOW,
        ca::DEFAULT_CA_VALIDITY,
    )
    .unwrap()
}

fn token(made: &Made) -> String {
    made.link.rsplit('/').next().unwrap().to_owned()
}

fn files(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn status(ca: &Ca, serial: crate::ledger::Serial, now: OffsetDateTime) -> Status {
    ca.ledger()
        .unwrap()
        .find(&serial.to_string())
        .unwrap()
        .1
        .status(now)
}

fn invite_state(ledger: &Ledger) -> InviteState {
    ledger.invites[0].state
}

#[test]
fn an_invite_hands_over_its_bundle_once() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    assert!(
        made.link.starts_with("https://k.example.org/i/"),
        "{}",
        made.link
    );
    let token = token(&made);
    assert_eq!(token.len(), 43);

    let opened = open(&folder, &token, NOW).unwrap();
    assert_eq!(
        (
            opened.ca.as_str(),
            opened.holder.as_str(),
            opened.stem.as_str()
        ),
        ("Test family", "Anna (phone)", "anna-phone")
    );
    assert_eq!(
        open(&folder, &token, NOW).unwrap(),
        opened,
        "opening does not use it up"
    );

    let collected = collect(&folder, &token, NOW, "203.0.113.7 iPhone").unwrap();
    assert_eq!(collected, opened);
    assert_eq!(
        collect(&folder, &token, NOW, "again"),
        Err(Unavailable::Gone)
    );
    assert_eq!(open(&folder, &token, NOW), Err(Unavailable::Gone));
    let id = id(&token).unwrap();
    assert_eq!(files(&folder), [format!("{id}.collected")]);
}

#[test]
fn the_folder_alone_reveals_nothing() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    let token = token(&made);
    let id = id(&token).unwrap();
    let text = fs::read_to_string(folder.join(format!("{id}.invite"))).unwrap();
    assert!(
        !text.contains(&token) && !text.contains("Anna") && !text.contains("PRIVATE"),
        "{text}"
    );
    assert!(!files(&folder).iter().any(|f| f.contains(&token)));
    let other = random_token().unwrap();
    assert_eq!(
        open(&folder, &other, NOW),
        Err(Unavailable::Gone),
        "another token opens nothing"
    );
    assert_eq!(open(&folder, "not a token", NOW), Err(Unavailable::Gone));
}

#[test]
fn an_expired_invite_cannot_be_collected_or_extended() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let token = token(&make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap());
    let later = NOW + VALIDITY + Duration::seconds(1);
    assert_eq!(open(&folder, &token, later), Err(Unavailable::Expired));
    assert_eq!(
        collect(&folder, &token, later, "x"),
        Err(Unavailable::Expired)
    );

    // Someone who can write to the folder pushes the expiry out: the seal no longer opens.
    let file = folder.join(format!("{}.invite", id(&token).unwrap()));
    let mut sealed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    sealed["expires"] = serde_json::json!(later.unix_timestamp() + 86_400);
    fs::write(&file, sealed.to_string()).unwrap();
    assert_eq!(open(&folder, &token, later), Err(Unavailable::Gone));
}

#[test]
fn tend_records_a_collected_invite_and_keeps_its_certificate() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    collect(&folder, &token(&made), NOW, "203.0.113.7 iPhone").unwrap();
    let tended = tend(&ca, &folder, NOW + Duration::hours(1)).unwrap();
    assert_eq!(
        tended,
        Tended {
            collected: vec!["Anna (phone)".into()],
            closed: vec![],
            revoked: false,
            problems: vec![]
        }
    );
    let ledger = ca.ledger().unwrap();
    assert_eq!(invite_state(&ledger), InviteState::Collected);
    assert_eq!(
        ledger.invites[0].collected_by.as_deref(),
        Some("203.0.113.7 iPhone")
    );
    assert_eq!(ledger.invites[0].closed, Some(NOW));
    assert_eq!(status(&ca, made.invite.serial, NOW), Status::Valid);
    assert!(
        files(&folder).is_empty(),
        "the receipt is moved into the ledger"
    );
    assert_eq!(
        tend(&ca, &folder, NOW + VALIDITY * 2).unwrap(),
        Tended::default(),
        "nothing more to do"
    );
}

#[test]
fn tend_revokes_what_was_never_collected() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    assert_eq!(
        tend(&ca, &folder, NOW + Duration::hours(23)).unwrap(),
        Tended::default(),
        "still open"
    );
    let later = NOW + VALIDITY;
    let tended = tend(&ca, &folder, later).unwrap();
    assert_eq!(tended.closed, ["Anna (phone)"]);
    assert!(tended.revoked);
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Expired);
    assert_eq!(status(&ca, made.invite.serial, later), Status::Revoked);
    assert!(files(&folder).is_empty());
}

#[test]
fn cancelling_deletes_the_invite_and_revokes_its_certificate() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    cancel(&ca, &folder, made.invite.serial, NOW).unwrap();
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Cancelled);
    assert_eq!(status(&ca, made.invite.serial, NOW), Status::Revoked);
    assert_eq!(open(&folder, &token(&made), NOW), Err(Unavailable::Gone));
    assert!(
        cancel(&ca, &folder, made.invite.serial, NOW).is_err(),
        "no longer open"
    );
}

#[test]
fn revoking_the_device_closes_its_invite() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    ca.revoke(Target::Holder(ANNA_PHONE), Reason::Retired, NOW)
        .unwrap();
    let tended = tend(&ca, &folder, NOW).unwrap();
    assert_eq!(tended.closed, ["Anna (phone)"]);
    assert!(!tended.revoked, "it was revoked already");
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Cancelled);
    assert_eq!(open(&folder, &token(&made), NOW), Err(Unavailable::Gone));
}

#[test]
fn an_invite_is_only_made_for_a_holder_the_ca_would_issue_to() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    ca.issue(ANNA_PHONE, Key::Generate, NOW, ca::DEFAULT_CLIENT_VALIDITY)
        .unwrap();
    ca.revoke(Target::Holder(ANNA_PHONE), Reason::Retired, NOW)
        .unwrap();
    let folder = dir.path().join("invites");
    assert!(
        make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW)
            .err()
            .unwrap()
            .to_string()
            .contains("retired")
    );
    assert!(ca.ledger().unwrap().invites.is_empty());
}

#[test]
fn the_folder_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make(&ca, &folder, ANNA_PHONE, "k.example.org", NOW).unwrap();
    assert_eq!(
        fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let file = folder.join(format!("{}.invite", id(&token(&made)).unwrap()));
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// The enrollment page writes the folder and could be compromised: what it leaves there is read as
/// root, under the CA's lock. Each of these is reported and closes its invite - revoking a
/// certificate nobody can show was handed over - and none stops the rest.
fn hostile_receipt(make: impl Fn(&Path)) -> (Tended, Status) {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let id = id(&token(&made)).unwrap();
    fs::remove_file(folder.join(format!("{id}.invite"))).unwrap();
    make(&folder.join(format!("{id}.collected")));
    let tended = tend(&ca, &folder, NOW).unwrap();
    (tended, status(&ca, made.invite.serial, NOW))
}

fn make_invite(ca: &Ca, folder: &Path) -> Made {
    make(ca, folder, ANNA_PHONE, "k.example.org", NOW).unwrap()
}

#[test]
fn a_receipt_that_is_a_link_is_not_followed() {
    let outside = crate::test_dir();
    let target = outside.path().join("secret");
    fs::write(&target, r#"{"at":"2026-10-03T12:00:00Z","by":"x"}"#).unwrap();
    let (tended, status) =
        hostile_receipt(|path| std::os::unix::fs::symlink(&target, path).unwrap());
    assert!(
        tended.problems[0].contains("not an ordinary file"),
        "{tended:?}"
    );
    assert_eq!(status, Status::Revoked);
    assert!(
        target.exists(),
        "the link is removed, not what it points to"
    );
}

#[test]
fn a_receipt_that_is_a_pipe_does_not_block() {
    let (tended, status) = hostile_receipt(|path| {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    });
    assert!(
        tended.problems[0].contains("not an ordinary file"),
        "{tended:?}"
    );
    assert_eq!(status, Status::Revoked);
}

#[test]
fn a_huge_or_damaged_receipt_is_refused() {
    let (tended, _) = hostile_receipt(|path| fs::write(path, vec![b' '; 1 << 20]).unwrap());
    assert!(tended.problems[0].contains("too large"), "{tended:?}");
    let (tended, _) = hostile_receipt(|path| fs::write(path, "garbage").unwrap());
    assert!(tended.problems[0].contains("damaged"), "{tended:?}");
}

#[test]
fn one_bad_receipt_does_not_stop_the_others() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let bad = make_invite(&ca, &folder);
    let good = make(
        &ca,
        &folder,
        Holder::Device {
            person: "Ben",
            device: "tablet",
        },
        "k.example.org",
        NOW,
    )
    .unwrap();
    let bad_id = id(&token(&bad)).unwrap();
    fs::remove_file(folder.join(format!("{bad_id}.invite"))).unwrap();
    fs::write(folder.join(format!("{bad_id}.collected")), "garbage").unwrap();
    collect(&folder, &token(&good), NOW, "203.0.113.7, iPad").unwrap();
    let tended = tend(&ca, &folder, NOW).unwrap();
    assert_eq!(tended.collected, ["Ben (tablet)"]);
    assert_eq!(tended.problems.len(), 1);
}

#[test]
fn what_the_page_writes_in_a_receipt_is_made_printable() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let id = id(&token(&made)).unwrap();
    fs::remove_file(folder.join(format!("{id}.invite"))).unwrap();
    let by = format!("\u{1b}[31mevil\nline{}", "x".repeat(1000));
    let receipt = Receipt { at: NOW, by };
    fs::write(
        folder.join(format!("{id}.collected")),
        serde_json::to_string(&receipt).unwrap(),
    )
    .unwrap();
    tend(&ca, &folder, NOW).unwrap();
    let by = ca.ledger().unwrap().invites[0]
        .collected_by
        .clone()
        .unwrap();
    assert!(!by.chars().any(char::is_control), "{by:?}");
    assert_eq!(by.chars().count(), 300);
}

/// The page renames the invite to a claim, writes the receipt, then removes the claim: a tend in
/// between must not take the invite for one never collected and revoke what is being handed over.
#[test]
fn an_invite_being_collected_is_left_alone() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let id = id(&token(&made)).unwrap();
    let claim = folder.join(format!("{id}.claimed"));
    // Claimed hours after it was made, as `collect` claims it.
    let claimed = NOW + Duration::hours(5);
    fs::rename(folder.join(format!("{id}.invite")), &claim).unwrap();
    touch(&claim, claimed).unwrap();
    let ledger_file = ca.store().path(crate::ledger::FILE);
    let inode = || fs::metadata(&ledger_file).unwrap().ino();
    let before = inode();
    assert_eq!(tend(&ca, &folder, claimed).unwrap(), Tended::default());
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Open);
    assert_eq!(inode(), before, "nothing changed, so nothing is written");
    // A claim never finished: after a while it counts as abandoned.
    let tended = tend(&ca, &folder, claimed + CLAIM_GRACE + Duration::seconds(1)).unwrap();
    assert_eq!(tended.closed, ["Anna (phone)"]);
    assert!(tended.revoked);
    assert!(!claim.exists());
}

/// The page sets the claim's time, and could set it far ahead to keep the invite open for ever.
#[test]
fn a_claim_from_the_future_counts_as_abandoned() {
    use std::time::{Duration as StdDuration, SystemTime};
    for claimed in [
        SystemTime::from(NOW + Duration::days(365 * 70)),
        SystemTime::UNIX_EPOCH,
        SystemTime::UNIX_EPOCH + StdDuration::from_secs(1 << 33),
    ] {
        let dir = crate::test_dir();
        let ca = ca(dir.path());
        let folder = dir.path().join("invites");
        let made = make_invite(&ca, &folder);
        let id = id(&token(&made)).unwrap();
        let claim = folder.join(format!("{id}.claimed"));
        fs::rename(folder.join(format!("{id}.invite")), &claim).unwrap();
        fs::File::options()
            .write(true)
            .open(&claim)
            .unwrap()
            .set_modified(claimed)
            .unwrap();
        let tended = tend(&ca, &folder, NOW + Duration::hours(1)).unwrap();
        assert_eq!(tended.closed, ["Anna (phone)"], "{claimed:?}");
        assert_eq!(
            status(&ca, made.invite.serial, NOW + Duration::hours(1)),
            Status::Revoked
        );
    }
}

/// `collect` claims no expired invite, so a claim kept fresh past the expiry is abandoned too.
#[test]
fn a_claim_is_abandoned_once_the_invite_is_long_expired() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let id = id(&token(&made)).unwrap();
    let claim = folder.join(format!("{id}.claimed"));
    fs::rename(folder.join(format!("{id}.invite")), &claim).unwrap();
    let late = made.invite.expires + CLAIM_GRACE;
    touch(&claim, late).unwrap();
    let tended = tend(&ca, &folder, late).unwrap();
    assert_eq!(tended.closed, ["Anna (phone)"]);
    assert_eq!(status(&ca, made.invite.serial, late), Status::Revoked);
}

/// Making an invite records it in the ledger before its file is written: a crash in between
/// leaves an invite without a file, whose certificate - its key was only ever in that file - is
/// revoked.
#[test]
fn an_invite_whose_file_was_never_written_is_cancelled_and_revoked() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    fs::remove_file(folder.join(format!("{}.invite", id(&token(&made)).unwrap()))).unwrap();
    let tended = tend(&ca, &folder, NOW).unwrap();
    assert_eq!(tended.closed, ["Anna (phone)"]);
    assert!(tended.revoked);
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Cancelled);
    assert_eq!(status(&ca, made.invite.serial, NOW), Status::Revoked);
}

/// The CRL cannot be written: cancelling says so, and the revocation, saved with the invite's
/// closing, reaches the next CRL.
#[test]
fn a_cancel_whose_crl_cannot_be_written_fails_and_the_revocation_is_kept() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let crl_path = ca.store().path(ca::CRL_FILE);
    let crl = fs::read(&crl_path).unwrap();
    fs::remove_file(&crl_path).unwrap();
    fs::create_dir_all(crl_path.join("in-the-way")).unwrap();
    assert!(cancel(&ca, &folder, made.invite.serial, NOW).is_err());
    assert_eq!(invite_state(&ca.ledger().unwrap()), InviteState::Cancelled);
    assert_eq!(status(&ca, made.invite.serial, NOW), Status::Revoked);

    fs::remove_dir_all(&crl_path).unwrap();
    fs::write(&crl_path, crl).unwrap();
    ca.refresh_crl(NOW).unwrap();
    let pem = fs::read_to_string(&crl_path).unwrap();
    let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
    let (_, parsed) = x509_parser::parse_x509_crl(&pem.contents).unwrap();
    assert!(
        parsed
            .iter_revoked_certificates()
            .any(|r| r.raw_serial() == made.invite.serial.0.as_slice())
    );
}

/// The page reads the invite file for every request: a link, a pipe or a huge file there opens
/// nothing, and blocks nothing.
#[test]
fn an_invite_file_that_is_not_an_ordinary_small_file_is_gone() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    let token = token(&made);
    let file = folder.join(format!("{}.invite", id(&token).unwrap()));
    let sealed = fs::read(&file).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::write(&elsewhere, &sealed).unwrap();
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    assert_eq!(open(&folder, &token, NOW), Err(Unavailable::Gone));
    assert_eq!(collect(&folder, &token, NOW, "x"), Err(Unavailable::Gone));
    fs::remove_file(&file).unwrap();
    let path = std::ffi::CString::new(file.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert_eq!(open(&folder, &token, NOW), Err(Unavailable::Gone));
    fs::remove_file(&file).unwrap();
    fs::write(&file, vec![b' '; 1 << 20]).unwrap();
    assert_eq!(open(&folder, &token, NOW), Err(Unavailable::Gone));
    fs::write(&file, sealed).unwrap();
    assert!(open(&folder, &token, NOW).is_ok());
}

/// Cancelling because a link leaked, just after someone collected it: the certificate they hold
/// is valid, and saying it was revoked would be the worst answer.
#[test]
fn cancelling_a_collected_invite_says_so_instead_of_claiming_a_revocation() {
    let dir = crate::test_dir();
    let ca = ca(dir.path());
    let folder = dir.path().join("invites");
    let made = make_invite(&ca, &folder);
    collect(&folder, &token(&made), NOW, "198.51.100.9, Android").unwrap();
    let error = cancel(&ca, &folder, made.invite.serial, NOW)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("collected the invite already, from 198.51.100.9, Android"),
        "{error}"
    );
    assert!(error.contains("revoke the device"), "{error}");
    assert_eq!(status(&ca, made.invite.serial, NOW), Status::Valid);
}

/// A second link to a file outside the folder - with `fs.protected_hardlinks` off, the page could
/// make one to any file - is not handed over: root would give that file away.
#[test]
fn handing_the_folder_over_skips_files_linked_from_elsewhere() {
    use std::os::unix::fs::MetadataExt;
    let mut groups = [0; 64];
    let count = unsafe { libc::getgroups(64, groups.as_mut_ptr()) };
    let mine = unsafe { libc::getegid() };
    let Some(&other) = groups[..count.max(0) as usize].iter().find(|&&g| g != mine) else {
        eprintln!("skipped: this user is in one group only");
        return;
    };
    let dir = crate::test_dir();
    let folder = dir.path().join("invites");
    fs::create_dir(&folder).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "").unwrap();
    let id = "ab".repeat(32);
    fs::hard_link(&outside, folder.join(format!("{id}.collected"))).unwrap();
    fs::write(folder.join(format!("{id}.invite")), "{}").unwrap();
    fs::write(folder.join("unrelated"), "").unwrap();
    let me = fs::metadata(&folder).unwrap().uid();
    hand_over_folder(&folder, me, other).unwrap();
    assert_eq!(fs::metadata(&folder).unwrap().gid(), other);
    assert_eq!(
        fs::metadata(folder.join(format!("{id}.invite")))
            .unwrap()
            .gid(),
        other
    );
    assert_eq!(fs::metadata(&outside).unwrap().gid(), mine);
    assert_eq!(fs::metadata(folder.join("unrelated")).unwrap().gid(), mine);
}

/// Handing the folder over gives away only what is really in it: a link left there by the page
/// points nowhere it can reach.
#[test]
fn handing_the_folder_over_never_follows_a_link() {
    let dir = crate::test_dir();
    let folder = dir.path().join("invites");
    fs::create_dir(&folder).unwrap();
    // Someone else's file (root's, or nobody's in a build sandbox): following the link and
    // changing it would fail, and did.
    std::os::unix::fs::symlink("/etc/passwd", folder.join("x")).unwrap();
    fs::write(folder.join("an.invite"), "{}").unwrap();
    let owner = fs::metadata("/etc/passwd").unwrap().uid();
    let me = fs::metadata(&folder).unwrap();
    hand_over_folder(&folder, me.uid(), me.gid()).unwrap();
    assert_eq!(fs::metadata("/etc/passwd").unwrap().uid(), owner);
}
