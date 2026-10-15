//! How Basecamp is listened to, worked out against the real thing: as an
//! agent of the chef's own, or as one of their users.

use serde_json::json;
use std::collections::BTreeMap;

use crate::config::{Call, Cursor, Latest, Listing, Pointers, Waiter};
use crate::setup::words;

/// Basecamp pushes nothing an agent or a user's command line can listen
/// to, so both are asked. One call, and no model, so often enough that a
/// mention is heard the way a person would hear it.
pub const EVERY_SECONDS: u64 = 15;

const NOTIFICATIONS_NOTE: &str = "That is the notification, not the comment: its excerpt and link are from whatever first put this on your desk, and stay the same however many comments follow. Being woken by it again means someone said something new there that you haven't seen. List the comments with your Basecamp tools and read the newest before you decide anything, every time, and answer where it was said, with those tools. Write what you post as Basecamp's editor does: plain text with <br><br> between paragraphs, <strong>, <code> and <a> where they help, <pre> for a block of code or a command with its lines as they are, and <ul><li> for a list. Not <p>: Basecamp shows adjacent paragraphs with no space between them.";

const INBOX_NOTE: &str ="That is an item from your Basecamp inbox, not what they wrote. Its lines are, in order: why it reached you, the kind of event, the project (bucket) id, the recording id, and the event's details when it has any. Read the recording with your Basecamp tools before you do anything, and answer where you were addressed: a comment on that recording, or on what it belongs to when the recording is itself a comment, and a line in the Campfire when it was said in a Campfire. Write what you post as Basecamp's editor does: plain text with <br><br> between paragraphs, <strong>, <code> and <a> where they help, <pre> for a block of code or a command with its lines as they are, and <ul><li> for a list. Not <p>: Basecamp shows adjacent paragraphs with no space between them.";

/// A Basecamp waiter an older setup wrote, as this one would write it: asked
/// every EVERY_SECONDS, without the `watch` the command line never got, and
/// with today's note. Says whether anything changed. Anything that isn't one
/// of setup's Basecamp waiters is left as it is.
pub fn up_to_date(waiter: &mut Waiter) -> bool {
    let (note, first) = match waiter.watch.tool.as_str() {
        "basecamp_eventfeed" => (INBOX_NOTE, "That is an item from your Basecamp inbox"),
        "basecamp_account" => (NOTIFICATIONS_NOTE, "That is the notification, not the comment"),
        _ => return false,
    };
    if waiter.server != "basecamp" {
        return false;
    }
    let before = waiter.clone();
    if [60, 900].contains(&waiter.every_seconds) {
        waiter.every_seconds = EVERY_SECONDS;
    }
    if waiter.trigger.as_ref().is_some_and(|it| it.args.iter().any(|arg| arg == "watch")) {
        waiter.trigger = None;
    }
    if waiter.note.as_deref().is_some_and(|it| it.starts_with(first)) {
        waiter.note = Some(note.to_string());
    }
    *waiter != before
}

/// How an agent hears things in Basecamp. It has no notifications — Basecamp
/// refuses an agent everything it hasn't opened to agents, the "Hey!" menu
/// included — and gets an inbox of its own instead: every event that reached
/// it, with why (mentioned, assigned, pinged, subscribed…). The inbox is
/// read from a position, and an item is an event, not what someone wrote: it
/// says who, in which project and on which recording, so the chef de partie is
/// told to read the recording before it acts. People are reported by id,
/// which is why the trusted ones were looked up when they were named. There
/// is no single call that answers, so the chef de partie answers with
/// Basecamp's own tools. `anyone` lets people who aren't trusted hand it
/// work; Basecamp already decides who can reach an agent at all — the people
/// in the projects it was added to.
pub fn agent_waiter(anyone: bool) -> Waiter {
    Waiter {
        server: "basecamp".to_string(),
        watch: Call { tool: "basecamp_eventfeed".to_string(), arguments: json!({ "action": "poll_inbox", "params": {} }) },
        every_seconds: EVERY_SECONDS,
        items: "/items".to_string(),
        // One event reaches the agent once per reason — mentioned, and
        // subscribed — and is one message, not two
        id: Pointers::One("/event/id".to_string()),
        conversation: "/event/recording_id".to_string(),
        sender: Some("/event/creator_id".to_string()),
        sender_name: None,
        anyone,
        trusted: BTreeMap::new(),
        text: Pointers::Several(words(&["/reason", "/event/kind", "/event/bucket_id", "/event/recording_id", "/event/details"])),
        reply: None,
        handled: None,
        cursor: Some(Cursor { from: "/position".to_string(), into: "/params/position".to_string() }),
        note: Some(INBOX_NOTE.to_string()),
        latest: None,
        trigger: None,
    }
}

