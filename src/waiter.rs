//! Reading a waiter: turning what a server's watch tool answers into
//! messages, remembering which ones were already seen, and answering back
//! into the conversation a message came from.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Call, Cursor, Latest, Listing, Opening, Pointers, Waiter};
use crate::conversation::{Conversation, Origin};
use crate::fsutil;
use crate::logs;
use crate::mcp::Catalog;
use crate::paths;

const LONGEST_KEY_PART: usize = 32;

#[derive(Debug, PartialEq)]
pub struct Message {
    pub id: String,
    pub conversation: String,
    /// Empty only from a waiter whose items name nobody, until `latest`
    /// has said who spoke.
    pub sender: String,
    pub sender_name: Option<String>,
    pub text: String,
    /// The item as the watch tool gave it, for `latest` to read values off.
    pub item: Value,
}

pub fn messages_in(waiter: &Waiter, answer: &str) -> Result<Vec<Message>> {
    let answer: Value = serde_json::from_str(answer).context("the watch tool did not answer with JSON")?;
    let items = answer
        .pointer(&waiter.items)
        .and_then(Value::as_array)
        .with_context(|| format!("there is no list at {} in the watch tool's answer", waiter.items))?;

    Ok(items
        .iter()
        .filter_map(|item| {
            let sender = match &waiter.sender {
                Some(pointer) => text_at(item, pointer)?,
                None => String::new(),
            };
            Some(Message {
                id: all_at(item, &waiter.id, ":")?,
                conversation: text_at(item, &waiter.conversation)?,
                sender,
                sender_name: waiter.sender_name.as_deref().and_then(|it| text_at(item, it)),
                text: all_at(item, &waiter.text, "\n\n")?,
                item: item.clone(),
            })
        })
        .collect())
}

/// What someone said, as the waiter reports it, and when, where the
/// waiter says.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub sender: String,
    pub sender_name: Option<String>,
    pub text: String,
    pub at: Option<String>,
}

const MOST_PAGES: u64 = 30;

/// Who spoke last where the message points: the newest of what each of the
/// waiter's listings that applies to it has, by when it was said. When
/// nothing was said there, whoever opened it, if the waiter can ask; None
/// otherwise.
pub fn newest_spoken(catalog: &Catalog, waiter: &Waiter, latest: &Latest, message: &Message) -> Result<Option<Spoken>> {
    let recording = last_id_in(&message.conversation).context("the conversation's name carries no id to ask about")?;
    let mut newest = None;
    for listing in std::iter::once(&latest.listing).chain(&latest.also).filter(|it| applies(it, &message.item)) {
        newest = newer(newest, newest_in(catalog, waiter, listing, &recording, &message.item)?);
    }
    match (newest, &latest.opening) {
        (None, Some(opening)) => {
            let arguments = filled_for(&opening.call.arguments, &recording, 1, &message.item);
            let answer = catalog.call(&waiter.server, &opening.call.tool, &arguments)?;
            opened_by(opening, &answer)
        }
        (newest, _) => Ok(newest),
    }
}

/// Tells the server a message was dealt with, for a waiter that can.
pub fn mark_handled(catalog: &Catalog, waiter: &Waiter, item: &Value) -> Result<()> {
    if let Some(handled) = &waiter.handled {
        catalog.call(&waiter.server, &handled.tool, &filled_for(&handled.arguments, "", 1, item))?;
    }
    Ok(())
}

/// The later of the two by when it was said; on a tie, or with no time to
/// go by, the one already found, which is the first listing's.
fn newer(found: Option<Spoken>, candidate: Option<Spoken>) -> Option<Spoken> {
    match (found, candidate) {
        (Some(found), Some(candidate)) if candidate.at > found.at => Some(candidate),
        (None, candidate) => candidate,
        (found, _) => found,
    }
}

fn applies(listing: &Listing, item: &Value) -> bool {
    match &listing.when {
        Some(when) => text_at(item, &when.pointer).is_some_and(|it| it == when.is),
        None => true,
    }
}

