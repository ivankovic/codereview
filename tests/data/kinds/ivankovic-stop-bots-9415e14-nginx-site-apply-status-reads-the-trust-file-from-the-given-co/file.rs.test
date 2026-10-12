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

//! Discovers NGINX sites by walking a config directory, and injects/removes a
//! sentinel-marked `if ($http_user_agent ...) { return <code>; }` block inside
//! each site's `server { ... }` block to block unwanted bots.
//!
//! The sentinel comments make the edit idempotent and easy to spot/undo by
//! hand: re-running only ever replaces the marked lines, never anything else
//! in the file.
//!
//! Everything that shapes the generated text lives in one [`BlockConfig`],
//! and [`site_apply_status`] compares the *rendered block* against what's on
//! disk rather than reconstructing individual fields back out of it. That's
//! the invariant to preserve when adding a knob here: put it on
//! `BlockConfig` and the staleness check keeps working; smuggle it in as a
//! separate argument to `block_text` and an already-applied site will read
//! as up to date while its config still carries the old text.

use crate::db::BlockResponse;
pub use crate::dirfd::DirFile;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const BLOCK_BEGIN: &str = "# BEGIN stop-bots (DO NOT EDIT)";
const BLOCK_END: &str = "# END stop-bots";

/// A `server { ... }` block found while scanning a config file.
#[derive(Debug, Clone)]
pub struct ServerBlock {
    pub names: Vec<String>,
    /// Whether this block terminates TLS — `listen ... ssl`, or an
    /// `ssl_certificate` directive.
    ///
    /// Load-bearing for exactly one thing: HTTP/1.x rejection. Browsers
    /// only negotiate HTTP/2 over TLS (h2c is effectively unused on the
    /// public web), so on a plain-`listen 80` block *every* request is
    /// HTTP/1.1 — the redirect a visitor's browser makes before it ever
    /// reaches the HTTPS block, ACME validation, everything. Emitting the
    /// rejection there would be 100% false positives, and a typical site
    /// has both blocks under one `server_name`, so a per-site setting
    /// reaches both. See `BlockConfig::reject_http_1x`.
    pub is_tls: bool,
    open: usize,
    close: usize,
}

/// A site discovered while scanning a config directory.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredSite {
    pub server_name: String,
    pub config_path: PathBuf,
}

/// A config file split the way NGINX's own reader (`ngx_conf_read_token`)
/// splits it: `{`, `}`, `;` and words, each with the byte offset it starts
/// at, plus the byte span of every `#` comment.
///
/// **Quote-aware, because what this tool writes is.** A sentinel block
/// embeds upstream bot patterns inside double quotes, and a pattern may
/// legitimately contain `#`, `{` or `}` — or, from a hostile list, the
/// marker text itself. A reader that took every `#` for a comment and
/// every brace for structure re-parsed its own output differently from
/// NGINX: a second run duplicated the whole block, left a stray
/// `return 403; }` outside it, or inserted the new block *inside* the
/// quoted string, which is a way to write directives.
///
/// The rules, each checked against a real `nginx -T`:
///
/// - A `"` or `'` opens a quoted word only where a word starts; inside,
///   a backslash escapes the next character. A quote in the middle of a
///   bare word is an ordinary character.
/// - `#` starts a comment only where a word could start. `a#b` is a word.
/// - A bare word ends at whitespace, `;` or `{` (not `${`, a variable).
///   A `}` inside one is an ordinary character (`add_header X a}b;` is
///   valid); only at the start of a token is it a block end.
/// - A backslash escapes the next character in a bare word too: `a\;b`
///   is one word.
struct Lexed {
    /// Quoted words carry their contents without the quotes (escapes are
    /// left as written; nothing here needs them undone).
    tokens: Vec<(String, usize)>,
    comments: Vec<(usize, usize)>,
}

fn lex(content: &str) -> Lexed {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b @ (b';' | b'{' | b'}') => {
                tokens.push(((b as char).to_string(), i));
                i += 1;
            }
            b'#' => {
                let end = content[i..].find('\n').map_or(len, |n| i + n);
                comments.push((i, end));
                i = end;
            }
            quote @ (b'"' | b'\'') => {
                let start = i;
                i += 1;
                let body_start = i;
                while i < len && bytes[i] != quote {
                    // Skipping the escaped byte is enough even when it
                    // starts a multi-byte character: its continuation bytes
                    // can never equal an ASCII quote.
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let body_end = i.min(len);
                tokens.push((
                    String::from_utf8_lossy(&bytes[body_start..body_end]).into_owned(),
                    start,
                ));
                i = (i + 1).min(len);
            }
            _ => {
                let start = i;
                while i < len {
                    match bytes[i] {
                        b' ' | b'\t' | b'\r' | b'\n' | b';' | b'{' => break,
                        b'\\' => i += 2,
                        b'$' if bytes.get(i + 1) == Some(&b'{') => i += 2,
                        _ => i += 1,
                    }
                }
                i = i.min(len);
                tokens.push((
                    String::from_utf8_lossy(&bytes[start..i]).into_owned(),
                    start,
                ));
            }
        }
    }
    Lexed { tokens, comments }
}

/// Finds every top-level `server { ... }` block in `content`, along with the
/// `server_name` values declared directly inside it.
fn parse_server_blocks(content: &str) -> Vec<ServerBlock> {
    let tokens = lex(content).tokens;

    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut names_stack: Vec<Vec<String>> = Vec::new();
    // Parallel to `names_stack`: whether the block currently being scanned
    // has shown any sign of terminating TLS.
    let mut tls_stack: Vec<bool> = Vec::new();
    let mut blocks = Vec::new();
    let mut prev_word: Option<&str> = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let (tok, off) = &tokens[idx];
        match tok.as_str() {
            "{" => {
                let is_server = prev_word == Some("server");
                stack.push((*off, is_server));
                if is_server {
                    names_stack.push(Vec::new());
                    tls_stack.push(false);
                }
                prev_word = None;
            }
            "}" => {
                if let Some((open, is_server)) = stack.pop() {
                    if is_server {
                        let names = names_stack.pop().unwrap_or_default();
                        let is_tls = tls_stack.pop().unwrap_or(false);
                        blocks.push(ServerBlock {
                            names,
                            is_tls,
                            open,
                            close: *off,
                        });
                    }
                }
                prev_word = None;
            }
            ";" => prev_word = None,
            word => {
                let directly_in_server =
                    stack.last().map(|(_, is_server)| *is_server) == Some(true);
                // Two independent signals, because configs express TLS
                // both ways: `listen 443 ssl;` (the `ssl` parameter, which
                // arrives as its own token) and a bare `ssl_certificate`
                // directive alongside a `listen` that doesn't say `ssl`.
                // Either is enough; neither is required to be first.
                if directly_in_server && (word == "ssl" || word.starts_with("ssl_certificate")) {
                    if let Some(top) = tls_stack.last_mut() {
                        *top = true;
                    }
                }
                if word == "server_name" && directly_in_server {
                    let mut j = idx + 1;
                    let mut names = Vec::new();
                    while j < tokens.len()
                        && tokens[j].0 != ";"
                        && tokens[j].0 != "{"
                        && tokens[j].0 != "}"
                    {
                        let name = tokens[j].0.trim_matches(['"', '\'']).to_string();
                        if !name.is_empty() {
                            names.push(name);
                        }
                        j += 1;
                    }
                    if let Some(top) = names_stack.last_mut() {
                        top.extend(names);
                    }
                    idx = j;
                    prev_word = Some(word);
                    continue;
                }
                prev_word = Some(word);
            }
        }
        idx += 1;
    }

    blocks
}

/// `regex` as an NGINX double-quoted string, quotes included, written so
/// that the text NGINX hands to PCRE is exactly `regex`. **Every regex this
/// module writes into a config goes through here**, and every check on a
/// regex ([`pattern_problem`]) is of the text before it: that is only the
/// regex NGINX compiles because this function is exact.
///
/// It used not to exist. Patterns went between the quotes verbatim, and
/// NGINX's config reader is not verbatim: it undoes escapes inside a quoted
/// word before any directive sees it. A manually blocked user agent ending
/// in `\|` was escaped for PCRE as `\\\|`, written as it was, read back by
/// NGINX as `\\|`, and compiled to "a backslash, or nothing": a rule that
/// turned away every visitor. `Evil\(` became an unclosed group that failed
/// `nginx -t`, and with it every later apply, unattended ones included.
///
/// What NGINX does, from `ngx_conf_read_token` in
/// `src/core/ngx_conf_file.c` (the same from 1.0 to 1.27):
///
/// 1. **Finding the end of the word.** A backslash makes the next byte
///    ordinary, so `\"` does not close the string. Nothing else inside
///    double quotes is special: not `'`, `;`, `{`, `}`, `#`, `$` or
///    whitespace.
/// 2. **Unescaping it**, left to right over the bytes between the quotes:
///    `\"`, `\'` and `\\` become their second byte; `\t`, `\r` and `\n`
///    become a tab, a carriage return and a line feed; a backslash before
///    anything else is kept, and the byte after it is then read as an
///    ordinary byte. [`nginx_unquoted`] is a model of this pass.
///
/// The encoding: a `"` is written `\"`, and a backslash is written twice
/// exactly when the byte written right after it is `\`, `"`, `'`, `t`, `r`
/// or `n`, or the closing quote; everywhere else once. Pass 2 gives back
/// every character, by induction over the written text:
///
/// - A `"` was written `\"`, which is read as `"`, consuming both bytes.
/// - A doubled backslash is `\\`, read as one backslash, consuming both.
/// - A single backslash is followed by a byte outside that set, so it is
///   kept and only it is consumed. That next byte is not a backslash or a
///   quote either: the character it starts would then be written with a
///   leading `\`, and this backslash would have been doubled.
/// - Any other character is written as itself and read as itself.
///
/// Pass 1 agrees on where the string ends: every backslash it pairs with
/// the next byte is one of the pairs above, or a single backslash followed
/// by an ordinary byte, so the only unpaired `"` is the closing one. A
/// regex ending in a backslash has it doubled, so it cannot swallow the
/// closing quote.
///
/// Doubling only where NGINX would otherwise read an escape, rather than
/// every backslash, keeps every block that was already right byte for byte
/// the same: `1h4x\.com` is still written `1h4x\.com`, so no site on an
/// upgraded host reads as stale over a change that did not affect it. The
/// only blocks whose text changes are the ones NGINX was misreading.
pub fn nginx_quoted(regex: &str) -> String {
    let chars: Vec<char> = regex.chars().collect();
    let mut out = String::with_capacity(regex.len() + 2);
    out.push('"');
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => {
                // The byte written right after this backslash: the next
                // character's first, which for a quote or a backslash is a
                // backslash; at the end, the closing quote.
                let next = match chars.get(i + 1) {
                    None | Some('"' | '\\') => '\\',
                    Some(&next) => next,
                };
                if matches!(next, '\\' | '"' | '\'' | 't' | 'r' | 'n') {
                    out.push('\\');
                }
                out.push('\\');
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What NGINX makes of the text between the quotes of a double-quoted
/// word: pass 2 of `ngx_conf_read_token`, as described at
/// [`nginx_quoted`].
///
/// A model, which [`nginx_quoted`] is checked against, and which reads a
/// quoted value this module wrote back out of a config file.
pub fn nginx_unquoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let unescaped = match chars.peek() {
            Some(&next @ ('"' | '\'' | '\\')) => next,
            Some('t') => '\t',
            Some('r') => '\r',
            Some('n') => '\n',
            _ => {
                out.push('\\');
                continue;
            }
        };
        out.push(unescaped);
        chars.next();
    }
    out
}

/// The bytes [`nginx_quoted`] writes for `regex`, quotes left out. An
/// upper bound for the same regex inside a longer `|`-join: the only
/// character whose encoding depends on what follows it is a final
/// backslash, which is doubled here and may not be there.
fn quoted_len(regex: &str) -> usize {
    nginx_quoted(regex).len() - 2
}

/// The fewest literal characters every match of a user-agent pattern must
/// contain for it to be written at all.
///
/// Nothing in NGINX refuses a regex that matches every request, and the
/// block turns away whatever it matches on every site at once. An empty
/// key in ai.robots.txt, `accepted: [""]` in well-known-bots, a lone `|`
/// line in the bad-bot list or `.*` from any of them all pass `nginx -t`
/// and 403 every visitor. Three is below every real upstream entry (the
/// shortest are tokens like `FDM` and `Y!J`) and above anything that
/// cannot help matching half the web.
const MIN_PATTERN_LITERALS: usize = 3;

/// The longest single pattern this tool writes, in bytes. The longest in
/// any upstream list is under 60; this is far above it and still well
/// inside one [`MAX_PATTERN_CHUNK_LEN`] chunk, so no pattern ever needs a
/// chunk of its own that NGINX's parameter limit could refuse.
pub const MAX_PATTERN_LEN: usize = 1024;

/// Why a user-agent pattern will not be written, if it will not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternProblem {
    /// Empty, or only whitespace.
    Empty,
    /// A newline, tab or other control character — no real user agent has
    /// one.
    ControlCharacter,
    /// Longer than [`MAX_PATTERN_LEN`].
    TooLong,
    /// Some way of matching it needs fewer than [`MIN_PATTERN_LITERALS`]
    /// literal characters, or it has an empty alternative — it matches
    /// (nearly) every user agent.
    MatchesTooMuch,
    /// A repeated group that itself repeats something, `(a+)+`: the shape
    /// that makes a backtracking engine take exponential time per request.
    Backtracks,
    /// Something PCRE refuses to compile: a range out of order (`[z-a]`), a
    /// quantifier whose numbers are out of order (`{2,1}`) or too big
    /// (`{99999}`). Written, it would fail `nginx -t` and roll back every
    /// apply after it.
    Invalid,
    /// Unbalanced, or a construct (lookaround, `\x`, `\Q`, inline flags)
    /// this check does not understand. Refused rather than guessed at.
    Unchecked,
}

impl std::fmt::Display for PatternProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PatternProblem::Empty => "it is empty",
            PatternProblem::ControlCharacter => "it contains a control character",
            PatternProblem::TooLong => "it is too long",
            PatternProblem::MatchesTooMuch => "it would match (nearly) every user agent",
            PatternProblem::Backtracks => "it nests one repetition inside another",
            PatternProblem::Invalid => "it is not a regex NGINX would compile",
            PatternProblem::Unchecked => "it is not a regex this tool can check",
        })
    }
}

/// Why `pattern` must not be joined into a sentinel block, or `None` when
/// it is safe to.
///
/// `pattern` is the regex as PCRE is to compile it, which is what NGINX
/// hands PCRE because [`nginx_quoted`] writes it exactly. Checking the text
/// before quoting was only ever right because of that: when patterns were
/// written verbatim, this checked one regex and NGINX compiled another.
///
/// **A structural check, not a regex engine.** Deciding whether a PCRE
/// pattern matches every string needs one, and the `regex` crate would add
/// about 1.5MB to a static musl binary for this one question. What is
/// decidable by walking the pattern is enough: the fewest literal
/// characters any match must contain. A pattern that can match with fewer
/// than [`MIN_PATTERN_LITERALS`] is refused, and that covers the empty
/// pattern, `.*`, `^`, `a?b?c?`, `(abc)?` and `Bot|.*` alike, because each
/// has a way to match nearly nothing. It errs towards refusing: anything it
/// does not understand is [`PatternProblem::Unchecked`].
///
/// **Nor a compiler.** It refuses what it can see PCRE would refuse — an
/// unbalanced group, a range or quantifier out of order, a count past
/// PCRE's limit, a POSIX class it does not know, an escape it does not
/// know — and everything it cannot check. What it cannot promise is that
/// every pattern it passes compiles: that would take PCRE itself. The one
/// real compiler on the host is `nginx -t`, which every apply runs before
/// reloading and which rolls the apply back if it fails; bisecting a
/// failure down to the one bad pattern would mean a `nginx -t` per step
/// against a scratch config tree, which an apply does not have.
pub fn pattern_problem(pattern: &str) -> Option<PatternProblem> {
    if pattern.trim().is_empty() {
        return Some(PatternProblem::Empty);
    }
    if pattern.chars().any(char::is_control) {
        return Some(PatternProblem::ControlCharacter);
    }
    if pattern.len() > MAX_PATTERN_LEN {
        return Some(PatternProblem::TooLong);
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut shape = Shape {
        chars: &chars,
        pos: 0,
    };
    match shape.alternation(0) {
        Err(problem) => Some(problem),
        Ok(_) if shape.pos != chars.len() => Some(PatternProblem::Unchecked),
        Ok(group) if group.min_literals < MIN_PATTERN_LITERALS => {
            Some(PatternProblem::MatchesTooMuch)
        }
        Ok(_) => None,
    }
}

/// Whether `pattern` may be joined into a sentinel block.
pub fn is_usable_pattern(pattern: &str) -> bool {
    pattern_problem(pattern).is_none()
}

/// What [`Shape`] learns about a stretch of pattern.
struct Measured {
    /// The fewest literal characters any match of it contains.
    min_literals: usize,
    /// Whether it contains a quantifier anywhere.
    quantified: bool,
    /// Whether a quantifier may follow it. An anchor (`^`, `\b`) matches a
    /// position rather than a character, and no list repeats one.
    repeatable: bool,
}

/// A quantifier, as [`Shape::quantifier`] reads it.
struct Quantifier {
    /// The fewest repetitions.
    min: usize,
    /// Whether it can match more than once.
    repeats: bool,
    /// For `{n}`, `{n,}` and `{n,m}`: how many copies of the repeated
    /// item PCRE compiles, which it does by writing the item out that
    /// many times.
    copies: Option<usize>,
}

/// What stands at a `{`: a quantifier, or a literal brace.
enum Brace {
    /// Not a quantifier in any PCRE: `{`, `{}`, `{a}`, `x server {`.
    Literal,
    /// `{n}`, `{n,}` or `{n,m}`, `len` characters long.
    Quantifier {
        min: usize,
        max: Option<usize>,
        len: usize,
    },
}

/// One member of a `[...]` class.
enum Member {
    /// A single character, which may start or end a range.
    Char(char),
    /// A set of characters (`\d`, `[:alpha:]`), which may not.
    Set,
}

/// The largest count PCRE accepts in a `{}` quantifier.
const PCRE_MAX_REPEAT: usize = 65_535;

/// How many characters of pattern a braced quantifier may expand into.
/// PCRE compiles `(abc){100}` by writing the group out a hundred times,
/// and a compiled pattern past its size limit fails `nginx -t` with
/// "regular expression is too large". The largest in any real list is
/// `\/{0,1}`.
const MAX_REPEAT_EXPANSION: usize = 4096;

/// The POSIX classes PCRE knows inside `[...]`. Any other name fails to
/// compile.
const POSIX_CLASSES: [&str; 14] = [
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// A recursive walk over a PCRE pattern, just deep enough for
/// [`pattern_problem`].
struct Shape<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Shape<'_> {
    /// Deep enough for any real pattern; a limit so a hostile one cannot
    /// recurse without bound.
    const MAX_DEPTH: usize = 16;

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// `a|b|c`, up to an unopened `)` or the end: the fewest literals of
    /// any alternative, since a match only needs one.
    fn alternation(&mut self, depth: usize) -> Result<Measured, PatternProblem> {
        if depth > Self::MAX_DEPTH {
            return Err(PatternProblem::Unchecked);
        }
        let mut min_literals = usize::MAX;
        let mut quantified = false;
        loop {
            let branch = self.sequence(depth)?;
            min_literals = min_literals.min(branch.min_literals);
            quantified |= branch.quantified;
            if self.peek() != Some('|') {
                return Ok(Measured {
                    min_literals,
                    quantified,
                    repeatable: true,
                });
            }
            self.pos += 1;
        }
    }

    /// Atoms one after another, each with its quantifier. An empty one is
    /// an empty alternative, which matches everything wherever it is.
    fn sequence(&mut self, depth: usize) -> Result<Measured, PatternProblem> {
        let start = self.pos;
        let mut min_literals = 0usize;
        let mut quantified = false;
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom_start = self.pos;
            let atom = self.atom(depth)?;
            let atom_len = self.pos - atom_start;
            match self.quantifier()? {
                Some(quantifier) => {
                    if !atom.repeatable {
                        return Err(PatternProblem::Unchecked);
                    }
                    if quantifier.copies.is_some_and(|copies| {
                        atom_len.saturating_mul(copies) > MAX_REPEAT_EXPANSION
                    }) {
                        return Err(PatternProblem::Unchecked);
                    }
                    if quantifier.repeats && atom.quantified {
                        return Err(PatternProblem::Backtracks);
                    }
                    min_literals = min_literals
                        .saturating_add(atom.min_literals.saturating_mul(quantifier.min));
                    quantified = true;
                }
                None => {
                    min_literals = min_literals.saturating_add(atom.min_literals);
                    quantified |= atom.quantified;
                }
            }
        }
        if self.pos == start {
            return Err(PatternProblem::MatchesTooMuch);
        }
        Ok(Measured {
            min_literals,
            quantified,
            repeatable: true,
        })
    }

    fn atom(&mut self, depth: usize) -> Result<Measured, PatternProblem> {
        let literal = |n| Measured {
            min_literals: n,
            quantified: false,
            repeatable: true,
        };
        let anchor = Measured {
            min_literals: 0,
            quantified: false,
            repeatable: false,
        };
        let c = self.peek().ok_or(PatternProblem::Unchecked)?;
        self.pos += 1;
        match c {
            '(' => {
                // `(?:` is the only `(?` form upstream lists use; the rest
                // (lookaround, flags, named groups, comments) are refused.
                if self.peek() == Some('?') {
                    if self.chars.get(self.pos + 1) != Some(&':') {
                        return Err(PatternProblem::Unchecked);
                    }
                    self.pos += 2;
                }
                let inner = self.alternation(depth + 1)?;
                if self.peek() != Some(')') {
                    return Err(PatternProblem::Unchecked);
                }
                self.pos += 1;
                Ok(inner)
            }
            '[' => {
                // `[:alpha:]` outside a class is an error in PCRE, not a
                // class of `:`, `a`, `l`...
                if self.posix_class_at(self.pos - 1).is_some() {
                    return Err(PatternProblem::Invalid);
                }
                let start = self.pos;
                self.class()?;
                // `[Tt]` is how upstream spells one letter in either case
                // (`i[Tt][Mm][Ss]`), and the match is case-insensitive
                // anyway, so it counts as the literal it is.
                let members = &self.chars[start..self.pos - 1];
                let one_letter = matches!(members, [a, b] if a.is_alphabetic() && a.to_lowercase().eq(b.to_lowercase()));
                Ok(literal(usize::from(one_letter)))
            }
            '\\' => {
                let escaped = self.peek().ok_or(PatternProblem::Unchecked)?;
                self.pos += 1;
                match escaped {
                    // Anchors: they match a position, not a character.
                    'b' | 'B' | 'A' | 'z' | 'Z' | 'G' => Ok(anchor),
                    // Character types: they match a character, but not a
                    // literal one.
                    'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'h' | 'H' | 'v' | 'V' => Ok(literal(0)),
                    // `\x41`, `\p{..}`, `\Q..\E`, back-references: each
                    // would need its own parsing to count honestly.
                    c if c.is_ascii_alphanumeric() => Err(PatternProblem::Unchecked),
                    _ => Ok(literal(1)),
                }
            }
            '^' | '$' => Ok(anchor),
            '.' => Ok(literal(0)),
            // A quantifier with nothing to repeat.
            '*' | '+' | '?' => Err(PatternProblem::Unchecked),
            // A `{` is a literal unless it reads as a quantifier, which
            // here would have nothing to repeat.
            '{' => {
                self.pos -= 1;
                match self.brace()? {
                    Brace::Quantifier { .. } => Err(PatternProblem::Unchecked),
                    Brace::Literal => {
                        self.pos += 1;
                        Ok(literal(1))
                    }
                }
            }
            _ => Ok(literal(1)),
        }
    }

    /// Checks a `[...]` class, already past its `[`: every member PCRE
    /// knows, and every range in order.
    fn class(&mut self) -> Result<(), PatternProblem> {
        if self.peek() == Some('^') {
            self.pos += 1;
        }
        // A `]` straight after the opening is a literal member.
        let mut first = true;
        loop {
            if self.peek().ok_or(PatternProblem::Unchecked)? == ']' && !first {
                self.pos += 1;
                return Ok(());
            }
            first = false;
            let from = self.class_member()?;
            // `a-z`; a `-` last, or first, is a literal member.
            let is_range =
                self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&c| c != ']');
            if !is_range {
                continue;
            }
            self.pos += 1;
            let to = self.class_member()?;
            match (from, to) {
                // Compared as bytes: NGINX does not ask PCRE for UTF-8
                // mode, so a range ending in `é` ends in its first byte.
                (Member::Char(from), Member::Char(to)) if from.is_ascii() && to.is_ascii() => {
                    if from > to {
                        return Err(PatternProblem::Invalid);
                    }
                }
                (Member::Char(_), Member::Char(_)) => return Err(PatternProblem::Unchecked),
                // `[\d-z]`: PCRE refuses a range from or to a set.
                _ => return Err(PatternProblem::Invalid),
            }
        }
    }

    /// One member of a class, consumed.
    fn class_member(&mut self) -> Result<Member, PatternProblem> {
        let c = self.peek().ok_or(PatternProblem::Unchecked)?;
        if c == '[' {
            if let Some((name, len)) = self.posix_class_at(self.pos) {
                if !POSIX_CLASSES.contains(&name.as_str()) {
                    return Err(PatternProblem::Invalid);
                }
                self.pos += len;
                return Ok(Member::Set);
            }
        }
        self.pos += 1;
        if c != '\\' {
            return Ok(Member::Char(c));
        }
        let escaped = self.peek().ok_or(PatternProblem::Unchecked)?;
        self.pos += 1;
        match escaped {
            'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'h' | 'H' | 'v' | 'V' => Ok(Member::Set),
            // `\x`, `\p`, `\N`, `\Q`...: each its own syntax, inside a
            // class as well as out of it.
            c if c.is_ascii_alphanumeric() => Err(PatternProblem::Unchecked),
            c => Ok(Member::Char(c)),
        }
    }

    /// The name and length of a POSIX class at `at`, if PCRE would read
    /// one there: `[:alpha:]`, or the unsupported `[.x.]` and `[=x=]`,
    /// whose name is given as empty so that it is never a known one.
    ///
    /// PCRE's own test (`check_posix_syntax`): a `[` and a terminator, then
    /// the same terminator right before a `]`, with no `]` or `[` plus
    /// terminator in between; `\]` and `\\` are skipped over.
    fn posix_class_at(&self, at: usize) -> Option<(String, usize)> {
        let rest = self.chars.get(at..)?;
        if rest.first() != Some(&'[') {
            return None;
        }
        let terminator = *rest.get(1).filter(|c| matches!(c, ':' | '.' | '='))?;
        let mut i = 2;
        while i < rest.len() {
            match rest[i] {
                '\\' if matches!(rest.get(i + 1), Some(']' | '\\')) => i += 1,
                '[' if rest.get(i + 1) == Some(&terminator) => return None,
                ']' => return None,
                c if c == terminator && rest.get(i + 1) == Some(&']') => {
                    let name = if terminator == ':' {
                        rest[2..i].iter().collect()
                    } else {
                        String::new()
                    };
                    return Some((name, i + 2));
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// A quantifier at the current position, consumed along with any lazy
    /// or possessive suffix. `None` if there is none, which includes a `{`
    /// that is a literal.
    fn quantifier(&mut self) -> Result<Option<Quantifier>, PatternProblem> {
        let found = match self.peek() {
            Some('{') => match self.brace()? {
                Brace::Literal => return Ok(None),
                Brace::Quantifier { min, max, len } => {
                    self.pos += len;
                    Quantifier {
                        min,
                        repeats: max.is_none_or(|max| max > 1),
                        // `{n,}` is n copies and a starred one.
                        copies: Some(max.map_or(min + 1, |max| max.max(min))),
                    }
                }
            },
            Some(symbol @ ('?' | '*' | '+')) => {
                self.pos += 1;
                Quantifier {
                    min: usize::from(symbol == '+'),
                    repeats: symbol != '?',
                    copies: None,
                }
            }
            _ => return Ok(None),
        };
        if matches!(self.peek(), Some('?' | '+')) {
            self.pos += 1;
        }
        Ok(Some(found))
    }

    /// What the `{` at the current position is. Leaves the position alone.
    ///
    /// Only the strict forms, `{n}`, `{n,}` and `{n,m}`, count as a
    /// quantifier. PCRE2 10.43 also reads `{,n}` and `{ n }` as one, where
    /// earlier versions read them as literal text: which one a host's NGINX
    /// has is not something to guess, so those are refused.
    fn brace(&self) -> Result<Brace, PatternProblem> {
        let rest = &self.chars[self.pos..];
        let body_len = rest
            .iter()
            .skip(1)
            .take_while(|&&c| c.is_ascii_digit() || c == ',' || c == ' ')
            .count();
        if body_len == 0 || rest.get(body_len + 1) != Some(&'}') {
            return Ok(Brace::Literal);
        }
        let body: String = rest[1..=body_len].iter().collect();
        let number = |text: &str| -> Result<usize, PatternProblem> {
            if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
                return Err(PatternProblem::Unchecked);
            }
            match text.parse::<usize>() {
                Ok(n) if n <= PCRE_MAX_REPEAT => Ok(n),
                _ => Err(PatternProblem::Invalid),
            }
        };
        let (min, max) = match body.split_once(',') {
            None => {
                let n = number(&body)?;
                (n, Some(n))
            }
            Some((min, "")) => (number(min)?, None),
            Some((min, max)) => (number(min)?, Some(number(max)?)),
        };
        if max.is_some_and(|max| max < min) {
            return Err(PatternProblem::Invalid);
        }
        Ok(Brace::Quantifier {
            min,
            max,
            len: body_len + 2,
        })
    }
}

/// The patterns in `patterns` that are safe to write (see
/// [`pattern_problem`]), in order. Empty is the same "nothing to block" case
/// as an empty list, which removes any existing sentinel block instead of
/// writing an empty one.
///
/// Checked again here, at render time, rather than trusted from wherever
/// they came from: a row stored by an older release, a list fetched before
/// a check existed. [`skipped_entries`] names what is left out, for the
/// status report.
fn usable_patterns(patterns: &[String]) -> Vec<&str> {
    patterns
        .iter()
        .map(String::as_str)
        .filter(|p| reaches_pcre_intact(p) && is_usable_pattern(p))
        .collect()
}

/// Every stored entry the render leaves out, one line each, for the
/// status report and `apply-blocks` to name: rows an older release stored
/// without today's checks, and bot-list patterns fetched before them.
/// Checked the way the render checks them, so the two cannot disagree.
///
/// A list, not an error: leaving an entry out is the safe failure, and an
/// apply must still write everything else. But an entry left out without a
/// word is a block or an exemption the operator believes is there.
pub fn skipped_entries(db: &crate::db::Db) -> Result<Vec<String>> {
    use crate::db;
    let mut skipped = Vec::new();
    for ua in db.list_blocked_user_agents()? {
        let regex = db::escape_regex_literal(&ua);
        if let Some(problem) = pattern_problem(&regex) {
            skipped.push(format!("blocked user agent {ua:?}: {problem}"));
        }
    }
    let unusable_bots = db
        .list_bots()?
        .iter()
        .filter(|bot| !is_usable_pattern(&bot.user_agent_pattern))
        .count();
    if unusable_bots > 0 {
        skipped.push(format!(
            "{unusable_bots} bot-list pattern(s) that are not a regex this tool can check"
        ));
    }
    for ua in db.list_trusted_user_agents()? {
        if let Err(err) = db::validate_trusted_user_agent(&ua) {
            skipped.push(format!("trusted user agent {ua:?}: {err}"));
        }
    }
    for site in db.list_sites()? {
        let name = &site.server_name;
        for path in db.site_path_exemptions(site.id)? {
            if let Err(err) = db::validate_exempt_path(&path) {
                skipped.push(format!("{name}: exempt path {path:?}: {err}"));
            }
        }
        for exemption in db.site_agent_exemptions(site.id)? {
            let (path, ua) = (&exemption.path, &exemption.user_agent);
            if let Err(err) =
                db::validate_exempt_path(path).and_then(|_| db::validate_exemption_user_agent(ua))
            {
                skipped.push(format!("{name}: exempt path {path:?} for {ua:?}: {err}"));
            }
        }
    }
    Ok(skipped)
}

/// Whether NGINX hands PCRE exactly `regex` when this module writes it:
/// [`nginx_quoted`] read back through [`nginx_unquoted`]. True of every
/// string by construction; asked anyway, because a regex that reached PCRE
/// as something else is how a blocked user agent once became a rule
/// against every visitor.
fn reaches_pcre_intact(regex: &str) -> bool {
    let quoted = nginx_quoted(regex);
    nginx_unquoted(&quoted[1..quoted.len() - 1]) == regex
}

/// Maximum byte length of a single chunk [`chunk_patterns`] produces, as
/// written: after [`nginx_quoted`] has escaped it.
///
/// NGINX's config-file parser has a hard ceiling on the length of a single
/// quoted parameter — confirmed empirically against a real `nginx -t`:
/// even a *properly terminated* quoted string fails with `too long
/// parameter, probably missing terminating """ character` once it crosses
/// roughly 4100 bytes (matching `NGX_CONF_BUFFER`), regardless of what
/// precedes it in the file. Having enough blocked bots is enough to reach
/// it: the `nginx-bad-bots` source alone is ~700 entries, easily exceeding
/// 4096 bytes once joined with `|`. Set well below the observed failure point to
/// stay safe regardless of how much unrelated content precedes the
/// sentinel block in a real config file (confirmed empirically too: a
/// 2000-byte quoted parameter still parses fine even after ~10KB of
/// preceding file content).
const MAX_PATTERN_CHUNK_LEN: usize = 2000;

/// Joins `patterns` with `|` into regexes of at most `max_len` bytes each
/// once written by [`nginx_quoted`], **never splitting a pattern**. A
/// single pattern longer than `max_len` becomes its own oversized chunk
/// rather than being dropped or cut.
///
/// This used to join everything first and then split the result on every
/// `|` — including the `|` of an escaped `\|` and the ones inside a
/// `(a|b)` group. A chunk could then end in the backslash of `\|`, which
/// escaped the closing quote of `if ($http_user_agent ~* "...")`, and NGINX
/// read the start of the next chunk as directives. Verified with a real
/// `nginx -t`: a line in a downloaded bot list could put an `include` into
/// a config that root loads, unattended, from cron. Whole patterns are
/// what keep each chunk a regex; [`nginx_quoted`] is what keeps it inside
/// its quotes whatever it ends in.
fn chunk_patterns(patterns: &[&str], max_len: usize) -> Vec<String> {
    let mut chunks: Vec<(String, usize)> = Vec::new();
    for pattern in patterns {
        let len = quoted_len(pattern);
        match chunks.last_mut() {
            Some((last, last_len)) if *last_len + 1 + len <= max_len => {
                last.push('|');
                last.push_str(pattern);
                *last_len += 1 + len;
            }
            _ => chunks.push((pattern.to_string(), len)),
        }
    }
    chunks.into_iter().map(|(chunk, _)| chunk).collect()
}

/// Where this project writes NGINX files it fully owns, as opposed to the
/// sentinel blocks it edits *into* files an admin owns. Currently just the
/// generated `robots.txt`.
///
/// A separate directory, not `/etc/nginx/`, for the same reason the
/// sentinel markers exist: everything in here can be deleted wholesale
/// without touching an admin's own config. Nothing NGINX globs lives here
/// either — these files are only ever reached through an explicit
/// directive inside a sentinel block, so a leftover file can't take effect
/// on its own.
pub const MANAGED_DIR: &str = "/etc/stop-bots/nginx";

/// Environment variable overriding [`MANAGED_DIR`].
///
/// This exists for the end-to-end tests, which drive the real binary and
/// would otherwise have to write to `/etc` (i.e. only pass as root). It is
/// read in exactly one place, so the path written and the path referenced
/// by the generated `alias` can never disagree. Not documented as a user
/// feature: an admin who wants these files elsewhere is better served by a
/// real setting, and this one is deliberately not persisted anywhere.
pub const MANAGED_DIR_ENV: &str = "STOP_BOTS_NGINX_DIR";

/// The directory this project writes its own NGINX files into.
pub fn managed_dir() -> PathBuf {
    std::env::var_os(MANAGED_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(MANAGED_DIR))
}

/// The generated `robots.txt`, served by the `location = /robots.txt`
/// block when robots.txt generation is on.
pub fn robots_txt_path() -> PathBuf {
    managed_dir().join("robots.txt")
}

/// Where the rate-limit zone definition goes.
///
/// Unlike [`MANAGED_DIR`] this **is** a directory NGINX globs — the stock
/// `nginx.conf` on every mainstream distro has `include
/// /etc/nginx/conf.d/*.conf;` inside its `http` block, and `limit_req_zone`
/// is an `http`-context directive that therefore cannot live in the
/// sentinel block with everything else. That makes a leftover file here
/// genuinely live rather than inert, which is why removal is handled
/// explicitly (see [`remove_unused_managed_files`]) instead of being left
/// to tidy up later.
pub const CONF_D_DIR: &str = "/etc/nginx/conf.d";

/// Environment override for [`CONF_D_DIR`]; same test-only rationale as
/// [`MANAGED_DIR_ENV`].
pub const CONF_D_DIR_ENV: &str = "STOP_BOTS_NGINX_CONF_D";

/// Where this host's `conf.d` actually is: the environment override if
/// one is set, else `conf.d` under the stored NGINX root.
///
/// Derived from [`root`] rather than fixed, and that is the whole point.
/// An NGINX in a container keeps its config tree somewhere like
/// `/srv/app/nginx`; `scan-sites` stores that as `nginx:root` and every
/// site file is found and rewritten there. The `http`-context files this
/// module also generates went to [`CONF_D_DIR`] regardless — a directory
/// that on such a host does not exist until this code creates it, and
/// that NGINX never reads.
///
/// The failure that produced was not a missing feature but a broken
/// server: `write_managed_files` runs first and reported success, the
/// site blocks were then rewritten to read `$stop_bots_trusted`, and only
/// the reload found out that nothing defined it. NGINX refuses to load
/// such a config at all, so the *whole* server — every unrelated site
/// included — was one restart away from not coming back.
///
/// **Only a `conf.d` that already exists**, and never one this code
/// creates. The root is wherever site configs were scanned from, which is
/// not always the NGINX prefix: the container suite passes
/// `--root /etc/nginx/sites-enabled`, and creating `conf.d` inside *that*
/// puts a directory where `include sites-enabled/*` globs, so NGINX tries
/// to `pread()` a directory and refuses to start. A `conf.d` that is
/// already there is one NGINX was built around; one this code invents is a
/// guess about a glob it cannot see.
///
/// Takes the root already resolved rather than reading it back out of the
/// database, because [`root`] lets a `--root` flag win over the stored
/// setting and the two must not disagree.
///
/// Unchanged for a normal host install: [`root`] falls back to
/// [`DEFAULT_ROOT`], whose `conf.d` exists, so this still resolves to
/// [`CONF_D_DIR`].
///
/// **The stock `conf.d` only for a root inside the stock tree.** A root
/// such as `/etc/nginx/sites-enabled` is a part of `/etc/nginx`, whose
/// `conf.d` is the one its NGINX reads. Any other root is a tree of its
/// own, and never reaches out of itself: `batch --root <scratch>` used to
/// read, and would have rewritten or removed, the host's own
/// `/etc/nginx/conf.d/stop-bots-trusted.conf`. Such a root without a
/// `conf.d` gets `<root>/conf.d`, which a write creates.
pub fn conf_d_dir(root: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os(CONF_D_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let beside_the_sites = root.join("conf.d");
    if beside_the_sites.is_dir() || !is_in_stock_tree(root) {
        return beside_the_sites;
    }
    PathBuf::from(CONF_D_DIR)
}

/// Whether `root` is [`DEFAULT_ROOT`] or a directory in it, as written: a
/// `..` anywhere in it means it may lead anywhere, so it does not count.
fn is_in_stock_tree(root: &Path) -> bool {
    root.is_absolute()
        && root.starts_with(DEFAULT_ROOT)
        && !root
            .components()
            .any(|part| part == std::path::Component::ParentDir)
}

/// The generated `limit_req_zone` file.
pub fn rate_limit_conf_path(conf_d: &Path) -> PathBuf {
    conf_d.join("stop-bots-limits.conf")
}

/// The generated trust file: which clients no block applies to.
///
/// In `conf.d` for the reason [`rate_limit_conf_path`] is. `geo` and `map`
/// are `http`-context directives, and `geo` is the only way NGINX has to
/// ask "is this address inside that CIDR" — a server-level `if` can only
/// compare `$remote_addr` as text, which cannot express a /28, let alone
/// the many spellings of an IPv6 prefix.
pub fn trusted_conf_path(conf_d: &Path) -> PathBuf {
    conf_d.join("stop-bots-trusted.conf")
}

/// The variable the trust file defines: `1` for a trusted client, `0`
/// otherwise. Read by every sentinel block that blocks anything.
const TRUSTED_VAR: &str = "$stop_bots_trusted";

/// The rate-limit key the trust file defines — empty for a trusted client,
/// which `limit_req_zone` does not account at all, and the client's
/// address for everyone else.
const LIMIT_KEY_VAR: &str = "$stop_bots_limit_key";

/// Holds the request path while an agent exemption is being checked, and
/// `""` otherwise — see [`agent_exemption_clears`]. Server-level, set
/// before every read, so no `map` or `http`-level declaration is needed.
const AGENT_EXEMPT_VAR: &str = "$stop_bots_exempt";

/// The zone a site's `limit_req` uses while anything is trusted: keyed on
/// [`LIMIT_KEY_VAR`], in a file of its own
/// ([`untrusted_rate_limit_conf_path`]).
///
/// **A second zone, not the first one re-keyed.** NGINX keeps a
/// `limit_req_zone`'s shared memory across a reload and refuses a reload
/// that gives the same zone a different key — `limit_req "stop_bots" uses
/// the "$binary_remote_addr" key while previously it used ...`. `nginx -t`
/// cannot see that, because it never compares against the running
/// config, so the reload is rejected after the test passed and NGINX
/// keeps serving the old config: every later apply silently does nothing.
/// The container suite caught exactly that. A zone that is only ever
/// created with one key, and a site that switches which zone it names,
/// never asks NGINX to re-key anything.
const UNTRUSTED_RATE_LIMIT_ZONE: &str = "stop_bots_untrusted";

/// The file declaring [`UNTRUSTED_RATE_LIMIT_ZONE`]. Its own file, rather
/// than a line in the trust file or the limits file, because it has to
/// exist exactly when both rate limiting is on and something is trusted,
/// and a file of its own gets the write-before, remove-after lifecycle
/// every other generated file already has.
pub fn untrusted_rate_limit_conf_path(conf_d: &Path) -> PathBuf {
    conf_d.join("stop-bots-limits-untrusted.conf")
}

/// The shared memory zone name used by both the generated
/// `limit_req_zone` and every `limit_req` that references it. One
/// constant, because a mismatch between the two is not a subtle bug: NGINX
/// refuses to start with "unknown limit_req_zone".
const RATE_LIMIT_ZONE: &str = "stop_bots";

/// An optional per-site rule that decides a request is unwanted from its
/// *shape* rather than from a user-agent pattern.
///
/// Each is a separate toggle rather than one "strict requests" switch, and
/// deliberately so: if four rules hid behind one setting and an admin's
/// monitoring stopped working, they'd have no way to tell which one did
/// it. Every variant is off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestRule {
    /// HTTP/1.0 and HTTP/1.1. Only emitted in TLS blocks — see
    /// [`for_block`].
    Http1x,
    /// No `Accept` header. Every browser sends one; a fair amount of
    /// tooling sends nothing.
    NoAccept,
    /// No `Accept-Language`. Browsers send it; most scripts don't. Weaker
    /// than `NoAccept` — some privacy configurations strip it.
    NoAcceptLanguage,
    /// Empty or absent `User-Agent`. Distinct from bot-pattern matching,
    /// which can only catch agents that identify themselves.
    NoUserAgent,
    /// A `Host` that is a bare IP address rather than a name. Scanners
    /// sweep address ranges; real visitors arrive by hostname.
    IpLiteralHost,
    /// TLS 1.0 and 1.1. The same "only modern clients" argument as
    /// [`Self::Http1x`] but on firmer ground — both are formally
    /// deprecated and no current browser offers them. Inert outside a TLS
    /// block, where `$ssl_protocol` is empty.
    OldTls,
}

impl RequestRule {
    pub const ALL: [RequestRule; 6] = [
        RequestRule::Http1x,
        RequestRule::NoAccept,
        RequestRule::NoAcceptLanguage,
        RequestRule::NoUserAgent,
        RequestRule::IpLiteralHost,
        RequestRule::OldTls,
    ];

    /// Stable id — a `site_request_rules.rule` value in every installed
    /// database, so never rename one without a migration.
    pub fn id(self) -> &'static str {
        match self {
            RequestRule::Http1x => "http_1x",
            RequestRule::NoAccept => "no_accept",
            RequestRule::NoAcceptLanguage => "no_accept_language",
            RequestRule::NoUserAgent => "no_user_agent",
            RequestRule::IpLiteralHost => "ip_literal_host",
            RequestRule::OldTls => "old_tls",
        }
    }

    pub fn from_id(id: &str) -> Option<RequestRule> {
        RequestRule::ALL.into_iter().find(|r| r.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            RequestRule::Http1x => "HTTP/1.x requests",
            RequestRule::NoAccept => "No Accept header",
            RequestRule::NoAcceptLanguage => "No Accept-Language",
            RequestRule::NoUserAgent => "No User-Agent",
            RequestRule::IpLiteralHost => "Host is an IP",
            RequestRule::OldTls => "TLS 1.0 / 1.1",
        }
    }

    /// A one-line note on what this turns away besides bots, shown next to
    /// the toggle. Every one of these has collateral; saying so at the
    /// point of the decision is the whole point.
    pub fn caveat(self) -> &'static str {
        match self {
            RequestRule::Http1x => "also turns away crawlers and API clients",
            RequestRule::NoAccept => "some API clients send none",
            RequestRule::NoAcceptLanguage => "privacy tooling strips it",
            RequestRule::NoUserAgent => "scripts and health checks often omit it",
            RequestRule::IpLiteralHost => "breaks reaching the site by IP",
            RequestRule::OldTls => "very old clients only",
        }
    }

    /// Whether the rule can only work in a TLS-terminating block.
    fn needs_tls(self) -> bool {
        matches!(self, RequestRule::Http1x | RequestRule::OldTls)
    }

    /// The NGINX condition, without the surrounding `if (...)`. A regex in
    /// one is written by [`nginx_quoted`], like every other.
    fn condition(self) -> String {
        let matches = |variable: &str, regex: &str| format!("{variable} ~ {}", nginx_quoted(regex));
        match self {
            RequestRule::Http1x => matches("$server_protocol", r"^HTTP/1\."),
            RequestRule::NoAccept => r#"$http_accept = """#.to_string(),
            RequestRule::NoAcceptLanguage => r#"$http_accept_language = """#.to_string(),
            RequestRule::NoUserAgent => r#"$http_user_agent = """#.to_string(),
            // Anchored, and matching both an IPv4 literal and a bracketed
            // IPv6 one. `$host` is already lowercased and port-stripped by
            // NGINX, which `$http_host` is not.
            RequestRule::IpLiteralHost => matches("$host", r"^(\d+\.\d+\.\d+\.\d+|\[)"),
            RequestRule::OldTls => matches("$ssl_protocol", r"^TLSv1(\.[01])?$"),
        }
    }
}