/// How a user of the agent's own hears things in Basecamp: its
/// notifications, the "Hey!" menu, worked out against the real thing. The
/// same notification is bumped for every new comment, so what makes one
/// new is its id and when it went unread; the conversation is what the
/// notification is about — the to-do, the message — named by its
/// subscription, because a comment, a mention and an assignment on one to-do
/// are each a notification of their own; the body is an excerpt, so the
/// chef de partie gets the title, the excerpt and the link to read the rest;
/// and there is no single call that answers, so the chef de partie answers
/// with Basecamp's own tools. The profile lives in the chef's own command
/// line config, where they logged it in, so nothing is pointed elsewhere.
pub fn notifications_waiter(anyone: bool) -> Waiter {
    Waiter {
        server: "basecamp".to_string(),
        watch: Call { tool: "basecamp_account".to_string(), arguments: json!({ "action": "get_my_notifications" }) },
        every_seconds: EVERY_SECONDS,
        items: "/unreads".to_string(),
        id: Pointers::Several(words(&["/id", "/unread_at"])),
        conversation: "/subscription_url".to_string(),
        sender: Some("/creator/id".to_string()),
        sender_name: Some("/creator/name".to_string()),
        anyone,
        trusted: BTreeMap::new(),
        text: Pointers::Several(words(&["/title", "/content_excerpt", "/app_url"])),
        reply: None,
        handled: None,
        cursor: None,
        note: Some(NOTIFICATIONS_NOTE.to_string()),
        // The unread item names whoever first put the recording on her
        // desk; the comments say who spoke last
        latest: Some(Latest {
            listing: Listing {
                call: Call {
                    tool: "basecamp_messages".to_string(),
                    arguments: json!({ "action": "list_comments", "params": { "recordingId": "{recording}", "page": "{page}" } }),
                },
                items: Pointers::Several(words(&["", "/results"])),
                sender: "/creator/id".to_string(),
                sender_name: Some("/creator/name".to_string()),
                text: "/content".to_string(),
                at: None,
                when: None,
            },
            also: Vec::new(),
            opening: None,
        }),
        trigger: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Trigger;

    #[test]
    fn a_basecamp_waiter_an_older_setup_wrote_is_brought_up_to_date() {
        let mut agent = agent_waiter(false);
        agent.every_seconds = 60;
        agent.note = Some("That is an item from your Basecamp inbox, not what they wrote. … answer with a new message on that project's message board".to_string());
        agent.trigger = Some(Trigger { command: "basecamp".to_string(), args: words(&["--profile", "botten", "watch", "--json"]), env: Default::default() });

        assert!(up_to_date(&mut agent));
        assert_eq!(agent, agent_waiter(false), "as setup writes it today");
        assert!(!up_to_date(&mut agent), "and only once");

        let mut chosen = notifications_waiter(true);
        chosen.every_seconds = 30;
        chosen.note = Some("Something the chef wrote themselves".to_string());
        assert!(!up_to_date(&mut chosen), "an interval and a note someone picked are theirs");

        let mut elsewhere = agent_waiter(false);
        elsewhere.server = "hey".to_string();
        elsewhere.every_seconds = 60;
        assert!(!up_to_date(&mut elsewhere));
    }
}