/// The last item of the last page that has any, asked page by page until
/// one comes back empty.
fn newest_in(catalog: &Catalog, waiter: &Waiter, listing: &Listing, recording: &str, item: &Value) -> Result<Option<Spoken>> {
    let mut newest = None;
    for page in 1..=MOST_PAGES {
        let arguments = filled_for(&listing.call.arguments, recording, page, item);
        let answer = catalog.call(&waiter.server, &listing.call.tool, &arguments)?;
        match last_on_page(listing, &answer)? {
            Some(spoken) => newest = Some(spoken),
            None => break,
        }
    }
    Ok(newest)
}

fn last_id_in(conversation: &str) -> Option<String> {
    conversation.rsplit(|it: char| !it.is_ascii_digit()).find(|it| !it.is_empty()).map(String::from)
}

fn filled_for(template: &Value, recording: &str, page: u64, item: &Value) -> Value {
    match template {
        Value::String(it) if it == "{recording}" => number_or_text(recording),
        Value::String(it) if it == "{page}" => Value::from(page),
        Value::String(it) if it.starts_with("{/") && it.ends_with('}') => item.pointer(&it[1..it.len() - 1]).cloned().unwrap_or(Value::Null),
        Value::String(it) => Value::from(it.replace("{recording}", recording).replace("{page}", &page.to_string())),
        Value::Array(items) => items.iter().map(|it| filled_for(it, recording, page, item)).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(name, value)| (name.clone(), filled_for(value, recording, page, item)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
}

fn opened_by(opening: &Opening, answer: &str) -> Result<Option<Spoken>> {
    let answer: Value = serde_json::from_str(answer).context("the opening did not answer with JSON")?;
    Ok(text_at(&answer, &opening.sender).map(|sender| Spoken {
        sender,
        sender_name: opening.sender_name.as_deref().and_then(|it| text_at(&answer, it)),
        text: text_at(&answer, &opening.text).unwrap_or_default(),
        at: None,
    }))
}

/// The last item on a page, or None for an empty page. The list is looked
/// for at each of the pointers, since a server's first page and its later
/// ones can differ in shape. An item with nothing written — a review that
/// is only its line comments — still says who spoke.
fn last_on_page(listing: &Listing, answer: &str) -> Result<Option<Spoken>> {
    let answer: Value = serde_json::from_str(answer).context("the listing did not answer with JSON")?;
    let items = listing
        .items
        .each()
        .into_iter()
        .find_map(|pointer| answer.pointer(pointer).and_then(Value::as_array))
        .with_context(|| format!("there is no list at {} in the listing's answer", listing.items.each().join(" or ")))?;
    Ok(items.last().and_then(|item| {
        Some(Spoken {
            sender: text_at(item, &listing.sender)?,
            sender_name: listing.sender_name.as_deref().and_then(|it| text_at(item, it)),
            text: text_at(item, &listing.text).unwrap_or_default(),
            at: listing.at.as_deref().and_then(|it| text_at(item, it)),
        })
    }))
}

/// The values at every pointer, joined. A pointer that finds nothing is
/// skipped, as long as one of them finds something.
fn all_at(item: &Value, pointers: &Pointers, separator: &str) -> Option<String> {
    let found: Vec<String> = pointers.each().into_iter().filter_map(|it| text_at(item, it)).collect();
    if found.is_empty() {
        None
    } else {
        Some(found.join(separator))
    }
}

/// Text and numbers as they are; anything with structure — an event's
/// details, say — as the JSON it is, since a chef de partie reads that fine.
fn text_at(item: &Value, pointer: &str) -> Option<String> {
    match item.pointer(pointer)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null | Value::Bool(_) => None,
        structured => Some(structured.to_string()),
    }
}

/// Where a waiter that is read from a position has got to.
pub struct Position {
    path: PathBuf,
    cursor: Cursor,
}

impl Position {
    pub fn of(waiter_name: &str, waiter: &Waiter) -> Option<Position> {
        let cursor = waiter.cursor.clone()?;
        Some(Position { path: paths::waiter_dir(waiter_name).join("position"), cursor })
    }

    /// The watch arguments, with the position put where the waiter wants it
    /// when there is one. The folders on the way are made: `/params/position`
    /// works whether or not the arguments already have `params`.
    pub fn in_arguments(&self, arguments: &Value) -> Value {
        match fs::read_to_string(&self.path) {
            Ok(position) if !position.trim().is_empty() => put_at(arguments, &self.cursor.into, position.trim()),
            _ => arguments.clone(),
        }
    }

    /// Only once everything in the answer has been handed on: a position
    /// moved first would lose the messages of a look that then failed.
    pub fn move_to_where(&self, answer: &str) -> Result<()> {
        let answer: Value = serde_json::from_str(answer).context("the watch tool did not answer with JSON")?;
        match text_at(&answer, &self.cursor.from) {
            Some(position) => fsutil::write_private(&self.path, &position),
            None => Ok(()),
        }
    }
}

fn put_at(arguments: &Value, pointer: &str, value: &str) -> Value {
    let mut arguments = arguments.clone();
    if !arguments.is_object() {
        arguments = json!({});
    }

    let names: Vec<&str> = pointer.split('/').skip(1).collect();
    let mut place = &mut arguments;
    for name in &names[..names.len().saturating_sub(1)] {
        if !place[*name].is_object() {
            place[*name] = json!({});
        }
        place = &mut place[*name];
    }
    if let Some(last) = names.last() {
        place[*last] = Value::from(value);
    }
    arguments
}

/// The ids of messages already handled, kept on disk so a restart doesn't
/// answer everything again.
pub struct Seen {
    path: PathBuf,
    ids: BTreeSet<String>,
    first_look: bool,
}

impl Seen {
    pub fn load(waiter_name: &str) -> Seen {
        Seen::load_from(paths::waiter_dir(waiter_name).join("seen.json"))
    }

    /// Whether this message is one to act on. Everything found on the very
    /// first look at a waiter is only remembered: that is the backlog from
    /// before she was listening, not something anyone asked her.
    pub fn is_new(&mut self, id: &str) -> bool {
        self.ids.insert(id.to_string()) && !self.first_look
    }

    /// For a message that couldn't be acted on yet: it is new again at the
    /// next look.
    pub fn forget(&mut self, id: &str) {
        self.ids.remove(id);
    }

    /// Written whole and then moved into place: a crash half-way through
    /// would otherwise leave a file that reads as a first look, and the
    /// next look would pass over everything new as backlog.
    pub fn save(&mut self) -> Result<()> {
        self.first_look = false;
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        fsutil::write_private(&self.path, &serde_json::to_string(&self.ids)?)
    }

    fn load_from(path: PathBuf) -> Seen {
        match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(ids) => Seen { path, ids, first_look: false },
                Err(error) => {
                    logs::event("waiter.seen_unreadable", json!({ "file": path, "error": error.to_string() }));
                    Seen { path, ids: BTreeSet::new(), first_look: true }
                }
            },
            Err(_) => Seen { path, ids: BTreeSet::new(), first_look: true },
        }
    }
}

