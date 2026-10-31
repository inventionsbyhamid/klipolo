//! Sessions (`Mcp-Session-Id`): one per `initialize`, with the name its app gave and what of the project it last read.
//! At most `MAX_SESSIONS` are kept, the least recently used going first; one unused for `IDLE` goes, so the apps shown
//! as connected are the ones that are.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_model::text_safety::clean;
use serde_json::Value;

/// What an app that gives no name, or one that reads as Klipolo's own, is called.
pub const UNNAMED: &str = "Unnamed app";
/// The most characters of an app's name kept.
pub const NAME_CHARS: usize = 60;
/// Sessions kept at once (each a name and what it read: a few hundred bytes).
pub const MAX_SESSIONS: usize = 32;
/// A session unused this long goes; its app opens a new one when it comes back.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// What of the project a session last read: which project, and where its main track's clips were
/// (`klipolo_chat::timeline_key`). A call naming times or clip refs needs it to match the project as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub project_id: String,
    pub key: u64,
}

/// One app's session.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    /// The name its `initialize` gave (`client_name`).
    pub client: String,
    /// Set by reading the summary, the context or the transcript; None until then.
    pub seen: Option<Seen>,
    /// When it was last used.
    pub used: Instant,
}

/// The name an app gives in `initialize` (`clientInfo.name`), as Klipolo shows it: control and invisible characters
/// dropped, runs of spaces one space, at most `NAME_CHARS` characters. `UNNAMED` when it gives none, or one that reads
/// as Klipolo's own ("Klipolo", "Klipolo chat", "Klipolo's chat" in any case, spacing or punctuation): an app never
/// shows as Klipolo itself.
pub fn client_name(params: &Value) -> String {
    let raw = (params.pointer("/clientInfo/name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = clean(raw, NAME_CHARS);
    if name.is_empty() || reads_as_klipolo(&name) {
        UNNAMED.to_owned()
    } else {
        name
    }
}

/// Whether `name`'s letters and digits, in lower case, spell Klipolo or its chat.
fn reads_as_klipolo(name: &str) -> bool {
    let letters: String = (name.chars())
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(letters.as_str(), "klipolo" | "klipolochat" | "klipoloschat")
}

/// A session, locked while a message of it is handled, and its app's name.
struct Entry {
    session: Arc<Mutex<Session>>,
    client: String,
}

/// The sessions by id. Locked only to find, open or drop one: a session's message is handled under its own lock, so
/// each app's messages go one at a time and different apps' at once.
#[derive(Default)]
pub(crate) struct Sessions {
    map: HashMap<String, Entry>,
}

/// When a session was last used; one being handled now counts as used now.
fn last_used(entry: &Entry, now: Instant) -> Instant {
    match entry.session.try_lock() {
        Ok(session) => session.used,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().used,
        Err(std::sync::TryLockError::WouldBlock) => now,
    }
}

impl Sessions {
    /// A new session for `client`, with a new random id (32 hex digits); the least recently used goes when there are
    /// `MAX_SESSIONS` already. O(sessions).
    pub(crate) fn open(&mut self, client: String, now: Instant) -> String {
        if self.map.len() >= MAX_SESSIONS {
            let oldest = (self.map.iter())
                .min_by_key(|(_, e)| last_used(e, now))
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.map.remove(&id);
            }
        }
        let id = loop {
            let id = format!("{:032x}", rand::random::<u128>());
            if !self.map.contains_key(&id) {
                break id;
            }
        };
        let session = Session {
            id: id.clone(),
            client: client.clone(),
            seen: None,
            used: now,
        };
        self.map.insert(
            id.clone(),
            Entry {
                session: Arc::new(Mutex::new(session)),
                client,
            },
        );
        id
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Mutex<Session>>> {
        self.map.get(id).map(|e| Arc::clone(&e.session))
    }

    /// The apps connected now, each once, sorted. O(sessions).
    pub(crate) fn connected(&self) -> Vec<String> {
        let mut apps: Vec<String> = self.map.values().map(|e| e.client.clone()).collect();
        apps.sort();
        apps.dedup();
        apps
    }

    /// Drops the sessions unused for `IDLE` at `now`; true when any went. O(sessions).
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        let before = self.map.len();
        self.map
            .retain(|_, e| now.saturating_duration_since(last_used(e, now)) < IDLE);
        self.map.len() != before
    }
}

/// `mutex`'s value, even if a thread panicked holding it: sessions hold nothing a half-done message could break.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
