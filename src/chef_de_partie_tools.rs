//! What a chef de partie can do through the broker: speak in its
//! conversation, and call commis. A commis works on after the call that
//! started it, and stays once it is done so the chef de partie can send it
//! back to clean up.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::broker::{Tool, text_of};
use crate::conversation::Conversation;
use crate::commis::Commis;
use crate::held::ReadHeldBack;
use crate::kitchen::Kitchen;
use crate::plating::{Plating, SentBack};
use crate::station::Station;

pub struct Reply {
    pub conversation: Arc<dyn Conversation>,
    pub plating: Arc<Plating>,
    pub sent_back: Arc<SentBack>,
    pub spoke: Arc<AtomicBool>,
}

impl Tool for Reply {
    fn name(&self) -> &str {
        "reply"
    }

    fn description(&self) -> &str {
        "Say something to the person in this conversation. This is the only way they hear from you."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let text = self.plating.polish(text_of(arguments, "text")?, &self.sent_back)?;
        self.conversation.say(&text)?;
        self.spoke.store(true, Ordering::Relaxed);
        Ok("Sent.".to_string())
    }
}

pub struct CallCommis {
    /// With the standing of the turn, which is what a commis it calls may be
    /// granted.
    pub kitchen: Kitchen,
    /// Whether anything for people has gone out this turn. A commis takes
    /// minutes and whoever asked hears nothing from it, so the first one
    /// waits for a word to them.
    pub spoke: Arc<AtomicBool>,
}

impl Tool for CallCommis {
    fn name(&self) -> &str {
        "call_commis"
    }

    fn description(&self) -> &str {
        "Call a commis, a sandboxed worker, into one project folder and give it a brief. It can read and write that folder and nothing else, has no network and none of your memory, so the brief must carry everything it needs to know. It has the languages installed here and builds from what is already fetched, so when the work needs a build, either fetch the project's dependencies yourself first (cargo fetch, bundle install, npm ci) or grant it the registries and say so in the brief. Its builds go to a folder of their own, not the project's. It works on its own: this answers at once with its id, and when it's done the pass checks the work against your brief and the verdict comes to you as a message of its own, in a turn of its own. Then send_back or dismiss it. Only one commis works in a folder at a time, and you may have only a few at work — `commis` says how many and which; past that this is refused, so wait for a verdict or dismiss one. A commis takes minutes, and whoever asked hears nothing from it, so the first commis of a turn is refused until you have told them what you're about to do, in one line, where they asked — one line, not an account of your reasoning. Only a turn nobody new is waiting on — one you scheduled for yourself, or one a commis's verdict woke, where they already heard you have it — calls a commis without a word, with `quietly`."
    }

    fn input_schema(&self) -> Value {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "project": { "type": "string", "description": "Absolute path of the project folder" },
                "brief": { "type": "string" },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Names of your own tools this commis may also call. Only tools that look things up can be given to a commis; anything that posts or changes something out there stays with you. Leave it out unless the work needs them.",
                },
                "services": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Ports on this machine's localhost the commis may reach, as the same port on its own localhost: a database its tests need, say. Only localhost; a commis has no network. Make sure what the commis will use there — a test database, say — is set up and won't collide with anyone else's before you start it.",
                },
                "registries": {
                    "type": "boolean",
                    "description": "Let the commis fetch from the package registries — rubygems, npm, crates.io, PyPI, mise, nodejs.org — and nothing else out there. For work that needs dependencies you haven't fetched. Git remotes stay yours.",
                },
                "quietly": {
                    "type": "boolean",
                    "description": "Call it without having said anything to anyone this turn. Only for a turn nobody new is waiting on: one you scheduled for yourself, or one a commis's verdict woke.",
                },
            },
            "required": ["project", "brief"],
        });
        if let Some(station) = station_property(&self.kitchen.runtime.config.stations) {
            schema["properties"]["station"] = station;
        }
        schema
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        may_start(self.spoke.load(Ordering::Relaxed), arguments["quietly"].as_bool().unwrap_or(false))?;
        let names: Vec<String> = arguments["tools"]
            .as_array()
            .map(|names| names.iter().filter_map(|it| it.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let runtime = &self.kitchen.runtime;
        let mut grant = runtime.catalog.grant(&names, self.kitchen.standing)?;
        if !grant.is_empty() {
            // Its tools' answers can be held back like anyone's
            grant.push(Box::new(ReadHeldBack { held: runtime.held.clone(), taster: runtime.taster.clone() }));
        }
        let ports = ports_in(&arguments["services"])?;

        let project = Path::new(text_of(arguments, "project")?);
        self.kitchen.refuse_if_busy(project)?;
        let registries = arguments["registries"].as_bool().unwrap_or(false);
        let stations = &runtime.config.stations;
        let station = match arguments["station"].as_str() {
            Some(name) => Some((name, stations.get(name).with_context(|| format!("there is no station called {name}"))?)),
            None => None,
        };
        let commis = Commis::start(project, grant, &ports, registries, station)?;
        self.kitchen.start(commis, text_of(arguments, "brief")?)
    }
}