/// A conversation on a waiter. Saying something is the waiter's reply call
/// with the conversation and the text filled in — when the waiter has one.
/// Where answering isn't a single call, there is nothing generic to say it
/// with, and the chef de partie is told to use the server's own tools.
pub struct Printed {
    key: String,
    waiter_name: String,
    conversation: String,
    server: String,
    reply: Option<Call>,
    catalog: Arc<Catalog>,
}

impl Printed {
    pub fn new(waiter_name: &str, waiter: &Waiter, conversation: &str, catalog: Arc<Catalog>) -> Printed {
        Printed {
            key: format!("{waiter_name}-{}", key_part(conversation)),
            waiter_name: waiter_name.to_string(),
            conversation: conversation.to_string(),
            server: waiter.server.clone(),
            reply: waiter.reply.clone(),
            catalog,
        }
    }
}

impl Conversation for Printed {
    fn key(&self) -> &str {
        &self.key
    }

    fn origin(&self) -> Origin {
        Origin {
            waiter: self.waiter_name.clone(),
            conversation: self.conversation.clone(),
        }
    }

    fn say(&self, text: &str) -> Result<()> {
        match &self.reply {
            Some(reply) => {
                let arguments = filled(&reply.arguments, &self.conversation, text);
                self.catalog.call(&self.server, &reply.tool, &arguments)?;
                Ok(())
            }
            None => bail!("there is no single way to reply on {}", self.server),
        }
    }

