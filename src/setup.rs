//! `sous-chef setup`: the one time the chef decides things for the agent. After
//! this it runs on its own.
//!
//! The walk: a name, a Claude login of its own, who the agent is (its
//! CLAUDE.md), how it writes, whether it starts with the computer, whether
//! Jev does its tasting, and then one block per tool whose command line is
//! installed — Basecamp, HEY, Fizzy.
//! Every question keeps what is already there when skipped, so running setup
//! again is how anything gets changed later.
//!
//! A tool can be given to the agent in two ways, and they are not the same
//! thing. As an agent of its own — Basecamp can do this — it has its own
//! profile, its own notifications, and it listens: mention it and it answers
//! as itself. As you, it gets the tools and nothing else. It does not listen
//! there, because everything addressed to you would wake it and it would
//! answer your colleagues in your name.
//!
//! Asking and doing are both handed in, which is what lets the whole walk be
//! tested without a terminal, a keyring, systemd, or anyone's Basecamp.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::{self, Config, Waiter};
use crate::fsutil;
use crate::paths;
use crate::prompt::{self, Asking, Question, Terminal};
use crate::secrets::Kept;
use crate::setup_machine::Machine;

pub fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let mut asking = Terminal::new(stdin.lock(), std::io::stdout(), prompt::styled_for_stdout());

    Wizard {
        asking: &mut asking,
        doing: &mut Machine,
        config_dir: paths::config_dir(),
    }
    .walk_through()
}

/// A tool the agent can be given, when its command line is installed.
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub program: &'static str,
    pub can_be_its_own_agent: bool,
}

const TOOLS: [Tool; 3] = [
    Tool { name: "basecamp", title: "Basecamp", program: "basecamp", can_be_its_own_agent: true },
    Tool { name: "hey", title: "HEY", program: "hey", can_be_its_own_agent: false },
    Tool { name: "fizzy", title: "Fizzy", program: "fizzy", can_be_its_own_agent: false },
];

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
}

pub use crate::setup_claude::ClaudeAccount;

/// What setup does to the machine, beyond asking.
pub trait Doing {
    fn load_config(&mut self) -> Result<Config>;
    fn save_config(&mut self, config: &Config) -> Result<()>;
    fn missing_programs(&mut self) -> Vec<&'static str>;
    fn has_program(&mut self, program: &str) -> bool;
    fn has_secret(&mut self, name: &str) -> bool;
    fn jev_key_works(&mut self, key: &str) -> bool;
    fn keep_secret(&mut self, name: &str, value: &str) -> Result<Kept>;
    fn install_service(&mut self, agent: &str) -> Result<()>;
    /// The address her own Claude folder is logged in as, if it is.
    fn claude_login(&mut self) -> Option<String>;
    /// The account the chef's own Claude Code is logged in to, if any.
    fn chefs_claude_login(&mut self) -> Option<ClaudeAccount>;
    /// Hands the terminal to Claude Code until her own folder is logged in,
    /// with `email` filled in when there is one; answers with the address.
    fn log_in_to_claude(&mut self, email: Option<&str>) -> Result<String>;
    /// Adds the login she works as to the accounts she rotates between.
    fn keep_claude_login(&mut self) -> Result<()>;
    /// The GitHub login of her own she already has, if any.
    fn github_login(&mut self) -> Option<String>;
    fn git_email(&mut self) -> Option<String>;
    /// Hands the terminal to gh until her own folder is logged in; answers
    /// with the login.
    fn log_in_to_github(&mut self) -> Result<String>;
    fn set_git_identity(&mut self, name: &str, email: &str) -> Result<()>;
    /// Who the tool's command line is logged in as, for the confirm question.
    fn logged_in_as(&mut self, tool: &str) -> Vec<String>;
    /// The profiles the tool's command line has, when it has profiles.
    fn profiles(&mut self, tool: &str) -> Vec<Profile>;
    /// Who one profile is logged in as.
    fn identity_of(&mut self, tool: &str, profile: Option<&str>) -> Option<String>;
    /// The ids the tool knows someone by, in every account the chef
    /// running setup can see. Empty when it knows nobody by that address.
    fn person_ids(&mut self, tool: &str, address: &str) -> Vec<String>;
    fn accounts(&mut self, tool: &str, profile: Option<&str>) -> Result<Vec<Account>>;
    fn can_connect_agents(&mut self, tool: &str) -> bool;
    /// Hands the terminal to the tool's own command line until the agent is
    /// connected under `profile`.
    fn connect_agent(&mut self, tool: &str, profile: &str, agent: &str) -> Result<()>;
    fn add_server(&mut self, name: &str, command: &[String], env: BTreeMap<String, String>, acts_as: ActsAs) -> Result<()>;
    /// Adds the waiter, or replaces the one of that name, keeping whoever it
    /// already trusted.
    fn add_waiter(&mut self, name: &str, waiter: Waiter) -> Result<()>;
}