/// Where a commis can work besides here, and what each place has, for
/// whoever picks one. Nothing when there is only here.
fn station_property(stations: &BTreeMap<String, Station>) -> Option<Value> {
    if stations.is_empty() {
        return None;
    }
    let described: Vec<String> = stations
        .iter()
        .map(|(name, station)| {
            let has = if station.has.is_empty() { String::new() } else { format!(", with {}", station.has.join(", ")) };
            format!("{name} ({:?} on {}{has})", station.os, station.arch)
        })
        .collect();
    Some(json!({
        "type": "string",
        "enum": stations.keys().collect::<Vec<_>>(),
        "description": format!(
            "Another machine for the commis to work on, when the work is heavy or needs what only that machine has: {}. Leave it out to work here. At a station the project goes by git, so commit what the commis needs first; services are that station's own ports; and what it commits comes back on top of the project when it's done.",
            described.join("; ")
        ),
    }))
}

/// Whoever asked hears nothing while a commis runs, so the first commis waits
/// for a word to them — the rule her instructions state, held here because
/// stating it wasn't enough.
fn may_start(spoke: bool, quietly: bool) -> Result<()> {
    if spoke || quietly {
        Ok(())
    } else {
        bail!("Nobody has heard from you this turn. Say what you're about to do, in one line, where the work was asked — then call the commis. If nobody new is waiting on this turn — you scheduled it for yourself, or a commis's verdict woke it — call it with quietly: true.")
    }
}

fn ports_in(services: &Value) -> Result<Vec<u16>> {
    services
        .as_array()
        .map(|it| it.iter())
        .into_iter()
        .flatten()
        .map(|it| u16::try_from(it.as_u64().context("a service is a port number")?).context("that is not a port number"))
        .collect()
}

pub struct SendBack {
    pub kitchen: Kitchen,
}

impl Tool for SendBack {
    fn name(&self) -> &str {
        "send_back"
    }

    fn description(&self) -> &str {
        "Send a commis that is done back to fix or finish its work. It remembers what it did. Like call_commis, this answers at once, and the pass's new verdict comes to you as a message of its own."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "commis": { "type": "string" }, "notes": { "type": "string" } },
            "required": ["commis", "notes"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        self.kitchen.send_back(text_of(arguments, "commis")?, text_of(arguments, "notes")?)
    }
}

/// How her Claude allowance is doing, read from ax: each account she can
/// work as, its session, weekly and Fable limits, and when they reset.
pub struct ClaudeUsage;

impl Tool for ClaudeUsage {
    fn name(&self) -> &str {
        "claude_usage"
    }

