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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use time::macros::datetime;

use super::*;

const NOW: OffsetDateTime = datetime!(2026-10-03 12:00 UTC);

fn screen(session: &mut Session) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal.draw(|frame| session.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(session: &mut Session, code: KeyCode) {
    session.handle_key(KeyEvent::new(code, KeyModifiers::NONE), NOW);
}

fn type_text(session: &mut Session, text: &str) {
    text.chars().for_each(|c| key(session, KeyCode::Char(c)));
}

/// Empties the focused field.
fn clear(session: &mut Session) {
    (0..64).for_each(|_| key(session, KeyCode::Backspace));
}

fn assert_shows(session: &mut Session, expected: &str) {
    let screen = screen(session);
    assert!(screen.contains(expected), "no {expected:?} on\n{screen}");
}

fn session_in(dir: &std::path::Path) -> Session {
    Session::new(Store::new(dir.join("state")), dir.join("out"), NOW)
}

#[test]
fn without_a_ca_the_ui_opens_on_its_setup() {
    let dir = crate::test_dir();
    let mut session = session_in(dir.path());
    let state = dir.path().join("state");
    assert_shows(
        &mut session,
        &format!("There is no CA in {} yet.", state.display()),
    );
    assert_shows(&mut session, "Create the CA");
    assert_shows(&mut session, "Valid for, in years  10");
    assert!(
        !state.exists(),
        "nothing is created before the administrator says so"
    );
}

#[test]
fn setting_up_creates_the_ca_and_says_what_comes_next() {
    let dir = crate::test_dir();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    let mut session = session_in(dir.path());
    clear(&mut session);
    type_text(&mut session, "Test family");
    key(&mut session, KeyCode::Enter);
    clear(&mut session);
    type_text(&mut session, "5");
    key(&mut session, KeyCode::Enter);

    let state = dir.path().join("state");
    let ca = Ca::open(&Store::new(&state)).unwrap();
    assert_eq!(ca.name(), "Test family");
    assert_eq!(ca.not_after(), NOW + ca::years(5));
    let screen = screen(&mut session);
    assert!(
        screen.contains("Test family is ready, valid until 3 Oct 2031."),
        "{screen}"
    );
    assert!(
        screen.contains("Next, on the Nginx tab, tell it where nginx is"),
        "{screen}"
    );

    key(&mut session, KeyCode::Enter);
    assert_shows(&mut session, "No one has a certificate yet");
    key(&mut session, KeyCode::Char('p'));
    assert_eq!(
        session.app().unwrap().form_purpose(),
        Some(&crate::tui::components::form::Purpose::NewPerson),
        "the CA works straight away"
    );
}

#[test]
fn a_validity_that_is_not_a_number_of_years_is_refused() {
    let dir = crate::test_dir();
    let mut session = session_in(dir.path());
    for years in ["0", "ten", "31"] {
        key(&mut session, KeyCode::Enter);
        clear(&mut session);
        type_text(&mut session, years);
        key(&mut session, KeyCode::Enter);
        assert_shows(
            &mut session,
            "The validity is a whole number of years, 1 to 30.",
        );
        key(&mut session, KeyCode::Up);
    }
    assert!(!dir.path().join("state").exists());
}

#[test]
fn a_folder_it_may_not_write_says_how_to_go_on() {
    let dir = crate::test_dir();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(locked.join("probe"), "").is_ok() {
        return; // Running as root, which may write anywhere: there is nothing to refuse.
    }
    let mut session = Session::new(Store::new(locked.join("state")), dir.path().to_owned(), NOW);
    key(&mut session, KeyCode::Enter);
    key(&mut session, KeyCode::Enter);
    let screen = screen(&mut session);
    assert!(screen.contains("Permission denied"), "{screen}");
    assert!(screen.contains("sudo"), "{screen}");
    assert!(screen.contains("--state-dir"), "{screen}");
    assert!(session.app().is_none());
}

#[test]
fn a_ca_that_cannot_be_opened_is_shown_not_touched() {
    let first = crate::test_dir();
    let second = crate::test_dir();
    for dir in [&first, &second] {
        Ca::create(
            &Store::new(dir.path().join("state")),
            "Test",
            NOW,
            ca::DEFAULT_CA_VALIDITY,
        )
        .unwrap();
    }
    let key_file = |dir: &tempfile::TempDir| dir.path().join("state").join(ca::KEY_FILE);
    std::fs::copy(key_file(&second), key_file(&first)).unwrap();
    let before = std::fs::read(key_file(&first)).unwrap();

    let mut session = session_in(first.path());
    assert_shows(&mut session, "cannot be opened");
    assert_shows(&mut session, "does not belong to");
    key(&mut session, KeyCode::Char('n'));
    assert_eq!(std::fs::read(key_file(&first)).unwrap(), before);
    key(&mut session, KeyCode::Char('q'));
    assert!(session.should_quit());
}

#[test]
fn an_existing_ca_opens_straight_away() {
    let dir = crate::test_dir();
    Ca::create(
        &Store::new(dir.path().join("state")),
        "Test family",
        NOW,
        ca::DEFAULT_CA_VALIDITY,
    )
    .unwrap();
    let mut session = session_in(dir.path());
    assert_shows(&mut session, "Friends and Family CA · Test family");
    assert!(session.app().is_some());
}

#[test]
fn setup_quits_on_esc_and_control_c_but_q_is_text() {
    let dir = crate::test_dir();
    let mut session = session_in(dir.path());
    key(&mut session, KeyCode::Char('q'));
    assert!(!session.should_quit());
    key(&mut session, KeyCode::Esc);
    assert!(session.should_quit());
    let mut session = session_in(dir.path());
    session.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        NOW,
    );
    assert!(session.should_quit());
}

#[test]
fn the_suggested_name_is_the_machines() {
    let name = default_name();
    assert!(!name.is_empty());
    assert!(
        name.chars().next().unwrap().is_uppercase()
            || !name.chars().next().unwrap().is_alphabetic()
    );
}