/// Whose name a server's tools act in. One that acts as the chef is only
/// offered to turns on a trusted person's word.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActsAs {
    Agent,
    Chef,
}

/// What a waiter reports its senders as, which is what its trusted people
/// have to be keyed by.
#[derive(Clone, Copy)]
pub enum RecognizedBy {
    /// Their id, looked up by their email address.
    PersonId,
    /// Their username, as GitHub reports who wrote a comment.
    Login,
}

pub enum GoOn {
    Connect,
    AsYou,
    Without,
}

pub struct Wizard<'a> {
    pub asking: &'a mut dyn Asking,
    pub doing: &'a mut dyn Doing,
    pub config_dir: PathBuf,
}

impl Wizard<'_> {
    pub fn walk_through(&mut self) -> Result<()> {
        let mut config = self.doing.load_config()?;
        self.asking.banner("SOUS-CHEF", "Yes, chef!")?;
        self.asking.note("\nEvery question is optional. Enter keeps what's already there.")?;
        self.check_prerequisites()?;

        self.name(&mut config)?;
        let agent = config.name.clone();
        self.claude(&agent)?;
        self.about(&agent)?;
        self.style(&agent)?;
        self.doing.save_config(&config)?;

        self.run_at_startup(&agent)?;
        self.jev(&agent)?;
        if self.doing.has_program("gh") {
            self.github(&agent)?;
        }
        for tool in &TOOLS {
            if self.doing.has_program(tool.program) {
                self.tool(tool, &agent)?;
            }
        }
        self.closing_notes(&agent)
    }

    fn check_prerequisites(&mut self) -> Result<()> {
        let missing = self.doing.missing_programs();
        if missing.is_empty() {
            self.asking.done("Found claude, bwrap and socat.")
        } else {
            self.asking.trouble(&format!(
                "Missing: {}. Commis can't run without them — install them before starting.",
                missing.join(", ")
            ))
        }
    }

    fn name(&mut self, config: &mut Config) -> Result<()> {
        let question = Question {
            title: "Name",
            question: "What do you want to name your agent?",
            hint: "This is how your agent will refer to itself.",
        };
        if let Some(name) = self.asking.string(&question, Some(&config.name))? {
            config.name = name;
        }
        self.asking.done(&format!("Your agent is called {}.", config.name))
    }

    fn about(&mut self, agent: &str) -> Result<()> {
        let title = format!("About {agent}");
        let question = format!("Give general guidance on how {agent} should behave.");
        let hint = format!("This will become {agent}'s CLAUDE.md file. You can change this later.");
        self.prose(&Question { title: &title, question: &question, hint: &hint }, "CLAUDE.md")
    }

    fn style(&mut self, agent: &str) -> Result<()> {
        let question = format!("How do you want {agent} to write prose?");
        let hint = format!("This will be used as guidance every time {agent} wants to write any kind of prose.");
        self.prose(&Question { title: "Style", question: &question, hint: &hint }, "style.md")
    }

    fn prose(&mut self, question: &Question, file: &str) -> Result<()> {
        let path = self.config_dir.join(file);
        let current = std::fs::read_to_string(&path).ok();

        match self.asking.text(question, current.as_deref())? {
            Some(text) => {
                fsutil::write_private(&path, &(text + "\n"))?;
                self.asking.done(&format!("Saved to {}.", path.display()))
            }
            None if current.is_some() => self.asking.done(&format!("Kept {}.", path.display())),
            None => self.asking.done("Skipped."),
        }
    }

    fn run_at_startup(&mut self, agent: &str) -> Result<()> {
        let question = format!("Do you want to start {agent} as soon as this computer starts?");
        let hint = format!("This will add a systemd entry for {agent}.");
        if !self.asking.yes(&Question { title: "Run at startup", question: &question, hint: &hint }, false)? {
            return self.asking.done(&format!("`sous-chef start` brings {agent} up whenever you want."));
        }

        match self.doing.install_service(agent) {
            Ok(()) => self.asking.done("Installed and enabled. `sous-chef start` and `sous-chef stop` go through it."),
            Err(error) => self.asking.trouble(&format!("Couldn't install it: {error:#}")),
        }
    }

    fn jev(&mut self, agent: &str) -> Result<()> {
        let question = format!("Do you want to improve {agent}'s speed using Jev?");
        let hint = format!("{agent} can use Jev to do its tasting quickly instead of using Claude.");
        let already = self.doing.has_secret(config::JEV_API_KEY);
        if !self.asking.yes(&Question { title: "Improved efficiency", question: &question, hint: &hint }, already)? {
            return self.asking.done(&format!("{agent} will ask Claude instead."));
        }
        if already {
            self.asking.note("  A key is already stored; enter keeps it.")?;
        }

        let key_question = Question { title: "Jev API key", question: "Jev API key", hint: "Your Jev API key, from typesafe.ai." };
        while let Some(key) = self.asking.secret(&key_question)? {
            if self.doing.jev_key_works(&key) {
                let place = match self.doing.keep_secret(config::JEV_API_KEY, &key)? {
                    Kept::InKeyring => "your keyring",
                    Kept::InFile => "secrets.json next to the config, readable only by you (no keyring answered)",
                };
                return self.asking.done(&format!("It works. Stored in {place}."));
            }
            self.asking.trouble("Jev refused that key. Try again, or enter to skip.")?;
        }
        self.asking.done("No key changed.")
    }

    fn closing_notes(&mut self, agent: &str) -> Result<()> {
        self.asking.banner("Done", &format!("{agent} is set up."))?;
        self.asking.note(&format!(
            "
Bring {agent} up, or talk from here:
    sous-chef start
    sous-chef chat \"hello\"

Any MCP server can be one of {agent}'s tools. Add it the way you would for Claude Code:
    sous-chef mcp add github -- github-mcp-server stdio
    sous-chef mcp list
A tool argument that carries text for people goes through the style first:
    sous-chef mcp prose github create_issue_comment body

More Claude subscriptions for {agent} to rotate between:
    sous-chef claude account add
"
        ))
    }
}