/// Everything that shapes one site's generated sentinel block. Grouped into
/// a struct rather than passed as loose arguments because the block's
/// *content* is what [`site_apply_status`] compares against disk: every
/// field here is something that, when changed, must make an
/// already-applied site read as `Stale`. Adding a knob that affects the
/// generated text without adding it here would silently break that check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockConfig {
    /// The per-site blocked user-agent patterns, as computed by
    /// `Db::blocked_user_agent_patterns_for_site`. Empty means "block
    /// nothing", which removes any existing sentinel block.
    pub patterns: Vec<String>,
    /// How a matched request is turned away — host-wide, but carried here
    /// per site since it's part of the rendered text.
    pub response: BlockResponse,
    /// Whether to serve the generated `robots.txt` (see
    /// [`robots_txt_path`]) from this site. Host-wide, like `response`.
    ///
    /// Only a flag, not the body: the body can run to several kilobytes
    /// with every AI bot listed, and NGINX's config parser rejects a
    /// single quoted parameter past roughly 4KB (the same ceiling
    /// [`MAX_PATTERN_CHUNK_LEN`] exists for), so `return 200 "<body>"` is
    /// not an option. The block `alias`es a file instead, so the body
    /// never appears in the config text at all — which also keeps the
    /// staleness comparison stable when only the body changes.
    pub serve_robots_txt: bool,
    /// Per-server rate limiting: `Some(burst)` emits a `limit_req`
    /// referencing the shared zone, `None` emits nothing.
    ///
    /// Only the burst, not the rate: the rate belongs to `limit_req_zone`,
    /// which is an `http`-context directive living in a separate file (see
    /// [`rate_limit_conf_path`]). Carrying the rate here too would invite
    /// writing it into a `server` block where NGINX rejects it.
    pub rate_limit_burst: Option<u32>,
    /// Request-path prefixes this site's bot block does not apply to.
    ///
    /// Non-empty switches the block to its second shape (see
    /// [`block_text`]): a `set $stop_bots_block` flag rather than a direct
    /// `return`, because NGINX has no way to say "match this user agent
    /// unless the path is one of these" in a single condition.
    pub exempt_paths: Vec<String>,
    /// Request-path prefixes exempt only for clients whose user agent
    /// contains the given string, sorted by user agent (see
    /// [`crate::db::AgentExemption`]). Rendered by
    /// [`agent_exemption_clears`], after every set like the other clears.
    pub agent_exemptions: Vec<crate::db::AgentExemption>,
    /// Per-site [`RequestRule`]s switched on for this site.
    ///
    /// These decide a request is unwanted from its shape rather than from
    /// a user-agent pattern, and they are the bluntest instruments in the
    /// project. Two things make them survivable, and both are enforced
    /// rather than documented:
    ///
    /// 1. **TLS-only rules are dropped in non-TLS blocks** (see
    ///    [`for_block`]). Browsers don't negotiate HTTP/2 without TLS, so
    ///    on a `listen 80` block every request is HTTP/1.1 — and a site's
    ///    port-80 and port-443 blocks routinely share one `server_name`,
    ///    which is what a per-site setting keys on.
    /// 2. **`/.well-known/` is always exempt** whenever any of these is on
    ///    (see `effective_exempt_paths`). ACME HTTP-01 validation is
    ///    fetched over HTTP/1.1 by a non-browser client with no `Accept`
    ///    and often no `User-Agent`; blocking it doesn't fail now, it
    ///    fails at certificate renewal weeks later.
    ///
    /// What no guard fixes: each rule turns away some legitimate
    /// non-browser client. See [`RequestRule::caveat`].
    pub request_rules: Vec<RequestRule>,
    /// The body of the trust file (see [`trusted_conf_body`]), or `None`
    /// when nothing is trusted.
    ///
    /// Only whether it is `Some` reaches the block text — one `if` that
    /// clears the flag. The body is carried anyway because it is part of
    /// what this site's protection *is*: trusting a second user agent
    /// changes no site's block, only the file, and a site whose file is
    /// out of date must still read as `Stale` or nobody will apply it. See
    /// [`trust_file_is_current`].
    pub trust: Option<String>,
}

impl BlockConfig {
    /// The common case: patterns plus the host-wide response setting.
    pub fn new(patterns: Vec<String>, response: BlockResponse) -> Self {
        BlockConfig {
            patterns,
            response,
            ..BlockConfig::default()
        }
    }
}

/// Builds the `robots.txt` body from the current blocking policy: one
/// `User-agent:` line per blocked bot, a single shared `Disallow: /`, and
/// a `Disallow:` for the honeypot trap path.
///
/// **Grouped under one `Disallow`, not one stanza per bot.** Both are
/// valid, and grouping roughly halves a file that can otherwise list well
/// over a thousand agents. It also makes the intent obvious at a glance
/// rather than burying it in repetition.
///
/// **A robots token, not a regex.** `bot.user_agent_pattern` is an NGINX
/// regex fragment (and often an alternation); robots.txt has no regex, so
/// the *name* is used, and only when it's a single word that could
/// plausibly be a real token. Anything with whitespace or regex
/// metacharacters is skipped rather than emitted as a broken rule — a
/// `User-agent:` line no crawler matches is worse than no line, because it
/// looks like coverage that isn't there.
///
/// **The honeypot line is emitted whether or not the honeypot detector is
/// on.** Publishing the trap path is what *creates* the trap; a detector
/// that's off simply doesn't act on hits yet. Publishing it only when the
/// detector is enabled would mean turning the detector on and then waiting
/// for crawlers to re-read robots.txt before it could ever fire.
pub fn robots_txt_body(db: &crate::db::Db) -> Result<String> {
    let blocked: Vec<String> = db
        .list_bots()?
        .into_iter()
        .filter(|bot| db.bot_is_blocked(bot).unwrap_or(false))
        .filter_map(|bot| robots_token(&bot))
        .collect();

    let mut out = managed_header();
    out.push('\n');
    if blocked.is_empty() {
        // Still a valid, meaningful robots.txt: "everyone may crawl
        // everything except the trap". Emitting nothing here would make an
        // enabled feature serve an empty file, which reads as broken.
        out.push_str("User-agent: *\nDisallow:\n\n");
    } else {
        for name in &blocked {
            out.push_str(&format!("User-agent: {name}\n"));
        }
        out.push_str("Disallow: /\n\n");
    }

    let trap = crate::protection::honeypot_path(db)?;
    out.push_str("User-agent: *\n");
    out.push_str(&format!("Disallow: {trap}\n"));
    Ok(out)
}

/// The `User-agent:` token to publish for `bot`, or `None` if nothing
/// usable can be derived.
///
/// **Prefers the user-agent pattern over the display name**, which is not
/// a cosmetic choice. A bot's `name` is humanised from its slug by the
/// well-known-bots parser — `ai-search-bot` becomes `Ai Search Bot` — so
/// it is both mis-cased *and* contains spaces, and a name with spaces is
/// no use as a robots token. Filtering on the name alone meant the
/// generated robots.txt listed **none** of that source's bots while still
/// being served: a file that forbade nobody, which is worse than no file,
/// because it looks like the feature is working.
///
/// The pattern is the substring the bot actually sends, which is exactly
/// what a robots token is. Backslash escapes are undone (patterns are
/// NGINX regex fragments: `Googlebot\/`), a trailing `/` is trimmed
/// (`Googlebot/` is a UA prefix, not a token), and only the first
/// alternative of a merged pattern is used — one `User-agent:` line has
/// room for one token.
fn robots_token(bot: &crate::db::Bot) -> Option<String> {
    let first = bot.user_agent_pattern.split('|').next().unwrap_or_default();
    let unescaped = first.replace('\\', "");
    let candidate = unescaped.trim_end_matches('/');
    if is_robots_token(candidate) {
        return Some(candidate.to_string());
    }
    // A name that happens to be a clean token (ai.robots.txt keys entries
    // by the real token, so this is the common path for that source).
    is_robots_token(&bot.name).then(|| bot.name.clone())
}

/// Whether `name` can be used verbatim as a robots.txt `User-agent:`
/// value. Deliberately strict: robots.txt matching is a simple
/// case-insensitive substring test with no escaping, so anything with
/// whitespace or punctuation beyond the few characters real tokens use
/// is rejected rather than guessed at.
fn is_robots_token(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

/// The burst to emit for this host, or `None` when rate limiting is off.
fn rate_limit_burst(db: &crate::db::Db) -> Result<Option<u32>> {
    if !db.get_rate_limit_enabled()? {
        return Ok(None);
    }
    Ok(Some(db.get_rate_limit_burst()? as u32))
}

/// The contents of the generated `limit_req_zone` file.
///
/// Keyed on `$binary_remote_addr` (4 bytes for IPv4, 16 for IPv6) rather
/// than `$remote_addr`'s string form, the conventional choice: the zone is
/// a fixed-size shared memory block and the binary key fits roughly four
/// times as many clients into it.
pub fn rate_limit_conf_body(rate_per_second: i64, zone_megabytes: i64) -> String {
    format!(
        "{}limit_req_zone $binary_remote_addr zone={RATE_LIMIT_ZONE}:{zone_megabytes}m \
         rate={rate_per_second}r/s;\n",
        managed_header()
    )
}

/// The first line of every file this module writes whole: which version
/// generated it, and that it is not for editing.
///
/// Only in whole files. The sentinel block inside an operator's site file
/// carries no version, for the reasons `crate::generated` gives.
fn managed_header() -> String {
    format!(
        "# {}. Edits will be overwritten.\n",
        crate::generated::generated_by()
    )
}

/// The same limit, in [`UNTRUSTED_RATE_LIMIT_ZONE`]: keyed on the trust
/// file's [`LIMIT_KEY_VAR`], which is that same binary address for
/// everyone but a trusted client — for whom it is empty, and NGINX does
/// not account a request with an empty key. `limit_req` is not allowed
/// inside `if`, so the key is the only place a rate limit can be lifted
/// for one client.
pub fn untrusted_rate_limit_conf_body(rate_per_second: i64, zone_megabytes: i64) -> String {
    format!(
        "{}limit_req_zone {LIMIT_KEY_VAR} zone={UNTRUSTED_RATE_LIMIT_ZONE}:{zone_megabytes}m \
         rate={rate_per_second}r/s;\n",
        managed_header()
    )
}

/// The trust file's body for what `db` trusts, or `None` when it trusts
/// nothing — in which case there is no file, and no block mentions it.
pub fn trusted_conf(db: &crate::db::Db) -> Result<Option<String>> {
    Ok(trusted_conf_body(
        &db.list_trusted_addresses()?,
        &db.list_trusted_user_agents()?,
    ))
}

/// The trust file: three variables, each defined in terms of the last.
///
/// - A `geo` over the client address, because it is the only CIDR-aware
///   lookup NGINX has.
/// - A `map` over the user agent whose *default* is that `geo` result, so
///   the one variable a sentinel block reads, [`TRUSTED_VAR`], is "the
///   address or the user agent is trusted".
/// - The rate-limit key (see [`untrusted_rate_limit_conf_body`]), defined
///   here so that it exists exactly when the variable it reads does. It
///   is harmless unreferenced.
///
/// User agents match the way a manual user-agent block does: a
/// case-insensitive, regex-escaped substring, written by [`nginx_quoted`]
/// with the `~*` that makes a `map` key a regex inside the quotes. Each
/// one has been through `db::validate_trusted_user_agent`, and is checked
/// again here rather than trusting the caller — a bad `map` key would not
/// break one site, it would stop NGINX loading at all.
///
/// All three are evaluated lazily, per request, and only when read.
pub fn trusted_conf_body(addresses: &[String], user_agents: &[String]) -> Option<String> {
    let user_agents: Vec<String> = user_agents
        .iter()
        .filter(|ua| is_stored_as_valid(ua, crate::db::validate_trusted_user_agent))
        .map(|ua| nginx_quoted(&format!("~*{}", crate::db::escape_regex_literal(ua))))
        .collect();
    // Trimmed before it is checked and written alike: the check trims, so
    // what it passed is the trimmed text, and that is all that is written.
    let addresses: Vec<&str> = addresses
        .iter()
        .map(|a| a.trim())
        .filter(|a| crate::db::is_valid_address(a))
        .collect();
    if addresses.is_empty() && user_agents.is_empty() {
        return None;
    }
    let mut out = managed_header();
    out.push_str("geo $stop_bots_trusted_address {\n    default 0;\n");
    for address in addresses {
        out.push_str(&format!("    {address} 1;\n"));
    }
    out.push_str("}\n");
    out.push_str(&format!(
        "map $http_user_agent {TRUSTED_VAR} {{\n    default $stop_bots_trusted_address;\n"
    ));
    for ua in user_agents {
        out.push_str(&format!("    {ua} 1;\n"));
    }
    out.push_str("}\n");
    out.push_str(&format!(
        "map {TRUSTED_VAR} {LIMIT_KEY_VAR} {{\n    1 \"\";\n    default $binary_remote_addr;\n}}\n"
    ));
    Some(out)
}

/// Whether the trust file on disk is the one `config` expects, given what
/// was read from it (`None`: missing or unreadable).
///
/// Only asked of a site that writes a block at all: with no block, nothing
/// on that site reads the file. And a site that expects *no* trust file is
/// current whatever is on disk — its block text is what says whether it
/// still references one, and that is compared separately.
///
/// Pure, taking the file's contents rather than reading them, so that it
/// can be tested without the environment-resolved path; see
/// [`MANAGED_DIR_ENV`].
///
/// The `Generated by` line is left out of the comparison: it names the
/// version that wrote the file, so comparing it would make every site on
/// a host that trusts anything `STALE` after every upgrade, with nothing
/// about what the file says having changed.
fn trust_file_is_current(config: &BlockConfig, on_disk: Option<&str>) -> bool {
    match (&config.trust, block_text(config)) {
        (Some(expected), Some(_)) => {
            on_disk.is_some_and(|text| crate::generated::same_but_for_the_version(text, expected))
        }
        _ => true,
    }
}

/// Creates or updates every file the config about to be written will
/// reference. **Runs before any site config is touched**, so an `alias` or
/// a `limit_req` never points at something that isn't there yet.
///
/// Deliberately does not delete anything — see
/// [`remove_unused_managed_files`] for why the two halves are separate.
pub fn write_managed_files(db: &crate::db::Db, root: &Path) -> Result<usize> {
    let files = planned_managed_files(db, root)?;
    record_managed_files(db, &files)?;
    write_planned_managed_files(&files)
}

/// What a generated NGINX file is, as the `managed_files` record spells
/// it.
///
/// Declared in the order these files can be removed in: a file that reads
/// a variable or a zone another one defines goes before it (see
/// [`unused_managed_files`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagedKind {
    /// `stop-bots-limits-untrusted.conf`, which reads the trust file's
    /// variable.
    UntrustedLimits,
    /// `stop-bots-limits.conf`.
    Limits,
    /// The generated `robots.txt`.
    RobotsTxt,
    /// `stop-bots-trusted.conf`, which the untrusted zone reads.
    Trusted,
}

impl ManagedKind {
    const ALL: [ManagedKind; 4] = [
        ManagedKind::UntrustedLimits,
        ManagedKind::Limits,
        ManagedKind::RobotsTxt,
        ManagedKind::Trusted,
    ];

    /// The id in `managed_files.kind`. Stored, so never renamed.
    pub fn id(self) -> &'static str {
        match self {
            ManagedKind::UntrustedLimits => "nginx-untrusted-limits",
            ManagedKind::Limits => "nginx-limits",
            ManagedKind::RobotsTxt => "nginx-robots-txt",
            ManagedKind::Trusted => "nginx-trusted",
        }
    }

    pub fn from_id(id: &str) -> Option<ManagedKind> {
        ManagedKind::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// The kind of a file this module writes, by its name — which is
    /// fixed for each kind wherever the file lives.
    pub fn of(path: &Path) -> Option<ManagedKind> {
        let name = path.file_name()?.to_str()?;
        let conf_d = Path::new("");
        ManagedKind::ALL.into_iter().find(|kind| {
            let own = match kind {
                ManagedKind::UntrustedLimits => untrusted_rate_limit_conf_path(conf_d),
                ManagedKind::Limits => rate_limit_conf_path(conf_d),
                ManagedKind::RobotsTxt => PathBuf::from("robots.txt"),
                ManagedKind::Trusted => trusted_conf_path(conf_d),
            };
            own.as_os_str() == name
        })
    }
}

/// Records every file in `files` as about to be written, and forgets
/// recorded ones that are gone and not about to come back.
///
/// **Before the write, not after.** The record is what cleaning up works
/// from, so it must never be missing a file that exists: a process that
/// dies between the two leaves a row for a file that may not exist, which
/// costs one no-op removal, rather than a file no row names, which would
/// never be removed at all.
///
/// Called wherever a `Db` is at hand ahead of [`write_planned_managed_files`]:
/// here, by [`write_managed_files`] and [`apply_site_and_reload`], and by
/// the TUI when it plans an apply on the main thread.
pub fn record_managed_files(db: &crate::db::Db, files: &[(PathBuf, String)]) -> Result<()> {
    let recorded: Vec<PathBuf> = files.iter().map(|(path, _)| recorded_form(path)).collect();
    for path in &recorded {
        if let Some(kind) = ManagedKind::of(path) {
            db.record_managed_file(path, kind.id())?;
        }
    }
    for row in db.managed_files()? {
        let known = ManagedKind::from_id(&row.kind).is_some();
        let planned = recorded.contains(&row.path);
        if known && !planned && fs::symlink_metadata(&row.path).is_err() {
            db.forget_managed_file(&row.path)?;
        }
    }
    Ok(())
}

