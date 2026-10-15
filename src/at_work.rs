//! What she has going right now, for whoever asks from the terminal.
//!
//! A count of busy conversations says she is doing something but not what,
//! and the things worth knowing are spread out: the board holds what a
//! chef de partie claimed, the expediter holds what is queued behind a turn,
//! and only the turn and its commis know when they started. None of that is kept
//! anywhere, so it is written down here while it happens and read back
//! whole by `sous-chef status`.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct AtWork {
    turns: Mutex<HashMap<String, Instant>>,
    /// By conversation. A commis works on after the turn that started it is
    /// over, so its chef de partie can listen meanwhile.
    commis: Mutex<HashMap<String, Vec<Commis>>>,
}

pub struct Commis {
    pub id: String,
    pub project: String,
    /// None when it works here.
    pub station: Option<String>,
    pub began: Instant,
}

/// A conversation with a turn running, commis working, or both. Its turn's
/// time is none between turns.
pub struct Going {
    pub conversation: String,
    pub taken: Option<Duration>,
    pub commis: Vec<CommisAt>,
}

pub struct CommisAt {
    pub commis: String,
    pub project: String,
    pub station: Option<String>,
    pub taken: Duration,
}

/// A turn is taken off the picture however it ends — a reply, an error, a
/// panic — so a crashed turn doesn't sit in `sous-chef status` for ever.
pub struct WhileItRuns {
    at_work: Arc<AtWork>,
    conversation: String,
}

impl AtWork {
    pub fn turn_began(self: &Arc<AtWork>, conversation: &str) -> WhileItRuns {
        self.turns.lock().unwrap().insert(conversation.to_string(), Instant::now());
        WhileItRuns { at_work: self.clone(), conversation: conversation.to_string() }
    }

    pub fn commis_began(&self, conversation: &str, id: &str, project: &str, station: Option<&str>) {
        let commis = Commis { id: id.to_string(), project: project.to_string(), station: station.map(String::from), began: Instant::now() };
        self.commis.lock().unwrap().entry(conversation.to_string()).or_default().push(commis);
    }

    pub fn commis_ended(&self, conversation: &str, id: &str) {
        let mut brigade = self.commis.lock().unwrap();
        if let Some(working) = brigade.get_mut(conversation) {
            working.retain(|commis| commis.id != id);
            if working.is_empty() {
                brigade.remove(conversation);
            }
        }
    }

    /// Every conversation with something going, by name, with the clock
    /// already read so the caller doesn't hold the locks to do it. By name
    /// rather than by how long each has run, because turns start and end
    /// every few seconds and a list in that order never holds still.
    pub fn going_on(&self) -> Vec<Going> {
        let now = Instant::now();
        let turns = self.turns.lock().unwrap();
        let brigade = self.commis.lock().unwrap();
        let conversations: BTreeSet<&String> = turns.keys().chain(brigade.keys()).collect();
        conversations
            .into_iter()
            .map(|conversation| Going {
                conversation: conversation.clone(),
                taken: turns.get(conversation).map(|began| now - *began),
                commis: brigade
                    .get(conversation)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|commis| CommisAt {
                        commis: commis.id.clone(),
                        project: commis.project.clone(),
                        station: commis.station.clone(),
                        taken: now - commis.began,
                    })
                    .collect(),
            })
            .collect()
    }
}

impl Drop for WhileItRuns {
    fn drop(&mut self) {
        self.at_work.turns.lock().unwrap().remove(&self.conversation);
    }
}

/// `38m`, `2h 5m`, `12s` — long enough to see at a glance whether something
/// is stuck.
pub fn how_long(taken: Duration) -> String {
    let seconds = taken.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_and_commis_are_there_while_they_run_and_gone_after() {
        let at_work = Arc::new(AtWork::default());
        assert!(at_work.going_on().is_empty());

        let running = at_work.turn_began("basecamp-one");
        at_work.commis_began("basecamp-one", "h1", "/home/x/shop", None);
        at_work.commis_began("basecamp-one", "h2", "/home/x/blog", Some("server"));
        at_work.commis_ended("basecamp-one", "h1");

        let going = at_work.going_on();
        assert_eq!(going.len(), 1);
        assert_eq!(going[0].conversation, "basecamp-one");
        assert!(going[0].taken.is_some());
        assert_eq!(going[0].commis.len(), 1, "the commis that ended is gone, the other one is still there");
        assert_eq!(going[0].commis[0].project, "/home/x/blog");
        assert_eq!(going[0].commis[0].station.as_deref(), Some("server"), "where it works is kept with it");

        drop(running);
        let going = at_work.going_on();
        assert_eq!(going.len(), 1, "the commis works on after its turn is over");
        assert_eq!((going[0].taken, going[0].commis[0].commis.as_str()), (None, "h2"));

        at_work.commis_ended("basecamp-one", "h2");
        assert!(at_work.going_on().is_empty());
    }

    #[test]
    fn what_is_going_on_holds_its_order_by_name_whatever_starts_or_ends() {
        let at_work = Arc::new(AtWork::default());
        at_work.commis_began("basecamp-two", "h1", "/home/x/shop", None);
        let longer = at_work.turn_began("basecamp-three");
        let newer = at_work.turn_began("basecamp-one");

        let names = |at_work: &AtWork| at_work.going_on().into_iter().map(|it| it.conversation).collect::<Vec<_>>();
        assert_eq!(names(&at_work), ["basecamp-one", "basecamp-three", "basecamp-two"]);
        drop(newer);
        assert_eq!(names(&at_work), ["basecamp-three", "basecamp-two"], "what ended leaves, and the rest stay where they were");
        drop(longer);
    }

    #[test]
    fn how_long_reads_at_a_glance() {
        assert_eq!(how_long(Duration::from_secs(12)), "12s");
        assert_eq!(how_long(Duration::from_secs(60)), "1m");
        assert_eq!(how_long(Duration::from_secs(38 * 60 + 40)), "38m");
        assert_eq!(how_long(Duration::from_secs(2 * 3600 + 5 * 60)), "2h 5m");
    }
}
