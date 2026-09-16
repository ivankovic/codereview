//! Who may see the page, and the sessions that lets them keep seeing it.
//!
//! Two ways in. A **password**, checked against an Argon2 hash the deployment supplies, is
//! what a site on a domain uses. A **token**, invented for the run and printed with the URL,
//! is what a local run uses so that opening a browser is enough. Either way the browser ends
//! up with a session: a cookie holding an identifier, and, embedded in the page, a second
//! secret the API asks for in a header. The cookie alone cannot drive the API, so a page on
//! another site cannot act as the signed-in browser.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// The cookie holding the session identifier.
pub(crate) const COOKIE: &str = "codereview_session";

/// How long a session lasts before the password is asked for again.
const SESSION_LIFE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The same, for the cookie.
pub(crate) const SESSION_MAX_AGE: u64 = SESSION_LIFE.as_secs();

/// How many browsers may be signed in at once. The one expiring soonest makes way.
const MAX_SESSIONS: usize = 32;

/// Wrong passwords tolerated before answering starts being delayed.
const FREE_ATTEMPTS: u32 = 5;

/// The longest that delay grows to.
const MAX_LOCKOUT: Duration = Duration::from_secs(15 * 60);

/// A token given through the environment must be worth having; a short one is a typo or a
/// password, and both are guessable.
const MIN_TOKEN_LEN: usize = 16;

/// What a browser must show to be let in.
pub(crate) enum Auth {
    /// A token printed at start-up and carried in the first URL.
    Token(String),
    /// A password, checked against this Argon2 hash.
    Password(String),
}

impl Auth {
    /// `CODEREVIEW_PASSWORD_HASH` when it is set, otherwise `CODEREVIEW_TOKEN`, otherwise a
    /// token invented for this run.
    pub(crate) fn from_env() -> Result<Self> {
        let hash = std::env::var("CODEREVIEW_PASSWORD_HASH").ok();
        let token = std::env::var("CODEREVIEW_TOKEN").ok();
        if hash.is_some() && token.is_some() {
            bail!("CODEREVIEW_PASSWORD_HASH and CODEREVIEW_TOKEN are both set; use one");
        }
        if let Some(hash) = hash {
            let hash = hash.trim().to_string();
            // Fail now rather than on the first sign-in attempt.
            argon2::PasswordHash::new(&hash)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context("CODEREVIEW_PASSWORD_HASH is not an Argon2 hash; make one with `codereview hash-password`")?;
            return Ok(Auth::Password(hash));
        }
        match token {
            Some(token) => {
                let token = token.trim().to_string();
                anyhow::ensure!(
                    token.len() >= MIN_TOKEN_LEN,
                    "CODEREVIEW_TOKEN must be at least {MIN_TOKEN_LEN} characters; \
                     `openssl rand -hex 32` makes a good one"
                );
                Ok(Auth::Token(token))
            }
            None => Ok(Auth::Token(random_secret())),
        }
    }

    pub(crate) fn is_password(&self) -> bool {
        matches!(self, Auth::Password(_))
    }

    /// The token that opens a session, when that is how this run works.
    pub(crate) fn token(&self) -> Option<&str> {
        match self {
            Auth::Token(t) => Some(t),
            Auth::Password(_) => None,
        }
    }

    /// Whether `password` is the one configured. Always false for a token run, so a stray
    /// sign-in attempt cannot succeed there.
    pub(crate) fn password_matches(&self, password: &str) -> bool {
        let Auth::Password(hash) = self else {
            return false;
        };
        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        match PasswordHash::new(hash) {
            Ok(parsed) => Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok(),
            Err(_) => false,
        }
    }
}

/// An Argon2id hash of `password`, in the usual `$argon2id$...` form, for
/// `CODEREVIEW_PASSWORD_HASH`.
pub fn hash_password(password: &str) -> Result<String> {
    use argon2::{Argon2, PasswordHasher};
    // The salt is generated here, from the system's randomness, and travels inside the
    // hash, so two people with one password still get two hashes.
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .to_string())
}

/// One signed-in browser.
struct Live {
    /// What its cookie carries.
    id: String,
    /// What its page sends the API.
    api_token: String,
    expires: Instant,
}

/// Every signed-in browser, and how sign-in has been going.
#[derive(Default)]
pub(crate) struct Sessions {
    live: Vec<Live>,
    failures: u32,
    locked_until: Option<Instant>,
}