/// `path` as the record keeps it: its directory resolved, so that the
/// record names it the way [`unused_managed_files`] will accept it back
/// (see [`DirFile::exactly`]). As it is if its directory is not there yet.
fn recorded_form(path: &Path) -> PathBuf {
    match (path.parent().map(fs::canonicalize), path.file_name()) {
        (Some(Ok(dir)), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

/// Whether the regular file `file` is one this project generated, by its
/// header. Read through its directory's descriptor, without following a
/// link or waiting on a FIFO.
///
/// Asked before removing a file the record names in a place the current
/// settings would not write to: the record says stop-bots wrote a file
/// there once, and this says it is still that file rather than one an
/// operator has since put in its place.
pub fn is_generated(file: &DirFile) -> bool {
    file.first_line()
        .is_some_and(|first| first.starts_with('#') && first.contains("Generated by stop-bots"))
}

/// The managed files the current settings call for, as `(path, body)`
/// pairs.
///
/// Split out of [`write_managed_files`] so that the `Db` reads and the
/// disk writes can happen in different places: the TUI resolves this on
/// the main thread (`Db` isn't `Sync`) and writes it on a background
/// thread, so an apply doesn't stall the event loop. The CLI still calls
/// [`write_managed_files`], which does both in one go.
pub fn planned_managed_files(db: &crate::db::Db, root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let conf_d = conf_d_dir(root);
    let mut files = Vec::new();
    if db.get_serve_robots_txt()? {
        files.push((robots_txt_path(), robots_txt_body(db)?));
    }
    // The trust file before the untrusted zone, which reads a variable it
    // defines. NGINX resolves variables after parsing every file, so on
    // disk the order does not matter — but a write that fails between the
    // two should leave the definition behind, not the reference.
    let trust = trusted_conf(db)?;
    if let Some(body) = &trust {
        files.push((trusted_conf_path(&conf_d), body.clone()));
    }
    if db.get_rate_limit_enabled()? {
        let (rps, megabytes) = (db.get_rate_limit_rps()?, db.get_rate_limit_zone_mb()?);
        // Written even while every site names the untrusted zone instead:
        // a site not yet rewritten still names this one, and removing it
        // is only safe once none does — which is `unused_managed_files`'s
        // job, and it would have to know which zone every site on disk
        // names. An idle zone costs its shared memory and nothing else.
        files.push((
            rate_limit_conf_path(&conf_d),
            rate_limit_conf_body(rps, megabytes),
        ));
        if trust.is_some() {
            files.push((
                untrusted_rate_limit_conf_path(&conf_d),
                untrusted_rate_limit_conf_body(rps, megabytes),
            ));
        }
    }
    Ok(files)
}

/// Writes what [`planned_managed_files`] resolved. Touches no `Db`.
///
/// Returns how many files it actually changed, because that can be the
/// whole of an apply: trusting a second user agent rewrites the trust
/// file and no site's block, and an apply that counted only site files
/// would decide NGINX had nothing new to read and never reload it.
pub fn write_planned_managed_files(files: &[(PathBuf, String)]) -> Result<usize> {
    let mut changed = 0;
    for (path, body) in files {
        if fs::read_to_string(path).ok().as_deref() != Some(body.as_str()) {
            write_managed(path, body)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Deletes every file this project owns that the current settings no
/// longer reference. **Runs after site configs have been rewritten**, and
/// that ordering is load-bearing rather than tidy:
///
/// A `server` block containing `limit_req zone=stop_bots;` whose
/// `limit_req_zone` has been deleted is not a degraded config, it's an
/// invalid one — NGINX refuses to load with "unknown limit_req_zone", so
/// `nginx -t` fails and the *whole* reload is rejected, including every
/// unrelated site. Removing the zone only once no block references it any
/// more means the config on disk is valid at every point in between, so
/// an apply that dies half way (a permission error on one file, say)
/// leaves a working NGINX rather than one that won't reload at all.
///
/// A missing file is success, not an error.
pub fn remove_unused_managed_files(db: &crate::db::Db, root: &Path) -> Result<usize> {
    let unused = unused_managed_files(db, root)?;
    remove_and_forget(db, &unused)
}

/// Removes `unused`, as [`unused_managed_files`] resolved it, and forgets
/// each one's row.
fn remove_and_forget(db: &crate::db::Db, unused: &[DirFile]) -> Result<usize> {
    let removed = remove_planned_managed_files(unused)?;
    for file in unused {
        db.forget_managed_file(file.path())?;
    }
    Ok(removed)
}

/// The managed files the current settings no longer reference. Split from
/// the removal for the same reason [`planned_managed_files`] is split from
/// the write.
///
/// Two sources, in the order they may be removed ([`ManagedKind`]'s):
///
/// - **The record** of what was written (`managed_files`): every file
///   stop-bots wrote that the current settings would not write where it
///   is. This is what finds a set left in a `conf.d` the root no longer
///   points at, and a file an earlier release wrote that this one does not
///   know by name. Each is checked to still be a generated file first.
/// - **The fixed list** of where each file would be now, for files written
///   before the record existed. Removing one that is not there is a no-op.
///
/// Each comes back with its directory held open ([`DirFile`]): what is
/// removed, read or put back later is in the directory checked here, even
/// if a name on the way to it has changed since.
pub fn unused_managed_files(db: &crate::db::Db, root: &Path) -> Result<Vec<DirFile>> {
    let planned: Vec<PathBuf> = planned_managed_files(db, root)?
        .iter()
        .map(|(path, _)| recorded_form(path))
        .collect();
    let mut files = Vec::new();
    for path in fallback_unused_managed_files(db, root)? {
        if let Some(file) = DirFile::locate(&path)
            .with_context(|| format!("failed to open the directory of {}", path.display()))?
        {
            files.push(file);
        }
    }
    let within = managed_directories(root);
    for row in db.managed_files()? {
        let Some(kind) = ManagedKind::from_id(&row.kind) else {
            continue;
        };
        if planned.contains(&row.path)
            || files.iter().any(|file| file.path() == row.path)
            || ManagedKind::of(&row.path) != Some(kind)
        {
            continue;
        }
        if let Some(file) = removable_record(&row.path, &within) {
            files.push(file);
        }
    }
    // Stable, so the fixed list's own order survives within a kind.
    files.sort_by_key(|file| ManagedKind::of(file.path()));
    Ok(files)
}

/// The directories a recorded file may be removed from: the `conf.d`
/// this root's files go into, and [`managed_dir`], each with its links
/// resolved. A directory that does not exist is left out.
fn managed_directories(root: &Path) -> Vec<PathBuf> {
    [conf_d_dir(root), managed_dir()]
        .iter()
        .filter_map(|dir| fs::canonicalize(dir).ok())
        .collect()
}

/// The `managed_files` row naming `path`, if it may be acted on: a
/// regular file, not a link, directly inside one of `within`, with this
/// project's header — and `path` already resolved, with no link and no
/// `..` anywhere in it.
///
/// **The record is not trusted.** The web console can write every row of
/// the database, and this runs as root: a row naming `/etc/passwd`, a
/// link to it, or `<conf.d>/../../passwd` must delete nothing. Nor may a
/// row that reaches `conf.d` through a directory of the console's own
/// (`/var/lib/stop-bots/link/...`): the console could point that link
/// somewhere else between this check and the removal. So such a row is
/// refused, and the one that passes comes back with its directory held
/// open, which is what every later use goes through.
fn removable_record(path: &Path, within: &[PathBuf]) -> Option<DirFile> {
    let file = DirFile::exactly(path).ok()?;
    (within.iter().any(|dir| dir == file.dir()) && file.is_regular() && is_generated(&file))
        .then_some(file)
}

/// The fixed list: where each file the current settings do not call for
/// would be, if it were written today.
fn fallback_unused_managed_files(db: &crate::db::Db, root: &Path) -> Result<Vec<PathBuf>> {
    let conf_d = conf_d_dir(root);
    let mut paths = Vec::new();
    if !db.get_serve_robots_txt()? {
        paths.push(robots_txt_path());
    }
    let rate_limited = db.get_rate_limit_enabled()?;
    let trusted = trusted_conf(db)?.is_some();
    if !rate_limited {
        paths.push(rate_limit_conf_path(&conf_d));
    }
    // Before the trust file: it reads a variable the trust file defines,
    // so for the moment between the two deletions it must be this one
    // that is already gone.
    if !(rate_limited && trusted) {
        paths.push(untrusted_rate_limit_conf_path(&conf_d));
    }
    if !trusted {
        paths.push(trusted_conf_path(&conf_d));
    }
    Ok(paths)
}

/// Deletes what [`unused_managed_files`] resolved, through each one's
/// directory descriptor. Touches no `Db`. Returns how many were actually
/// there to delete — see [`write_planned_managed_files`] for why an apply
/// needs the count.
pub fn remove_planned_managed_files(files: &[DirFile]) -> Result<usize> {
    let mut removed = 0;
    for file in files {
        if remove_managed(file)? {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Writes a file this project owns outright. Replaced, never written
/// through — see [`write_atomically`].
fn write_managed(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_atomically(path, body.as_bytes(), 0o644, None)
}

/// Replaces `path` with `content`: a new file beside it, then a rename
/// over it.
///
/// **A rename replaces a symlink; a write follows it.** Everything here
/// runs as root, and on a host whose NGINX tree is a bind mount an
/// unprivileged user may be able to create files in it. `fs::write` on a
/// `conf.d/stop-bots-trusted.conf` they had made a link to `/etc/shadow`
/// would have written our text there. The temporary file is created with
/// `O_EXCL` under a random name, so a link planted at a guessed name makes
/// the create fail rather than be followed. It is also what makes the
/// write atomic: NGINX, or a reload that races us, sees the old file or
/// the new one, never half of one.
///
/// A leading dot keeps the temporary file out of NGINX's own globs
/// (`include sites-enabled/*`), which, like the shell's, skip dot files.
fn write_atomically(
    path: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    for _ in 0..8 {
        let temp = path.with_file_name(format!(".{name}.stop-bots-{:016x}", random_u64()));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to write {}", path.display()))
            }
        };
        let written = (|| -> std::io::Result<()> {
            file.write_all(content)?;
            if let Some((uid, gid)) = owner {
                // Best effort: only root can give a file away, and a
                // non-root run that got this far owns what it replaces.
                let _ = std::os::unix::fs::fchown(&file, Some(uid), Some(gid));
            }
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            file.sync_all()?;
            fs::rename(&temp, path)
        })();
        if let Err(err) = written {
            let _ = fs::remove_file(&temp);
            return Err(err).with_context(|| format!("failed to write {}", path.display()));
        }
        return Ok(());
    }
    anyhow::bail!(
        "failed to write {}: could not create a temporary file beside it",
        path.display()
    )
}

/// Unpredictable enough for a temporary file name, without a dependency:
/// std seeds every `RandomState` from the OS.
pub(crate) fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

/// Where an operator's site file really is: `path` itself, or, when a
/// symlink is involved, its target — which must be inside `root`.
///
/// Site files are legitimately links (`sites-enabled/x` to
/// `../sites-available/x`), so a link is followed rather than replaced.
/// But the same writable-tree case [`write_atomically`] is about applies:
/// a link to a file outside the NGINX config is not one this tool was
/// pointed at, and root rewriting whatever `server {` block it finds there
/// is exactly what the link was planted for.
fn resolve_site_file(path: &Path, root: &Path) -> Result<PathBuf> {
    let resolved =
        fs::canonicalize(path).with_context(|| format!("failed to read {}", path.display()))?;
    let as_given = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if resolved == as_given {
        return Ok(resolved);
    }
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to read the NGINX root {}", root.display()))?;
    if !resolved.starts_with(&root) {
        anyhow::bail!(
            "refusing to write {}: it links to {}, outside the NGINX root {}",
            path.display(),
            resolved.display(),
            root.display()
        );
    }
    Ok(resolved)
}

/// Writes an operator's site file in place of `path`, following a link to
/// it (see [`resolve_site_file`]) and keeping its mode and owner.
///
/// A file the operator made read-only stays unwritten: a rename would
/// replace it regardless of its permission bits, which is not what
/// `chmod 444` was asking for — and running without root is reported as
/// the permission error it is.
fn write_site_file(path: &Path, root: &Path, content: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let resolved = resolve_site_file(path, root)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&resolved)
        .with_context(|| format!("failed to write {}", path.display()))?;
    let meta = fs::metadata(&resolved)
        .with_context(|| format!("failed to read {}", resolved.display()))?;
    write_atomically(
        &resolved,
        content.as_bytes(),
        meta.mode() & 0o7777,
        Some((meta.uid(), meta.gid())),
    )
}

/// Removes a generated file, treating "already gone" as success.
///
/// Deleting rather than merely leaving it unreferenced matters most for
/// the rate-limit zone: [`CONF_D_DIR`] is globbed by the stock
/// `nginx.conf`, so a leftover file there stays *live* — it would keep
/// allocating its shared memory zone forever after the feature was
/// switched off. The `robots.txt` case is only inert-but-misleading, and
/// is removed for consistency: a stale generated artifact on disk invites
/// being wired back up by hand.
fn remove_managed(file: &DirFile) -> Result<bool> {
    file.remove()
        .with_context(|| format!("failed to remove {}", file.path().display()))
}

/// The [`BlockConfig`] currently in effect for one site — its own resolved
/// patterns (per-site overrides layered over the global cascade) plus every
/// host-wide setting that shapes the generated text. Every caller that
/// applies or status-checks a *specific* site goes through here rather than
/// assembling the struct itself, so a newly added host-wide knob reaches
/// all of them at once.
pub fn block_config_for_site(db: &crate::db::Db, site_id: i64) -> Result<BlockConfig> {
    Ok(BlockConfig {
        patterns: db.blocked_user_agent_patterns_for_site(site_id)?,
        response: db.get_block_response()?,
        serve_robots_txt: db.get_serve_robots_txt()?,
        rate_limit_burst: rate_limit_burst(db)?,
        exempt_paths: db.site_path_exemptions(site_id)?,
        agent_exemptions: db.site_agent_exemptions(site_id)?,
        request_rules: site_request_rules(db, site_id)?,
        trust: trusted_conf(db)?,
    })
}

/// The request-shape rules switched on for one site, in a stable order.
/// An unrecognised stored id is skipped rather than erroring: a database
/// written by a newer build must not stop an older one from applying
/// anything at all.
pub fn site_request_rules(db: &crate::db::Db, site_id: i64) -> Result<Vec<RequestRule>> {
    let mut rules: Vec<RequestRule> = db
        .site_request_rules(site_id)?
        .iter()
        .filter_map(|id| RequestRule::from_id(id))
        .collect();
    rules.sort();
    Ok(rules)
}

/// The [`BlockConfig`] for a `server` block with no site-specific overrides
/// — the global cascade only. Used as `apply_blocks_to_file`'s
/// `default_config` for blocks discovered on disk that were never scanned
/// into the database.
pub fn default_block_config(db: &crate::db::Db) -> Result<BlockConfig> {
    Ok(BlockConfig {
        patterns: db.blocked_user_agent_patterns()?,
        response: db.get_block_response()?,
        serve_robots_txt: db.get_serve_robots_txt()?,
        rate_limit_burst: rate_limit_burst(db)?,
        // A block with no site row has no per-site settings by
        // definition — both are keyed on `sites.id`.
        exempt_paths: Vec::new(),
        agent_exemptions: Vec::new(),
        request_rules: Vec::new(),
        trust: trusted_conf(db)?,
    })
}

/// Renders the sentinel block content (without surrounding blank lines) for
/// `config`, or `None` when there's nothing to block — an empty pattern
/// list, or one whose every entry [`pattern_problem`] rejects. `None` is what
/// makes [`apply_block`] *remove* an existing block rather than write an
/// empty one.
///
/// The patterns are split (via [`chunk_patterns`]) into one `if` statement per
/// chunk when it's long enough to need it. Multiple sequential
/// `if ($http_user_agent ~* "...") { return <code>; }` statements are
/// equivalent to one big alternation — whichever fires first returns — so
/// splitting changes nothing about what gets blocked, only how it's
/// written, and a pattern short enough for one chunk renders as a single
/// `if`.
fn block_text(config: &BlockConfig) -> Option<String> {
    let chunks = chunk_patterns(&usable_patterns(&config.patterns), MAX_PATTERN_CHUNK_LEN);
    let pattern = (!chunks.is_empty()).then_some(chunks);
    // Nothing to block and nothing to serve means no block at all, which
    // is what makes `apply_block` *remove* an existing one. Note this is
    // not "no patterns" alone: robots.txt, rate limiting and HTTP/1.x
    // rejection are each reason enough to keep a block, and treating any
    // of them as nothing would delete the block carrying it the moment
    // every bot happened to be allowed.
    if pattern.is_none()
        && !config.serve_robots_txt
        && config.rate_limit_burst.is_none()
        && config.request_rules.is_empty()
    {
        return None;
    }
    let code = config.response.status_code();
    let (prefixes, exact) = effective_exempt_paths(config);
    let exemptions = exemption_regex(&prefixes, exact);

    let mut out = format!("    {BLOCK_BEGIN}\n");

    // Two things can decide a request is unwanted: its user agent, and
    // (optionally) its HTTP version. NGINX's `if` takes exactly one
    // condition and they don't compose, so more than one reason — or any
    // reason at all combined with an exemption — means the flag idiom:
    // initialise a variable, let each reason set it, clear it for exempt
    // paths, and act on it last. Order is the whole mechanism.
    //
    // A single reason with no exemptions keeps the older direct-`return`
    // form, so a plain bot-blocking site's config doesn't churn.
    //
    // Trust is a clear like an exemption, but only where there is a block
    // to clear: with nothing blocking, a trusted client already gets
    // through, and the flag dance would be noise. (Rate limiting needs no
    // `if` — it cannot have one — and is lifted by naming a zone keyed so
    // that a trusted client is not counted; see
    // `UNTRUSTED_RATE_LIMIT_ZONE`.)
    let blocks_something = pattern.is_some() || !config.request_rules.is_empty();
    let clears_trusted = config.trust.is_some() && blocks_something;
    // Like trust, and unlike the plain exemptions, only where there is a
    // block to clear: they are narrower than a plain exemption, so a site
    // with nothing blocking already lets their clients through.
    let agent_clears = if blocks_something {
        agent_exemption_clears(&config.agent_exemptions)
    } else {
        String::new()
    };
    let uses_flag = exemptions.is_some()
        || clears_trusted
        || !agent_clears.is_empty()
        || (pattern.is_some() && !config.request_rules.is_empty());

    if uses_flag {
        out.push_str("    set $stop_bots_block 0;\n");
    }
    // Tarpit is a normal rejection whose body is throttled to a crawl:
    // `$limit_rate` is a writable NGINX variable, and one byte per second
    // turns the few hundred bytes of a default error page into minutes of
    // held connection. Chosen over the `limit_req`-without-`nodelay`
    // idiom deliberately — that one needs an http-context `map` over
    // `$stop_bots_block`, and if the variable is ever undefined NGINX
    // refuses to start. This fails the other way: if a small body turns
    // out not to be throttled, the client simply gets an ordinary 403.
    let throttle = if config.response.is_tarpit() {
        "set $limit_rate 1;\n        "
    } else {
        ""
    };

    // The tail shared by every blocker's `if`: set the flag, or return
    // directly when there's only one reason and nothing to exempt.
    let set_blocked = if uses_flag {
        "set $stop_bots_block 1;\n    }\n".to_string()
    } else {
        format!("{throttle}return {code};\n    }}\n")
    };

    if let Some(chunks) = &pattern {
        for chunk in chunks {
            out.push_str(&format!(
                "    if ($http_user_agent ~* {}) {{\n        {set_blocked}",
                nginx_quoted(chunk)
            ));
        }
    }
    for rule in &config.request_rules {
        out.push_str(&format!(
            "    if ({}) {{\n        {set_blocked}",
            rule.condition()
        ));
    }
    if let Some(exemptions) = &exemptions {
        out.push_str(&format!(
            "    if ($uri ~* {}) {{\n        set $stop_bots_block 0;\n    }}\n",
            nginx_quoted(exemptions)
        ));
    }
    out.push_str(&agent_clears);
    if clears_trusted {
        // After every set, like the exemption clear, and for the same
        // reason: order is the mechanism. A trusted client is let through
        // whatever the reason it matched — a bot pattern, a request-shape
        // rule, a manual user-agent block.
        out.push_str(&format!(
            "    if ({TRUSTED_VAR}) {{\n        set $stop_bots_block 0;\n    }}\n"
        ));
    }
    if uses_flag {
        out.push_str(&format!(
            "    if ($stop_bots_block) {{\n        {throttle}return {code};\n    }}\n"
        ));
    }

    if let Some(burst) = config.rate_limit_burst {
        // `nodelay` so a visitor who briefly exceeds the rate is served
        // immediately from the burst allowance rather than queued —
        // queueing makes an ordinary page load feel broken while doing
        // nothing extra to a bot, which just waits.
        //
        // 429 rather than NGINX's default 503: a client that is being rate
        // limited is not being told the server is unavailable, and a
        // well-behaved one backs off correctly when told the truth.
        let zone = if config.trust.is_some() {
            UNTRUSTED_RATE_LIMIT_ZONE
        } else {
            RATE_LIMIT_ZONE
        };
        out.push_str(&format!(
            "    limit_req zone={zone} burst={burst} nodelay;\n    limit_req_status 429;\n"
        ));
    }
    if config.serve_robots_txt {
        // `alias` rather than `return 200 "<body>"` — see
        // `BlockConfig::serve_robots_txt`. `location = ` is an exact match,
        // so this only ever intercepts /robots.txt itself, and it sits
        // inside the sentinel markers like everything else, so removing the
        // feature removes the directive with it.
        out.push_str(&format!(
            "    location = /robots.txt {{\n        alias {};\n        default_type text/plain;\n    }}\n",
            robots_txt_path().display()
        ));
    }
    out.push_str(&format!("    {BLOCK_END}\n"));
    Some(out)
}

/// The exemptions actually applied, as `(prefixes, exact paths)`: the
/// site's configured prefixes, plus `/robots.txt` itself — exactly that
/// file, not everything under it — whenever this block serves it.
///
/// That addition is not a convenience, it's what makes robots.txt work at
/// all. Server-level `if`/`return` run in NGINX's **server rewrite
/// phase**, which happens *before* location selection — so a user agent
/// caught by the block never reaches the `location = /robots.txt` block
/// underneath it. Without this, the generated robots.txt would list
/// exactly the agents that can never read it, and the polite layer would
/// be pure decoration.
///
/// Letting a blocked crawler read robots.txt is also the behaviour worth
/// wanting on its own: a bot that can fetch the file can learn to stop
/// asking, whereas one that gets a bare 403 on everything learns nothing
/// and keeps coming back. Serving it costs a single small static file.
fn effective_exempt_paths(config: &BlockConfig) -> (Vec<String>, &'static [&'static str]) {
    let mut paths = config.exempt_paths.clone();
    let exact: &[&str] = if config.serve_robots_txt {
        &["/robots.txt"]
    } else {
        &[]
    };
    if !config.request_rules.is_empty() {
        // Never optional, and never surfaced as a setting to switch off.
        // `/.well-known/` is where ACME HTTP-01 validation is fetched
        // from, by a non-browser client speaking HTTP/1.1. Rejecting it
        // doesn't break anything today — it breaks certificate renewal
        // weeks later, which is about the least traceable failure this
        // project could ship. The same path prefix carries security.txt
        // and a pile of other machine-fetched documents, none of which
        // negotiate HTTP/2 either.
        paths.push("/.well-known/".to_string());
    }
    (paths, exact)
}

/// Narrows a site's config to what is actually safe to emit into *this*
/// `server` block.
///
/// Only one setting needs this today: HTTP/1.x rejection is dropped in a
/// block that doesn't terminate TLS. Browsers don't speak HTTP/2 without
/// it, so there every request is 1.1 and the rule would reject all
/// traffic — and a site's port-80 redirect and port-443 blocks routinely
/// share a `server_name`, which means one per-site setting reaches both.
/// Silently narrowing beats both alternatives: refusing to apply would
/// make a legitimate setting unusable on a normal two-block site, and
/// emitting it anyway would take the site down.
///
/// Status checks go through here too, so a site whose rule is correctly
/// omitted from its plain-HTTP block still reads as `UP TO DATE` rather
/// than permanently `STALE`.
///
/// It also widens one thing: a block that serves the console (the `Web
/// Access` panel's path mode puts its `location` there) exempts the
/// console's prefix, see [`console_prefix_in`].
fn for_block(config: &BlockConfig, content: &str, block: &ServerBlock) -> BlockConfig {
    let mut config = config.clone();
    if !block.is_tls {
        config.request_rules.retain(|rule| !rule.needs_tls());
    }
    if let Some(prefix) = console_prefix_in(content, block) {
        if !config.exempt_paths.contains(&prefix) {
            config.exempt_paths.push(prefix);
        }
    }
    config
}

/// The prefix of the console `location` this tool wrote into `block`, if
/// it wrote one — read from the block itself, so the exemption is always
/// for the location that is really there, on the block it is really in.
///
/// **Why the console is exempt.** Every check in a sentinel block runs in
/// NGINX's server rewrite phase, before a `location` is chosen, so the
/// console's own `location` inherits every one of them: a blocked user
/// agent, a request-shape rule, the operator's own browser. An operator who
/// turned on "No Accept-Language" and uses a client that sends none was
/// locked out of the one place they could turn it off. The console has its
/// own login and throttle; the bot rules are for the site.
///
/// What this cannot exempt it from is `limit_req`. It is inherited by
/// every `location` that has none of its own and cannot be switched off in
/// one, and a `limit_req` there would need a zone of its own in `conf.d`.
/// It refuses with a 429 for as long as the client exceeds the rate, and
/// no longer, so it slows the console down rather than locking anyone out.
fn console_prefix_in(content: &str, block: &ServerBlock) -> Option<String> {
    let (start, end) = locate_console_block(content, block)?;
    let tokens = lex(&content[start..end]).tokens;
    let at = tokens.iter().position(|(word, _)| word == "location")?;
    let (word, offset) = tokens.get(at + 1)?;
    // The quoted form this tool writes now, or the bare `location /prefix/`
    // that versions before 0.0.13 wrote between the same markers — a host
    // that set up Web Access then and never re-applied it still has that,
    // and its operator needs the exemption as much. A modifier (`=`, `^~`,
    // `~`) is somebody's edit and not a prefix to guess at: the validator
    // refuses anything that does not start with `/`.
    let prefix = if content[start + offset..].starts_with('"') {
        nginx_unquoted(word)
    } else {
        word.clone()
    };
    is_stored_as_valid(&prefix, crate::db::validate_exempt_path).then_some(prefix)
}

/// Whether `value`, as stored, is exactly what `validate` would store:
/// how a row is checked again at render time rather than trusted, whatever
/// wrote it.
fn is_stored_as_valid(value: &str, validate: fn(&str) -> Result<String>) -> bool {
    validate(value).is_ok_and(|valid| valid == value)
}

/// Builds the `$uri` regex that clears the block flag, or `None` when
/// there are no usable exemptions: `prefixes` match anything under them,
/// `exact` only themselves.
///
/// **An exemption is a literal path prefix, never a regex.** Anchored with
/// `^` and alternated, so `/blog` exempts `/blog` and `/blog/post` but not
/// `/notablog`. Each path is regex-escaped: these are URL prefixes typed by
/// an admin, not hand-written regex, and an unescaped `.` or `?` in one
/// would quietly widen the exemption far beyond what was asked for —
/// which, unlike a too-narrow pattern, fails *open*. The whole regex is
/// then written by [`nginx_quoted`]: before it was, a stored `/x\|` reached
/// PCRE as `^(/x\\|)`, which exempted every path on the site.
///
/// Each path is checked again with `db::validate_exempt_path` rather than
/// trusted, so a row an older release stored without that check is left
/// out (see [`skipped_entries`]) instead of written.
///
/// Matched against `$uri`, never `$request_uri`, for the reason
/// [`agent_exemption_clears`] gives: the raw request line is not the path
/// NGINX serves, and `/robots.txt/../wp-login.php` begins with an exempt
/// prefix while resolving to a blocked one.
fn exemption_regex(prefixes: &[String], exact: &[&str]) -> Option<String> {
    let usable = |p: &&str| is_stored_as_valid(p, crate::db::validate_exempt_path);
    let escaped: Vec<String> = prefixes
        .iter()
        .map(String::as_str)
        .filter(usable)
        .map(crate::db::escape_regex_literal)
        .chain(
            exact
                .iter()
                .copied()
                .filter(usable)
                .map(|p| format!("{}$", crate::db::escape_regex_literal(p))),
        )
        .collect();
    (!escaped.is_empty()).then(|| format!("^({})", escaped.join("|")))
}

/// The statements that clear the block flag for agent exemptions: one
/// group per user agent, or `""` when none is usable.
///
/// NGINX's `if` takes one condition, and "this user agent *and* this path"
/// is two. So each group puts the path into [`AGENT_EXEMPT_VAR`] only when
/// the user agent matches, and then tests that variable against the paths:
///
/// ```nginx
/// set $stop_bots_exempt "";
/// if ($http_user_agent ~* "okhttp") { set $stop_bots_exempt $uri; }
/// if ($stop_bots_exempt ~* "^(/remote\.php/dav/)") { set $stop_bots_block 0; }
/// ```
///
/// The reset opens every group, not just the first: a variable left
/// holding the path by one group's user agent would otherwise let the next
/// group's paths through for a client that never matched it.
///
/// `$uri`, not `$request_uri`, like the plain exemptions. `$uri` has had
/// `..` and `//` resolved and is decoded, so `/remote.php/dav/../../login`
/// is `/login` and matches nothing here; `$request_uri` is the raw request
/// line, where that same string starts with the exempt prefix while the
/// application behind the proxy resolves it to somewhere else. The plain
/// exemptions read `$request_uri` too until the same bypass was found
/// there.
///
/// The user agent is matched as an escaped, case-insensitive substring,
/// the same as a trusted one, and checked again here rather than trusting
/// the caller, since a user agent that matched more than it says would
/// exempt every client.
fn agent_exemption_clears(exemptions: &[crate::db::AgentExemption]) -> String {
    let mut out = String::new();
    for group in exemptions.chunk_by(|a, b| a.user_agent == b.user_agent) {
        let user_agent = &group[0].user_agent;
        if !is_stored_as_valid(user_agent, crate::db::validate_exemption_user_agent) {
            continue;
        }
        let paths: Vec<String> = group.iter().map(|e| e.path.clone()).collect();
        let Some(paths) = exemption_regex(&paths, &[]) else {
            continue;
        };
        out.push_str(&format!(
            "    set {AGENT_EXEMPT_VAR} \"\";\n    if ($http_user_agent ~* {}) {{\n        set {AGENT_EXEMPT_VAR} $uri;\n    }}\n    if ({AGENT_EXEMPT_VAR} ~* {}) {{\n        set $stop_bots_block 0;\n    }}\n",
            nginx_quoted(&crate::db::escape_regex_literal(user_agent)),
            nginx_quoted(&paths)
        ));
    }
    out
}

/// Finds the byte range of an existing sentinel block's lines within
/// `block`, if one is already present.
fn locate_existing_block(content: &str, block: &ServerBlock) -> Option<(usize, usize)> {
    locate_marked(content, block, BLOCK_BEGIN, BLOCK_END)
}

/// The whole lines from a `begin` marker comment to the next `end` marker
/// comment inside `block`.
///
/// Markers are matched only as real comments, as [`lex`] finds them, and
/// only when the comment is exactly the marker. A text search found them
/// anywhere, including inside a quoted bot pattern: `# END stop-bots` in a
/// hand-blocked user agent ended the block half way through its first
/// `if`, and every later run left another copy behind.
fn locate_marked(
    content: &str,
    block: &ServerBlock,
    begin: &str,
    end: &str,
) -> Option<(usize, usize)> {
    let comments: Vec<(usize, usize)> = lex(content)
        .comments
        .into_iter()
        .filter(|&(start, _)| start > block.open && start < block.close)
        .collect();
    let is =
        |&(start, stop): &(usize, usize), marker: &str| content[start..stop].trim_end() == marker;
    let begin_at = comments.iter().find(|c| is(c, begin))?.0;
    let end_at = comments.iter().find(|c| c.0 > begin_at && is(c, end))?.1;
    let line_start = content[..begin_at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = content[end_at..]
        .find('\n')
        .map_or(content.len(), |i| end_at + i + 1);
    Some((line_start, line_end))
}

/// Inserts, replaces or removes the sentinel bot-blocking block inside
/// `block`. A `config` that renders to nothing (see [`block_text`]) removes
/// any existing block. Idempotent: applying the same config twice yields
/// identical output.
fn apply_block(content: &str, block: &ServerBlock, config: &BlockConfig) -> String {
    let new_block = block_text(&for_block(config, content, block));

    match locate_existing_block(content, block) {
        Some((start, end)) => {
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..start]);
            if let Some(b) = &new_block {
                out.push_str(b);
            }
            out.push_str(&content[end..]);
            out
        }
        None => match new_block {
            Some(b) => {
                let mut out = String::with_capacity(content.len() + b.len() + 1);
                out.push_str(&content[..block.open + 1]);
                out.push('\n');
                out.push_str(&b);
                out.push_str(&content[block.open + 1..]);
                out
            }
            None => content.to_string(),
        },
    }
}

// ---- reaching the console from outside ----

/// Sentinel markers for the console's own `location` block.
///
/// Separate from [`BLOCK_BEGIN`]/[`BLOCK_END`] on purpose: the two live in
/// the same `server` block and are rewritten by different actions, so one
/// pair of markers would have "apply blocks" and "set up web access"
/// deleting each other's work.
const CONSOLE_BEGIN: &str = "# BEGIN stop-bots console (DO NOT EDIT)";
const CONSOLE_END: &str = "# END stop-bots console";

/// How the console is reached from outside this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleAccess {
    /// Its own `server` block on `host`, written as a new config file.
    ///
    /// Served over plain HTTP. Adding a certificate is a separate step
    /// (certbot), and until it is taken, the console's password form and
    /// its session cookie cross the network in the clear — which is why
    /// [`ConsoleAccess::Path`] is the default the panel offers.
    Subdomain { host: String },
    /// A `location` block inserted into an existing site's `server` block,
    /// which is how the console inherits that site's certificate.
    Path {
        /// Normalised with a leading and trailing slash, e.g. `/stop-bots/`.
        prefix: String,
        /// The site's config file, from `Db::list_sites`.
        config_path: PathBuf,
        /// The `server_name` whose block to edit.
        server_name: String,
    },
}

/// The `location` block that proxies to the console, shared by both modes.
///
/// `proxy_pass` deliberately has **no** trailing slash, and the location
/// keeps its prefix: this server matches the full path including the
/// prefix and generates links that do too, so a `proxy_pass` that stripped
/// it would leave every link pointing outside the location block. See
/// `--base-path` in `main.rs` for the same warning aimed at whoever writes
/// this by hand.
///
/// The prefix is quoted, with any `"` or `\` inside it escaped. It has
/// already passed `BasePath::parse`, which admits nothing NGINX treats as
/// syntax; the quotes are what keep that true if a prefix ever reaches
/// here some other way. This file is loaded by a root NGINX, and an
/// unquoted `;` or `}` in it is a directive of the caller's choosing.
fn console_location(prefix: &str, upstream: &std::net::SocketAddr) -> String {
    let prefix = prefix.replace('\\', r"\\").replace('"', r#"\""#);
    format!(
        "    {CONSOLE_BEGIN}\n    \
         location \"{prefix}\" {{\n        \
         proxy_pass http://{upstream};\n        \
         proxy_set_header Host $host;\n        \
         proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        \
         proxy_set_header X-Forwarded-Proto $scheme;\n    \
         }}\n    {CONSOLE_END}\n"
    )
}

/// A whole `server` block serving the console on `host` at `/`.
pub fn console_server_block(host: &str, upstream: &std::net::SocketAddr) -> String {
    format!(
        "# {} (`Web Access` panel). Safe to edit or delete.\n\
         #\n\
         # Plain HTTP: the console's password and session cookie are in the clear\n\
         # until this host has a certificate for {host}. Get one with:\n\
         #\n\
         #     certbot --nginx -d {host}\n\
         #\n\
         # then run `stop-bots set-web --secure-cookie true` so the cookie is\n\
         # HTTPS-only.\n\
         server {{\n    \
         listen 80;\n    \
         listen [::]:80;\n    \
         server_name {host};\n    \
         # The console shows what attackers sent, and its requests carry it:\n    \
         # logged where the detectors read, they would block its operator.\n    \
         access_log off;\n\n\
         {}}}\n",
        crate::generated::generated_by(),
        console_location("/", upstream)
    )
}

/// Where a subdomain-mode config file goes: `conf.d/<name>.conf` under the
/// config root.
///
/// `conf.d` rather than `sites-available` plus a symlink into
/// `sites-enabled`: Debian's default `nginx.conf` includes both, `conf.d`
/// needs no second step to take effect, and one file is one thing to
/// delete when an operator changes their mind.
pub fn console_site_path(root: &Path) -> PathBuf {
    root.join("conf.d").join("stop-bots-console.conf")
}

/// Inserts (or updates) the console's `location` block inside
/// `server_name`'s block in `content`.
///
/// Returns `None` if that file has no matching `server` block — a
/// mismatch between the database's idea of where a site lives and what is
/// on disk, which is a refusal rather than something to guess at.
fn with_console_location(
    content: &str,
    server_name: &str,
    prefix: &str,
    upstream: &std::net::SocketAddr,
) -> Option<String> {
    let blocks = parse_server_blocks(content);
    // Prefer the TLS block. A site normally has two — a port-80 redirect
    // and the real HTTPS one — and putting the console in the first would
    // serve its login form over cleartext on a host that has a
    // certificate sitting right there.
    let block = blocks
        .iter()
        .filter(|b| b.names.iter().any(|n| n == server_name))
        .max_by_key(|b| b.is_tls)?;

    let new_block = console_location(prefix, upstream);
    Some(match locate_console_block(content, block) {
        Some((start, end)) => {
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..start]);
            out.push_str(&new_block);
            out.push_str(&content[end..]);
            out
        }
        None => {
            let mut out = String::with_capacity(content.len() + new_block.len() + 1);
            out.push_str(&content[..block.open + 1]);
            out.push('\n');
            out.push_str(&new_block);
            out.push_str(&content[block.open + 1..]);
            out
        }
    })
}

/// [`locate_existing_block`] for the console markers.
fn locate_console_block(content: &str, block: &ServerBlock) -> Option<(usize, usize)> {
    locate_marked(content, block, CONSOLE_BEGIN, CONSOLE_END)
}

/// Writes `content` to `path`, validates the whole NGINX config, and puts
/// the previous state back if validation fails.
///
/// The reason this exists rather than a plain write: `apply_all_sites`
/// writes then tests, which is survivable when it edited an existing file
/// — but a *new* `server` block that fails `nginx -t` leaves the config
/// unloadable, and nothing looks wrong because the running NGINX keeps
/// serving from memory. The next reload is then somebody else's problem,
/// most likely certbot's renewal hook at 3am.
///
/// `write` does the writing, both of `content` and of the restore, so the
/// console's own file is replaced rather than written through
/// ([`write_managed`]) and an operator's site file is written only where
/// it really lives inside the root ([`write_site_file`]).
fn write_validated(
    path: &Path,
    content: &str,
    commands: &NginxCommands,
    write: impl Fn(&Path, &str) -> Result<()>,
) -> Result<()> {
    let _lock = crate::applylock::hold()?;
    let previous = fs::read_to_string(path).ok();
    write(path, content).map_err(crate::hint::sudo_if_denied)?;

    if let Err(err) = test_config(commands) {
        // Put it back before reporting, so the operator is not left with a
        // config that cannot be loaded.
        let restored = match &previous {
            Some(text) => write(path, text),
            None => fs::remove_file(path).map_err(Into::into),
        };
        let note = if restored.is_ok() {
            "the previous config was restored"
        } else {
            "AND RESTORING THE PREVIOUS CONFIG ALSO FAILED — fix this by hand"
        };
        anyhow::bail!("{err:#}\n\n{}: {}", note, path.display());
    }
    Ok(())
}

/// Sets the console up to be reachable, and returns what it did.
///
/// Validated before it can take effect (see [`write_validated`]) and
/// reloaded only on success. The caller is responsible for the two
/// database settings that have to agree with this config — `web:base_path`
/// and `web:allowed_hosts` — because a console reachable at a path it does
/// not serve, or under a `Host` it refuses, is broken in a way that looks
/// like a 404 or a 403 rather than like a configuration mistake.
pub fn apply_console_access(
    root: &Path,
    access: &ConsoleAccess,
    upstream: &std::net::SocketAddr,
    commands: &NginxCommands,
) -> Result<PathBuf> {
    // Checked again here, where they are written into a file a root NGINX
    // loads, whatever checked them before: a front-end, or a request that
    // came over the helper's socket.
    match access {
        ConsoleAccess::Subdomain { host } if !crate::webaccess::is_plausible_host(host) => {
            anyhow::bail!("refusing to write a server block for {host:?}: it is not a host name")
        }
        ConsoleAccess::Path { prefix, .. }
            if crate::web::BasePath::parse(prefix)
                .ok()
                .filter(|base| !base.is_root())
                .map(|base| base.url("/"))
                .as_deref()
                != Some(prefix.as_str()) =>
        {
            anyhow::bail!("refusing to write a location for {prefix:?}: it is not a path prefix")
        }
        _ => {}
    }
    match access {
        ConsoleAccess::Subdomain { host } => {
            let path = console_site_path(root);
            write_validated(
                &path,
                &console_server_block(host, upstream),
                commands,
                write_managed,
            )?;
            Ok(path)
        }
        ConsoleAccess::Path {
            prefix,
            config_path,
            server_name,
        } => {
            let content = fs::read_to_string(config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            let updated = with_console_location(&content, server_name, prefix, upstream)
                .with_context(|| {
                    format!(
                        "{} has no `server` block for {server_name} — re-scan sites and try again",
                        config_path.display()
                    )
                })?;
            write_validated(config_path, &updated, commands, |path, text| {
                write_site_file(path, root, text)
            })?;
            Ok(config_path.clone())
        }
    }
}

/// What to do when the NGINX config root is not there: most often NGINX
/// is in a container, or installed under another prefix.
fn missing_root_hint() -> &'static str {
    "If NGINX's config lives somewhere else, store where with \
     `stop-bots set-nginx-commands --root <dir>`, or pass `--root <dir>` for one run."
}

/// Recursively walks `root` looking for NGINX config files containing
/// `server { ... }` blocks, returning one [`DiscoveredSite`] per block (named
/// after its first `server_name`). Files with no server block, or that
/// aren't valid UTF-8 text, are silently skipped.
///
/// A `root` that does not exist *is* an error, unlike anything unreadable
/// inside one. The difference matters: a mistyped `--root` used to walk
/// nothing and report "0 site(s)", which reads exactly like a correct run
/// against a server that has no sites — so the mistake looked like an
/// answer. A file that can't be read inside a real root is a different
/// thing, and still skipped.
pub fn discover_sites(root: &Path) -> Result<Vec<DiscoveredSite>> {
    if !root.exists() {
        return Err(crate::hint::with(
            anyhow::anyhow!("{} does not exist", root.display()),
            missing_root_hint(),
        ));
    }
    let mut sites = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for block in parse_server_blocks(&content) {
            if let Some(name) = block.names.first() {
                sites.push(DiscoveredSite {
                    server_name: name.clone(),
                    config_path: path.to_path_buf(),
                });
            }
        }
    }
    Ok(sites)
}

/// The file under `root` that declares `server_name`, found on disk.
///
/// The database records where a site was when it was scanned, and the
/// web console can write that record. Whatever writes a site's file as
/// root therefore asks the disk, and uses `recorded` only to tell apart
/// two files that both declare the name: the one at the recorded path if
/// it is among them, the only one if there is one, and otherwise none.
pub fn locate_site(root: &Path, server_name: &str, recorded: Option<&Path>) -> Result<PathBuf> {
    let discovered = discover_sites(root)?;
    if let Some(found) = pick_site(&discovered, server_name, recorded) {
        return Ok(found.to_path_buf());
    }
    let candidates = discovered
        .iter()
        .filter(|site| site.server_name == server_name)
        .count();
    match candidates {
        0 => anyhow::bail!(
            "no `server` block under {} is named {server_name} — re-scan sites and try again",
            root.display()
        ),
        _ => anyhow::bail!(
            "{candidates} files under {} declare {server_name}, and none is the one scanned — \
             re-scan sites and try again",
            root.display()
        ),
    }
}

/// [`locate_site`]'s choice among sites already discovered: the one named
/// `server_name` at `recorded`, else the only one named `server_name`.
fn pick_site<'a>(
    discovered: &'a [DiscoveredSite],
    server_name: &str,
    recorded: Option<&Path>,
) -> Option<&'a Path> {
    let mut named = discovered
        .iter()
        .filter(|site| site.server_name == server_name)
        .map(|site| site.config_path.as_path());
    let all: Vec<&Path> = named.by_ref().collect();
    if let Some(found) = recorded.and_then(|recorded| all.iter().find(|p| **p == recorded)) {
        return Some(found);
    }
    match all.as_slice() {
        [only] => Some(only),
        _ => None,
    }
}