    fn description(&self) -> &str {
        "How much of your Claude allowance is used on each account you can work as — the session, weekly and Fable limits and when each resets — and which account you're on now. Use it when someone asks how your tokens or limits are doing, and answer in your own words."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn call(&self, _arguments: &Value) -> Result<String> {
        let reports = ax::usage::of_every_account()?;
        if reports.is_empty() {
            Ok("No accounts are stored for you, so there is nothing to read usage from. `sous-chef claude account add` stores the one you're logged in as.".to_string())
        } else {
            Ok(ax::usage::described(&reports, ax::clock::now_seconds()))
        }
    }
}

pub struct Dismiss {
    pub kitchen: Kitchen,
}

impl Tool for Dismiss {
    fn name(&self) -> &str {
        "dismiss"
    }

    fn description(&self) -> &str {
        "Let a commis go once you accept its work or give up on it. A commis still working is stopped. Its changes to the project folder stay."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "commis": { "type": "string" } }, "required": ["commis"] })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        self.kitchen.dismiss(text_of(arguments, "commis")?)
    }
}

pub struct ListCommis {
    pub kitchen: Kitchen,
}

impl Tool for ListCommis {
    fn name(&self) -> &str {
        "commis"
    }

    fn description(&self) -> &str {
        "Your commis: those at work, with what you asked of each and for how long, how many you may have at work, and those that are done and waiting for you to send back or dismiss. Look here before call_commis when you aren't sure you can call one, and to find a commis to dismiss."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn call(&self, _arguments: &Value) -> Result<String> {
        Ok(self.kitchen.described())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_station_is_offered_with_what_it_has_and_only_when_there_is_one() {
        assert_eq!(station_property(&BTreeMap::new()), None, "with only here, the tool doesn't change");

        let stations = BTreeMap::from([(
            "server".to_string(),
            Station { host: "someone@server.example.com".to_string(), os: crate::station::Os::Debian, arch: "x86_64".to_string(), has: vec!["rust".to_string(), "docker".to_string()] },
        )]);
        let station = station_property(&stations).unwrap();
        assert_eq!(station["enum"], json!(["server"]));
        let described = station["description"].as_str().unwrap();
        assert!(described.contains("server (Debian on x86_64, with rust, docker)"), "{described}");
        assert!(described.contains("commit what the commis needs first"));
    }

    struct Recorded {
        said: Mutex<Vec<String>>,
    }

    impl Conversation for Recorded {
        fn key(&self) -> &str {
            "recorded"
        }

        fn origin(&self) -> crate::conversation::Origin {
            crate::conversation::Origin {
                waiter: "test".to_string(),
                conversation: "recorded".to_string(),
            }
        }

        fn say(&self, text: &str) -> Result<()> {
            self.said.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn replies_land_in_the_conversation() {
        let conversation = Arc::new(Recorded { said: Mutex::new(Vec::new()) });
        let spoke = Arc::new(AtomicBool::new(false));
        let plating = Arc::new(Plating::new(None, Arc::new(crate::taster::Taster::claude("haiku")), crate::config::Chain::of(&["haiku"])));
        let reply = Reply { conversation: conversation.clone(), plating, sent_back: Arc::default(), spoke: spoke.clone() };

        assert!(reply.call(&json!({ "text": "  " })).is_err());
        assert!(!spoke.load(Ordering::Relaxed));

        reply.call(&json!({ "text": "On it." })).unwrap();
        assert_eq!(*conversation.said.lock().unwrap(), ["On it."]);
        assert!(spoke.load(Ordering::Relaxed));
    }

    #[test]
    fn the_first_commis_waits_for_a_word_to_whoever_asked() {
        let refused = may_start(false, false).unwrap_err().to_string();
        assert!(refused.contains("Nobody has heard from you this turn"), "{refused}");
        assert!(may_start(true, false).is_ok(), "once something went out, commis may start");
        assert!(may_start(false, true).is_ok(), "a turn nobody is waiting on says so and goes ahead");
    }
}