    fn answered_otherwise(&self) -> Option<String> {
        match self.reply {
            Some(_) => None,
            None => Some(format!(
                "There is no reply tool in this conversation. Answer where the message came from, using the {} tools and the link in the message. Anything you write to a person there is held to the same style as a reply.",
                self.server
            )),
        }
    }
}

/// The conversation's id, made safe for a folder name and short enough for a
/// socket path, which is capped at 108 bytes. A long id — Basecamp's signed
/// ids run to hundreds of characters — keeps its start and gets a hash of the
/// whole, so the same conversation always lands on the same chef de partie.
fn key_part(conversation: &str) -> String {
    let safe: String = conversation
        .chars()
        .map(|it| if it.is_ascii_alphanumeric() { it } else { '-' })
        .collect();

    // Only an id that went through unchanged can stand for itself: `room/a`
    // and `room-a` are different conversations that would otherwise share a
    // chef de partie, its history and its standing
    if safe.len() <= LONGEST_KEY_PART && safe == conversation {
        safe
    } else {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in conversation.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{}-{hash:016x}", &safe[..safe.len().min(12)])
    }
}

fn filled(template: &Value, conversation: &str, text: &str) -> Value {
    match template {
        Value::String(it) if it == "{conversation}" => number_or_text(conversation),
        Value::String(it) => Value::from(it.replace("{conversation}", conversation).replace("{text}", text)),
        Value::Array(items) => items.iter().map(|it| filled(it, conversation, text)).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(name, value)| (name.clone(), filled(value, conversation, text)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
}

/// An id that came in as a number goes back out as one; servers that take a
/// numeric id usually refuse a string.
fn number_or_text(value: &str) -> Value {
    match value.parse::<i64>() {
        Ok(number) => Value::from(number),
        Err(_) => Value::from(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn waiter() -> Waiter {
        serde_json::from_value(json!({
            "server": "basecamp",
            "watch": { "tool": "my_notifications" },
            "items": "/unread",
            "id": "/id",
            "conversation": "/recording/id",
            "sender": "/creator/email",
            "text": "/content",
            "reply": { "tool": "create_comment", "arguments": { "recording_id": "{conversation}", "content": "{text}" } },
        }))
        .unwrap()
    }

    #[test]
    fn messages_are_picked_out_of_the_answer_by_pointer() {
        let answer = json!({ "unread": [
            { "id": 11, "recording": { "id": 900 }, "creator": { "email": "marta@example.com" }, "content": "Can you look at this?" },
            { "id": 12, "recording": { "id": 901 }, "creator": {}, "content": "No sender, so skipped" },
        ]});

        assert_eq!(
            messages_in(&waiter(), &answer.to_string()).unwrap(),
            [Message {
                id: "11".to_string(),
                conversation: "900".to_string(),
                sender: "marta@example.com".to_string(),
                sender_name: None,
                text: "Can you look at this?".to_string(),
                item: answer["unread"][0].clone(),
            }]
        );
        assert_eq!(waiter().every_seconds, 60);
        assert!(messages_in(&waiter(), "not json").is_err());
        assert!(messages_in(&waiter(), "{}").is_err());
    }

    #[test]
    fn a_basecamp_notification_is_new_again_when_its_thread_gets_another_comment() {
        let basecamp: Waiter = serde_json::from_value(json!({
            "server": "basecamp",
            "watch": { "tool": "basecamp_account", "arguments": { "action": "get_my_notifications" } },
            "trigger": { "command": "basecamp", "args": ["watch", "--json"] },
            "items": "/unreads",
            "id": ["/id", "/unread_at"],
            "conversation": "/readable_sgid",
            "sender": "/creator/email_address",
            "text": ["/title", "/content_excerpt", "/app_url"],
        }))
        .unwrap();
        assert_eq!(basecamp.reply, None);

        let reading = |unread_at: &str| {
            json!({ "unreads": [{
                "id": 501,
                "unread_at": unread_at,
                "readable_sgid": "BAh7CEkiCG".repeat(20),
                "creator": { "email_address": "marta@example.com" },
                "title": "Re: Deploy checklist",
                "content_excerpt": "Can you take the staging part?",
                "app_url": "https://3.basecamp.com/1/buckets/2/messages/3",
            }]})
            .to_string()
        };

        let first = messages_in(&basecamp, &reading("2026-09-21T08:00:00Z")).unwrap().remove(0);
        let second = messages_in(&basecamp, &reading("2026-09-21T09:30:00Z")).unwrap().remove(0);
        assert_eq!(first.id, "501:2026-09-21T08:00:00Z");
        assert_ne!(first.id, second.id);
        assert_eq!(first.conversation, second.conversation);
        assert_eq!(
            first.text,
            "Re: Deploy checklist\n\nCan you take the staging part?\n\nhttps://3.basecamp.com/1/buckets/2/messages/3"
        );
    }

    #[test]
    fn long_conversation_ids_are_shortened_the_same_way_every_time() {
        let sgid = "BAh7CEkiCG--".repeat(30);

        assert_eq!(key_part("900"), "900");
        assert_eq!(key_part("thread-7"), "thread-7");
        assert!(key_part("thread/7").starts_with("thread-7-"));
        assert_ne!(key_part("thread/7"), key_part("thread-7"), "ids that only look alike once made safe are still two conversations");
        assert_ne!(key_part("thread/7"), key_part("thread.7"));
        assert_eq!(key_part(&sgid).len(), 12 + 1 + 16);
        assert_ne!(key_part(&sgid), key_part(&format!("{sgid}x")));
    }

    #[test]
    fn a_waiter_read_from_a_position_carries_it_into_the_next_call() {
        let inbox: Waiter = serde_json::from_value(json!({
            "server": "basecamp",
            "watch": { "tool": "basecamp_eventfeed", "arguments": { "action": "poll_inbox", "params": {} } },
            "items": "/items",
            "id": "/event/id",
            "conversation": "/event/recording_id",
            "sender": "/event/creator_id",
            "text": ["/reason", "/event/kind", "/event/bucket_id", "/event/recording_id", "/event/details"],
            "cursor": { "from": "/position", "into": "/params/position" },
        }))
        .unwrap();
        let answer = json!({
            "items": [{
                "addressing_id": 71, "reason": "mentioned", "addressed_at": "2026-09-21T18:00:00Z",
                "event": { "id": 9001, "kind": "comment_created", "bucket_id": 2, "creator_id": 1001, "recording_id": 3, "details": { "excerpt": "Can you look?" } },
            }],
            "position": "p-71",
        })
        .to_string();

        let message = messages_in(&inbox, &answer).unwrap().remove(0);
        assert_eq!(message.id, "9001", "one event, however many reasons it reached her for");
        assert_eq!(message.sender, "1001", "people are reported by id");
        assert_eq!(message.conversation, "3");
        assert_eq!(message.text, "mentioned\n\ncomment_created\n\n2\n\n3\n\n{\"excerpt\":\"Can you look?\"}");

        let path = std::env::temp_dir().join(format!("sous-chef-position-{}/position", std::process::id()));
        let position = Position { path: path.clone(), cursor: inbox.cursor.clone().unwrap() };
        assert_eq!(position.in_arguments(&inbox.watch.arguments), inbox.watch.arguments, "the first call goes out as written: from now");

        position.move_to_where(&answer).unwrap();
        assert_eq!(position.in_arguments(&inbox.watch.arguments), json!({ "action": "poll_inbox", "params": { "position": "p-71" } }));
        assert_eq!(position.in_arguments(&json!({ "action": "poll_inbox" }))["params"]["position"], "p-71", "the way there is made when it's missing");

        position.move_to_where(&json!({ "items": [] }).to_string()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "p-71", "an answer without a position leaves her where she was");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn who_spoke_last_is_the_last_item_of_the_last_page_that_has_any() {
        let latest: Latest = serde_json::from_value(json!({
            "call": { "tool": "basecamp_messages", "arguments": { "action": "list_comments", "params": { "recordingId": "{recording}", "page": "{page}" } } },
            "items": ["", "/results"],
            "sender": "/creator/id", "sender_name": "/creator/name", "text": "/content",
        }))
        .unwrap();
        let listing = &latest.listing;

        let first_page = r#"[{"creator":{"id":1,"name":"Sam"},"content":"<p>first</p>"},{"creator":{"id":2,"name":"Marta"},"content":"go"}]"#;
        let later_page = r#"{"next_page":3,"results":[{"creator":{"id":3,"name":"Ivo"},"content":"wait"}]}"#;
        let empty = r#"{"next_page":4,"results":[]}"#;
        assert_eq!(last_on_page(listing, first_page).unwrap(), Some(Spoken { sender: "2".to_string(), sender_name: Some("Marta".to_string()), text: "go".to_string(), at: None }));
        assert_eq!(last_on_page(listing, later_page).unwrap().unwrap().sender, "3", "a later page wraps its list");
        assert_eq!(last_on_page(listing, empty).unwrap(), None);
        assert!(last_on_page(listing, r#"{"error":"nope"}"#).is_err());

        let filled = filled_for(&listing.call.arguments, "12345678901", 2, &json!({}));
        assert_eq!(filled, json!({ "action": "list_comments", "params": { "recordingId": 12345678901u64, "page": 2 } }));
        assert_eq!(last_id_in("https://3.basecampapi.com/1234567/buckets/87654321/recordings/12345678901/subscription.json").as_deref(), Some("12345678901"));
        assert_eq!(last_id_in("main"), None);
    }

    #[test]
    fn of_comments_and_reviews_the_newest_is_who_spoke_last() {
        let latest: Latest = serde_json::from_value(json!({
            "call": { "tool": "issue_read" }, "items": "", "sender": "/user/login", "text": "/body", "at": "/created_at",
            "also": [{
                "call": { "tool": "pull_request_read" }, "items": "", "sender": "/user/login", "text": "/body", "at": "/submitted_at",
                "when": { "pointer": "/subject/type", "is": "PullRequest" },
            }],
        }))
        .unwrap();
        let reviews = &latest.also[0];
        let comment = last_on_page(&latest.listing, r#"[{"user":{"login":"marta"},"body":"go ahead","created_at":"2026-10-15T09:00:00Z"}]"#).unwrap();
        let review = last_on_page(reviews, r#"[{"user":{"login":"ivo"},"body":"","submitted_at":"2026-10-15T10:00:00Z"}]"#).unwrap();
        assert_eq!(review.as_ref().unwrap().text, "", "a review that is only line comments still says who spoke");

        assert_eq!(newer(comment.clone(), review.clone()).unwrap().sender, "ivo", "a later review outranks an earlier comment");
        assert_eq!(newer(review.clone(), comment.clone()).unwrap().sender, "ivo", "whichever was asked first");
        assert_eq!(newer(None, comment.clone()).unwrap().sender, "marta");
        assert_eq!(newer(comment.clone(), None).unwrap().sender, "marta");

        assert!(applies(reviews, &json!({ "subject": { "type": "PullRequest" } })));
        assert!(!applies(reviews, &json!({ "subject": { "type": "Issue" } })), "an issue has no reviews to ask for");
        assert!(applies(&latest.listing, &json!({})));
    }

    #[test]
    fn a_waiter_whose_items_name_nobody_reads_them_with_no_sender_yet() {
        let waiter: Waiter = serde_json::from_value(json!({
            "server": "github", "watch": { "tool": "list_notifications" }, "items": "",
            "id": "/id", "conversation": "/subject/url", "text": "/subject/title",
        }))
        .unwrap();
        let answer = json!([{ "id": "1", "subject": { "title": "Fix the build", "url": "https://api.example.com/repos/shop/web/issues/42" } }]);

        let messages = messages_in(&waiter, &answer.to_string()).unwrap();
        assert_eq!(messages[0].sender, "", "who spoke is left for latest to find");
        assert_eq!(messages[0].item, answer[0]);
    }

    #[test]
    fn what_was_said_is_asked_about_with_values_off_the_item() {
        let item = json!({ "repository": { "name": "web", "owner": { "login": "shop" } } });
        let arguments = json!({ "owner": "{/repository/owner/login}", "repo": "{/repository/name}", "issue_number": "{recording}", "page": "{page}", "gone": "{/nowhere}" });

        assert_eq!(
            filled_for(&arguments, "42", 3, &item),
            json!({ "owner": "shop", "repo": "web", "issue_number": 42, "page": 3, "gone": null })
        );
    }

    #[test]
    fn a_conversation_nobody_answered_was_last_spoken_in_by_whoever_opened_it() {
        let opening: Opening = serde_json::from_value(json!({ "call": { "tool": "issue_read" }, "sender": "/user/login", "text": "/body" })).unwrap();

        assert_eq!(
            opened_by(&opening, r#"{"user":{"login":"marta"},"body":"Please fix the build"}"#).unwrap(),
            Some(Spoken { sender: "marta".to_string(), sender_name: None, text: "Please fix the build".to_string(), at: None })
        );
        assert_eq!(opened_by(&opening, r#"{"user":{"login":"marta"},"body":null}"#).unwrap().unwrap().text, "", "an empty description is still hers");
        assert_eq!(opened_by(&opening, r#"{"message":"Not Found"}"#).unwrap(), None);
        assert!(opened_by(&opening, "not json").is_err());
    }

    #[test]
    fn the_backlog_is_remembered_but_only_later_messages_are_new() {
        let path = std::env::temp_dir().join(format!("sous-chef-seen-{}/seen.json", std::process::id()));
        let mut seen = Seen::load_from(path.clone());
        assert!(!seen.is_new("old-1"));
        seen.save().unwrap();

        assert!(seen.is_new("new-1"));
        assert!(!seen.is_new("new-1"));
        seen.save().unwrap();

        let mut reloaded = Seen::load_from(path.clone());
        assert!(!reloaded.is_new("old-1"));
        assert!(!reloaded.is_new("new-1"));
        assert!(reloaded.is_new("new-2"));

        reloaded.forget("new-2");
        assert!(reloaded.is_new("new-2"), "a message that couldn't be acted on yet is new again at the next look");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn replies_fill_in_the_conversation_and_the_text() {
        let arguments = filled(&waiter().reply.unwrap().arguments, "900", "On it.");
        assert_eq!(arguments, json!({ "recording_id": 900, "content": "On it." }));

        let nested = filled(&json!({ "to": ["thread/{conversation}"], "draft": false }), "abc", "hi");
        assert_eq!(nested, json!({ "to": ["thread/abc"], "draft": false }));
    }
}