/// The status of every scanned site, each judged against the file found
/// for it on disk under `root` (see [`locate_site`]) rather than the one
/// its row names. A site with no file there is [`SiteApplyStatus::NotFound`].
pub fn site_statuses(db: &crate::db::Db, root: &Path) -> Result<Vec<(i64, SiteApplyStatus)>> {
    let discovered = discover_sites(root).unwrap_or_default();
    let conf_d = conf_d_dir(root);
    let mut statuses = Vec::new();
    for site in db.list_sites()? {
        let status = match pick_site(
            &discovered,
            &site.server_name,
            Some(Path::new(&site.config_path)),
        ) {
            Some(path) => site_apply_status(
                path,
                &site.server_name,
                &block_config_for_site(db, site.id)?,
                &conf_d,
            ),
            None => SiteApplyStatus::NotFound,
        };
        statuses.push((site.id, status));
    }
    Ok(statuses)
}

/// Applies the bot-blocking rule to every `server { ... }` block found in
/// `config_path`. Each block gets the [`BlockConfig`] from `site_configs`
/// belonging to its own `server_name` (its `names.first()`), or
/// `default_config` if that name isn't in `site_configs` at all (e.g. a
/// block discovered on disk that was never scanned into the db yet). A
/// config that renders to nothing removes any existing block there.
/// Returns whether the file was actually changed on disk.
///
/// Two blocks sharing the same `server_name` (e.g. a port-80-redirect block
/// plus the real port-443 block for the same site) resolve to the same
/// `site_configs` entry and so still get the same rule; two blocks with
/// *different* names in the same file now correctly get independent rules.
///
/// `root` is the NGINX config root: a `config_path` that is a symlink is
/// written through only to a target inside it (see [`write_site_file`]).
pub fn apply_blocks_to_file(
    config_path: &Path,
    root: &Path,
    site_configs: &[(String, BlockConfig)],
    default_config: &BlockConfig,
) -> Result<bool> {
    let before = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let after = blocks_for_file(&before, config_path, site_configs, default_config)?;
    if after == before {
        return Ok(false);
    }
    write_site_file(config_path, root, &after)?;
    Ok(true)
}

/// What [`apply_blocks_to_file`] would make of `content`, without writing
/// it — the half a preview shares with the apply, so the two cannot differ.
pub fn blocks_for_file(
    content: &str,
    config_path: &Path,
    site_configs: &[(String, BlockConfig)],
    default_config: &BlockConfig,
) -> Result<String> {
    let mut content = content.to_string();
    let block_count = parse_server_blocks(&content).len();

    // Editing a block shifts the byte offsets of every block after it, so
    // re-parse before each edit rather than reusing stale spans. Block order
    // is stable across re-parses since edits only rewrite sentinel lines
    // inside existing braces, never add or remove server blocks.
    for i in 0..block_count {
        let blocks = parse_server_blocks(&content);
        let block = nth_block(&blocks, i, block_count, config_path)?;
        let config = block
            .names
            .first()
            .and_then(|name| site_configs.iter().find(|(n, _)| n == name))
            .map(|(_, config)| config)
            .unwrap_or(default_config);
        content = apply_block(&content, block, config);
    }
    Ok(content)
}

/// Block `i` of `blocks`, which must still number `expected`.
///
/// Re-parsing between edits relies on an edit never adding or removing a
/// `server` block. The sentinel block only holds quoted text and
/// `location`s, so it cannot, unless the reader and NGINX disagree about
/// where a quoted string ends — which is how a bot pattern once turned
/// into a second `server` block. Refusing is the safe answer if they ever
/// disagree again.
fn nth_block<'a>(
    blocks: &'a [ServerBlock],
    i: usize,
    expected: usize,
    config_path: &Path,
) -> Result<&'a ServerBlock> {
    if blocks.len() != expected {
        anyhow::bail!(
            "refusing to write {}: editing it changed how many server blocks it has",
            config_path.display()
        );
    }
    Ok(&blocks[i])
}

/// Whether a site's on-disk config currently matches the blocking rule
/// that would be computed for it right now — backs the TUI's per-site
/// status tag in NGINX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SiteApplyStatus {
    /// Every `server` block for this name already carries the expected
    /// rule (or none is expected and none is present).
    UpToDate,
    /// At least one block for this name is missing the expected rule, or
    /// carries a different one.
    Stale,
    /// The config file couldn't be read, or no `server` block in it
    /// declares this `server_name` anymore (e.g. renamed or removed on
    /// disk since the last scan).
    NotFound,
}

/// The exact text of the sentinel block currently written inside `block`,
/// if one is present at all. Anchored to the sentinel's own line range (via
/// `locate_existing_block`), not searched for anywhere in the whole block —
/// a hand-written `if ($http_user_agent ~* "...")` or similar elsewhere in
/// the same `server { ... }` must never be mistaken for ours.
///
/// Deliberately the *whole* block text rather than just the user-agent
/// pattern extracted back out of it, which is what this used to compare.
/// Once anything besides the pattern can vary between two valid blocks —
/// the response code, and later exemptions/rate limiting — reconstructing
/// one field and comparing only that would report a site as `UpToDate`
/// while its on-disk block still returns the old code. Comparing rendered
/// text against rendered text has no such blind spot, and it stays correct
/// for free as `BlockConfig` grows.
fn current_block_text(content: &str, block: &ServerBlock) -> Option<String> {
    let (start, end) = locate_existing_block(content, block)?;
    Some(content[start..end].to_string())
}

/// Every `server_name` on the block that declares `server_name`, that one
/// included.
///
/// `scan-sites` stores a site under the *first* name on its block, because
/// one row needs one name. NGINX answers for all of them, so anything that
/// has to agree with NGINX about which hosts reach a site — the console's
/// own allowlist, so far — needs the rest too.
///
/// Falls back to just `server_name` if the file cannot be read or no block
/// declares it: a caller that gets one name behaves as it did before this
/// existed, which is the right way for this to fail.
pub fn server_names_for(config_path: &Path, server_name: &str) -> Vec<String> {
    let Ok(content) = fs::read_to_string(config_path) else {
        return vec![server_name.to_string()];
    };
    parse_server_blocks(&content)
        .into_iter()
        .find(|block| block.names.iter().any(|name| name == server_name))
        .map(|block| block.names)
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| vec![server_name.to_string()])
}

/// Whether a `server` block declaring `server_name` in `config_path`
/// terminates TLS — which is the block the console's `location` goes into
/// when there is one (see `with_console_location`). `false` when the file
/// cannot be read, which is the answer that turns nothing on.
pub fn serves_tls(config_path: &Path, server_name: &str) -> bool {
    fs::read_to_string(config_path).is_ok_and(|content| {
        parse_server_blocks(&content)
            .iter()
            .any(|block| block.is_tls && block.names.iter().any(|name| name == server_name))
    })
}

/// The application that answers for `server_name` in `config_path`, as
/// [`crate::services::recognise`] reads it from the directives of every
/// `server` block there that declares one of the site's names: the
/// HTTPS block is the one that passes requests on, and it is not always
/// the one the site was scanned from.
///
/// This project's own blocks are taken out first. The bot block quotes
/// upstream user-agent patterns, which could name anything, and the
/// console's `location` passes to the console.
///
/// `None` when the file cannot be read, which allows nothing.
pub fn site_service(config_path: &Path, server_name: &str) -> Option<crate::services::Service> {
    let content = without_injected_blocks(&fs::read_to_string(config_path).ok()?);
    let blocks = parse_server_blocks(&content);
    let names = blocks
        .iter()
        .find(|block| block.names.iter().any(|name| name == server_name))?
        .names
        .clone();
    let tokens = lex(&content).tokens;
    let mut directives = Vec::new();
    for block in blocks
        .iter()
        .filter(|block| block.names.iter().any(|name| names.contains(name)))
    {
        directives.extend(directives_within(&tokens, block.open, block.close));
    }
    crate::services::recognise(&directives)
}

/// Each directive whose tokens lie between the byte offsets `open` and
/// `close`, as its name and its arguments: a word at the start of a
/// statement, and the words after it up to its `;` or `{`. Those of
/// nested blocks (`location`, `if`) included.
fn directives_within(
    tokens: &[(String, usize)],
    open: usize,
    close: usize,
) -> Vec<(String, Vec<String>)> {
    let mut directives = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for (token, _) in tokens
        .iter()
        .filter(|(_, offset)| *offset > open && *offset < close)
    {
        match token.as_str() {
            ";" | "{" | "}" => directives.extend(current.take()),
            word => match &mut current {
                Some((_, arguments)) => arguments.push(word.to_string()),
                None => current = Some((word.to_string(), Vec::new())),
            },
        }
    }
    directives
}

/// Compares what's actually written in `config_path` for `server_name`
/// against `config` (the currently computed blocking rule for that site)
/// without changing anything on disk.
pub fn site_apply_status(
    config_path: &Path,
    server_name: &str,
    config: &BlockConfig,
    conf_d: &Path,
) -> SiteApplyStatus {
    let Ok(content) = fs::read_to_string(config_path) else {
        return SiteApplyStatus::NotFound;
    };
    let blocks = parse_server_blocks(&content);
    let matching: Vec<&ServerBlock> = blocks
        .iter()
        .filter(|b| b.names.first().map(String::as_str) == Some(server_name))
        .collect();
    if matching.is_empty() {
        return SiteApplyStatus::NotFound;
    }
    let trust_on_disk = config
        .trust
        .as_ref()
        .and_then(|_| fs::read_to_string(trusted_conf_path(conf_d)).ok());
    if matching.iter().all(|block| {
        current_block_text(&content, block) == block_text(&for_block(config, &content, block))
    }) && trust_file_is_current(config, trust_on_disk.as_deref())
    {
        SiteApplyStatus::UpToDate
    } else {
        SiteApplyStatus::Stale
    }
}

/// Applies `config` only to the `server { ... }` block(s) in `config_path`
/// whose first `server_name` is `server_name`, leaving every other block in
/// the file completely untouched — unlike [`apply_blocks_to_file`], which
/// resets every block it has no explicit entry for back to
/// `default_config`. This backs the TUI's per-site "Apply now" action:
/// applying one site's overrides must never silently rewrite an unrelated
/// site sharing the same file. Returns whether the file was actually
/// changed on disk. `root` is as for [`apply_blocks_to_file`].
pub fn apply_block_for_site(
    config_path: &Path,
    root: &Path,
    server_name: &str,
    config: &BlockConfig,
) -> Result<bool> {
    let before = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let after = block_for_site(&before, config_path, server_name, config)?;
    if after == before {
        return Ok(false);
    }
    write_site_file(config_path, root, &after)?;
    Ok(true)
}

/// What [`apply_block_for_site`] would make of `content`, without writing
/// it.
pub fn block_for_site(
    content: &str,
    config_path: &Path,
    server_name: &str,
    config: &BlockConfig,
) -> Result<String> {
    let mut content = content.to_string();
    let block_count = parse_server_blocks(&content).len();

    // Same re-parse-before-each-edit approach as `blocks_for_file`:
    // editing a block shifts the byte offsets of every block after it.
    for i in 0..block_count {
        let blocks = parse_server_blocks(&content);
        let block = nth_block(&blocks, i, block_count, config_path)?;
        if block.names.first().map(String::as_str) != Some(server_name) {
            continue;
        }
        content = apply_block(&content, block, config);
    }
    Ok(content)
}

/// One file an apply would change: its content before and after, `None`
/// for a file that does not exist (before) or would be removed (after).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl FileChange {
    /// A unified diff of the change, labelled with the path.
    pub fn diff(&self) -> String {
        let label = self.path.display().to_string();
        crate::diff::unified(
            self.before.as_deref(),
            self.after.as_deref(),
            &label,
            &label,
        )
    }

    /// `(new)`, `(removed)` or nothing, for a summary line.
    pub fn kind(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, Some(_)) => " (new)",
            (Some(_), None) => " (removed)",
            _ => "",
        }
    }
}

/// Every file [`apply_all_sites_and_reload`] would change under `root`,
/// and how, without changing any of them: the generated files it would
/// write or remove, and each site file it would rewrite. Through the same
/// functions the apply uses, so a preview cannot promise something the
/// apply then does differently.
pub fn preview_all_sites(db: &crate::db::Db, root: &Path) -> Result<Vec<FileChange>> {
    let sites = discover_sites(root)?;
    let mut changes: Vec<FileChange> = Vec::new();
    for (path, body) in planned_managed_files(db, root)? {
        let before = fs::read_to_string(&path).ok();
        if before.as_deref() != Some(body.as_str()) {
            changes.push(FileChange {
                path,
                before,
                after: Some(body),
            });
        }
    }

    let default_config = default_block_config(db)?;
    let known_sites = db.list_sites()?;
    let mut config_paths: Vec<_> = sites.iter().map(|s| s.config_path.clone()).collect();
    config_paths.sort();
    config_paths.dedup();
    for path in config_paths {
        // The same per-file join `apply_all_sites` makes, for its reason.
        let site_configs: Vec<(String, BlockConfig)> = known_sites
            .iter()
            .filter(|s| Path::new(&s.config_path) == path.as_path())
            .map(|s| Ok((s.server_name.clone(), block_config_for_site(db, s.id)?)))
            .collect::<Result<_>>()?;
        let before = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let after = blocks_for_file(&before, &path, &site_configs, &default_config)?;
        if after != before {
            changes.push(FileChange {
                path,
                before: Some(before),
                after: Some(after),
            });
        }
    }

    for file in unused_managed_files(db, root)? {
        if let Some(before) = file.read_to_string() {
            changes.push(FileChange {
                path: file.path().to_path_buf(),
                before: Some(before),
                after: None,
            });
        }
    }
    Ok(changes)
}

/// What [`apply_all_sites_and_reload`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApplyAllOutcome {
    pub sites: usize,
    pub files: usize,
    /// How many files were actually rewritten, generated ones (the trust
    /// file, the rate-limit zone) included. Zero means NGINX has nothing
    /// new to read, so there is no point reloading it.
    pub changed: usize,
    /// Whether the new config passed the test and NGINX was reloaded.
    pub reloaded: bool,
}

/// Applies the current blocking policy to every site discovered under
/// `root`, and — given `commands` — tests the result and reloads NGINX.
/// The one way every front-end applies everything.
///
/// **A config that fails the test is put back.** Every file the apply may
/// touch is read first ([`Snapshot`]); if the test then fails, each is
/// restored byte for byte and each generated file this apply created is
/// removed, before the error is returned. Writing and leaving it was
/// survivable only in appearance: the running NGINX keeps serving the old
/// config from memory, and the broken one is loaded by whatever reloads
/// next — certbot's renewal hook at 3am, or a reboot — taking every site
/// down together. A write that fails half way is put back the same way.
///
/// `None` writes without testing or reloading: `--no-reload` and
/// `--no-apply`, for a host whose NGINX this process must not touch.
///
/// **One at a time**, across processes: it holds [`crate::applylock`]
/// from before the snapshot to after the reload, and a second apply waits
/// briefly and then reports that another stop-bots is applying.
pub fn apply_all_sites_and_reload(
    db: &crate::db::Db,
    root: &Path,
    commands: Option<&NginxCommands>,
) -> Result<ApplyAllOutcome> {
    // Before the snapshot, not after: what a failed test restores has to
    // be the state no other apply was halfway through changing.
    let _lock = crate::applylock::hold()?;
    let sites = discover_sites(root)?;
    // Resolved once, for the snapshot and the apply alike: what is put
    // back on a failure is exactly what the apply may have changed.
    let planned = planned_managed_files(db, root)?;
    let unused = unused_managed_files(db, root)?;
    let snapshot = Snapshot::of_apply(
        root,
        planned.iter().map(|(path, _)| path.clone()),
        &unused,
        sites.iter().map(|site| site.config_path.as_path()),
    )?;

    let outcome = match apply_all_sites(db, root, &sites, &planned, &unused) {
        Ok(outcome) => outcome,
        Err(err) => return Err(crate::hint::sudo_if_denied(snapshot.restore_after(err))),
    };
    let Some(commands) = commands.filter(|_| outcome.changed > 0) else {
        return Ok(outcome);
    };
    test_or_restore(&snapshot, commands)?;
    run(&commands.reload, "the NGINX reload")
        .context("the new config passed the test, but the reload failed")?;
    Ok(ApplyAllOutcome {
        reloaded: true,
        ..outcome
    })
}

/// The writing half of [`apply_all_sites_and_reload`].
///
/// The ordering below is load-bearing: generated files are written
/// *before* any config that aliases them, and unreferenced ones are
/// deleted only *after* every config has been rewritten (see
/// [`remove_unused_managed_files`]).
fn apply_all_sites(
    db: &crate::db::Db,
    root: &Path,
    sites: &[DiscoveredSite],
    planned: &[(PathBuf, String)],
    unused: &[DirFile],
) -> Result<ApplyAllOutcome> {
    record_managed_files(db, planned)?;
    let mut changed = write_planned_managed_files(planned)?;
    let default_config = default_block_config(db)?;
    // Sites already known to the db (i.e. previously scanned) — the only
    // ones that can carry a per-site override at all.
    let known_sites = db.list_sites()?;

    // A single config file commonly holds multiple `server` blocks for the
    // same site (e.g. an HTTP redirect block plus the HTTPS one), so dedupe
    // by file and apply once per file rather than once per discovered site.
    let mut config_paths: Vec<_> = sites.iter().map(|s| s.config_path.clone()).collect();
    config_paths.sort();
    config_paths.dedup();

    for path in &config_paths {
        // Built fresh per file, filtered to sites that actually live in
        // *this* file: `server_name` alone isn't unique across the whole
        // `sites` table (two different files can share one, e.g. a stale
        // config left behind after a rename), so a single map built once
        // for the whole run could leak one site's override onto another's
        // same-named block in a different file. Note this join is a
        // textual `config_path` match, only valid when `root` here matches
        // whatever root was used at scan time — a mismatch just falls
        // through to `default_config`, not an error.
        let site_configs: Vec<(String, BlockConfig)> = known_sites
            .iter()
            .filter(|s| Path::new(&s.config_path) == path.as_path())
            .map(|s| Ok((s.server_name.clone(), block_config_for_site(db, s.id)?)))
            .collect::<Result<_>>()?;
        if apply_blocks_to_file(path, root, &site_configs, &default_config)? {
            changed += 1;
        }
    }

    // Only now that every config has been rewritten is it safe to delete a
    // generated file the new config no longer references.
    changed += remove_and_forget(db, unused)?;

    Ok(ApplyAllOutcome {
        sites: sites.len(),
        files: config_paths.len(),
        changed,
        reloaded: false,
    })
}

/// Applies one site's policy — its file and the generated files it reads —
/// and, given `commands`, tests and reloads the way
/// [`apply_all_sites_and_reload`] does. Returns the site's name and
/// whether anything changed.
pub fn apply_site_and_reload(
    db: &crate::db::Db,
    root: &Path,
    site: &crate::db::Site,
    commands: Option<&NginxCommands>,
) -> Result<(bool, bool)> {
    let _lock = crate::applylock::hold()?;
    // The file found on disk for this site, not the one its row names:
    // see `locate_site`.
    let config_path = &locate_site(root, &site.server_name, Some(Path::new(&site.config_path)))?;
    let managed = planned_managed_files(db, root)?;
    record_managed_files(db, &managed)?;
    let snapshot = Snapshot::of_apply(
        root,
        managed.iter().map(|(path, _)| path.clone()),
        &[],
        [config_path.as_path()],
    )?;

    let written = (|| -> Result<bool> {
        let managed_changed = write_planned_managed_files(&managed)?;
        let config = block_config_for_site(db, site.id)?;
        let site_changed = apply_block_for_site(config_path, root, &site.server_name, &config)?;
        Ok(managed_changed > 0 || site_changed)
    })();
    let changed = match written {
        Ok(changed) => changed,
        Err(err) => return Err(crate::hint::sudo_if_denied(snapshot.restore_after(err))),
    };
    let Some(commands) = commands.filter(|_| changed) else {
        return Ok((changed, false));
    };
    test_or_restore(&snapshot, commands)?;
    run(&commands.reload, "the NGINX reload")
        .context("the new config passed the test, but the reload failed")?;
    Ok((changed, true))
}

// ---- taking it all out again: `stop-bots uninstall` ----

/// `content` with every block stop-bots injected taken out of every
/// `server` block — the bot-blocking block and the console's `location`,
/// found by their markers — and the line break each insertion added.
///
/// The exact inverse of an insertion: a block is put right after the
/// `server {` brace with a newline in front of it, so where that is still
/// how it sits, the newline goes too and the file is byte for byte what it
/// was before stop-bots touched it. A block somebody has moved elsewhere
/// in the `server` block loses only its own lines.
pub fn without_injected_blocks(content: &str) -> String {
    let mut content = content.to_string();
    // One block per pass, re-parsed each time: taking one out shifts the
    // offsets of everything after it. Each pass removes at least one
    // marker line, so this ends.
    //
    // Within a `server` block, the one nearest the brace goes first. Both
    // kinds are inserted right after the brace, so that is the one
    // inserted last, and taking insertions back in reverse order is what
    // leaves the brace's own line break where it was.
    loop {
        let blocks = parse_server_blocks(&content);
        let found = blocks.iter().find_map(|block| {
            [
                locate_existing_block(&content, block),
                locate_console_block(&content, block),
            ]
            .into_iter()
            .flatten()
            .min()
            .map(|range| (block.open, range))
        });
        let Some((open, (start, end))) = found else {
            return content;
        };
        let inserted_newline = start == open + 2 && content.as_bytes()[open + 1] == b'\n';
        let start = if inserted_newline { open + 1 } else { start };
        content.replace_range(start..end, "");
    }
}

/// Whether `path` is the console's own site file, as the `Web Access`
/// panel writes it (see [`console_site_path`]) — by its first line, which
/// every version has written, so that a file an operator put at the same
/// path is left alone.
pub fn is_console_site_file(path: &Path) -> bool {
    use std::io::BufRead;
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    std::io::BufReader::new(file)
        .read_line(&mut first)
        .is_ok_and(|_| {
            first.starts_with('#')
                && first.contains("by stop-bots")
                && first.contains("(`Web Access` panel)")
        })
}

/// What [`remove_everything`] did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    /// Site files that had a block taken out.
    pub site_files: Vec<PathBuf>,
    /// Generated files that were there and were deleted.
    pub generated: Vec<PathBuf>,
    /// Whether the result passed the test and NGINX was reloaded.
    pub reloaded: bool,
}

/// Takes every injected block out of every site file under `root` and
/// deletes `generated`, as one change: tested with `commands`, **put back
/// byte for byte if the test fails**, and reloaded only once it passes.
///
/// The same machinery an apply uses ([`Snapshot`], [`test_or_restore`]),
/// for the same reason: a config NGINX rejects is not loaded by this
/// reload, it is loaded by the next one — certbot's at 3am.
///
/// The site files are rewritten before the generated files go, the
/// reverse of an apply's order and for the same reason: nothing may be
/// left referring to a zone or variable that is gone.
///
/// `commands` is `None` for a tree that is not the running NGINX's
/// (`uninstall --prefix`); `dry_run` works out the same answer and writes
/// nothing.
///
/// `generated` is held by directory descriptor ([`DirFile`]): some of it
/// comes from the database's record, and is removed, and put back if the
/// test fails, in the directory it was checked in.
pub fn remove_everything(
    root: &Path,
    generated: &[DirFile],
    commands: Option<&NginxCommands>,
    dry_run: bool,
) -> Result<Removal> {
    // The same lock every apply holds, from the first read of a site file
    // to the reload: a console or a cron `batch` applying while this runs
    // would otherwise write a block back into a file this has just read.
    // A dry run writes nothing and takes nothing.
    let _lock = if dry_run {
        None
    } else {
        Some(crate::applylock::hold()?)
    };
    let mut edits = Vec::new();
    let mut paths: Vec<PathBuf> = discover_sites(root)?
        .into_iter()
        .map(|site| site.config_path)
        .collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let stripped = without_injected_blocks(&content);
        if stripped != content {
            edits.push((path, stripped));
        }
    }
    let present: Vec<DirFile> = generated
        .iter()
        .filter(|file| file.is_regular())
        .cloned()
        .collect();
    let mut removal = Removal {
        site_files: edits.iter().map(|(path, _)| path.clone()).collect(),
        generated: present
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect(),
        reloaded: false,
    };
    if dry_run || (edits.is_empty() && present.is_empty()) {
        return Ok(removal);
    }

    let snapshot = Snapshot::of_apply(
        root,
        [],
        &present,
        edits.iter().map(|(path, _)| path.as_path()),
    )?;
    let written = (|| -> Result<()> {
        for (path, content) in &edits {
            write_site_file(path, root, content)?;
        }
        remove_planned_managed_files(&present)?;
        Ok(())
    })();
    if let Err(err) = written {
        return Err(snapshot.restore_after(err));
    }
    let Some(commands) = commands else {
        return Ok(removal);
    };
    test_or_restore(&snapshot, commands)?;
    run(&commands.reload, "the NGINX reload")
        .context("the config without stop-bots passed the test, but the reload failed")?;
    removal.reloaded = true;
    Ok(removal)
}

/// Every file an apply may touch, as it was before the apply began, so
/// that a config NGINX rejects can be taken back.
///
/// Holds the bytes and the mode of each regular file, and "absent" for
/// anything else — so restoring removes a file the apply created. A
/// symlink counts as absent: the writes here replace a link rather than
/// following it (see [`write_atomically`]), and putting a planted one back
/// would put back the thing that was planted.
///
/// A file named by the database's record is held by its directory's
/// descriptor ([`DirFile`]) and put back through it, never through its
/// path: a path in a directory the web console owns can lead somewhere
/// else by the time a failed test puts it back, and root would write the
/// held bytes there.
pub struct Snapshot {
    files: Vec<(Place, Option<Held>)>,
}

/// Where a [`Snapshot`]ed file is: a path this program derived itself (a
/// site file, a generated file the settings call for), or a file held
/// through its directory.
enum Place {
    Path(PathBuf),
    Held(DirFile),
}

impl Place {
    fn path(&self) -> &Path {
        match self {
            Place::Path(path) => path,
            Place::Held(file) => file.path(),
        }
    }

    /// The regular file there, or `None` for nothing, a link or anything
    /// else. Opened without following a link or waiting on a FIFO.
    fn read(&self) -> std::io::Result<Option<Held>> {
        match self {
            Place::Held(file) => file.read(),
            Place::Path(path) => {
                use std::io::Read;
                use std::os::unix::fs::MetadataExt;
                let mut file = match crate::db::guard::open_nofollow(path) {
                    Ok(Some(file)) => file,
                    Ok(None) => return Ok(None),
                    Err(err) if err.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
                    Err(err) => return Err(err),
                };
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Ok(None);
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Ok(Some((bytes, meta.mode() & 0o7777)))
            }
        }
    }

    fn write(&self, bytes: &[u8], mode: u32) -> Result<()> {
        match self {
            Place::Path(path) => write_atomically(path, bytes, mode, None),
            Place::Held(file) => file
                .write_atomically(bytes, mode)
                .with_context(|| format!("failed to write {}", file.path().display())),
        }
    }

    /// Removes the regular file there, if there is one.
    fn remove_file(&self) -> Result<()> {
        let removed = match self {
            Place::Path(path) => match fs::symlink_metadata(path) {
                Ok(meta) if meta.is_file() => fs::remove_file(path),
                _ => Ok(()),
            },
            Place::Held(file) if file.is_regular() => file.remove().map(|_| ()),
            Place::Held(_) => Ok(()),
        };
        removed.with_context(|| format!("failed to remove {}", self.path().display()))
    }
}

/// A regular file's contents and permission bits, as [`Snapshot`] found
/// them: `(bytes, mode)`.
type Held = (Vec<u8>, u32);

impl Snapshot {
    /// Reads every file in `paths`. Fails if one exists and cannot be
    /// read, because an apply that could not be undone should not start.
    pub fn take(paths: impl IntoIterator<Item = PathBuf>) -> Result<Snapshot> {
        Snapshot::take_places(paths.into_iter().map(Place::Path))
    }