impl Sessions {
    /// Starts a session: the cookie's identifier and the page's API token.
    pub(crate) fn open(&mut self) -> (String, String) {
        self.sweep();
        if self.live.len() >= MAX_SESSIONS {
            let soonest = self
                .live
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.expires)
                .map(|(i, _)| i);
            if let Some(i) = soonest {
                self.live.remove(i);
            }
        }
        let (id, api_token) = (random_secret(), random_secret());
        self.live.push(Live {
            id: id.clone(),
            api_token: api_token.clone(),
            expires: Instant::now() + SESSION_LIFE,
        });
        (id, api_token)
    }

    /// The API token of the session a cookie names, when it is still live.
    pub(crate) fn api_token_for(&mut self, cookie: &str) -> Option<String> {
        self.sweep();
        self.live
            .iter()
            .find(|s| constant_eq(cookie, &s.id))
            .map(|s| s.api_token.clone())
    }

    pub(crate) fn accepts(&mut self, api_token: &str) -> bool {
        self.sweep();
        self.live
            .iter()
            .any(|s| constant_eq(api_token, &s.api_token))
    }

    /// Ends the session holding `api_token`, if any.
    pub(crate) fn close(&mut self, api_token: &str) {
        self.live.retain(|s| !constant_eq(api_token, &s.api_token));
    }

    /// How long sign-in refuses to answer, after too many wrong passwords. Guessing is
    /// slowed for everybody at once, which needs no notion of who is asking: behind a proxy
    /// the server cannot tell them apart anyway.
    pub(crate) fn locked_for(&self) -> Option<Duration> {
        let until = self.locked_until?;
        until.checked_duration_since(Instant::now())
    }

    pub(crate) fn note_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        if self.failures > FREE_ATTEMPTS {
            let steps = (self.failures - FREE_ATTEMPTS).min(10);
            let delay = Duration::from_secs(1 << steps).min(MAX_LOCKOUT);
            self.locked_until = Some(Instant::now() + delay);
        }
    }

    pub(crate) fn note_success(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }

    fn sweep(&mut self) {
        let now = Instant::now();
        self.live.retain(|s| s.expires > now);
    }
}

/// 128 random bits as hexadecimal: a token, a session identifier, a cookie value.
pub(crate) fn random_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two secrets without stopping at the first wrong byte, so timing cannot be used
/// to guess one of them a byte at a time. Their length is not a secret.
pub(crate) fn constant_eq(given: &str, want: &str) -> bool {
    let (given, want) = (given.as_bytes(), want.as_bytes());
    given.len() == want.len()
        && given
            .iter()
            .zip(want)
            .fold(0u8, |differences, (a, b)| differences | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_hash() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        let auth = Auth::Password(hash.clone());
        assert!(auth.password_matches("correct horse"));
        assert!(!auth.password_matches("correct horse "));
        assert!(!auth.password_matches(""));
        assert!(auth.token().is_none());
        // Two hashes of one password differ: each carries its own salt.
        assert_ne!(hash, hash_password("correct horse").unwrap());
        // A token run never accepts a password.
        assert!(!Auth::Token("x".into()).password_matches("x"));
    }

    #[test]
    fn sessions_open_expire_and_are_capped() {
        let mut sessions = Sessions::default();
        let (id, api) = sessions.open();
        assert_eq!(sessions.api_token_for(&id).as_deref(), Some(api.as_str()));
        assert!(sessions.accepts(&api));
        assert!(!sessions.accepts("nonsense"));
        assert!(sessions.api_token_for("nonsense").is_none());
        sessions.close(&api);
        assert!(!sessions.accepts(&api));
        assert!(sessions.api_token_for(&id).is_none());
        // The cap holds, and the first session opened is the one that goes.
        let first = sessions.open().0;
        for _ in 0..MAX_SESSIONS {
            sessions.open();
        }
        assert_eq!(sessions.live.len(), MAX_SESSIONS);
        assert!(sessions.api_token_for(&first).is_none());
    }

    #[test]
    fn guessing_gets_slower() {
        let mut sessions = Sessions::default();
        assert!(sessions.locked_for().is_none());
        for _ in 0..FREE_ATTEMPTS {
            sessions.note_failure();
        }
        assert!(sessions.locked_for().is_none(), "the first few are free");
        sessions.note_failure();
        let first = sessions.locked_for().expect("locked out");
        for _ in 0..4 {
            sessions.note_failure();
        }
        assert!(sessions.locked_for().unwrap() > first, "the delay grows");
        for _ in 0..100 {
            sessions.note_failure();
        }
        assert!(
            sessions.locked_for().unwrap() <= MAX_LOCKOUT,
            "and is capped"
        );
        sessions.note_success();
        assert!(sessions.locked_for().is_none());
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(constant_eq("abcd", "abcd"));
        assert!(!constant_eq("abcd", "abce"));
        assert!(!constant_eq("abcd", "abcde"));
        assert!(!constant_eq("", "a"));
        assert_eq!(random_secret().len(), 32);
        assert_ne!(random_secret(), random_secret());
    }
}