/// Basecamp shows adjacent `<p>`s with no space between them; its own editor
/// writes paragraphs as text with a blank line (`<br><br>`) between, which
/// is what both notes say to do.
/// What carries text for people on each tool's server, so the plating sees
/// it and commis never get it. Basecamp's tools are gateways: one tool per
/// area, the action's arguments under `params`.
pub fn prose_of(tool: &str) -> BTreeMap<String, Vec<String>> {
    match tool {
        "basecamp" => BTreeMap::from([
            ("basecamp_messages".to_string(), words(&["/params/content", "/params/subject"])),
            // A to-do's `content` is its title: a label, not writing
            ("basecamp_todos".to_string(), words(&["/params/description"])),
            ("basecamp_campfires".to_string(), words(&["/params/content"])),
            ("basecamp_cards".to_string(), words(&["/params/content", "/params/title"])),
        ]),
        "hey" => BTreeMap::from([("hey_threads".to_string(), words(&["/params/body", "/params/subject"]))]),
        "github" => BTreeMap::from([
            ("add_issue_comment".to_string(), words(&["/body"])),
            ("add_reply_to_pull_request_comment".to_string(), words(&["/body"])),
            ("add_comment_to_pending_review".to_string(), words(&["/body"])),
            ("pull_request_review_write".to_string(), words(&["/body"])),
            ("issue_write".to_string(), words(&["/body", "/title"])),
            ("create_pull_request".to_string(), words(&["/body", "/title"])),
            ("update_pull_request".to_string(), words(&["/body", "/title"])),
        ]),
        _ => BTreeMap::new(),
    }
}