    fn take_places(places: impl IntoIterator<Item = Place>) -> Result<Snapshot> {
        let mut files: Vec<(Place, Option<Held>)> = Vec::new();
        for place in places {
            if files.iter().any(|(seen, _)| seen.path() == place.path()) {
                continue;
            }
            let before = place
                .read()
                .with_context(|| format!("failed to read {}", place.path().display()))?;
            files.push((place, before));
        }
        Ok(Snapshot { files })
    }

    /// A snapshot of what an apply under `root` may touch: the `planned`
    /// generated files at their own paths, the `held` ones (those the
    /// record names) through their directories, and each of `site_files`
    /// where it really is, which is where [`write_site_file`] will write
    /// it. A site file that cannot be resolved is left out; its write will
    /// fail with the same error, and that failure is what undoes the
    /// apply.
    pub fn of_apply<'a>(
        root: &Path,
        planned: impl IntoIterator<Item = PathBuf>,
        held: &[DirFile],
        site_files: impl IntoIterator<Item = &'a Path>,
    ) -> Result<Snapshot> {
        Snapshot::take_places(
            planned
                .into_iter()
                .map(Place::Path)
                .chain(held.iter().cloned().map(Place::Held))
                .chain(
                    site_files
                        .into_iter()
                        .filter_map(|path| resolve_site_file(path, root).ok())
                        .map(Place::Path),
                ),
        )
    }

    /// Puts every file back as it was. Carries on past a failure, so one
    /// stuck file does not leave the rest half restored, and names each
    /// one that could not be.
    pub fn restore(&self) -> Result<()> {
        let mut failed = Vec::new();
        for (place, before) in &self.files {
            let restored = match before {
                Some((bytes, _))
                    if place.read().ok().flatten().map(|(now, _)| now).as_ref() == Some(bytes) =>
                {
                    Ok(())
                }
                Some((bytes, mode)) => place.write(bytes, *mode),
                None => place.remove_file(),
            };
            if let Err(err) = restored {
                failed.push(format!("{err:#}"));
            }
        }
        if !failed.is_empty() {
            anyhow::bail!("{}", failed.join("; "));
        }
        Ok(())
    }

    /// `err`, with whether the restore that follows it worked.
    fn restore_after(&self, err: anyhow::Error) -> anyhow::Error {
        match self.restore() {
            Ok(()) => err.context("nothing was applied: every file it changed was put back"),
            Err(restore) => err.context(format!(
                "AND PUTTING THE PREVIOUS FILES BACK FAILED, fix these by hand: {restore:#}"
            )),
        }
    }
}

/// Runs the configured test against what was just written, and on a
/// failure puts back everything `snapshot` holds before reporting.
pub fn test_or_restore(snapshot: &Snapshot, commands: &NginxCommands) -> Result<()> {
    test_config(commands).map_err(|err| snapshot.restore_after(err))
}

/// How to test and reload NGINX.
///
/// Two commands rather than two hardcoded invocations, because NGINX is
/// not always a service on this host. The case that forced it: NGINX in a
/// container, with its config on a bind mount this tool writes to. The
/// files are ours to edit, but `systemctl reload nginx` reloads nothing —
/// there is no such unit — and `nginx -t` either isn't installed or tests
/// a different config than the one the container will read. Both become
/// `docker exec <name> nginx ...` there.
///
/// Read from the host's settings ([`crate::hostconf`]), never from the
/// database: these run as root, and the web console can write every row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NginxCommands {
    /// Argv for the config check. Must exit non-zero on a bad config.
    pub test: Vec<String>,
    /// Argv for the reload.
    pub reload: Vec<String>,
}

impl NginxCommands {
    /// What a normal host install needs, and what every caller got before
    /// these were configurable.
    pub const DEFAULT_TEST: &'static str = "nginx -t";
    pub const DEFAULT_RELOAD: &'static str = "systemctl reload nginx";

    /// The `settings` keys these were stored under before they became host
    /// settings. Read only by [`crate::hostconf::migrate`], once.
    pub const TEST_KEY: &'static str = crate::db::keys::NGINX_TEST_COMMAND;
    pub const RELOAD_KEY: &'static str = crate::db::keys::NGINX_RELOAD_COMMAND;
    /// Where this host's site configs actually live.
    pub const ROOT_KEY: &'static str = crate::db::keys::NGINX_ROOT;

    /// The same, from the host's settings — see [`crate::hostconf`].
    pub fn from_host(host: &crate::hostconf::HostConf) -> Result<Self> {
        host.commands()
    }
}

/// The stock location, and what every host had before this was configurable.
pub const DEFAULT_ROOT: &str = "/etc/nginx";

/// Where to look for site configs: the flag if one was given, else the
/// host setting, else the stock path. See [`crate::hostconf::HostConf::root`].
///
/// An NGINX in a container moves three things away from their defaults —
/// the commands that drive it, the logs it writes, and the directory its
/// config lives in — and all three are host settings now. Flag beats
/// setting: a one-off run against a checkout or a staging tree must not
/// require the stored value to be changed and put back.
pub fn root(host: &crate::hostconf::HostConf, flag: Option<&Path>) -> PathBuf {
    host.root(flag)
}

impl Default for NginxCommands {
    fn default() -> Self {
        Self {
            test: split_command(Self::DEFAULT_TEST).expect("the default test command parses"),
            reload: split_command(Self::DEFAULT_RELOAD).expect("the default reload command parses"),
        }
    }
}

/// Splits a configured command into argv.
///
/// Deliberately *not* a shell: the string is never handed to `sh -c`, so
/// there is no expansion, no globbing, no `;` and no pipelines. What a
/// reload command needs is words, and words with spaces in them — a path
/// under `/Applications/...`, a container named with a space — which is
/// exactly single and double quotes and nothing else.
///
/// Keeping the shell out of it is the security-relevant half. This command
/// runs as root; `sh -c` would turn a settings row into arbitrary code, and
/// the settings table is reachable from the web UI.
pub fn split_command(input: &str) -> Result<Vec<String>> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut has_word = false;
    let mut quote: Option<char> = None;

    for c in input.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '\'' || c == '"' => {
                // An empty quoted string is still an argument, so the
                // word has to be marked as started here and not only by
                // pushing a character.
                quote = Some(c);
                has_word = true;
            }
            None if c.is_whitespace() => {
                if has_word {
                    argv.push(std::mem::take(&mut current));
                    has_word = false;
                }
            }
            None => {
                current.push(c);
                has_word = true;
            }
        }
    }

    if quote.is_some() {
        anyhow::bail!("unbalanced quote in `{input}`");
    }
    if has_word {
        argv.push(current);
    }
    if argv.is_empty() {
        anyhow::bail!("`{input}` is empty");
    }
    Ok(argv)
}

/// Runs `argv`, returning its stderr on a non-zero exit.
///
/// `output`, not `status`: `status` inherits stdout and stderr, so anything
/// the command says lands directly on the TUI's alternate screen and
/// corrupts it. Run non-root, `systemctl` additionally pulls in a polkit
/// agent, which takes over the terminal outright to ask for a password.
fn run(argv: &[String], what: &str) -> Result<()> {
    let (program, args) = argv.split_first().expect("argv is never empty");
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|err| spawn_failure(argv, err))?;
    if !output.status.success() {
        anyhow::bail!(
            "{what} (`{}`) exited with {}: {}",
            argv.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Why `argv` could not be started, and — when its program is not there
/// at all, the usual state of a host whose NGINX is in a container —
/// where the commands are set.
fn spawn_failure(argv: &[String], err: std::io::Error) -> anyhow::Error {
    let not_found = err.kind() == std::io::ErrorKind::NotFound;
    let err = anyhow::Error::new(err).context(format!("failed to run `{}`", argv.join(" ")));
    if !not_found {
        return err;
    }
    crate::hint::with(
        err,
        &format!(
            "`{}` isn't on PATH. If NGINX runs in a container, point stop-bots at it: \
             see `stop-bots set-nginx-commands --help`.",
            argv[0]
        ),
    )
}

/// Validates the currently-installed NGINX config. Run before every
/// [`reload_with`] so a malformed config — ours or an unrelated hand edit
/// elsewhere in the same install — is reported as a clear error here rather
/// than left for the admin to dig out of `systemctl status`.
fn test_config(commands: &NginxCommands) -> Result<()> {
    run(&commands.test, "the NGINX config test")
}

/// Reloads NGINX so a just-written blocking rule (from
/// [`apply_blocks_to_file`] or [`apply_block_for_site`]) actually takes
/// effect — writing the sentinel block to a site's config file alone does
/// nothing until NGINX re-reads it. Always preceded by [`test_config`]:
/// `systemctl reload` refuses a config that fails validation on its own too,
/// but checking explicitly here gets a message callers can show directly
/// rather than send the admin to `systemctl status`/`journalctl`.
pub fn reload_with(commands: &NginxCommands) -> Result<()> {
    test_config(commands)?;
    run(&commands.reload, "the NGINX reload")
}

#[cfg(test)]
mod tests {
    /// The gap this closes. An NGINX in a container moves three things:
    /// the commands that drive it, the logs it writes, and the directory
    /// its config lives in. The first two were stored; this one had to be
    /// repeated on five subcommands, and forgetting it anywhere scanned an
    /// empty /etc/nginx and reported success over nothing.
    #[test]
    fn a_stored_root_is_used_when_no_flag_is_given() {
        assert_eq!(
            root(&host_with_root("/srv/domaci/nginx"), None),
            std::path::PathBuf::from("/srv/domaci/nginx")
        );
    }

    #[test]
    fn nothing_stored_falls_back_to_the_stock_path() {
        assert_eq!(
            root(&crate::hostconf::HostConf::default(), None),
            std::path::PathBuf::from(DEFAULT_ROOT)
        );
    }

    /// A one-off run against a checkout must not require the stored value
    /// to be changed and put back -- the same precedence `LogPaths` uses.
    #[test]
    fn an_explicit_flag_beats_the_stored_root() {
        assert_eq!(
            root(
                &host_with_root("/srv/domaci/nginx"),
                Some(std::path::Path::new("/tmp/fixture"))
            ),
            std::path::PathBuf::from("/tmp/fixture")
        );
    }

    /// Host settings whose NGINX root is `root`.
    fn host_with_root(root: &str) -> crate::hostconf::HostConf {
        crate::hostconf::HostConf {
            nginx_root: Some(root.into()),
            ..Default::default()
        }
    }

    use super::*;

    // ---- configured test/reload commands ----

    #[test]
    fn split_command_splits_on_whitespace() {
        assert_eq!(
            split_command("docker exec nginx nginx -s reload").unwrap(),
            ["docker", "exec", "nginx", "nginx", "-s", "reload"]
        );
    }

    #[test]
    fn split_command_collapses_runs_of_whitespace() {
        assert_eq!(
            split_command("  nginx\t\t-t  ").unwrap(),
            ["nginx", "-t"],
            "leading, trailing and repeated whitespace must not produce empty argv entries"
        );
    }

    #[test]
    fn split_command_keeps_quoted_spaces_in_one_argument() {
        for (input, expected) in [
            (r#"docker exec "my nginx" nginx -t"#, "my nginx"),
            (r#"docker exec 'my nginx' nginx -t"#, "my nginx"),
        ] {
            let argv = split_command(input).unwrap();
            assert_eq!(argv[2], expected, "input was: {input}");
            assert_eq!(argv.len(), 5, "input was: {input}");
        }
    }

    #[test]
    fn split_command_treats_an_empty_quoted_string_as_an_argument() {
        assert_eq!(
            split_command(r#"nginx "" -t"#).unwrap(),
            ["nginx", "", "-t"],
            "an empty argument is still an argument, and dropping it shifts every flag after it"
        );
    }

    #[test]
    fn split_command_rejects_an_unbalanced_quote() {
        let err = split_command(r#"docker exec "my nginx nginx -t"#).unwrap_err();
        assert!(
            err.to_string().contains("unbalanced quote"),
            "error was: {err}"
        );
    }

    #[test]
    fn split_command_rejects_a_command_with_no_words() {
        for input in ["", "   "] {
            assert!(
                split_command(input).is_err(),
                "{input:?} has no program to run, so it must not parse"
            );
        }
    }

    #[test]
    fn split_command_does_not_interpret_shell_metacharacters() {
        // The command is never handed to `sh -c`, so these are ordinary
        // characters in an argument. This test is the guard on that: if
        // anyone ever routes it through a shell, it starts failing.
        assert_eq!(
            split_command("nginx -t; rm -rf /").unwrap(),
            ["nginx", "-t;", "rm", "-rf", "/"],
            "`;` must be part of a word, not a command separator"
        );
    }

    #[test]
    fn nginx_commands_default_to_the_host_install() {
        let commands = NginxCommands::default();
        assert_eq!(commands.test, ["nginx", "-t"]);
        assert_eq!(commands.reload, ["systemctl", "reload", "nginx"]);
    }

    #[test]
    fn nginx_commands_fall_back_per_setting() {
        let host = crate::hostconf::HostConf {
            nginx_reload_command: Some("docker exec web nginx -s reload".into()),
            ..Default::default()
        };

        let commands = NginxCommands::from_host(&host).unwrap();
        assert_eq!(
            commands.reload,
            ["docker", "exec", "web", "nginx", "-s", "reload"]
        );
        assert_eq!(
            commands.test,
            ["nginx", "-t"],
            "setting only the reload command must leave the test command at its default"
        );
    }

    #[test]
    fn nginx_commands_reject_a_stored_command_that_does_not_parse() {
        let host = crate::hostconf::HostConf {
            nginx_test_command: Some(r#"docker exec "web nginx -t"#.into()),
            ..Default::default()
        };

        // Not a silent fallback: reloading the host's NGINX because the
        // container command had an unbalanced quote is the exact surprise
        // worth failing loudly over.
        let err = NginxCommands::from_host(&host).unwrap_err();
        assert!(
            err.to_string().contains("nginx_test_command"),
            "the error must name the setting at fault; it was: {err}"
        );
    }
    use std::fs;

    const FIXTURES_ROOT: &str = "tests/fixtures/nginx";

    /// Every `site_apply_status` test here leaves `trust` unset, so the
    /// trust file is never read and this path is never touched. Named
    /// rather than repeated inline so that a test which *does* set
    /// `trust` stands out by passing a real directory.
    fn no_trust_file() -> &'static Path {
        Path::new("/nonexistent/conf.d")
    }

    /// A [`BlockConfig`] with the default 403 response, for the many tests
    /// that only care about which patterns end up in the file.
    fn cfg(patterns: &[&str]) -> BlockConfig {
        BlockConfig::new(
            patterns.iter().map(|p| p.to_string()).collect(),
            BlockResponse::Forbidden,
        )
    }

    #[test]
    fn discover_sites_finds_both_fixture_sites() {
        let mut sites = discover_sites(Path::new(FIXTURES_ROOT)).unwrap();
        sites.sort_by(|a, b| a.server_name.cmp(&b.server_name));

        let names: Vec<&str> = sites.iter().map(|s| s.server_name.as_str()).collect();
        assert_eq!(names, vec!["example.com", "localhost"]);
    }

    #[test]
    fn parse_server_blocks_ignores_commented_braces() {
        let content = fs::read_to_string(format!("{FIXTURES_ROOT}/conf.d/server.conf")).unwrap();
        let blocks = parse_server_blocks(&content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].names, vec!["localhost"]);
    }

    #[test]
    fn parse_server_blocks_reads_multiple_server_names() {
        let content =
            fs::read_to_string(format!("{FIXTURES_ROOT}/sites-enabled/example.com")).unwrap();
        let blocks = parse_server_blocks(&content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].names, vec!["example.com", "www.example.com"]);
    }

    #[test]
    fn apply_block_inserts_and_is_idempotent() {
        let content = "server {\n    listen 80;\n}\n".to_string();
        let block = parse_server_blocks(&content).remove(0);

        let once = apply_block(&content, &block, &cfg(&["BadBot|EvilCrawler"]));
        assert!(once.contains(BLOCK_BEGIN));
        assert!(once.contains("BadBot|EvilCrawler"), "once was:\n{once}");
        assert!(once.contains("return 403;"), "once was:\n{once}");

        let block_again = parse_server_blocks(&once).remove(0);
        let twice = apply_block(&once, &block_again, &cfg(&["BadBot|EvilCrawler"]));
        assert_eq!(once, twice);
    }

    #[test]
    fn apply_block_replaces_pattern_on_update() {
        let content = "server {\n    listen 80;\n}\n".to_string();
        let block = parse_server_blocks(&content).remove(0);
        let first = apply_block(&content, &block, &cfg(&["OldBot"]));

        let block_again = parse_server_blocks(&first).remove(0);
        let second = apply_block(&first, &block_again, &cfg(&["NewBot"]));

        assert!(!second.contains("OldBot"), "second was:\n{second}");
        assert!(second.contains("NewBot"), "second was:\n{second}");
        assert_eq!(second.matches(BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn apply_block_with_no_patterns_removes_existing_block() {
        let content = "server {\n    listen 80;\n}\n".to_string();
        let block = parse_server_blocks(&content).remove(0);
        let with_block = apply_block(&content, &block, &cfg(&["BadBot"]));

        let block_again = parse_server_blocks(&with_block).remove(0);
        let removed = apply_block(&with_block, &block_again, &BlockConfig::default());

        assert!(!removed.contains(BLOCK_BEGIN));
        assert!(removed.contains("listen 80;"), "removed was:\n{removed}");
    }

    #[test]
    fn apply_block_only_touches_the_targeted_server_block() {
        let content =
            "server {\n    server_name a.example;\n}\nserver {\n    server_name b.example;\n}\n";
        let blocks = parse_server_blocks(content);
        assert_eq!(blocks.len(), 2);

        let target = blocks
            .iter()
            .find(|b| b.names == vec!["b.example"])
            .unwrap();
        let updated = apply_block(content, target, &cfg(&["BadBot"]));

        let updated_blocks = parse_server_blocks(&updated);
        let a_region = &updated[updated_blocks[0].open..updated_blocks[0].close];
        let b_region = &updated[updated_blocks[1].open..updated_blocks[1].close];
        assert!(!a_region.contains(BLOCK_BEGIN));
        assert!(b_region.contains(BLOCK_BEGIN));
    }

    #[test]
    fn apply_blocks_to_file_writes_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.com");
        fs::write(
            &path,
            fs::read_to_string(format!("{FIXTURES_ROOT}/sites-enabled/example.com")).unwrap(),
        )
        .unwrap();

        let config = cfg(&["BadBot", "EvilCrawler"]);
        let changed = apply_blocks_to_file(&path, dir.path(), &[], &config).unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains(BLOCK_BEGIN));
        assert!(
            written.contains("BadBot|EvilCrawler"),
            "written was:\n{written}"
        );

        let changed_again = apply_blocks_to_file(&path, dir.path(), &[], &config).unwrap();
        assert!(!changed_again);
    }

    #[test]
    fn apply_blocks_to_file_with_no_server_blocks_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nginx.conf");
        fs::write(&path, "events {}\nhttp {\n    include conf.d/*.conf;\n}\n").unwrap();

        let changed = apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["BadBot"])).unwrap();
        assert!(!changed);
        assert!(!fs::read_to_string(&path).unwrap().contains(BLOCK_BEGIN));
    }

    /// Regression test: a redirect-plus-main-site file has two `server`
    /// blocks sharing the same `server_name`. Both must get the rule, not
    /// just whichever one a name lookup happens to find first.
    #[test]
    fn apply_blocks_to_file_covers_every_block_with_the_same_server_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.com");
        fs::write(
            &path,
            "server {\n    listen 80;\n    server_name example.com;\n    return 301 https://$host$request_uri;\n}\n\
             server {\n    listen 443 ssl;\n    server_name example.com;\n    root /var/www;\n}\n",
        )
        .unwrap();

        let changed = apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["BadBot"])).unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(written.matches(BLOCK_BEGIN).count(), 2);

        let blocks = parse_server_blocks(&written);
        assert_eq!(blocks.len(), 2);
        for block in &blocks {
            let region = &written[block.open..block.close];
            assert!(region.contains(BLOCK_BEGIN));
        }
    }

    #[test]
    fn apply_blocks_to_file_applies_different_patterns_per_server_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi-site.conf");
        fs::write(
            &path,
            "server {\n    listen 80;\n    server_name a.example;\n}\n\
             server {\n    listen 80;\n    server_name b.example;\n}\n",
        )
        .unwrap();

        let site_configs = vec![
            ("a.example".to_string(), cfg(&["OnlyOnA"])),
            ("b.example".to_string(), cfg(&["OnlyOnB"])),
        ];
        let changed =
            apply_blocks_to_file(&path, dir.path(), &site_configs, &BlockConfig::default())
                .unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        let blocks = parse_server_blocks(&written);
        let a_region = &written[blocks[0].open..blocks[0].close];
        let b_region = &written[blocks[1].open..blocks[1].close];
        assert!(a_region.contains("OnlyOnA"), "a_region was:\n{a_region}");
        assert!(!a_region.contains("OnlyOnB"), "a_region was:\n{a_region}");
        assert!(b_region.contains("OnlyOnB"), "b_region was:\n{b_region}");
        assert!(!b_region.contains("OnlyOnA"), "b_region was:\n{b_region}");
    }

    #[test]
    fn apply_blocks_to_file_falls_back_to_default_patterns_for_an_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unscanned.conf");
        fs::write(
            &path,
            "server {\n    listen 80;\n    server_name unscanned.example;\n}\n",
        )
        .unwrap();

        // No entry for "unscanned.example" in site_patterns at all (as if
        // it was just discovered on disk but never scanned into the db).
        let changed =
            apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["GlobalDefaultBot"])).unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("GlobalDefaultBot"),
            "written was:\n{written}"
        );
    }

    #[test]
    fn site_apply_status_reports_not_found_for_a_missing_file() {
        let status = site_apply_status(
            Path::new("/nonexistent/does-not-exist.conf"),
            "example.com",
            &cfg(&["BadBot"]),
            no_trust_file(),
        );
        assert_eq!(status, SiteApplyStatus::NotFound);
    }

    #[test]
    fn site_apply_status_reports_not_found_when_the_server_name_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        let status = site_apply_status(&path, "b.example", &cfg(&["BadBot"]), no_trust_file());
        assert_eq!(status, SiteApplyStatus::NotFound);
    }

    #[test]
    fn site_apply_status_is_up_to_date_when_nothing_is_expected_and_nothing_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        let status =
            site_apply_status(&path, "a.example", &BlockConfig::default(), no_trust_file());
        assert_eq!(status, SiteApplyStatus::UpToDate);
    }

    #[test]
    fn site_apply_status_is_stale_when_a_rule_is_expected_but_not_yet_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        let status = site_apply_status(&path, "a.example", &cfg(&["BadBot"]), no_trust_file());
        assert_eq!(status, SiteApplyStatus::Stale);
    }

    #[test]
    fn site_apply_status_is_up_to_date_once_the_matching_rule_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot", "EvilBot"])).unwrap();

        let status = site_apply_status(
            &path,
            "a.example",
            &cfg(&["BadBot", "EvilBot"]),
            no_trust_file(),
        );
        assert_eq!(status, SiteApplyStatus::UpToDate);
    }

    /// Regression test: a hand-written user-agent regex condition elsewhere
    /// in the same server block, ahead of our sentinel, must not be
    /// mistaken for our own applied pattern.
    #[test]
    fn site_apply_status_ignores_a_hand_written_user_agent_check_ahead_of_the_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(
            &path,
            "server {\n    server_name a.example;\n    if ($http_user_agent ~* \"AdminBot\") { return 403; }\n}\n",
        )
        .unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();

        let status = site_apply_status(&path, "a.example", &cfg(&["BadBot"]), no_trust_file());
        assert_eq!(status, SiteApplyStatus::UpToDate);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("AdminBot"), "written was:\n{written}");
        assert!(written.contains("BadBot"), "written was:\n{written}");
    }

    #[test]
    fn site_apply_status_is_stale_when_the_applied_pattern_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["OldBot"])).unwrap();

        let status = site_apply_status(&path, "a.example", &cfg(&["NewBot"]), no_trust_file());
        assert_eq!(status, SiteApplyStatus::Stale);
    }

    #[test]
    fn apply_block_for_site_only_touches_the_named_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi-site.conf");
        fs::write(
            &path,
            "server {\n    listen 80;\n    server_name a.example;\n}\n\
             server {\n    listen 80;\n    server_name b.example;\n}\n",
        )
        .unwrap();

        let changed =
            apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["OnlyOnA"])).unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        let blocks = parse_server_blocks(&written);
        let a_region = &written[blocks[0].open..blocks[0].close];
        let b_region = &written[blocks[1].open..blocks[1].close];
        assert!(a_region.contains("OnlyOnA"), "a_region was:\n{a_region}");
        assert!(!b_region.contains(BLOCK_BEGIN));
    }

    #[test]
    fn apply_block_for_site_is_a_noop_once_already_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        assert!(apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap());
        assert!(!apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap());
    }

    #[test]
    fn apply_block_for_site_is_a_noop_for_an_unknown_server_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        let original = "server {\n    server_name a.example;\n}\n";
        fs::write(&path, original).unwrap();

        let changed =
            apply_block_for_site(&path, dir.path(), "unknown.example", &cfg(&["BadBot"])).unwrap();
        assert!(!changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    // ---- writing a regex into a quoted string, exactly ----

    /// Where NGINX's reader ends a double-quoted word that starts at
    /// `text[0]`: pass 1 of `ngx_conf_read_token`, where a backslash makes
    /// the next byte ordinary. The index of the closing quote.
    fn closing_quote(text: &str) -> Option<usize> {
        let bytes = text.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => return Some(i),
                _ => i += 1,
            }
        }
        None
    }

    /// Every string over the characters NGINX's reader treats specially,
    /// up to five long: written, it ends at its own closing quote, and
    /// reads back as itself. The whole of the claim `nginx_quoted` makes.
    /// (`r` is left out: NGINX treats it exactly as it does `t` and `n`.)
    #[test]
    fn every_regex_reaches_pcre_exactly_as_written() {
        let alphabet = ['a', '\\', '"', '\'', 't', 'n', '|'];
        let mut strings = vec![String::new()];
        for _ in 0..5 {
            let longer: Vec<String> = strings
                .iter()
                .filter(|s| s.chars().count() == strings.last().unwrap().chars().count())
                .flat_map(|s| alphabet.iter().map(move |c| format!("{s}{c}")))
                .collect();
            strings.extend(longer);
        }
        for regex in &strings {
            let quoted = nginx_quoted(regex);
            assert_eq!(
                closing_quote(&quoted),
                Some(quoted.len() - 1),
                "{regex:?} written as {quoted} does not end at its own quote"
            );
            assert_eq!(
                &nginx_unquoted(&quoted[1..quoted.len() - 1]),
                regex,
                "{regex:?} written as {quoted} reaches PCRE as something else"
            );
        }
    }

    /// The cases the review found, as written and as NGINX then reads them.
    #[test]
    fn a_backslash_is_doubled_only_where_nginx_would_read_an_escape() {
        for (regex, written) in [
            // A blocked `a\|`, escaped for PCRE: every backslash before
            // another is doubled, the last one is not.
            (r"a\\\|", r#""a\\\\\|""#),
            (r"x\\\*", r#""x\\\\\*""#),
            (r"Evil\\\(", r#""Evil\\\\\(""#),
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r#"a\\\""#, r#""a\\\\\\\"""#),
            (r"end\\", r#""end\\\\""#),
            (r"\\t and \\n", r#""\\\\t and \\\\n""#),
            (r"it\'s", r#""it\\'s""#),
            // Already right before, and byte for byte the same now.
            (r"1h4x\.com", r#""1h4x\.com""#),
            (r"ALittle\ Client", r#""ALittle\ Client""#),
            (r"^(\d+\.\d+\.\d+\.\d+|\[)", r#""^(\d+\.\d+\.\d+\.\d+|\[)""#),
            ("$foo};", r#""$foo};""#),
        ] {
            assert_eq!(nginx_quoted(regex), written, "{regex:?}");
            assert_eq!(nginx_unquoted(&written[1..written.len() - 1]), regex);
        }
    }

    #[test]
    fn usable_patterns_drops_only_the_unsafe_entries() {
        let patterns = vec![
            "GoodBot".to_string(),
            "Trailing\\".to_string(),
            "1h4x\\.com".to_string(),
            "Quoted\"Bot".to_string(),
        ];
        assert_eq!(
            usable_patterns(&patterns),
            ["GoodBot", "1h4x\\.com", "Quoted\"Bot"]
        );
    }

    #[test]
    fn usable_patterns_is_empty_when_every_pattern_is_unsafe() {
        let patterns = vec!["Trailing\\".to_string(), "Evil(".to_string()];
        assert!(usable_patterns(&patterns).is_empty());
    }

    /// Regression test for a real bug: a bot pattern ending in a backslash,
    /// if it happened to land last in the joined `|`-separated regex, wrote
    /// an NGINX config that failed `nginx -t` with "too long parameter,
    /// probably missing terminating \" character" — confirmed against a
    /// real `nginx -t` binary. `apply_blocks_to_file` must silently drop
    /// such a pattern rather than write it.
    #[test]
    fn apply_blocks_to_file_drops_a_pattern_with_a_trailing_backslash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.com");
        fs::write(
            &path,
            fs::read_to_string(format!("{FIXTURES_ROOT}/sites-enabled/example.com")).unwrap(),
        )
        .unwrap();

        let config = cfg(&["GoodBot", "TrailingBackslash\\"]);
        apply_blocks_to_file(&path, dir.path(), &[], &config).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("GoodBot"), "written was:\n{written}");
        assert!(
            !written.contains("TrailingBackslash"),
            "written was:\n{written}"
        );
        // The sentinel's own closing quote must be the last character
        // before the closing paren, i.e. immediately followed by `) {` —
        // not swallowed into an unterminated string.
        assert!(
            written.contains("~* \"GoodBot\") {"),
            "written was:\n{written}"
        );
    }

    #[test]
    fn chunk_patterns_keeps_everything_in_one_chunk_when_it_fits() {
        let chunks = chunk_patterns(&["AAA", "BBB", "CCC"], 2000);
        assert_eq!(chunks, vec!["AAA|BBB|CCC".to_string()]);
    }

    #[test]
    fn chunk_patterns_splits_only_between_patterns_once_the_limit_is_hit() {
        // Each pattern is 5 bytes; a limit of 11 fits exactly two plus
        // their separator (5 + 1 + 5 = 11) but not three.
        let patterns = ["AAAA0", "AAAA1", "AAAA2", "AAAA3"];
        let chunks = chunk_patterns(&patterns, 11);
        assert_eq!(
            chunks,
            vec!["AAAA0|AAAA1".to_string(), "AAAA2|AAAA3".to_string()]
        );
        // Rejoining every chunk with `|` must reconstruct the original,
        // order preserved.
        assert_eq!(chunks.join("|"), patterns.join("|"));
    }

    #[test]
    fn chunk_patterns_gives_an_oversized_pattern_its_own_chunk_rather_than_dropping_it() {
        let huge = "x".repeat(50);
        let chunks = chunk_patterns(&["AAA", &huge, "BBB"], 10);
        assert_eq!(chunks, ["AAA", huge.as_str(), "BBB"]);
    }

    /// The limit is NGINX's, on the bytes in the file, so a chunk is
    /// measured as written: `a\\` is three characters of regex and five
    /// of config.
    #[test]
    fn chunk_patterns_measures_each_chunk_as_written() {
        let patterns = [r"a\\x", r"b\\x", r"c\\x"];
        let chunks = chunk_patterns(&patterns, 13);
        assert_eq!(chunks, [r"a\\x|b\\x", r"c\\x"]);
        for chunk in &chunks {
            assert!(nginx_quoted(chunk).len() - 2 <= 13, "{chunk} is too long");
        }
    }

    /// Regression test for the real bug: NGINX's config parser rejects any
    /// single quoted parameter beyond roughly 4100 bytes with `too long
    /// parameter, probably missing terminating """ character` — confirmed
    /// against a real `nginx -t`, and easily reached by a realistic
    /// default bot list (`nginx-bad-bots` alone is ~700 entries). A single
    /// giant `if` block must not be written; `block_text` should split into
    /// several instead, each safely under the limit.
    #[test]
    fn apply_blocks_to_file_splits_a_long_pattern_list_into_multiple_if_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.com");
        fs::write(
            &path,
            fs::read_to_string(format!("{FIXTURES_ROOT}/sites-enabled/example.com")).unwrap(),
        )
        .unwrap();

        // Comfortably more than MAX_PATTERN_CHUNK_LEN once joined with `|`.
        let patterns: Vec<String> = (0..500).map(|i| format!("BadBot{i}Agent")).collect();
        let config = BlockConfig::new(patterns, BlockResponse::Forbidden);
        apply_blocks_to_file(&path, dir.path(), &[], &config).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        let if_count = written.matches("if ($http_user_agent").count();
        assert!(if_count > 1, "expected multiple if blocks, got {if_count}");
        for line in written.lines() {
            assert!(
                line.len() < MAX_PATTERN_CHUNK_LEN + 100,
                "line exceeds the safe chunk length: {} bytes",
                line.len()
            );
        }
        assert!(written.contains("BadBot0Agent"), "written was:\n{written}");
        assert!(
            written.contains("BadBot499Agent"),
            "written was:\n{written}"
        );
    }

    /// A chunked block (multiple `if` statements) must still be read back
    /// correctly by `site_apply_status` — otherwise every site with a long
    /// enough pattern list would permanently read `Stale` even right after
    /// applying, since `current_block_pattern` would only ever see the
    /// first chunk.
    #[test]
    fn site_apply_status_is_up_to_date_after_applying_a_chunked_pattern_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        let patterns: Vec<String> = (0..500).map(|i| format!("BadBot{i}Agent")).collect();
        let config = BlockConfig::new(patterns, BlockResponse::Forbidden);
        apply_block_for_site(&path, dir.path(), "a.example", &config).unwrap();

        let status = site_apply_status(&path, "a.example", &config, no_trust_file());
        assert_eq!(status, SiteApplyStatus::UpToDate);
    }

    // ---- writing through links ----

    /// A link planted where a generated file goes is replaced, not written
    /// through: root would otherwise put our text wherever it pointed.
    #[test]
    fn a_generated_file_replaces_a_planted_link_instead_of_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "precious\n").unwrap();
        let path = dir.path().join("stop-bots-trusted.conf");
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        write_planned_managed_files(&[(path.clone(), "ours\n".to_string())]).unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), "ours\n");
    }

    fn one_site() -> &'static str {
        "server {\n    server_name a.example;\n}\n"
    }

    #[test]
    fn a_site_file_linking_outside_the_root_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nginx");
        fs::create_dir(&root).unwrap();
        let victim = dir.path().join("elsewhere.conf");
        fs::write(&victim, one_site()).unwrap();
        let link = root.join("a.conf");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let err = apply_blocks_to_file(&link, &root, &[], &cfg(&["BadBot"])).unwrap_err();

        assert!(
            err.to_string().contains("outside the NGINX root"),
            "error was: {err}"
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), one_site());
    }

    /// `sites-enabled/x` linking to `../sites-available/x` is the normal
    /// Debian layout: written through, and still a link afterwards.
    #[test]
    fn a_site_file_linking_inside_the_root_is_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("sites-available")).unwrap();
        fs::create_dir(root.join("sites-enabled")).unwrap();
        let target = root.join("sites-available/a.conf");
        fs::write(&target, one_site()).unwrap();
        let link = root.join("sites-enabled/a.conf");
        std::os::unix::fs::symlink("../sites-available/a.conf", &link).unwrap();

        assert!(apply_blocks_to_file(&link, root, &[], &cfg(&["BadBot"])).unwrap());

        assert!(fs::read_to_string(&target).unwrap().contains("BadBot"));
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    }

    #[test]
    fn a_rewritten_site_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.conf");
        fs::write(&path, one_site()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["BadBot"])).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o640);
    }

    // ---- taking an apply back ----

    #[test]
    fn a_snapshot_puts_back_changed_files_and_removes_created_ones() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a.conf");
        let created = dir.path().join("stop-bots-limits.conf");
        fs::write(&existing, one_site()).unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o640)).unwrap();

        let snapshot = Snapshot::take([existing.clone(), created.clone()]).unwrap();
        fs::write(&existing, "broken").unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&created, "new").unwrap();
        snapshot.restore().unwrap();

        assert_eq!(fs::read_to_string(&existing).unwrap(), one_site());
        assert_eq!(
            fs::metadata(&existing).unwrap().permissions().mode() & 0o7777,
            0o640
        );
        assert!(!created.exists());
    }

    #[test]
    fn a_failed_test_puts_the_site_back_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.conf");
        fs::write(&path, one_site()).unwrap();
        let snapshot = Snapshot::of_apply(dir.path(), [], &[], [path.as_path()]).unwrap();
        apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["BadBot"])).unwrap();

        let err = test_or_restore(&snapshot, &commands_that(false)).unwrap_err();

        assert_eq!(fs::read_to_string(&path).unwrap(), one_site());
        assert!(
            format!("{err:#}").contains("put back"),
            "error was: {err:#}"
        );
    }

    // ---- chunking never splits a pattern ----

    /// Patterns whose `|`-join is exactly `len` bytes, so a test can put the
    /// next pattern right on a chunk boundary.
    fn filler(len: usize) -> Vec<String> {
        let mut patterns = Vec::new();
        let mut used = 0;
        while used + 14 + 20 < len {
            used += usize::from(!patterns.is_empty()) + 13;
            patterns.push(format!("FillerBot{:04}", patterns.len()));
        }
        patterns.push("P".repeat(len - used - 1));
        patterns
    }

    /// The quoted regex of every `$http_user_agent` test in `text`.
    fn user_agent_regexes(text: &str) -> Vec<&str> {
        text.split("if ($http_user_agent ~* \"")
            .skip(1)
            .map(|rest| &rest[..rest.find("\") {\n").expect("closing quote")])
            .collect()
    }

    /// The injection: a chunk that ended in the backslash of `\|` escaped
    /// NGINX's closing quote, and the next chunk was read as directives.
    #[test]
    fn a_chunk_boundary_never_lands_inside_an_escaped_pipe() {
        let mut patterns = filler(1994);
        patterns.push(r"Evil\|Bot".to_string());
        let text = block_text(&BlockConfig::new(patterns, BlockResponse::Forbidden)).unwrap();

        let regexes = user_agent_regexes(&text);
        assert!(regexes.len() > 1, "the test needs two chunks:\n{text}");
        for regex in &regexes {
            assert!(
                !regex.ends_with('\\'),
                "unsafe chunk ends {:?}",
                &regex[regex.len() - 12..]
            );
        }
        assert!(
            regexes.iter().any(|r| r.contains(r"Evil\|Bot")),
            "the pattern must be written whole:\n{text}"
        );
    }

    /// Not an injection, but the same cut: a group split across two `if`s
    /// is two regexes NGINX refuses to compile.
    #[test]
    fn a_chunk_boundary_never_lands_inside_a_group() {
        let mut patterns = filler(1990);
        patterns.push("(Alpha|Beta)Bot".to_string());
        let text = block_text(&BlockConfig::new(patterns, BlockResponse::Forbidden)).unwrap();

        let regexes = user_agent_regexes(&text);
        assert!(regexes.len() > 1, "the test needs two chunks:\n{text}");
        assert!(
            regexes.iter().any(|r| r.contains("(Alpha|Beta)Bot")),
            "the group must be written whole:\n{text}"
        );
    }

    #[test]
    fn chunks_hold_whole_patterns() {
        let patterns = ["AAAA0", r"A\|A1", "AAAA2"];
        assert_eq!(chunk_patterns(&patterns, 11), ["AAAA0|A\\|A1", "AAAA2"]);
    }

    // ---- patterns that match every visitor ----

    /// Each of these, joined into the block, turns every visitor of every
    /// site away — and `nginx -t` accepts every one of them.
    #[test]
    fn a_pattern_that_matches_everyone_is_never_written() {
        for (pattern, why) in [
            ("", "empty"),
            ("   ", "whitespace only"),
            ("|", "two empty alternatives"),
            ("|EvilBot", "a leading empty alternative"),
            ("EvilBot|", "a trailing empty alternative"),
            ("Evil||Bot", "an empty alternative in the middle"),
            ("(|Evil)Bot", "an empty alternative opening a group"),
            ("Evil(Bot|)", "an empty alternative closing a group"),
            (".*", "a wildcard"),
            (".", "any one character"),
            (".+", "one or more of anything"),
            ("^", "an anchor"),
            ("^.*$", "anchors and a wildcard"),
            ("ab", "two literal characters"),
            ("a.b", "two literal characters and a wildcard"),
            ("a?b?c?", "every literal optional"),
            ("(abc)?", "an optional group"),
            ("(abc)*", "a starred group"),
            ("[a-z]{3}", "classes are not literals"),
            ("Bot|.*", "one match-all alternative is enough"),
        ] {
            assert!(
                !is_usable_pattern(pattern),
                "{pattern:?} ({why}) must be dropped"
            );
            assert!(
                block_text(&cfg(&[pattern])).is_none(),
                "{pattern:?} ({why}) must not produce a block"
            );
        }
    }

    #[test]
    fn a_pattern_that_can_backtrack_catastrophically_is_never_written() {
        for pattern in [
            "EvilBot(a+)+",
            "EvilBot(.*)*",
            "EvilBot(x?){2,}",
            "Evil(Bot(a*))+",
        ] {
            assert!(!is_usable_pattern(pattern), "{pattern:?} must be dropped");
        }
    }

    #[test]
    fn a_pattern_this_tool_cannot_check_is_never_written() {
        for pattern in [
            "Evil(Bot",
            "EvilBot)",
            "[EvilBot",
            "(?=Evil)Bot",
            r"Evil\x41Bot",
            "*EvilBot",
            "Evil\nBot",
            "Evil\tBot",
        ] {
            assert!(!is_usable_pattern(pattern), "{pattern:?} must be dropped");
        }
        assert!(!is_usable_pattern(&"E".repeat(MAX_PATTERN_LEN + 1)));
    }

    /// Shapes the real upstream lists use, which must all survive. Checked
    /// once by hand against all 1,675 entries of the three live lists
    /// (September 2026): none is dropped.
    #[test]
    fn real_upstream_pattern_shapes_are_kept() {
        for pattern in [
            "FDM",
            "Y!J",
            r"Googlebot\/",
            r"1h4x\.com",
            r"ALittle\ Client",
            "Qwant(?:ify|bot)",
            "[cC]laude(?:[bB]ot|-[Ww]eb)",
            "i[Tt][Mm][Ss]",
            r"Datadog(?:\/{0,1}Synthetics| Synthetic)",
            r"^Mozilla\/5\.0 \(compatible; Evil\)$",
            r"Chrome\/\d+ Evil",
            "ChatGPT Agent",
            "EvilBot(ab)+",
            "x server {",
        ] {
            assert!(is_usable_pattern(pattern), "{pattern:?} must be kept");
        }
    }

    // ---- re-reading what this tool wrote: quotes, comments, braces ----

    /// Patterns NGINX reads as plain text inside the quoted regex, but
    /// that a quote-blind reader takes for a comment, a brace, a new
    /// `server` block or one of this tool's own markers.
    fn awkward_patterns() -> Vec<String> {
        vec![
            "Evil#Bot".to_string(),
            "Evil}Bot".to_string(),
            "x server {".to_string(),
            crate::db::escape_regex_literal("x # END stop-bots"),
            crate::db::escape_regex_literal(BLOCK_BEGIN),
        ]
    }

    /// Applies `config` to a one-site file three times and returns what
    /// each run left on disk.
    fn apply_three_times(original: &str, config: &BlockConfig) -> [String; 3] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, original).unwrap();
        std::array::from_fn(|_| {
            apply_blocks_to_file(&path, dir.path(), &[], config).unwrap();
            fs::read_to_string(&path).unwrap()
        })
    }

    #[test]
    fn reapplying_is_idempotent_whatever_a_pattern_contains() {
        let site = "server {\n    listen 80;\n    server_name a.example;\n}\n";
        for pattern in awkward_patterns() {
            let [first, second, third] = apply_three_times(site, &cfg(&["GoodBot", &pattern]));
            assert_eq!(
                second, first,
                "a second run changed the file for {pattern:?}"
            );
            assert_eq!(third, first, "a third run changed the file for {pattern:?}");
            assert_eq!(
                first.matches(&pattern).count(),
                1,
                "{pattern:?} must be written exactly once:\n{first}"
            );
            assert_eq!(
                parse_server_blocks(&first).len(),
                1,
                "{pattern:?} must not read as a second server block:\n{first}"
            );
        }
    }

    #[test]
    fn every_awkward_pattern_at_once_is_still_idempotent() {
        let site = "server {\n    listen 80;\n    server_name a.example;\n}\n";
        let patterns = awkward_patterns();
        let config = BlockConfig::new(patterns.clone(), BlockResponse::Forbidden);
        let [first, second, third] = apply_three_times(site, &config);
        assert_eq!(second, first);
        assert_eq!(third, first);
        assert_eq!(first.matches("return 403;").count(), 1, "was:\n{first}");
    }

    /// An operator's own quoted `#` and braces are no more a comment or a
    /// block than ours are.
    #[test]
    fn an_operators_quoted_hash_and_braces_survive_reapplying() {
        let site = "server {\n    listen 80;\n    server_name a.example;\n    \
                    add_header X-Note \"see #docs {here}\";\n    \
                    add_header X-Other 'it''s #fine';\n}\n";
        let [first, second, _] = apply_three_times(site, &cfg(&["BadBot"]));
        assert_eq!(second, first);
        assert!(
            first.contains("add_header X-Note \"see #docs {here}\";"),
            "was:\n{first}"
        );
        assert_eq!(parse_server_blocks(&first)[0].names, ["a.example"]);
    }

    #[test]
    fn a_hash_inside_a_bare_word_is_not_a_comment() {
        let blocks = parse_server_blocks("server {\n    server_name a#b.example;\n}\n");
        assert_eq!(blocks[0].names, ["a#b.example"]);
    }

    #[test]
    fn a_brace_inside_a_quoted_word_is_not_structure() {
        let content = "server {\n    server_name a.example;\n    add_header X \"}\";\n}\n\
                       server {\n    server_name b.example;\n}\n";
        let names: Vec<_> = parse_server_blocks(content)
            .into_iter()
            .map(|b| b.names)
            .collect();
        assert_eq!(names, [["a.example"], ["b.example"]]);
    }

    // ---- BlockResponse (403 vs 444) ----

    fn cfg_444(patterns: &[&str]) -> BlockConfig {
        BlockConfig::new(
            patterns.iter().map(|p| p.to_string()).collect(),
            BlockResponse::Close,
        )
    }

    #[test]
    fn block_text_renders_the_configured_response_code() {
        let forbidden = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(
            forbidden.contains("return 403;"),
            "forbidden was:\n{forbidden}"
        );
        assert!(
            !forbidden.contains("return 444;"),
            "forbidden was:\n{forbidden}"
        );

        let close = block_text(&cfg_444(&["BadBot"])).unwrap();
        assert!(close.contains("return 444;"), "close was:\n{close}");
        assert!(!close.contains("return 403;"), "close was:\n{close}");
    }

    #[test]
    fn block_text_is_none_when_there_is_nothing_to_block() {
        assert!(block_text(&BlockConfig::default()).is_none());
        // Every pattern rejected by `pattern_problem` is the same case as
        // an empty list: nothing safe left to write.
        assert!(block_text(&cfg(&["Trailing\\"])).is_none());
    }

    #[test]
    fn a_chunked_pattern_list_renders_the_response_code_in_every_chunk() {
        let patterns: Vec<String> = (0..500).map(|i| format!("BadBot{i}Agent")).collect();
        let config = BlockConfig::new(patterns, BlockResponse::Close);
        let text = block_text(&config).unwrap();

        let if_count = text.matches("if ($http_user_agent").count();
        assert!(if_count > 1, "expected chunking, got {if_count} if(s)");
        assert_eq!(text.matches("return 444;").count(), if_count);
    }

    /// The whole point of comparing rendered text rather than an extracted
    /// pattern: the patterns are identical here, only the response code
    /// differs, and the old pattern-only check reported this as up to date.
    #[test]
    fn site_apply_status_is_stale_when_only_the_response_code_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();

        assert_eq!(
            site_apply_status(&path, "a.example", &cfg(&["BadBot"]), no_trust_file()),
            SiteApplyStatus::UpToDate
        );
        assert_eq!(
            site_apply_status(&path, "a.example", &cfg_444(&["BadBot"]), no_trust_file()),
            SiteApplyStatus::Stale
        );
    }

    #[test]
    fn re_applying_with_a_new_response_code_rewrites_the_block_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();

        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();
        let changed =
            apply_block_for_site(&path, dir.path(), "a.example", &cfg_444(&["BadBot"])).unwrap();
        assert!(changed);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("return 444;"), "written was:\n{written}");
        assert!(!written.contains("return 403;"), "written was:\n{written}");
        // Still exactly one sentinel block, not a second one appended.
        assert_eq!(written.matches(BLOCK_BEGIN).count(), 1);
        assert_eq!(
            site_apply_status(&path, "a.example", &cfg_444(&["BadBot"]), no_trust_file()),
            SiteApplyStatus::UpToDate
        );
    }

    // ---- robots.txt generation ----

    fn cfg_robots(patterns: &[&str]) -> BlockConfig {
        BlockConfig {
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            response: BlockResponse::Forbidden,
            serve_robots_txt: true,
            rate_limit_burst: None,
            exempt_paths: Vec::new(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        }
    }

    #[test]
    fn block_text_emits_a_robots_location_only_when_enabled() {
        let without = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(!without.contains("/robots.txt"), "without was:\n{without}");

        let with = block_text(&cfg_robots(&["BadBot"])).unwrap();
        assert!(with.contains("location = /robots.txt"), "with was:\n{with}");
        assert!(
            with.contains("default_type text/plain;"),
            "with was:\n{with}"
        );
        // The body is aliased, never inlined — see `serve_robots_txt`.
        assert!(with.contains("alias "), "with was:\n{with}");
        assert!(!with.contains("User-agent:"), "with was:\n{with}");
    }

    /// Serving robots.txt is reason enough to keep a sentinel block even
    /// with nothing to block, otherwise allowing every bot would silently
    /// delete the block that serves it.
    #[test]
    fn a_block_with_no_patterns_still_renders_when_robots_txt_is_on() {
        let config = BlockConfig {
            patterns: Vec::new(),
            response: BlockResponse::Forbidden,
            serve_robots_txt: true,
            rate_limit_burst: None,
            exempt_paths: Vec::new(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();
        assert!(text.contains("location = /robots.txt"), "text was:\n{text}");
        assert!(!text.contains("if ($http_user_agent"), "text was:\n{text}");
    }

    #[test]
    fn site_apply_status_is_stale_when_only_robots_txt_is_toggled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();

        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_robots(&["BadBot"]),
                no_trust_file()
            ),
            SiteApplyStatus::Stale
        );
    }

    #[test]
    fn is_robots_token_rejects_anything_that_is_not_a_plain_token() {
        assert!(is_robots_token("GPTBot"));
        assert!(is_robots_token("Mozilla/5.0"));
        assert!(is_robots_token("ai-bot_2.0"));
        assert!(!is_robots_token(""));
        assert!(!is_robots_token("Some Crawler"));
        assert!(!is_robots_token("Bad|Bot"));
        assert!(!is_robots_token("(compatible;"));
        assert!(!is_robots_token(&"x".repeat(65)));
    }

    fn seed_bot(db: &crate::db::Db, slug: &str, name: &str, is_ai: bool) {
        db.upsert_source(&crate::db::Source {
            id: "s".to_string(),
            name: "s".to_string(),
            url: "https://example.invalid/".to_string(),
            last_fetched_at: None,
            bot_count: 0,
        })
        .unwrap();
        db.upsert_bot(&crate::db::NewBot {
            slug: slug.to_string(),
            name: name.to_string(),
            is_ai,
            is_search_engine: false,
            is_scanner: false,
            user_agent_pattern: name.to_string(),
            source_id: "s".to_string(),
        })
        .unwrap();
    }

    #[test]
    fn robots_txt_body_groups_blocked_bots_under_one_disallow() {
        let db = crate::db::Db::open_in_memory().unwrap();
        // AI is blocked by default; Search is not.
        seed_bot(&db, "gptbot", "GPTBot", true);
        seed_bot(&db, "ccbot", "CCBot", true);

        let body = robots_txt_body(&db).unwrap();

        assert!(body.contains("User-agent: GPTBot\n"), "body was:\n{body}");
        assert!(body.contains("User-agent: CCBot\n"), "body was:\n{body}");
        // One shared Disallow: / for the whole group, not one per bot.
        assert_eq!(body.matches("Disallow: /\n").count(), 1);
    }

    #[test]
    fn robots_txt_body_skips_names_that_are_not_usable_tokens() {
        let db = crate::db::Db::open_in_memory().unwrap();
        seed_bot(&db, "spacey", "Some AI Crawler", true);
        let body = robots_txt_body(&db).unwrap();
        assert!(!body.contains("Some AI Crawler"), "body was:\n{body}");
    }

    #[test]
    fn robots_txt_body_omits_bots_that_are_not_blocked() {
        let db = crate::db::Db::open_in_memory().unwrap();
        seed_bot(&db, "gptbot", "GPTBot", true);
        db.set_bot_status("gptbot", crate::db::BotStatus::Allowed)
            .unwrap();

        let body = robots_txt_body(&db).unwrap();
        assert!(!body.contains("GPTBot"), "body was:\n{body}");
    }

    /// The trap path is published whether or not the honeypot detector is
    /// switched on — publishing is what creates the trap; the detector
    /// only decides whether hits are acted on.
    #[test]
    fn robots_txt_body_always_publishes_the_honeypot_path() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let body = robots_txt_body(&db).unwrap();
        assert!(body.contains(&format!(
            "Disallow: {}",
            crate::protection::HONEYPOT_PATH_DEFAULT
        )));
    }

    /// With nothing blocked the file must still be a meaningful
    /// robots.txt, not an empty one that reads as broken.
    #[test]
    fn robots_txt_body_is_valid_with_nothing_blocked() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let body = robots_txt_body(&db).unwrap();
        assert!(body.contains("User-agent: *"), "body was:\n{body}");
        assert!(body.contains("Disallow:"), "body was:\n{body}");
    }

    #[test]
    fn write_managed_files_writes_then_removes_the_robots_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("robots.txt");
        let db = crate::db::Db::open_in_memory().unwrap();

        // Exercised through the same body/enable logic the real path uses,
        // but against a temp file: `write_managed_files` resolves its
        // location from the environment, which unit tests running in one
        // shared process can't safely set.
        db.set_serve_robots_txt(true).unwrap();
        fs::write(&path, robots_txt_body(&db).unwrap()).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("User-agent"));

        // Removal is idempotent: a missing file is success, not an error.
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            fs::remove_file(&path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        ));
    }

    // ---- rate limiting ----

    fn cfg_rate(burst: u32) -> BlockConfig {
        BlockConfig {
            patterns: vec!["BadBot".to_string()],
            response: BlockResponse::Forbidden,
            serve_robots_txt: false,
            rate_limit_burst: Some(burst),
            exempt_paths: Vec::new(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        }
    }

    #[test]
    fn block_text_emits_limit_req_only_when_enabled() {
        let without = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(!without.contains("limit_req"), "without was:\n{without}");

        let with = block_text(&cfg_rate(20)).unwrap();
        assert!(
            with.contains("limit_req zone=stop_bots burst=20 nodelay;"),
            "with was:\n{with}"
        );
        assert!(with.contains("limit_req_status 429;"), "with was:\n{with}");
    }

    /// The zone name in the server-level directive and the one in the
    /// generated http-level file must be identical — a mismatch makes
    /// NGINX refuse to load with "unknown limit_req_zone".
    #[test]
    fn the_limit_req_zone_name_matches_between_the_block_and_the_conf_file() {
        let block = block_text(&cfg_rate(20)).unwrap();
        let conf = rate_limit_conf_body(10, 10);
        assert!(block.contains(&format!("zone={RATE_LIMIT_ZONE} ")));
        assert!(conf.contains(&format!("zone={RATE_LIMIT_ZONE}:")));
    }

    #[test]
    fn rate_limit_conf_body_uses_the_binary_address_key_and_given_rate() {
        let conf = rate_limit_conf_body(30, 16);
        assert!(
            conf.contains("limit_req_zone $binary_remote_addr"),
            "conf was:\n{conf}"
        );
        assert!(conf.contains("zone=stop_bots:16m"), "conf was:\n{conf}");
        assert!(conf.contains("rate=30r/s"), "conf was:\n{conf}");
    }

    /// Rate limiting alone is reason enough to keep a sentinel block, the
    /// same as robots.txt: otherwise allowing every bot would delete the
    /// block carrying the `limit_req`.
    #[test]
    fn a_block_with_no_patterns_still_renders_when_rate_limiting_is_on() {
        let config = BlockConfig {
            patterns: Vec::new(),
            response: BlockResponse::Forbidden,
            serve_robots_txt: false,
            rate_limit_burst: Some(5),
            exempt_paths: Vec::new(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();
        assert!(text.contains("limit_req"), "text was:\n{text}");
        assert!(!text.contains("if ($http_user_agent"), "text was:\n{text}");
    }

    #[test]
    fn site_apply_status_is_stale_when_only_the_rate_limit_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg_rate(20)).unwrap();

        assert_eq!(
            site_apply_status(&path, "a.example", &cfg_rate(20), no_trust_file()),
            SiteApplyStatus::UpToDate
        );
        assert_eq!(
            site_apply_status(&path, "a.example", &cfg_rate(50), no_trust_file()),
            SiteApplyStatus::Stale
        );
        assert_eq!(
            site_apply_status(&path, "a.example", &cfg(&["BadBot"]), no_trust_file()),
            SiteApplyStatus::Stale
        );
    }

    #[test]
    fn remove_managed_treats_a_missing_file_as_success() {
        let dir = tempfile::tempdir().unwrap();
        remove_managed(&held(&dir.path().join("never-existed.conf"))).unwrap();
    }

    #[test]
    fn write_managed_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/stop-bots-limits.conf");
        write_managed(&path, "x\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
    }

    // ---- per-site path exemptions ----

    fn cfg_exempt(patterns: &[&str], paths: &[&str]) -> BlockConfig {
        BlockConfig {
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            response: BlockResponse::Forbidden,
            serve_robots_txt: false,
            rate_limit_burst: None,
            exempt_paths: paths.iter().map(|p| p.to_string()).collect(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        }
    }

    /// With no exemptions the block keeps its original direct-`return`
    /// shape, so upgrading doesn't rewrite every already-applied site.
    #[test]
    fn no_exemptions_keeps_the_direct_return_form() {
        let text = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(
            text.contains("if ($http_user_agent ~* \"BadBot\") {"),
            "text was:\n{text}"
        );
        assert!(text.contains("return 403;"), "text was:\n{text}");
        assert!(!text.contains("$stop_bots_block"), "text was:\n{text}");
    }

    #[test]
    fn exemptions_switch_the_block_to_the_flag_form_in_the_right_order() {
        let text = block_text(&cfg_exempt(&["BadBot"], &["/blog"])).unwrap();

        let set_zero = text.find("set $stop_bots_block 0;").unwrap();
        let set_one = text.find("set $stop_bots_block 1;").unwrap();
        let clear = text.rfind("set $stop_bots_block 0;").unwrap();
        let act = text.find("if ($stop_bots_block) {").unwrap();

        // Order is the whole mechanism: initialise, set on a UA match,
        // clear for exempt paths, and only then act.
        assert!(set_zero < set_one, "initialise before the UA match");
        assert!(
            set_one < clear,
            "the exemption must clear *after* the match"
        );
        assert!(clear < act, "act last");
        assert!(
            text.contains("if ($uri ~* \"^(/blog)\")"),
            "text was:\n{text}"
        );
        assert!(text.contains("return 403;"), "text was:\n{text}");
    }

    /// `$request_uri` is the raw request line. `GET
    /// /blog/../wp-login.php` starts with `/blog` there, while NGINX and the
    /// application behind it serve `/wp-login.php` — a blocked bot reaching
    /// any path by prefixing an exempt one. `$uri` is already resolved.
    #[test]
    fn a_plain_exemption_matches_the_resolved_path_not_the_raw_one() {
        let text = block_text(&cfg_exempt(&["BadBot"], &["/blog"])).unwrap();
        assert!(
            text.contains("if ($uri ~* \"^(/blog)\")"),
            "text was:\n{text}"
        );
        assert!(!text.contains("$request_uri"), "text was:\n{text}");
    }

    /// `/robots.txt` is one file, not a prefix: `/robots.txt/../x` and
    /// `/robots.txtanything` are not it.
    #[test]
    fn the_robots_txt_exemption_is_exact() {
        let config = BlockConfig {
            serve_robots_txt: true,
            ..cfg_exempt(&["BadBot"], &["/blog"])
        };
        let text = block_text(&config).unwrap();
        assert!(
            text.contains("if ($uri ~* \"^(/blog|/robots\\.txt$)\")"),
            "text was:\n{text}"
        );
    }

    #[test]
    fn the_exemption_regex_is_anchored_and_alternated() {
        assert_eq!(
            exemption_regex(&["/blog".to_string(), "/feed".to_string()], &[]),
            Some("^(/blog|/feed)".to_string())
        );
    }

    /// Unescaped regex metacharacters in a literal URL prefix would widen
    /// the exemption — which fails *open*, unlike a too-narrow pattern.
    #[test]
    fn the_exemption_regex_escapes_metacharacters() {
        let regex = exemption_regex(&["/a.b?c".to_string()], &[]).unwrap();
        assert!(regex.contains("\\."), "regex was: {regex}");
        assert!(regex.contains("\\?"), "regex was: {regex}");
    }

    #[test]
    fn the_exemption_regex_drops_unusable_paths() {
        // Not anchored at the start: could never match, so it's dropped
        // rather than silently widening or narrowing anything.
        assert_eq!(exemption_regex(&["blog".to_string()], &[]), None);
        // Would terminate the quoted config string.
        assert_eq!(exemption_regex(&["/a\"b".to_string()], &[]), None);
        assert_eq!(exemption_regex(&[], &[]), None);
    }

    // ---- agent exemptions ----

    fn agent(user_agent: &str, path: &str) -> crate::db::AgentExemption {
        crate::db::AgentExemption {
            path: path.to_string(),
            user_agent: user_agent.to_string(),
        }
    }

    /// `patterns` blocked, with `exemptions` as the site's agent
    /// exemptions — sorted by user agent, as the database returns them.
    fn cfg_agent(patterns: &[&str], exemptions: &[(&str, &str)]) -> BlockConfig {
        BlockConfig {
            agent_exemptions: exemptions
                .iter()
                .map(|(ua, path)| agent(ua, path))
                .collect(),
            ..cfg(patterns)
        }
    }

    /// The Boox reader and the Jellyfin player: two apps a bot list
    /// catches by their HTTP library, each let through on its own paths,
    /// beside a plain exemption.
    #[test]
    fn an_agent_exemption_block_matches_the_golden() {
        let config = BlockConfig {
            exempt_paths: vec!["/ocs/v2.php/cloud/capabilities".to_string()],
            ..cfg_agent(
                &["okhttp", "BadBot"],
                &[
                    ("Jellyfin Android", "/videos/"),
                    ("okhttp", "/remote.php/dav/"),
                    ("okhttp", "/remote.php/webdav/"),
                ],
            )
        };
        crate::golden::assert_golden(
            "nginx-block-agent-exemptions.conf",
            &block_text(&config).unwrap(),
        );
    }

    #[test]
    fn an_agent_exemption_forces_the_flag_form_and_clears_after_every_set() {
        let text = block_text(&cfg_agent(&["okhttp"], &[("okhttp", "/remote.php/dav/")])).unwrap();

        let set_one = text.find("set $stop_bots_block 1;").unwrap();
        let clear = text.rfind("set $stop_bots_block 0;").unwrap();
        let act = text.find("if ($stop_bots_block) {").unwrap();
        assert!(set_one < clear, "the clear must follow the match:\n{text}");
        assert!(clear < act, "act last:\n{text}");
    }

    /// Without the reset, a variable left holding the path by one group's
    /// user agent would let the next group's paths through for a client
    /// that never matched that group.
    #[test]
    fn every_agent_group_starts_by_resetting_the_variable() {
        let text = block_text(&cfg_agent(
            &["BadBot"],
            &[("Alpha", "/a/"), ("Beta", "/b/")],
        ))
        .unwrap();
        let resets: Vec<usize> = text
            .match_indices("set $stop_bots_exempt \"\";")
            .map(|(i, _)| i)
            .collect();
        let alpha = text.find("~* \"Alpha\"").unwrap();
        let beta = text.find("~* \"Beta\"").unwrap();
        assert_eq!(resets.len(), 2, "one reset per user agent:\n{text}");
        assert!(
            resets[0] < alpha && alpha < resets[1] && resets[1] < beta,
            "each group's reset must precede its own match:\n{text}"
        );
    }

    /// `$request_uri` is the raw request line: `/remote.php/dav/../../x`
    /// starts with the exempt prefix there, and the application resolves it
    /// to `/x`. `$uri` has already been resolved by NGINX.
    #[test]
    fn an_agent_exemption_matches_the_resolved_path_not_the_raw_one() {
        let text = block_text(&cfg_agent(&["okhttp"], &[("okhttp", "/remote.php/dav/")])).unwrap();
        assert!(
            text.contains("set $stop_bots_exempt $uri;"),
            "text was:\n{text}"
        );
        assert!(!text.contains("$request_uri"), "text was:\n{text}");
    }

    #[test]
    fn an_agent_exemption_escapes_the_user_agent_and_the_paths() {
        let text = block_text(&cfg_agent(
            &["okhttp"],
            &[("okhttp/4.10.0", "/remote.php/dav/")],
        ))
        .unwrap();
        for expected in [
            "if ($http_user_agent ~* \"okhttp/4\\.10\\.0\")",
            "if ($stop_bots_exempt ~* \"^(/remote\\.php/dav/)\")",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected}; text was:\n{text}"
            );
        }
    }

    /// Narrower than a plain exemption, so with nothing blocking there is
    /// nothing for them to do — and, unlike a plain one, they don't even
    /// force the flag form beside robots.txt.
    #[test]
    fn agent_exemptions_alone_write_nothing() {
        let exemptions = &[("okhttp", "/remote.php/dav/")];
        assert!(block_text(&cfg_agent(&[], exemptions)).is_none());

        let robots_only = BlockConfig {
            serve_robots_txt: true,
            ..cfg_agent(&[], exemptions)
        };
        let text = block_text(&robots_only).unwrap();
        assert!(!text.contains("stop_bots_exempt"), "text was:\n{text}");
    }

    /// Each would either break the quoted config string or never match —
    /// and a group left with no usable path is dropped whole, so the block
    /// falls back to the direct-`return` form rather than an empty clear.
    #[test]
    fn unusable_agent_exemptions_are_dropped() {
        for (description, user_agent, path) in [
            ("a quote in the user agent", "ok\"http", "/dav/"),
            ("a control character", "ok\thttp", "/dav/"),
            ("an empty user agent", "", "/dav/"),
            ("a path with no leading slash", "okhttp", "dav/"),
            ("a quote in the path", "okhttp", "/da\"v/"),
        ] {
            let text = block_text(&cfg_agent(&["okhttp"], &[(user_agent, path)])).unwrap();
            assert!(
                !text.contains("stop_bots_exempt") && text.contains("return 403;"),
                "{description} should have been dropped; text was:\n{text}"
            );
        }
    }

    #[test]
    fn site_apply_status_is_stale_when_only_an_agent_exemption_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["okhttp"])).unwrap();
        let exempted = cfg_agent(&["okhttp"], &[("okhttp", "/remote.php/dav/")]);

        let before = site_apply_status(&path, "a.example", &exempted, no_trust_file());
        apply_block_for_site(&path, dir.path(), "a.example", &exempted).unwrap();
        let after = site_apply_status(&path, "a.example", &exempted, no_trust_file());

        assert_eq!(
            (before, after),
            (SiteApplyStatus::Stale, SiteApplyStatus::UpToDate)
        );
    }

    #[test]
    fn a_sites_block_config_carries_its_agent_exemptions() {
        let db = crate::db::Db::open_in_memory().unwrap();
        db.upsert_site("a.example", "/etc/nginx/a.conf").unwrap();
        let site_id = db.list_sites().unwrap()[0].id;
        db.add_site_agent_exemption(site_id, "/remote.php/dav/", "okhttp")
            .unwrap();

        let config = block_config_for_site(&db, site_id).unwrap();
        assert_eq!(
            config.agent_exemptions,
            vec![agent("okhttp", "/remote.php/dav/")]
        );
    }

    /// The bug this guards against: server-level `if`/`return` run before
    /// location selection, so without an implicit `/robots.txt` exemption
    /// the generated robots.txt would be 403'd for precisely the user
    /// agents it names.
    #[test]
    fn serving_robots_txt_exempts_robots_txt_from_the_block() {
        let config = BlockConfig {
            patterns: vec!["BadBot".to_string()],
            response: BlockResponse::Forbidden,
            serve_robots_txt: true,
            rate_limit_burst: None,
            exempt_paths: Vec::new(),
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();

        // The presence of robots.txt alone forces the flag form, because
        // the direct-`return` form has nowhere to put an exemption.
        assert!(text.contains("$stop_bots_block"), "text was:\n{text}");
        assert!(text.contains("/robots\\.txt"), "text was:\n{text}");
        // And the clear still happens after the set, before the act.
        let set_one = text.find("set $stop_bots_block 1;").unwrap();
        let clear = text.rfind("set $stop_bots_block 0;").unwrap();
        let act = text.find("if ($stop_bots_block) {").unwrap();
        assert!(set_one < clear && clear < act);
    }

    #[test]
    fn robots_txt_exemption_combines_with_configured_ones() {
        let config = BlockConfig {
            patterns: vec!["BadBot".to_string()],
            response: BlockResponse::Forbidden,
            serve_robots_txt: true,
            rate_limit_burst: None,
            exempt_paths: vec!["/blog".to_string()],
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();
        assert!(text.contains("/blog"), "text was:\n{text}");
        assert!(text.contains("/robots\\.txt"), "text was:\n{text}");
    }

    /// Not serving robots.txt means no implicit exemption, so a site with
    /// no configured exemptions keeps the direct-`return` form.
    #[test]
    fn no_robots_txt_means_no_implicit_exemption() {
        let text = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(!text.contains("$stop_bots_block"), "text was:\n{text}");
        assert!(!text.contains("robots"), "text was:\n{text}");
    }

    #[test]
    fn a_chunked_pattern_list_sets_the_flag_in_every_chunk() {
        let patterns: Vec<String> = (0..500).map(|i| format!("BadBot{i}Agent")).collect();
        let config = BlockConfig {
            patterns,
            response: BlockResponse::Forbidden,
            serve_robots_txt: false,
            rate_limit_burst: None,
            exempt_paths: vec!["/blog".to_string()],
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();

        let ifs = text.matches("if ($http_user_agent").count();
        assert!(ifs > 1, "expected chunking, got {ifs}");
        // Every chunk sets the flag; exactly one clears it and one acts.
        assert_eq!(text.matches("set $stop_bots_block 1;").count(), ifs);
        assert_eq!(text.matches("if ($uri ").count(), 1);
        assert_eq!(text.matches("if ($stop_bots_block) {").count(), 1);
    }

    #[test]
    fn site_apply_status_is_stale_when_only_an_exemption_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();

        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_exempt(&["BadBot"], &["/blog"]),
                no_trust_file()
            ),
            SiteApplyStatus::Stale
        );

        apply_block_for_site(
            &path,
            dir.path(),
            "a.example",
            &cfg_exempt(&["BadBot"], &["/blog"]),
        )
        .unwrap();
        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_exempt(&["BadBot"], &["/blog"]),
                no_trust_file()
            ),
            SiteApplyStatus::UpToDate
        );
    }

    /// An exemption with nothing to exempt from writes no block at all —
    /// exemptions only ever *narrow* an existing rule.
    #[test]
    fn exemptions_alone_do_not_create_a_block() {
        assert!(block_text(&cfg_exempt(&[], &["/blog"])).is_none());
    }

    // ---- goldens ----

    /// The plain form: patterns and a 403, nothing else. This is the block
    /// most installs run, so its exact shape is worth pinning.
    #[test]
    fn simple_block_matches_the_golden() {
        let text = block_text(&cfg(&["BadBot", "EvilCrawler"])).unwrap();
        crate::golden::assert_golden("nginx-block-simple.conf", &text);
    }

    /// Every feature at once: 444, exemptions (which force the flag form),
    /// rate limiting and robots.txt. The golden is the exact text to paste
    /// into a server block and hand to `nginx -t` on a machine that has
    /// one — see `crate::golden`.
    #[test]
    fn kitchen_sink_block_matches_the_golden() {
        let config = BlockConfig {
            patterns: vec!["BadBot".to_string(), "EvilCrawler".to_string()],
            response: BlockResponse::Close,
            serve_robots_txt: true,
            rate_limit_burst: Some(20),
            exempt_paths: vec!["/blog".to_string(), "/feed.xml".to_string()],
            agent_exemptions: Vec::new(),
            request_rules: Vec::new(),
            trust: None,
        };
        let text = block_text(&config).unwrap();
        crate::golden::assert_golden("nginx-block-full.conf", &text);
    }

    #[test]
    fn robots_txt_matches_the_golden() {
        let db = crate::db::Db::open_in_memory().unwrap();
        seed_bot(&db, "gptbot", "GPTBot", true);
        seed_bot(&db, "ccbot", "CCBot", true);
        // Allowed bots must not appear.
        seed_bot(&db, "goodbot", "GoodBot", true);
        db.set_bot_status("goodbot", crate::db::BotStatus::Allowed)
            .unwrap();
        crate::golden::assert_golden("robots.txt", &robots_txt_body(&db).unwrap());
    }

    #[test]
    fn rate_limit_conf_matches_the_golden() {
        crate::golden::assert_golden("stop-bots-limits.conf", &rate_limit_conf_body(10, 10));
    }

    // ---- HTTP/1.x rejection ----

    fn cfg_http1x(patterns: &[&str]) -> BlockConfig {
        BlockConfig {
            request_rules: vec![RequestRule::Http1x],
            ..cfg(patterns)
        }
    }

    #[test]
    fn parse_detects_tls_from_a_listen_ssl_parameter() {
        let content = "server {\n    listen 443 ssl;\n    server_name a.example;\n}\n";
        assert!(parse_server_blocks(content)[0].is_tls);
    }

    #[test]
    fn parse_detects_tls_from_an_ssl_certificate_directive() {
        let content = concat!(
            "server {\n",
            "    listen 8443;\n",
            "    ssl_certificate /etc/ssl/a.pem;\n",
            "    server_name a.example;\n}\n"
        );
        assert!(parse_server_blocks(content)[0].is_tls);
    }

    #[test]
    fn parse_marks_a_plain_listen_block_as_not_tls() {
        let content = "server {\n    listen 80;\n    server_name a.example;\n}\n";
        assert!(!parse_server_blocks(content)[0].is_tls);
    }

    #[test]
    fn rejecting_http_1x_emits_a_server_protocol_condition() {
        let text = block_text(&cfg_http1x(&["BadBot"])).unwrap();
        assert!(
            text.contains(r#"if ($server_protocol ~ "^HTTP/1\.")"#),
            "text was:\n{text}"
        );
        // Two reasons to block now, so the flag form rather than a direct
        // return — otherwise the two conditions couldn't both apply.
        assert!(
            text.contains("set $stop_bots_block 1;"),
            "text was:\n{text}"
        );
        assert!(
            text.contains("if ($stop_bots_block) {"),
            "text was:\n{text}"
        );
    }

    /// The guard that keeps certificate renewal working. ACME HTTP-01 is
    /// fetched over HTTP/1.1 by a non-browser client; without this the
    /// site keeps working until its certificate expires weeks later.
    #[test]
    fn rejecting_http_1x_always_exempts_well_known() {
        let text = block_text(&cfg_http1x(&["BadBot"])).unwrap();
        assert!(text.contains(r"/\.well-known/"), "text was:\n{text}");
    }

    #[test]
    fn not_rejecting_http_1x_leaves_well_known_alone() {
        let text = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(!text.contains("well-known"), "text was:\n{text}");
    }

    /// Rejection alone, with nothing else configured, is still a block
    /// worth writing.
    #[test]
    fn rejecting_http_1x_alone_still_writes_a_block() {
        let config = BlockConfig {
            request_rules: vec![RequestRule::Http1x],
            ..BlockConfig::default()
        };
        let text = block_text(&config).unwrap();
        assert!(text.contains("$server_protocol"), "text was:\n{text}");
        assert!(
            !text.contains("$http_user_agent"),
            "nothing to match on the UA; text was:\n{text}"
        );
    }

    /// The guard that keeps the feature from taking a site offline: a
    /// site's port-80 and port-443 blocks share one `server_name`, so the
    /// setting reaches both, and on the plain one every request is 1.1.
    #[test]
    fn rejecting_http_1x_is_dropped_in_a_non_tls_server_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.example");
        fs::write(
            &path,
            concat!(
                "server {\n    listen 80;\n    server_name a.example;\n}\n",
                "server {\n    listen 443 ssl;\n    server_name a.example;\n}\n"
            ),
        )
        .unwrap();

        apply_block_for_site(&path, dir.path(), "a.example", &cfg_http1x(&["BadBot"])).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(
            written.matches("$server_protocol").count(),
            1,
            "only the TLS block may reject HTTP/1.x; written was:\n{written}"
        );
        let blocks = parse_server_blocks(&written);
        let plain = &written[blocks[0].open..blocks[0].close];
        assert!(
            !plain.contains("$server_protocol"),
            "the plain-HTTP block must not reject HTTP/1.x; it was:\n{plain}"
        );
        // ...and it still gets the ordinary user-agent blocking.
        assert!(plain.contains("$http_user_agent"), "plain was:\n{plain}");
    }

    /// The narrowing has to apply to the status check too, or the
    /// plain-HTTP block reads as permanently STALE.
    #[test]
    fn a_correctly_narrowed_non_tls_block_reads_as_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.example");
        fs::write(
            &path,
            concat!(
                "server {\n    listen 80;\n    server_name a.example;\n}\n",
                "server {\n    listen 443 ssl;\n    server_name a.example;\n}\n"
            ),
        )
        .unwrap();

        let config = cfg_http1x(&["BadBot"]);
        apply_block_for_site(&path, dir.path(), "a.example", &config).unwrap();
        assert_eq!(
            site_apply_status(&path, "a.example", &config, no_trust_file()),
            SiteApplyStatus::UpToDate
        );
    }

    #[test]
    fn site_apply_status_is_stale_when_http_1x_rejection_is_switched_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(
            &path,
            "server {\n    listen 443 ssl;\n    server_name a.example;\n}\n",
        )
        .unwrap();
        apply_block_for_site(&path, dir.path(), "a.example", &cfg(&["BadBot"])).unwrap();

        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_http1x(&["BadBot"]),
                no_trust_file()
            ),
            SiteApplyStatus::Stale
        );
    }

    #[test]
    fn http_1x_rejection_matches_the_golden() {
        let config = BlockConfig {
            request_rules: vec![RequestRule::Http1x],
            ..cfg(&["BadBot"])
        };
        crate::golden::assert_golden("nginx-block-http1x.conf", &block_text(&config).unwrap());
    }

    // ---- trusted clients ----

    fn trust_body() -> String {
        trusted_conf_body(
            &["203.0.113.7".to_string(), "2001:db8::/48".to_string()],
            &["UptimeRobot".to_string(), "Pingdom.com_bot".to_string()],
        )
        .unwrap()
    }

    fn cfg_trusted(patterns: &[&str]) -> BlockConfig {
        BlockConfig {
            trust: Some(trust_body()),
            ..cfg(patterns)
        }
    }

    /// The trust file is the exact text to put in `conf.d` and hand to
    /// `nginx -t` — the container suite does exactly that.
    #[test]
    fn the_trust_file_matches_the_golden() {
        crate::golden::assert_golden("stop-bots-trusted.conf", &trust_body());
    }

    /// Bot patterns and a request-shape rule, both set the flag; trust
    /// clears it after them, so a trusted client gets through whichever
    /// one it tripped.
    #[test]
    fn a_trusted_block_matches_the_golden() {
        let config = BlockConfig {
            request_rules: vec![RequestRule::NoAcceptLanguage],
            exempt_paths: vec!["/blog".to_string()],
            agent_exemptions: Vec::new(),
            ..cfg_trusted(&["BadBot"])
        };
        crate::golden::assert_golden("nginx-block-trusted.conf", &block_text(&config).unwrap());
    }

    /// The same ordering property the exemption test pins, for the same
    /// reason: a clear before a set clears nothing. The trust clear has to
    /// come after every chunk's set, even when the pattern list is long
    /// enough to be split.
    #[test]
    fn the_trust_clear_comes_after_every_set_and_before_the_return() {
        let many: Vec<String> = (0..400).map(|i| format!("Bot{i:04}")).collect();
        let config = BlockConfig {
            patterns: many,
            ..cfg_trusted(&[])
        };
        let text = block_text(&config).unwrap();
        let clear = text
            .find("if ($stop_bots_trusted)")
            .expect("no trust clear");
        let last_set = text.rfind("set $stop_bots_block 1;").unwrap();
        let act = text.find("if ($stop_bots_block)").unwrap();
        assert!(
            last_set < clear && clear < act,
            "trust must clear after every set and before the return:\n{text}"
        );
    }

    /// A direct `return` has nowhere to put a clear, so trust must force
    /// the flag form even for the plainest block.
    #[test]
    fn trust_switches_a_plain_block_to_the_flag_form() {
        let text = block_text(&cfg_trusted(&["BadBot"])).unwrap();
        assert!(
            text.contains("set $stop_bots_block 1;"),
            "text was:\n{text}"
        );
        assert!(!text.contains("    if ($http_user_agent ~* \"BadBot\") {\n        return"));
    }

    /// Trust only ever narrows a block, like an exemption: with nothing
    /// blocked there is no block to write.
    #[test]
    fn trust_alone_does_not_create_a_block() {
        assert!(block_text(&cfg_trusted(&[])).is_none());
    }

    #[test]
    fn nothing_trusted_means_no_trust_file() {
        assert_eq!(trusted_conf_body(&[], &[]), None);
    }

    /// What is written is what was validated: the trimmed address. The
    /// whitespace around one a console wrote never reaches the file.
    #[test]
    fn a_trusted_address_is_written_as_it_was_checked() {
        let body = trusted_conf_body(&[" \t192.0.2.9\n ".to_string()], &[]).unwrap();
        assert!(body.contains("\n    192.0.2.9 1;\n"), "body was:\n{body}");
    }

    /// Validated at entry, and filtered here again: one quote in a `map`
    /// key would stop NGINX loading every site, not just this one.
    #[test]
    fn the_trust_file_drops_an_entry_that_would_break_nginx() {
        let body = trusted_conf_body(
            &["not-an-address".to_string(), "203.0.113.7".to_string()],
            &["Evil\"Bot".to_string(), "Good.Bot".to_string()],
        )
        .unwrap();
        assert!(!body.contains("not-an-address"), "body was:\n{body}");
        assert!(!body.contains("Evil"), "body was:\n{body}");
        assert!(
            body.contains("\"~*Good\\.Bot\" 1;"),
            "a trusted user agent is a literal, so its dot must be escaped:\n{body}"
        );
    }

    #[test]
    fn with_anything_trusted_a_site_limits_through_the_zone_that_skips_it() {
        let config = BlockConfig {
            rate_limit_burst: Some(20),
            ..cfg_trusted(&["BadBot"])
        };
        let block = block_text(&config).unwrap();
        assert!(
            block.contains(&format!(
                "limit_req zone={UNTRUSTED_RATE_LIMIT_ZONE} burst=20"
            )),
            "block was:\n{block}"
        );
        let conf = untrusted_rate_limit_conf_body(10, 10);
        assert!(
            conf.contains(&format!(
                "limit_req_zone {LIMIT_KEY_VAR} zone={UNTRUSTED_RATE_LIMIT_ZONE}:"
            )),
            "conf was:\n{conf}"
        );
        assert!(
            trust_body().contains(&format!("map {TRUSTED_VAR} {LIMIT_KEY_VAR}")),
            "the key the zone reads must be defined by the trust file"
        );
    }

    /// The bug the container suite found: re-keying a zone NGINX already
    /// has makes it reject the reload, after `nginx -t` passed. The
    /// original zone must keep its original key whatever is trusted.
    #[test]
    fn the_original_zone_keeps_its_key_whatever_is_trusted() {
        assert_eq!(
            rate_limit_conf_body(10, 10),
            format!(
                "# Generated by stop-bots {}. Edits will be overwritten.\n\
                 limit_req_zone $binary_remote_addr zone=stop_bots:10m rate=10r/s;\n",
                crate::generated::VERSION
            )
        );
    }

    /// The trust file carries the version that wrote it, and every site
    /// that reads it is compared against it. A version bump alone must not
    /// make those sites `STALE` — nor must a file 0.0.x wrote, whose header
    /// had no version at all.
    #[test]
    fn a_trust_file_from_another_version_is_still_current() {
        let config = cfg_trusted(&["BadBot"]);
        let expected = config.trust.clone().unwrap();
        let ours = format!("# {}.", crate::generated::generated_by());
        assert!(expected.starts_with(&ours), "trust file was:\n{expected}");
        for older in [
            "# Generated by stop-bots.",
            "# Generated by stop-bots 0.0.99.",
        ] {
            let on_disk = expected.replacen(&ours, older, 1);
            assert!(
                trust_file_is_current(&config, Some(&on_disk)),
                "a trust file headed {older:?} read as stale"
            );
        }
    }

    /// The markers are how every release finds the block an earlier one
    /// injected into an operator's site file — and `uninstall` how it finds
    /// what to take out. **Frozen, and pinned literally here rather than
    /// through the constants**: a changed marker makes every block already
    /// on disk invisible, so the next apply adds a second one beside it,
    /// and `uninstall` leaves the first behind. Nor may a version ever go
    /// inside them, which would change them with every release.
    #[test]
    fn the_sentinel_markers_are_frozen() {
        assert_eq!(BLOCK_BEGIN, "# BEGIN stop-bots (DO NOT EDIT)");
        assert_eq!(BLOCK_END, "# END stop-bots");
        assert_eq!(CONSOLE_BEGIN, "# BEGIN stop-bots console (DO NOT EDIT)");
        assert_eq!(CONSOLE_END, "# END stop-bots console");
    }

    /// Nor does the block itself carry a version: `site_apply_status`
    /// compares it as text, and every site would read `STALE` after every
    /// upgrade.
    #[test]
    fn the_injected_block_names_no_version() {
        let block = block_text(&cfg(&["BadBot"])).unwrap();
        assert!(
            !block.contains(crate::generated::VERSION) && !block.contains("Generated by"),
            "block was:\n{block}"
        );
    }

    /// Trusting a second user agent changes the trust file and no block,
    /// and a site has to read as out of date or nobody applies it.
    #[test]
    fn a_site_is_current_only_when_the_trust_file_is_the_expected_one() {
        let config = cfg_trusted(&["BadBot"]);
        let expected = config.trust.clone().unwrap();
        for (why, on_disk, current) in [
            ("the file matches", Some(expected.as_str()), true),
            ("the file is missing", None, false),
            ("the file is an older list", Some("geo {}\n"), false),
        ] {
            assert_eq!(
                trust_file_is_current(&config, on_disk),
                current,
                "wrong answer when {why}"
            );
        }
        // A site that writes no block reads nothing from the file.
        assert!(trust_file_is_current(&cfg_trusted(&[]), None));
        // Nor does one that expects nothing trusted.
        assert!(trust_file_is_current(&cfg(&["BadBot"]), Some("leftover")));
    }

    #[test]
    fn writing_generated_files_counts_only_the_ones_that_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conf.d/stop-bots-trusted.conf");
        let files = vec![(path.clone(), trust_body())];

        assert_eq!(write_planned_managed_files(&files).unwrap(), 1);
        assert_eq!(
            write_planned_managed_files(&files).unwrap(),
            0,
            "an unchanged file must not count, or every apply reloads NGINX"
        );
        assert_eq!(remove_planned_managed_files(&[held(&path)]).unwrap(), 1);
        assert_eq!(remove_planned_managed_files(&[held(&path)]).unwrap(), 0);
    }

    /// The file at `path`, held through its directory, which must exist.
    fn held(path: &Path) -> DirFile {
        DirFile::locate(path).unwrap().unwrap()
    }

    /// Where each of `files` is.
    fn paths_of(files: &[DirFile]) -> Vec<PathBuf> {
        files.iter().map(|file| file.path().to_path_buf()).collect()
    }

    /// A root whose `conf.d` exists: a temp tree standing in for the
    /// containerised host this was found on, whose NGINX config lives
    /// under `/srv/.../nginx` and whose `conf.d` is the directory NGINX
    /// globs.
    fn root_with_a_conf_d() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("conf.d")).unwrap();
        dir
    }

    /// The bug this family exists to prevent, and the one that actually
    /// happened on a host in September 2026: the `http`-context files went
    /// to a fixed `/etc/nginx/conf.d` while every site file was found and
    /// rewritten under the stored root. NGINX never read that directory,
    /// so the blocks referenced `$stop_bots_trusted` with nothing defining
    /// it — and NGINX refuses to load such a config at all, so one restart
    /// would have taken down every site on the box, not just the one
    /// setting had gone wrong.
    #[test]
    fn the_generated_http_files_land_in_the_roots_own_conf_d() {
        let dir = root_with_a_conf_d();
        let db = crate::db::Db::open_in_memory().unwrap();
        db.trust_user_agent("Nextcloud").unwrap();
        db.set_rate_limit_enabled(true).unwrap();
        let host = host_with_root(dir.path().to_str().unwrap());

        let planned: Vec<PathBuf> = planned_managed_files(&db, &root(&host, None))
            .unwrap()
            .into_iter()
            .map(|(path, _)| path)
            .collect();

        for name in [
            "stop-bots-trusted.conf",
            "stop-bots-limits.conf",
            "stop-bots-limits-untrusted.conf",
        ] {
            let expected = dir.path().join("conf.d").join(name);
            assert!(
                planned.contains(&expected),
                "{name} must be written where NGINX reads it; planned:\n{planned:#?}"
            );
        }
    }

    /// The removal half has to look in the same place, or switching rate
    /// limiting off deletes nothing and leaves a live `limit_req_zone`
    /// behind while no site names it.
    #[test]
    fn the_removal_half_looks_in_the_same_directory() {
        let dir = root_with_a_conf_d();
        let db = crate::db::Db::open_in_memory().unwrap();
        let host = host_with_root(dir.path().to_str().unwrap());

        let unused = paths_of(&unused_managed_files(&db, &root(&host, None)).unwrap());
        for name in ["stop-bots-trusted.conf", "stop-bots-limits.conf"] {
            let expected = dir.path().join("conf.d").join(name);
            assert!(
                unused.contains(&expected),
                "{name} must be removed from where it was written; unused:\n{unused:#?}"
            );
        }
    }

    // ---- the record of what was written ----

    /// A file written by a generated-file writer, recorded as one would be.
    fn written_and_recorded(db: &crate::db::Db, path: &Path, body: &str) {
        record_managed_files(db, &[(path.to_path_buf(), body.to_string())]).unwrap();
        write_planned_managed_files(&[(path.to_path_buf(), body.to_string())]).unwrap();
    }

    /// The record is a row the web console can write, and an apply runs as
    /// root. So it is acted on only inside the directories this tree's
    /// files go into: a set left in the `conf.d` of a root the host no
    /// longer uses is `uninstall`'s to collect, not an apply's, because
    /// a row naming *anywhere* is exactly what an attacker would write.
    #[test]
    fn a_recorded_file_outside_the_managed_directories_is_never_collected() {
        let (old, new) = (root_with_a_conf_d(), root_with_a_conf_d());
        let db = crate::db::Db::open_in_memory().unwrap();
        db.set_rate_limit_enabled(true).unwrap();
        let left_behind = rate_limit_conf_path(&old.path().join("conf.d"));
        written_and_recorded(&db, &left_behind, &rate_limit_conf_body(10, 10));

        let unused = paths_of(&unused_managed_files(&db, new.path()).unwrap());

        assert!(
            !unused.contains(&left_behind),
            "a file outside this root's conf.d was collected: {unused:#?}"
        );
        assert!(
            !unused.contains(&rate_limit_conf_path(&new.path().join("conf.d"))),
            "the file the settings call for was collected: {unused:#?}"
        );
        let found = DirFile::exactly(&left_behind).unwrap();
        assert!(
            found.is_regular() && is_generated(&found),
            "uninstall, which an administrator runs, still knows it for ours"
        );
    }

    /// Hostile rows, each written straight into `managed_files` as a
    /// compromised console could: none of them may be deleted, even though
    /// every target carries this project's header, so the header alone is
    /// not what protects them.
    #[test]
    fn a_hostile_managed_files_row_deletes_nothing() {
        let root = root_with_a_conf_d();
        let conf_d = root.path().join("conf.d");
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("stop-bots-limits.conf");
        let header = rate_limit_conf_body(10, 10);
        fs::write(&victim, &header).unwrap();
        let linked = conf_d.join("stop-bots-trusted.conf");
        std::os::unix::fs::symlink(&victim, &linked).unwrap();
        let misnamed = conf_d.join("default.conf");
        fs::write(&misnamed, &header).unwrap();

        let db = crate::db::Db::open_in_memory().unwrap();
        let climbing = conf_d
            .join("..")
            .join("..")
            .join(outside.path().strip_prefix("/").unwrap())
            .join("stop-bots-limits.conf");
        for (path, kind) in [
            (victim.clone(), ManagedKind::Limits),
            (climbing.clone(), ManagedKind::Limits),
            (linked.clone(), ManagedKind::Trusted),
            (misnamed.clone(), ManagedKind::Limits),
            (PathBuf::from("/etc/passwd"), ManagedKind::Limits),
        ] {
            db.record_managed_file(&path, kind.id()).unwrap();
        }

        let collected = unused_managed_files(&db, root.path()).unwrap();
        remove_planned_managed_files(&collected).unwrap();
        let unused = paths_of(&collected);

        for path in [&victim, &climbing, &misnamed] {
            assert!(!unused.contains(path), "{} was collected", path.display());
        }
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            header,
            "the target was deleted"
        );
        assert_eq!(fs::read_to_string(&misnamed).unwrap(), header);
        assert!(
            !unused.contains(&PathBuf::from("/etc/passwd")),
            "{unused:#?}"
        );
    }

    /// A row that reaches this root's `conf.d` through a link in a
    /// directory the console owns is refused: the console could point the
    /// link elsewhere between the check and the removal. The file itself is
    /// still collected, by its own resolved path.
    #[test]
    fn a_recorded_path_through_a_link_is_refused() {
        let root = root_with_a_conf_d();
        let conf_d = fs::canonicalize(root.path().join("conf.d")).unwrap();
        let generated = rate_limit_conf_path(&conf_d);
        fs::write(&generated, rate_limit_conf_body(10, 10)).unwrap();
        let console = tempfile::tempdir().unwrap();
        let link = console.path().join("link");
        std::os::unix::fs::symlink(&conf_d, &link).unwrap();
        let db = crate::db::Db::open_in_memory().unwrap();
        db.record_managed_file(&rate_limit_conf_path(&link), ManagedKind::Limits.id())
            .unwrap();

        let unused = paths_of(&unused_managed_files(&db, root.path()).unwrap());

        assert!(
            unused.iter().all(|path| !path.starts_with(console.path())),
            "a path through the console's link was collected: {unused:#?}"
        );
        assert!(unused.contains(&generated), "{unused:#?}");
    }

    /// **Checked once, used through what was checked.** After the files
    /// to remove are resolved, the directory they were found in is swapped
    /// for a link to another one holding a file of the same name. The
    /// removal, and the restore after NGINX refuses the result, both
    /// happen in the directory that was checked; the other is untouched,
    /// and nothing is written into it.
    #[test]
    fn a_directory_swapped_after_the_check_is_not_the_one_used() {
        let root = root_with_a_conf_d();
        let conf_d = root.path().join("conf.d");
        let body = rate_limit_conf_body(10, 10);
        fs::write(rate_limit_conf_path(&conf_d), &body).unwrap();
        let victim = tempfile::tempdir().unwrap();
        let precious = victim.path().join("stop-bots-limits.conf");
        fs::write(&precious, "precious\n").unwrap();
        let db = crate::db::Db::open_in_memory().unwrap();

        let unused = unused_managed_files(&db, root.path()).unwrap();
        let snapshot = Snapshot::of_apply(root.path(), [], &unused, []).unwrap();
        let checked = root.path().join("checked");
        fs::rename(&conf_d, &checked).unwrap();
        std::os::unix::fs::symlink(victim.path(), &conf_d).unwrap();

        assert_eq!(remove_planned_managed_files(&unused).unwrap(), 1);
        assert!(!rate_limit_conf_path(&checked).exists(), "not removed");
        assert_eq!(fs::read_to_string(&precious).unwrap(), "precious\n");
        fs::remove_file(&precious).unwrap();

        test_or_restore(&snapshot, &commands_that(false)).unwrap_err();

        assert_eq!(
            fs::read_to_string(rate_limit_conf_path(&checked)).unwrap(),
            body,
            "not put back where it was"
        );
        assert_eq!(
            fs::read_dir(victim.path()).unwrap().count(),
            0,
            "the restore wrote into the directory the link now points at"
        );
    }

    /// A FIFO where a generated file goes is not read (a read would wait
    /// for ever), and neither it nor a directory there is collected
    /// through the record.
    #[test]
    fn a_fifo_or_a_directory_at_a_recorded_name_is_neither_read_nor_collected() {
        let (old, new) = (root_with_a_conf_d(), root_with_a_conf_d());
        let fifo = rate_limit_conf_path(&new.path().join("conf.d"));
        crate::testing::mkfifo(&fifo);
        let old_conf_d = fs::canonicalize(old.path().join("conf.d")).unwrap();
        let dir = trusted_conf_path(&old_conf_d);
        fs::create_dir(&dir).unwrap();
        let db = crate::db::Db::open_in_memory().unwrap();
        for (path, kind) in [
            (fifo.clone(), ManagedKind::Limits),
            (dir.clone(), ManagedKind::Trusted),
        ] {
            db.record_managed_file(&path, kind.id()).unwrap();
        }

        let changes = preview_all_sites(&db, new.path()).unwrap();
        let snapshot = Snapshot::take([fifo.clone()]).unwrap();
        let unused = paths_of(&unused_managed_files(&db, new.path()).unwrap());

        assert!(changes.is_empty(), "{changes:#?}");
        assert!(!unused.contains(&dir), "{unused:#?}");
        snapshot.restore().unwrap();
        assert!(fs::symlink_metadata(&fifo).is_ok(), "the FIFO was touched");
    }

    /// The record says stop-bots wrote a file there once; an operator's own
    /// file under the same name since is theirs.
    #[test]
    fn a_recorded_path_now_holding_someone_else_s_file_is_left_alone() {
        let (old, new) = (root_with_a_conf_d(), root_with_a_conf_d());
        let db = crate::db::Db::open_in_memory().unwrap();
        let path = rate_limit_conf_path(&old.path().join("conf.d"));
        written_and_recorded(&db, &path, &rate_limit_conf_body(10, 10));
        fs::write(
            &path,
            "limit_req_zone $binary_remote_addr zone=mine:1m rate=1r/s;\n",
        )
        .unwrap();

        let unused = paths_of(&unused_managed_files(&db, new.path()).unwrap());

        assert!(!unused.contains(&path), "unused: {unused:#?}");
    }

    /// The order is what keeps the config valid between two removals: a
    /// file that reads the trust file's variable goes before it.
    #[test]
    fn recorded_files_are_removed_in_dependency_order() {
        let root = root_with_a_conf_d();
        let db = crate::db::Db::open_in_memory().unwrap();
        let conf_d = root.path().join("conf.d");
        for (path, body) in [
            (
                trusted_conf_path(&conf_d),
                trusted_conf_body(&["192.0.2.1".into()], &[]).unwrap(),
            ),
            (
                untrusted_rate_limit_conf_path(&conf_d),
                untrusted_rate_limit_conf_body(1, 1),
            ),
        ] {
            written_and_recorded(&db, &path, &body);
        }

        let unused = paths_of(&unused_managed_files(&db, root.path()).unwrap());

        let at = |path: PathBuf| unused.iter().position(|p| p == &path).unwrap();
        assert!(
            at(untrusted_rate_limit_conf_path(&conf_d)) < at(trusted_conf_path(&conf_d)),
            "{unused:#?}"
        );
    }

    /// A row for a file that is gone, and is not about to be written, is
    /// dropped, so the record does not grow for ever.
    #[test]
    fn recording_forgets_files_that_are_gone() {
        let dir = root_with_a_conf_d();
        let db = crate::db::Db::open_in_memory().unwrap();
        let gone = rate_limit_conf_path(&dir.path().join("conf.d"));
        let coming = trusted_conf_path(&dir.path().join("conf.d"));
        db.record_managed_file(&gone, ManagedKind::Limits.id())
            .unwrap();

        record_managed_files(&db, &[(coming.clone(), String::new())]).unwrap();

        let recorded: Vec<PathBuf> = db
            .managed_files()
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(recorded, [coming], "the vanished file's row survived");
    }

    #[test]
    fn every_managed_kind_is_known_by_its_file_name_and_its_id() {
        let conf_d = Path::new("/etc/nginx/conf.d");
        for (path, kind) in [
            (rate_limit_conf_path(conf_d), ManagedKind::Limits),
            (
                untrusted_rate_limit_conf_path(conf_d),
                ManagedKind::UntrustedLimits,
            ),
            (trusted_conf_path(conf_d), ManagedKind::Trusted),
            (robots_txt_path(), ManagedKind::RobotsTxt),
        ] {
            assert_eq!(ManagedKind::of(&path), Some(kind), "{}", path.display());
            assert_eq!(ManagedKind::from_id(kind.id()), Some(kind));
        }
        assert_eq!(
            ManagedKind::of(Path::new("/etc/nginx/conf.d/default.conf")),
            None
        );
    }

    /// And a root inside the stock tree with no `conf.d` of its own keeps
    /// the stock one rather than inventing one. The container suite scans
    /// `/etc/nginx/sites-enabled`, which `include sites-enabled/*` globs:
    /// a `conf.d` directory created in there is something NGINX tries to
    /// `pread()` as a config file, and it refuses to start.
    #[test]
    fn a_root_inside_the_stock_tree_uses_the_stock_conf_d() {
        assert_eq!(
            conf_d_dir(Path::new("/etc/nginx/sites-enabled")),
            PathBuf::from(CONF_D_DIR)
        );
    }

    /// Any other root is its own tree: `batch --root <scratch>` read the
    /// host's own `/etc/nginx/conf.d/stop-bots-trusted.conf`, and an apply
    /// there would have rewritten or removed it.
    #[test]
    fn a_root_outside_the_stock_tree_never_reaches_out_of_itself() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(conf_d_dir(dir.path()), dir.path().join("conf.d"));
        // Nor by walking out of the stock tree with `..`.
        let sneaky = Path::new("/etc/nginx/../../tmp/scratch");
        assert_eq!(conf_d_dir(sneaky), sneaky.join("conf.d"));
    }

    /// A normal host install must be exactly as it was. `root` falls back
    /// to `/etc/nginx`, so this resolves to the stock path without the
    /// setting existing at all.
    #[test]
    fn conf_d_falls_back_to_the_stock_path_when_no_root_is_stored() {
        assert_eq!(
            conf_d_dir(&root(&crate::hostconf::HostConf::default(), None)),
            PathBuf::from(CONF_D_DIR)
        );
    }

    /// The staleness check reads the trust file too, and has to read it
    /// from the same directory it is written to. Reading the wrong one
    /// finds nothing, which is indistinguishable from an out-of-date file:
    /// every site would report `Stale` for ever, and an operator applying
    /// again would change nothing and be told so.
    #[test]
    fn site_apply_status_reads_the_trust_file_from_the_given_conf_d() {
        let dir = tempfile::tempdir().unwrap();
        let conf_d = dir.path().join("conf.d");
        fs::create_dir_all(&conf_d).unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        let config = cfg_trusted(&["BadBot"]);
        apply_block_for_site(&path, dir.path(), "a.example", &config).unwrap();

        assert_eq!(
            site_apply_status(&path, "a.example", &config, &conf_d),
            SiteApplyStatus::Stale,
            "the block is applied but the trust file it reads is not there yet"
        );

        fs::write(conf_d.join("stop-bots-trusted.conf"), trust_body()).unwrap();
        assert_eq!(
            site_apply_status(&path, "a.example", &config, &conf_d),
            SiteApplyStatus::UpToDate,
            "with the file in place the site is applied"
        );
    }

    // ---- request-shape rules ----

    fn cfg_rules(rules: &[RequestRule]) -> BlockConfig {
        BlockConfig {
            request_rules: rules.to_vec(),
            ..cfg(&["BadBot"])
        }
    }

    #[test]
    fn every_request_rule_has_a_distinct_stable_id() {
        let mut ids: Vec<&str> = RequestRule::ALL.iter().map(|r| r.id()).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
        for rule in RequestRule::ALL {
            assert_eq!(RequestRule::from_id(rule.id()), Some(rule));
        }
        assert_eq!(RequestRule::from_id("nope"), None);
    }

    /// Each rule is its own toggle, so each must emit its own condition —
    /// a shared one would make them indistinguishable in the config too.
    #[test]
    fn each_rule_emits_its_own_condition() {
        for rule in RequestRule::ALL {
            let text = block_text(&cfg_rules(&[rule])).unwrap();
            assert!(
                text.contains(&rule.condition()),
                "{} should emit {:?}; text was:\n{text}",
                rule.label(),
                rule.condition()
            );
        }
    }

    /// Every rule carries collateral, and the UI shows the caveat next to
    /// the toggle. An empty one would render as a bare switch and quietly
    /// imply there's no downside.
    #[test]
    fn every_rule_states_what_else_it_turns_away() {
        for rule in RequestRule::ALL {
            assert!(!rule.caveat().is_empty(), "{} needs a caveat", rule.label());
        }
    }

    #[test]
    fn any_request_rule_forces_the_well_known_exemption() {
        for rule in RequestRule::ALL {
            let text = block_text(&cfg_rules(&[rule])).unwrap();
            assert!(
                text.contains(r"/\.well-known/"),
                "{} must not break ACME renewal; text was:\n{text}",
                rule.label()
            );
        }
    }

    /// Only the two rules that depend on TLS are dropped in a plain block.
    /// The header-shape rules work identically over HTTP, so dropping them
    /// there would silently disable them on a redirect block.
    #[test]
    fn only_the_tls_dependent_rules_are_dropped_in_a_plain_block() {
        let plain = ServerBlock {
            names: vec!["a.example".to_string()],
            is_tls: false,
            open: 0,
            close: 0,
        };
        let narrowed = for_block(&cfg_rules(&RequestRule::ALL), "", &plain);

        assert!(!narrowed.request_rules.contains(&RequestRule::Http1x));
        assert!(!narrowed.request_rules.contains(&RequestRule::OldTls));
        assert!(narrowed.request_rules.contains(&RequestRule::NoAccept));
        assert!(narrowed.request_rules.contains(&RequestRule::NoUserAgent));
        assert!(narrowed.request_rules.contains(&RequestRule::IpLiteralHost));
    }

    #[test]
    fn a_tls_block_keeps_every_rule() {
        let tls = ServerBlock {
            names: vec!["a.example".to_string()],
            is_tls: true,
            open: 0,
            close: 0,
        };
        let narrowed = for_block(&cfg_rules(&RequestRule::ALL), "", &tls);
        assert_eq!(narrowed.request_rules.len(), RequestRule::ALL.len());
    }

    /// A stored id from a newer build must not stop an older one applying
    /// anything at all.
    #[test]
    fn an_unrecognised_stored_rule_id_is_skipped_not_fatal() {
        let db = crate::db::Db::open_in_memory().unwrap();
        db.upsert_site("a.example", "/etc/nginx/a.conf").unwrap();
        let site = db.list_sites().unwrap().into_iter().next().unwrap();
        db.set_site_request_rule(site.id, "from_the_future", true)
            .unwrap();
        db.set_site_request_rule(site.id, RequestRule::NoAccept.id(), true)
            .unwrap();

        assert_eq!(
            site_request_rules(&db, site.id).unwrap(),
            vec![RequestRule::NoAccept]
        );
    }

    #[test]
    fn every_request_rule_together_matches_the_golden() {
        let config = BlockConfig {
            request_rules: RequestRule::ALL.to_vec(),
            ..cfg(&["BadBot"])
        };
        crate::golden::assert_golden(
            "nginx-block-request-rules.conf",
            &block_text(&config).unwrap(),
        );
    }

    // ---- block responses ----

    fn cfg_response(response: BlockResponse) -> BlockConfig {
        BlockConfig {
            response,
            ..cfg(&["BadBot"])
        }
    }

    #[test]
    fn every_response_renders_its_own_status_code() {
        for response in BlockResponse::ALL {
            let text = block_text(&cfg_response(response)).unwrap();
            assert!(
                text.contains(&format!("return {};", response.status_code())),
                "{} should return {}; text was:\n{text}",
                response.label(),
                response.status_code()
            );
        }
    }

    /// Every option needs to say what it is *for*, or the chooser reads as
    /// five interchangeable numbers.
    #[test]
    fn every_response_explains_itself() {
        for response in BlockResponse::ALL {
            assert!(!response.rationale().is_empty(), "{:?}", response);
            assert!(!response.label().is_empty(), "{:?}", response);
        }
    }

    #[test]
    fn only_tarpit_throttles_the_response_body() {
        for response in BlockResponse::ALL {
            let text = block_text(&cfg_response(response)).unwrap();
            let throttled = text.contains("set $limit_rate 1;");
            assert_eq!(
                throttled,
                response.is_tarpit(),
                "{} throttling should be {}; text was:\n{text}",
                response.label(),
                response.is_tarpit()
            );
        }
    }

    /// The throttle has to land inside the same `if` as the return, in
    /// both block shapes — `$limit_rate` set anywhere else would apply to
    /// every visitor.
    #[test]
    fn the_tarpit_throttle_sits_with_the_return_in_both_block_shapes() {
        // Direct form: no exemptions, one reason.
        let direct = block_text(&cfg_response(BlockResponse::Tarpit)).unwrap();
        assert!(
            direct.contains("set $limit_rate 1;\n        return 403;"),
            "direct form was:\n{direct}"
        );

        // Flag form: an exemption forces it.
        let flagged = block_text(&BlockConfig {
            response: BlockResponse::Tarpit,
            exempt_paths: vec!["/blog".to_string()],
            agent_exemptions: Vec::new(),
            ..cfg(&["BadBot"])
        })
        .unwrap();
        assert!(
            flagged.contains(
                "if ($stop_bots_block) {\n        set $limit_rate 1;\n        return 403;"
            ),
            "flag form was:\n{flagged}"
        );
        assert_eq!(
            flagged.matches("set $limit_rate").count(),
            1,
            "throttling must apply once, to blocked requests only; was:\n{flagged}"
        );
    }

    #[test]
    fn changing_the_response_makes_an_applied_site_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, "server {\n    server_name a.example;\n}\n").unwrap();
        apply_block_for_site(
            &path,
            dir.path(),
            "a.example",
            &cfg_response(BlockResponse::Gone),
        )
        .unwrap();

        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_response(BlockResponse::Gone),
                no_trust_file()
            ),
            SiteApplyStatus::UpToDate
        );
        assert_eq!(
            site_apply_status(
                &path,
                "a.example",
                &cfg_response(BlockResponse::Tarpit),
                no_trust_file()
            ),
            SiteApplyStatus::Stale
        );
    }

    #[test]
    fn tarpit_matches_the_golden() {
        crate::golden::assert_golden(
            "nginx-block-tarpit.conf",
            &block_text(&cfg_response(BlockResponse::Tarpit)).unwrap(),
        );
    }
    // ---- reaching the console from outside ----

    /// `NginxCommands` whose test either passes or fails, without needing
    /// a real NGINX. `/bin/true` and `/bin/false` rather than a written
    /// script: fewer moving parts, and nothing to make executable.
    fn commands_that(pass: bool) -> NginxCommands {
        let program = if pass { "true" } else { "false" };
        NginxCommands {
            test: vec![program.to_string()],
            reload: vec!["true".to_string()],
        }
    }

    const TWO_BLOCK_SITE: &str = "\
server {
    listen 80;
    server_name example.com;
    return 301 https://$host$request_uri;
}
server {
    listen 443 ssl;
    server_name example.com;
    ssl_certificate /etc/letsencrypt/live/example.com/fullchain.pem;
    root /var/www/example.com;
}
";

    fn upstream() -> std::net::SocketAddr {
        "127.0.0.1:8787".parse().unwrap()
    }

    /// The console's login form must not end up on the port-80 redirect
    /// block when the site has a certificate sitting right there.
    #[test]
    fn path_mode_picks_the_tls_block_not_the_redirect() {
        let updated =
            with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
                .expect("the site has a matching block");

        // Against the parsed block's own span, not the order of
        // directives: the block is inserted at the top of the `server`
        // block, so it precedes `listen 443` while still being inside it.
        let console_at = updated.find(CONSOLE_BEGIN).expect("the block was inserted");
        let blocks = parse_server_blocks(&updated);
        let tls = blocks
            .iter()
            .find(|b| b.is_tls)
            .expect("the fixture has a TLS block");
        assert!(
            console_at > tls.open && console_at < tls.close,
            "the console landed outside the TLS block:\n{updated}"
        );
    }

    /// `proxy_pass` must not have a trailing slash and the location must
    /// keep its prefix — otherwise NGINX strips the prefix and every link
    /// this console generates points outside the location block.
    #[test]
    fn the_console_location_does_not_strip_its_prefix() {
        let block = console_location("/stop-bots/", &upstream());

        assert!(
            block.contains(r#"location "/stop-bots/" {"#),
            "block was:\n{block}"
        );
        assert!(
            block.contains("proxy_pass http://127.0.0.1:8787;"),
            "a trailing slash here strips the prefix:\n{block}"
        );
    }

    /// The whole site file after path mode edits it — the exact bytes to
    /// hand `nginx -t`, and the pin on the quoted `location` prefix.
    #[test]
    fn the_path_mode_console_block_matches_the_golden() {
        let updated =
            with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
                .unwrap();
        crate::golden::assert_golden("nginx-console-path.conf", &updated);
    }

    #[test]
    fn the_subdomain_console_block_matches_the_golden() {
        let block = console_server_block("console.example.com", &upstream());
        crate::golden::assert_golden("nginx-console-subdomain.conf", &block);
    }

    /// Quoting is the second line behind `BasePath::parse`, not a
    /// substitute for it: `ConsoleAccess::Path` is a public type, and a
    /// prefix that reaches it some other way must still be one token. A
    /// quote or backslash inside it is escaped, so it cannot close the
    /// string early.
    #[test]
    fn the_console_location_keeps_a_hostile_prefix_inside_its_quotes() {
        let block = console_location(r#"/a\" { } ;"#, &upstream());

        assert!(
            block.contains(r#"location "/a\\\" { } ;" {"#),
            "block was:\n{block}"
        );
    }

    /// The console's own file is one this tool owns outright, so a link
    /// planted at its name is replaced, never written through.
    #[test]
    fn subdomain_mode_replaces_a_planted_link_instead_of_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nginx");
        fs::create_dir_all(root.join("conf.d")).unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "precious\n").unwrap();
        std::os::unix::fs::symlink(&victim, console_site_path(&root)).unwrap();

        apply_console_access(
            &root,
            &ConsoleAccess::Subdomain {
                host: "console.example.com".to_string(),
            },
            &upstream(),
            &commands_that(true),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
        let written = console_site_path(&root);
        assert!(fs::symlink_metadata(&written).unwrap().is_file());
        assert!(fs::read_to_string(&written)
            .unwrap()
            .contains("console.example.com"));
    }

    /// Path mode edits an operator's site file, which may be a link — but
    /// not one that leads out of the NGINX root.
    #[test]
    fn path_mode_does_not_write_a_site_file_linking_outside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nginx");
        fs::create_dir(&root).unwrap();
        let victim = dir.path().join("elsewhere.conf");
        fs::write(&victim, TWO_BLOCK_SITE).unwrap();
        let link = root.join("example.com");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let err = apply_console_access(
            &root,
            &ConsoleAccess::Path {
                prefix: "/stop-bots/".to_string(),
                config_path: link,
                server_name: "example.com".to_string(),
            },
            &upstream(),
            &commands_that(true),
        )
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("outside the NGINX root"),
            "error was: {err:#}"
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), TWO_BLOCK_SITE);
    }

    #[test]
    fn path_mode_is_idempotent() {
        let once = with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
            .unwrap();
        let twice =
            with_console_location(&once, "example.com", "/stop-bots/", &upstream()).unwrap();

        assert_eq!(once, twice);
        assert_eq!(twice.matches(CONSOLE_BEGIN).count(), 1);
    }

    /// Rewritten in place, so changing the prefix does not leave the old
    /// location block behind alongside the new one.
    #[test]
    fn changing_the_prefix_replaces_the_block_rather_than_adding_one() {
        let first =
            with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
                .unwrap();
        let second =
            with_console_location(&first, "example.com", "/console/", &upstream()).unwrap();

        assert_eq!(second.matches(CONSOLE_BEGIN).count(), 1);
        assert!(
            second.contains(r#"location "/console/" {"#),
            "was:\n{second}"
        );
        assert!(!second.contains("/stop-bots/"), "was:\n{second}");
    }

    /// The console markers must not collide with the bot-blocking ones:
    /// both live in the same `server` block and are rewritten by different
    /// actions.
    #[test]
    fn the_console_block_and_the_blocking_block_coexist() {
        let with_console =
            with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
                .unwrap();
        let blocks = parse_server_blocks(&with_console);
        let tls = blocks.iter().find(|b| b.is_tls).unwrap();
        let config = BlockConfig::new(vec!["BadBot".to_string()], BlockResponse::Forbidden);

        let both = apply_block(&with_console, tls, &config);

        assert!(both.contains(CONSOLE_BEGIN), "console block lost:\n{both}");
        assert!(both.contains(BLOCK_BEGIN), "blocking block lost:\n{both}");
        assert!(both.contains(r#"location "/stop-bots/" {"#), "was:\n{both}");
    }

    #[test]
    fn a_site_with_no_matching_block_is_a_refusal() {
        assert!(
            with_console_location(TWO_BLOCK_SITE, "other.example.org", "/x/", &upstream())
                .is_none()
        );
    }

    /// The failure this guards against: a new `server` block that does not
    /// parse leaves the whole config unloadable, and nothing looks wrong
    /// because the running NGINX keeps serving from memory.
    #[test]
    fn a_config_that_fails_validation_is_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conf.d/stop-bots-console.conf");

        let err = write_validated(
            &path,
            "server { broken",
            &commands_that(false),
            write_managed,
        )
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("restored"),
            "the operator must be told the rollback happened: {err:#}"
        );
        assert!(
            !path.exists(),
            "a file that never validated must not be left behind"
        );
    }

    /// Rolling back an *edit* puts the previous content back, rather than
    /// deleting somebody's site config.
    #[test]
    fn a_failed_edit_restores_the_previous_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.com");
        fs::write(&path, TWO_BLOCK_SITE).unwrap();

        let _ = write_validated(
            &path,
            "server { broken",
            &commands_that(false),
            write_managed,
        );

        assert_eq!(fs::read_to_string(&path).unwrap(), TWO_BLOCK_SITE);
    }

    #[test]
    fn a_config_that_validates_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conf.d/stop-bots-console.conf");
        let block = console_server_block("console.example.com", &upstream());

        write_validated(&path, &block, &commands_that(true), write_managed).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), block);
    }

    /// Plain HTTP is a real downgrade for a page with a password form, so
    /// the generated file has to say so where whoever opens it will look.
    #[test]
    fn the_subdomain_block_warns_that_it_is_cleartext() {
        let block = console_server_block("console.example.com", &upstream());

        assert!(
            block.contains("certbot --nginx -d console.example.com"),
            "was:\n{block}"
        );
        assert!(block.contains("in the clear"), "was:\n{block}");
        assert!(
            block.contains("server_name console.example.com;"),
            "was:\n{block}"
        );
    }

    /// The deprecated `web --secure-cookie` is not what the file tells an
    /// operator to run.
    #[test]
    fn the_subdomain_block_points_at_set_web() {
        let block = console_server_block("console.example.com", &upstream());
        assert!(
            block.contains("stop-bots set-web --secure-cookie true"),
            "was:\n{block}"
        );
    }

    // ---- uninstall ----

    /// A site file with both kinds of block in it, inserted the way the
    /// product inserts them, in either order.
    fn with_both_blocks(site: &str, console_first: bool) -> String {
        let with_block = |content: &str| {
            let block = parse_server_blocks(content)
                .into_iter()
                .find(|b| b.is_tls)
                .unwrap();
            apply_block(content, &block, &cfg(&["BadBot"]))
        };
        let with_console = |content: &str| {
            with_console_location(content, "example.com", "/stop-bots/", &upstream()).unwrap()
        };
        if console_first {
            with_block(&with_console(site))
        } else {
            with_console(&with_block(site))
        }
    }

    /// "Back as it was" means the bytes: every insertion is taken back
    /// with the line break it added, whichever order they went in.
    #[test]
    fn taking_the_blocks_out_restores_the_file_byte_for_byte() {
        for console_first in [true, false] {
            let applied = with_both_blocks(TWO_BLOCK_SITE, console_first);
            assert!(applied.contains(BLOCK_BEGIN) && applied.contains(CONSOLE_BEGIN));

            assert_eq!(
                without_injected_blocks(&applied),
                TWO_BLOCK_SITE,
                "console first: {console_first}; applied was:\n{applied}"
            );
        }
    }

    /// Every `server` block, not just the first one that has a block.
    #[test]
    fn taking_the_blocks_out_covers_every_server_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.conf");
        fs::write(&path, TWO_BLOCK_SITE).unwrap();
        apply_blocks_to_file(&path, dir.path(), &[], &cfg(&["BadBot"])).unwrap();
        let applied = fs::read_to_string(&path).unwrap();
        assert_eq!(applied.matches(BLOCK_BEGIN).count(), 2, "{applied}");

        assert_eq!(without_injected_blocks(&applied), TWO_BLOCK_SITE);
    }

    /// A file stop-bots never touched comes back unchanged — including one
    /// with a hand-written `if` that merely looks like ours.
    #[test]
    fn a_file_without_blocks_is_left_as_it_is() {
        let own = "server {\n    # BEGIN my stuff\n    if ($http_user_agent ~* \"x\") { return 403; }\n}\n";
        assert_eq!(without_injected_blocks(own), own);
    }

    fn site_root_with_blocks() -> (tempfile::TempDir, PathBuf) {
        let dir = root_with_a_conf_d();
        let path = dir.path().join("site.conf");
        fs::write(&path, with_both_blocks(TWO_BLOCK_SITE, false)).unwrap();
        (dir, path)
    }

    #[test]
    fn removing_everything_takes_out_blocks_and_generated_files() {
        let (dir, site) = site_root_with_blocks();
        let limits = rate_limit_conf_path(&dir.path().join("conf.d"));
        fs::write(&limits, rate_limit_conf_body(10, 10)).unwrap();

        let removal = remove_everything(
            dir.path(),
            &[held(&limits)],
            Some(&commands_that(true)),
            false,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&site).unwrap(), TWO_BLOCK_SITE);
        assert!(!limits.exists(), "the generated file is still there");
        assert_eq!(removal.site_files, [site]);
        assert_eq!(removal.generated, [limits]);
        assert!(removal.reloaded);
    }

    /// The rule every apply keeps: a config NGINX refuses is put back,
    /// not left for the next reload to find.
    #[test]
    fn removing_everything_is_put_back_when_the_test_fails() {
        let (dir, site) = site_root_with_blocks();
        let before = fs::read_to_string(&site).unwrap();
        let limits = rate_limit_conf_path(&dir.path().join("conf.d"));
        fs::write(&limits, rate_limit_conf_body(10, 10)).unwrap();

        let err = remove_everything(
            dir.path(),
            &[held(&limits)],
            Some(&commands_that(false)),
            false,
        )
        .unwrap_err();

        assert!(format!("{err:#}").contains("put back"), "was: {err:#}");
        assert_eq!(fs::read_to_string(&site).unwrap(), before);
        assert_eq!(
            fs::read_to_string(&limits).unwrap(),
            rate_limit_conf_body(10, 10)
        );
    }

    #[test]
    fn a_dry_run_of_removing_everything_changes_nothing() {
        let (dir, site) = site_root_with_blocks();
        let before = fs::read_to_string(&site).unwrap();

        let removal = remove_everything(dir.path(), &[], Some(&commands_that(true)), true).unwrap();

        assert_eq!(removal.site_files, std::slice::from_ref(&site));
        assert!(!removal.reloaded);
        assert_eq!(fs::read_to_string(&site).unwrap(), before);
    }

    /// The console's own file is recognised by the first line every
    /// version wrote, and a file of the operator's at that path is not.
    #[test]
    fn the_console_site_file_is_known_by_its_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stop-bots-console.conf");
        for (first_line, ours) in [
            (
                "# Written by stop-bots (`Web Access` panel). Safe to edit or delete.",
                true,
            ),
            (
                &format!(
                    "# {} (`Web Access` panel). Safe to edit or delete.",
                    crate::generated::generated_by()
                )[..],
                true,
            ),
            ("server {", false),
        ] {
            fs::write(&path, format!("{first_line}\nserver {{\n}}\n")).unwrap();
            assert_eq!(is_console_site_file(&path), ours, "{first_line}");
        }
    }

    // ---- hostile strings, as NGINX will read them ----

    /// User agents a site that logs in JSON (`escape=json`) can put in
    /// front of an operator, each with a character NGINX's config reader
    /// or PCRE treats specially. The container suite checks the same list
    /// against a real NGINX.
    const HOSTILE_USER_AGENTS: [&str; 11] = [
        r"Scraper/2.1 a\|",
        r"Scraper/2.1 x\*",
        r"Scraper/2.1 Evil\(",
        r#"Scraper/2.1 say "hi""#,
        r#"Scraper/2.1 \"quoted\""#,
        r"Scraper/2.1 trailing\",
        r"Scraper/2.1 tab\t",
        r"Scraper/2.1 line\n",
        "Scraper/2.1 $foo",
        "Scraper/2.1 }",
        "Scraper/2.1 ;",
    ];

    /// The regex of every `$http_user_agent` test in `text`, as NGINX
    /// hands it to PCRE: found the way NGINX's reader finds the end of
    /// the quoted word, then unescaped the way it unescapes one.
    fn user_agent_regexes_as_nginx_reads_them(text: &str) -> Vec<String> {
        text.split("if ($http_user_agent ~* ")
            .skip(1)
            .map(|rest| {
                let end = closing_quote(rest).expect("the quoted regex is closed");
                assert!(
                    rest[end + 1..].starts_with(") {"),
                    "the regex's own quote does not end the condition:\n{rest}"
                );
                nginx_unquoted(&rest[1..end])
            })
            .collect()
    }

    /// The finding: the regex NGINX compiles for a blocked user agent is
    /// exactly the escaped literal that `pattern_problem` checked — for
    /// every one of these, not a match-all and not a broken group.
    #[test]
    fn a_hostile_blocked_user_agent_reaches_pcre_as_the_literal_that_was_checked() {
        for user_agent in HOSTILE_USER_AGENTS {
            let literal = crate::db::escape_regex_literal(user_agent);
            assert_eq!(pattern_problem(&literal), None, "{user_agent:?}");
            let text = block_text(&cfg(&[&literal])).unwrap();
            assert_eq!(
                user_agent_regexes_as_nginx_reads_them(&text),
                [literal],
                "{user_agent:?} was written as:\n{text}"
            );
        }
    }

    /// The same list as a stored host would have it, written at once: the
    /// exact bytes to hand `nginx -t`.
    #[test]
    fn hostile_user_agents_match_the_golden() {
        let literals: Vec<String> = HOSTILE_USER_AGENTS
            .iter()
            .map(|ua| crate::db::escape_regex_literal(ua))
            .collect();
        let text = block_text(&BlockConfig::new(literals, BlockResponse::Forbidden)).unwrap();
        crate::golden::assert_golden("nginx-block-hostile.conf", &text);
    }

    /// Trusted and exempted user agents are written into quoted strings
    /// too, and read back the same way.
    #[test]
    fn a_trusted_user_agent_reaches_pcre_as_the_literal_it_is() {
        let body = trusted_conf_body(&[], &["Pingdom.com_bot (x)".to_string()]).unwrap();
        let key = body
            .lines()
            .find(|line| line.trim_start().starts_with("\"~*"))
            .expect("a user-agent key")
            .trim();
        let end = closing_quote(key).unwrap();
        assert_eq!(
            nginx_unquoted(&key[1..end]),
            r"~*Pingdom\.com_bot \(x\)",
            "key was: {key}"
        );
    }

    /// Patterns PCRE refuses to compile: each would fail `nginx -t` and
    /// roll back every apply after the one that wrote it.
    #[test]
    fn a_pattern_nginx_would_not_compile_is_never_written() {
        for (pattern, why) in [
            ("Bot[z-a]x", "a range out of order"),
            ("Bot[a-Z]x", "a range out of order, by case"),
            (r"Bot[\d-z]x", "a range from a set"),
            ("Botx{2,1}", "quantifier numbers out of order"),
            ("Botx{99999}", "a count past PCRE's limit"),
            (
                "Botx{123456789012345678901234567890}",
                "a count past any limit",
            ),
            ("Bot[[:nope:]]x", "an unknown POSIX class"),
            ("Bot[[.a.]]x", "a collating element"),
            ("[:alpha:]Bot", "a POSIX class outside a class"),
        ] {
            assert_eq!(
                pattern_problem(pattern),
                Some(PatternProblem::Invalid),
                "{pattern:?} ({why})"
            );
        }
    }

    /// Constructs this check cannot be sure of, refused rather than
    /// guessed at.
    #[test]
    fn a_pattern_whose_meaning_depends_on_the_pcre_version_is_refused() {
        for (pattern, why) in [
            ("Botx{,5}", "a quantifier only PCRE2 10.43 reads as one"),
            ("Botx{ 5 }", "likewise, with spaces"),
            ("Bot^*x", "a repeated anchor"),
            (r"Bot\b+x", "a repeated word boundary"),
            (
                "Bot(?:abcdefgh){1000}",
                "a group PCRE would write out a thousand times",
            ),
            ("Bot(?i)x", "an inline flag"),
            ("Bot(?#c)x", "a comment"),
            (r"Bot[\p]x", "an escape with its own syntax, in a class"),
        ] {
            assert_eq!(
                pattern_problem(pattern),
                Some(PatternProblem::Unchecked),
                "{pattern:?} ({why})"
            );
        }
    }

    /// And the class and quantifier forms that are fine stay fine.
    #[test]
    fn well_formed_classes_and_quantifiers_are_kept() {
        for pattern in [
            "Bot[a-z]x",
            "Bot[Z-a]x",
            "Bot[a-z-]x",
            "Bot[-a]x",
            "Bot[]a]x",
            "Bot[^]a]x",
            "Bot[[:alpha:]]x",
            r"Bot[\d\s.-]x",
            r"Bot[\]]x",
            "Botx{2}",
            "Botx{2,}",
            "Botx{0,1}",
            "Bot{}",
            "Bot{a}",
            "Bot x{",
        ] {
            assert_eq!(pattern_problem(pattern), None, "{pattern:?} must be kept");
        }
    }

    /// A path exemption stored before it was validated: `/x\|` reached
    /// PCRE as `^(/x\\|)` and exempted every path; `/x\(` failed `nginx
    /// -t`. Now neither is written, and the valid one beside it is.
    #[test]
    fn a_stored_path_exemption_that_is_not_a_literal_path_is_left_out() {
        let text = block_text(&cfg_exempt(&["BadBot"], &[r"/x\|", r"/x\(", "/blog"])).unwrap();
        assert!(
            text.contains(r#"if ($uri ~* "^(/blog)") {"#),
            "text was:\n{text}"
        );
        assert!(!text.contains("/x"), "text was:\n{text}");
    }

    /// A literal path is a literal: every regex character in one is
    /// escaped, and the escape reaches PCRE.
    #[test]
    fn a_path_exemption_reaches_pcre_as_a_literal_prefix() {
        let text = block_text(&cfg_exempt(&["BadBot"], &["/a.b+(c)"])).unwrap();
        let rest = text.split("if ($uri ~* ").nth(1).unwrap();
        let end = closing_quote(rest).unwrap();
        assert_eq!(nginx_unquoted(&rest[1..end]), r"^(/a\.b\+\(c\))");
    }

    /// A console `location` from before 0.0.13, unquoted, is still read as
    /// the console's; an edited one with a modifier is not.
    #[test]
    fn a_console_location_written_before_it_was_quoted_is_exempt_too() {
        // What 0.0.x wrote, and what a real host still carries.
        let site = "server {\n    listen 443 ssl;\n    server_name example.com;\n    \
            # BEGIN stop-bots console (DO NOT EDIT)\n    location /stop-bots/ {\n        \
            proxy_pass http://127.0.0.1:8787;\n    }\n    # END stop-bots console\n}\n";
        let blocks = parse_server_blocks(site);
        assert_eq!(
            console_prefix_in(site, &blocks[0]).as_deref(),
            Some("/stop-bots/"),
            "site was:\n{site}"
        );
        for edited in [
            "location = /stop-bots/ {",
            "location ^~ /stop-bots/ {",
            "location ~ ^/x {",
        ] {
            let site = site.replace("location /stop-bots/ {", edited);
            let blocks = parse_server_blocks(&site);
            assert_eq!(console_prefix_in(&site, &blocks[0]), None, "{edited}");
        }
    }

    /// The Web Access panel's path mode, then an apply: the block in the
    /// server that carries the console exempts the console's prefix, so a
    /// rule that would refuse the operator's own client cannot lock them
    /// out of the page that turns it off.
    #[test]
    fn the_server_that_serves_the_console_exempts_its_prefix() {
        let with_console =
            with_console_location(TWO_BLOCK_SITE, "example.com", "/stop-bots/", &upstream())
                .unwrap();
        let config = BlockConfig {
            request_rules: vec![RequestRule::NoAcceptLanguage],
            ..cfg(&["BadBot"])
        };
        let applied = blocks_for_file(&with_console, Path::new("site"), &[], &config).unwrap();
        crate::golden::assert_golden("nginx-console-path-blocked.conf", &applied);

        let blocks = parse_server_blocks(&applied);
        let tls = blocks.iter().find(|b| b.is_tls).unwrap();
        let plain = blocks.iter().find(|b| !b.is_tls).unwrap();
        let tls_block = current_block_text(&applied, tls).unwrap();
        assert!(
            tls_block.contains(r#"if ($uri ~* "^(/stop-bots/|/\.well-known/)") {"#),
            "the console's server must exempt it:\n{tls_block}"
        );
        // Only there: the redirect block serves no console.
        let plain_block = current_block_text(&applied, plain).unwrap();
        assert!(
            !plain_block.contains("/stop-bots/"),
            "a block without the console exempts nothing of it:\n{plain_block}"
        );
        // And the site reads as applied, not stale, with it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site");
        fs::write(&path, &applied).unwrap();
        assert_eq!(
            site_apply_status(&path, "example.com", &config, dir.path()),
            SiteApplyStatus::UpToDate
        );
    }

    /// What the render leaves out is named, so that it is not left out in
    /// silence.
    #[test]
    fn skipped_entries_names_every_stored_row_the_render_leaves_out() {
        let db = crate::db::Db::open_in_memory().unwrap();
        assert!(skipped_entries(&db).unwrap().is_empty());

        db.upsert_site("a.example", "/tmp/a").unwrap();
        let site = db.list_sites().unwrap()[0].id.to_string();
        db.insert_unvalidated_row_for_tests(
            "site_path_exemptions",
            &[("site_id", &site), ("path", r"/x\|")],
        );
        db.insert_unvalidated_row_for_tests(
            "site_agent_exemptions",
            &[
                ("site_id", &site),
                ("path", "/dav/"),
                ("user_agent", r#"ok"http"#),
            ],
        );
        db.insert_unvalidated_row_for_tests(
            "blocked_user_agents",
            &[("user_agent", "ab"), ("blocked_at", "0")],
        );
        // Stored before backslashes were refused, and written exactly now:
        // not left out.
        db.insert_unvalidated_row_for_tests(
            "blocked_user_agents",
            &[("user_agent", r"Scraper/2.1 a\|"), ("blocked_at", "0")],
        );

        let skipped = skipped_entries(&db).unwrap();
        // Named as `{:?}` spells them, so a control character shows.
        for needle in [r"/x\\|", r#"ok\"http"#, r#""ab""#] {
            assert!(
                skipped.iter().any(|line| line.contains(needle)),
                "{needle} is not named: {skipped:#?}"
            );
        }
        assert_eq!(skipped.len(), 3, "{skipped:#?}");
    }

    /// The redirect block declares the name first; the HTTPS block is
    /// the one that says what answers.
    #[test]
    fn site_service_reads_every_block_that_declares_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("media.conf");
        std::fs::write(
            &config,
            "server {\n    listen 80;\n    server_name media.example.com;\n    \
             return 301 https://$host$request_uri;\n}\n\
             server {\n    listen 443 ssl;\n    server_name media.example.com www.media.example.com;\n    \
             location / {\n        proxy_pass http://jellyfin:8096;\n    }\n}\n\
             server {\n    listen 443 ssl;\n    server_name photos.example.com;\n    \
             location / {\n        proxy_pass http://immich:2283;\n    }\n}\n",
        )
        .unwrap();

        let cases = [
            (
                "media.example.com",
                Some(crate::services::Service::Jellyfin),
            ),
            ("photos.example.com", Some(crate::services::Service::Immich)),
            ("elsewhere.example.com", None),
        ];
        for (name, expected) in cases {
            assert_eq!(site_service(&config, name), expected, "{name}");
        }
    }

    #[test]
    fn site_service_of_an_unreadable_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            site_service(&dir.path().join("missing.conf"), "a.example.com"),
            None
        );
    }
}