/// The environment a tool runs in when the agent has a profile of its own
/// there: its config lives in the agent's folder, not the chef's.
pub fn own_tool_config() -> BTreeMap<String, String> {
    BTreeMap::from([("XDG_CONFIG_HOME".to_string(), config::HER_TOOLS.to_string())])
}

/// A name to suggest for an address: marta.k@example.com is probably Marta.
pub fn name_from(address: &str) -> String {
    let first = address.split(['@', '.', '+', '_', '-']).next().unwrap_or_default();
    let mut letters = first.chars();
    match letters.next() {
        Some(initial) => initial.to_uppercase().chain(letters).collect(),
        None => address.to_string(),
    }
}

pub fn profile_name(agent: &str) -> String {
    let name: String = agent
        .chars()
        .filter(|it| it.is_ascii_alphanumeric())
        .map(|it| it.to_ascii_lowercase())
        .collect();
    if name.is_empty() { "agent".to_string() } else { name }
}

pub fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|it| it.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Cursor;
    use crate::setup_rehearsal::{Pretend, Scripted, walk};
    use serde_json::json;

    #[test]
    fn an_agent_of_her_own_listens_and_a_tool_used_as_you_does_not() {
        let mut asking = Scripted::answering(&[
            "Botten",                                  // Name
            "You are dry and direct.",                 // About
            "Short sentences.",                        // Style
            "y",                                       // Run at startup
            "y", "bad-key", "good-key",                // Jev, a refused key, a good one
            "y", "y",                                  // Basecamp, as an agent
            "y", "",                                 // trust whoever runs setup, under the suggested name
            "marta.k@example.com", "Marta K", "",      // and one more person
            "n",                                       // nobody else may assign work
            "y", "y",                                  // HEY, confirm as me
        ]);
        let mut doing = Pretend { installed: vec!["basecamp", "hey"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "agent");

        assert_eq!(doing.config.as_ref().unwrap().name, "Botten");
        assert_eq!(std::fs::read_to_string(config_dir.join("CLAUDE.md")).unwrap(), "You are dry and direct.\n");
        assert_eq!(std::fs::read_to_string(config_dir.join("style.md")).unwrap(), "Short sentences.\n");
        assert_eq!(doing.service.as_deref(), Some("Botten"));
        assert_eq!(doing.kept, [("jev_api_key".to_string(), "good-key".to_string())]);
        assert!(asking.said.iter().any(|it| it.contains("Jev refused that key")));

        assert_eq!(doing.connected, ["basecamp botten Botten"], "the profile is named after the agent");
        assert!(!asking.asked.iter().any(|it| it.contains("secret")), "connecting is the command line's, and setup never asks for a secret");
        assert_eq!(doing.servers[0], ("basecamp".to_string(), words(&["basecamp", "--profile", "botten", "mcp"])));
        assert_eq!(doing.acts_as, [ActsAs::Agent, ActsAs::Chef], "the agent acts as itself; HEY used as you acts as you");
        assert_eq!(doing.server_env[0]["XDG_CONFIG_HOME"], "{tools}", "her Basecamp profile lives in her own folder, wherever that is");
        assert!(config::environment(&doing.server_env[0])["XDG_CONFIG_HOME"].ends_with("/sous-chef/tools"));
        assert!(doing.server_env[1].is_empty(), "a tool used as you runs with your own config");
        let (name, waiter) = &doing.waiters[0];
        assert_eq!(name, "basecamp");
        assert_eq!(
            doing.trusted(),
            [("1001".to_string(), "Me".to_string()), ("1002".to_string(), "Marta K".to_string()), ("2002".to_string(), "Marta K".to_string())],
            "an agent's inbox reports people by id, so that is what they are trusted as — in every account they are in"
        );
        assert_eq!(waiter.watch.arguments, json!({ "action": "poll_inbox", "params": {} }), "an agent has an inbox, not notifications");
        assert_eq!(waiter.sender.as_deref(), Some("/event/creator_id"));
        assert_eq!(waiter.cursor, Some(Cursor { from: "/position".to_string(), into: "/params/position".to_string() }));
        assert!(waiter.note.as_ref().unwrap().contains("Read the recording"));
        assert!(!waiter.anyone, "nobody but the trusted people is heard");
        assert!(asking.said.iter().any(|it| it.contains("acts only on the people you named")));
        assert_eq!(waiter.reply, None);
        assert_eq!((waiter.trigger.as_ref(), waiter.every_seconds), (None, 15), "Basecamp pushes nothing to listen to, so its inbox is asked often");
        let note = waiter.note.as_ref().unwrap();
        assert!(note.contains("answer where you were addressed") && !note.contains("message board"), "an agent comments and chats like anyone: {note}");

        assert_eq!(doing.servers[1], ("hey".to_string(), words(&["hey", "mcp"])));
        assert_eq!(doing.waiters.len(), 1, "a tool used as you gets no waiter");
        assert!(asking.asked.iter().any(|it| it == "Confirm: Do you want to allow Botten to use HEY as me@hey.example.com?"));
        assert!(!asking.asked.iter().any(|it| it.contains("Fizzy")), "fizzy isn't installed, so it isn't offered");

        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn with_nobody_offered_setup_asks_who_to_trust_not_who_else() {
        let mut asking = Scripted::answering(&["Botten", "", "", "n", "n", "y", "botten@example.com", "y", "", "n"]);
        let mut doing = Pretend { installed: vec!["gh", "github-mcp-server"], nobody_logged_in: true, ..Pretend::default() };
        walk(&mut asking, &mut doing, "trust-nobody-offered");

        assert!(asking.asked.iter().any(|it| it == "Trusted person: Who should Botten trust in GitHub? Their username."), "{:?}", asking.asked);
    }

    #[test]
    fn saying_nothing_changes_nothing() {
        let mut asking = Scripted::answering(&[]);
        let mut doing = Pretend { installed: vec!["basecamp", "hey", "fizzy"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "empty");

        assert_eq!(doing.config.as_ref().unwrap().name, "Anna");
        assert!(!config_dir.join("CLAUDE.md").exists());
        assert!(!config_dir.join("style.md").exists());
        assert_eq!(doing.service, None);
        assert!(doing.kept.is_empty());
        assert!(doing.servers.is_empty());
        assert!(doing.waiters.is_empty());
        assert!(asking.asked.iter().any(|it| it.starts_with("Fizzy:")));
    }

    #[test]
    fn basecamp_as_you_asks_which_account_and_adds_tools_only() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "n", "y", "1"]);
        let mut doing = Pretend { installed: vec!["basecamp"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "as-you");

        assert_eq!(doing.servers, [("basecamp".to_string(), words(&["basecamp", "mcp", "--account", "111"]))]);
        assert!(doing.waiters.is_empty());
        assert!(doing.connected.is_empty());
        assert!(!asking.asked.iter().any(|it| it.starts_with("Profile:") || it.starts_with("Listen:")), "one profile, and it is the chef's: nothing to pick, nothing to listen as");
        assert!(!asking.asked.iter().any(|it| it.starts_with("Trusted person:")), "she listens nowhere as you, so there is nowhere to trust anyone");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn a_profile_logged_in_as_her_own_user_is_hers_to_listen_with() {
        // Basecamp, not as an agent, the second profile, confirm, the first account, listen: yes, trust me, my name, nobody else, anyone may assign: no
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "n", "2", "y", "1", "y", "y", "", "", "n"]);
        let mut doing = Pretend { installed: vec!["basecamp"], profiles: vec!["personal", "bot"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "own-user");

        assert!(asking.asked.iter().any(|it| it == "Confirm: Do you want to allow Anna to use Basecamp as bot@basecamp.example.com?"));
        assert_eq!(doing.servers, [("basecamp".to_string(), words(&["basecamp", "--profile", "bot", "mcp", "--account", "111"]))]);
        assert!(doing.server_env[0].is_empty(), "the profile is in the chef's own command line config, where they logged it in");
        assert_eq!(doing.acts_as, [ActsAs::Agent], "her own user acts as her");
        let (_, waiter) = &doing.waiters[0];
        assert_eq!(waiter.watch.tool, "basecamp_account", "a user hears through notifications, the way a person does");
        assert_eq!(waiter.sender.as_deref(), Some("/creator/id"), "a user that isn't an admin sees addresses redacted");
        assert_eq!((waiter.trigger.as_ref(), waiter.every_seconds), (None, 15), "notifications are asked as often as an agent's inbox");
        assert!(!waiter.anyone);
        assert_eq!(doing.trusted(), [("1001".to_string(), "Me".to_string())], "so the trusted person is known by id");
        let _ = std::fs::remove_dir_all(config_dir);

        // The same, but the chef's own profile picked: no listening offered
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "n", "1", "y", "1", "y", ""]);
        let mut doing = Pretend { installed: vec!["basecamp"], profiles: vec!["personal", "bot"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "own-profile");
        assert_eq!(doing.servers[0].1, words(&["basecamp", "--profile", "personal", "mcp", "--account", "111"]));
        assert_eq!(doing.acts_as, [ActsAs::Chef], "the chef's own profile is only for turns on a trusted word");
        assert!(doing.waiters.is_empty());
        assert!(!asking.asked.iter().any(|it| it.starts_with("Listen:")));
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn anyone_can_hand_an_agent_work_while_only_trusted_people_direct_it() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "y", "", "", ""]);
        let mut doing = Pretend { installed: vec!["basecamp"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "anyone");

        assert_eq!(doing.trusted().len(), 1);
        assert!(doing.waiters[0].1.anyone, "enter says yes: anyone may assign work");
        assert!(asking.asked.iter().any(|it| it == "Work from anyone: Should anyone in Basecamp be able to assign Anna work?"));
        assert!(asking.said.iter().any(|it| it.contains("only the people you named can direct it")));
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn an_agent_that_would_hear_nobody_is_pointed_out() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "n", "", "n"]);
        let mut doing = Pretend { installed: vec!["basecamp"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "nobody");

        assert!(doing.trusted().is_empty());
        assert!(!doing.waiters[0].1.anyone);
        assert!(asking.said.iter().any(|it| it.starts_with("TROUBLE Nobody is trusted and nobody else may assign work")));
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn someone_basecamp_does_not_know_is_not_trusted_on_paper() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "n", "ghost@example.com", "", "n"]);
        let mut doing = Pretend { installed: vec!["basecamp"], ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "ghost");

        assert!(doing.trusted().is_empty());
        assert!(asking.said.iter().any(|it| it.starts_with("TROUBLE Couldn't find ghost@example.com in Basecamp")));
        assert!(!asking.asked.iter().any(|it| it.starts_with("Their name")), "nobody is named who couldn't be found");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn a_name_is_suggested_from_the_address() {
        assert_eq!(name_from("marta.k@example.com"), "Marta");
        assert_eq!(name_from("ivo@example.com"), "Ivo");
        assert_eq!(name_from("@example.com"), "@example.com");
    }

    #[test]
    fn setup_waits_while_an_old_basecamp_is_updated() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "1", "1", "y", "", "", ""]);
        let mut doing = Pretend { installed: vec!["basecamp"], too_old_for_checks: 2, ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "updated");

        assert_eq!(asking.said.iter().filter(|it| it.starts_with("TROUBLE The installed basecamp can't connect agents")).count(), 2);
        assert_eq!(doing.connected, ["basecamp anna Anna"], "checking again after the update carries on from where it was");
        assert_eq!(doing.waiters.len(), 1);
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn an_old_basecamp_can_still_be_used_as_you_or_left_for_later() {
        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "2", "y", "1"]);
        let mut doing = Pretend { installed: vec!["basecamp"], too_old_for_checks: usize::MAX, ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "old-as-you");

        assert_eq!(doing.servers, [("basecamp".to_string(), words(&["basecamp", "mcp", "--account", "111"]))]);
        assert!(doing.connected.is_empty());
        assert!(doing.waiters.is_empty());
        let _ = std::fs::remove_dir_all(config_dir);

        let mut asking = Scripted::answering(&["", "", "", "", "", "y", "y", "3"]);
        let mut doing = Pretend { installed: vec!["basecamp"], too_old_for_checks: usize::MAX, ..Pretend::default() };
        let config_dir = walk(&mut asking, &mut doing, "old-later");

        assert!(doing.servers.is_empty());
        assert!(asking.said.iter().any(|it| it.contains("stays out of Basecamp for now")));
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn profile_names_are_safe_for_a_command_line() {
        assert_eq!(profile_name("Botten Anna"), "bottenanna");
        assert_eq!(profile_name("--profile evil"), "profileevil");
        assert_eq!(profile_name("🤖"), "agent");
    }
}
