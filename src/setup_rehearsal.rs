//! The setup walk rehearsed, for its tests: answers given in order instead of
//! a terminal, and a machine that writes down what it was asked to do instead
//! of doing it.

use anyhow::Result;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use crate::config::{Config, Waiter};
use crate::prompt::{Asking, Question};
use crate::secrets::Kept;
use crate::setup::{Account, ActsAs, ClaudeAccount, Doing, Profile, Wizard};

/// Answers in the order the questions come, and a record of what was
/// asked and said.
#[derive(Default)]
pub struct Scripted {
    answers: VecDeque<&'static str>,
    pub asked: Vec<String>,
    pub said: Vec<String>,
}

impl Scripted {
    pub fn answering(answers: &[&'static str]) -> Scripted {
        Scripted { answers: answers.iter().copied().collect(), ..Scripted::default() }
    }

    fn next(&mut self, question: &Question) -> String {
        self.asked.push(format!("{}: {}", question.title, question.question));
        self.answers.pop_front().unwrap_or_default().to_string()
    }

    fn optional(&mut self, question: &Question) -> Option<String> {
        Some(self.next(question)).filter(|it| !it.is_empty())
    }
}

impl Asking for Scripted {
    fn banner(&mut self, _title: &str, _subtitle: &str) -> Result<()> {
        Ok(())
    }

    fn string(&mut self, question: &Question, _current: Option<&str>) -> Result<Option<String>> {
        Ok(self.optional(question))
    }

    fn text(&mut self, question: &Question, _current: Option<&str>) -> Result<Option<String>> {
        Ok(self.optional(question))
    }

    fn secret(&mut self, question: &Question) -> Result<Option<String>> {
        Ok(self.optional(question))
    }

    fn yes(&mut self, question: &Question, default: bool) -> Result<bool> {
        Ok(match self.next(question).as_str() {
            "" => default,
            answer => answer == "y",
        })
    }

    fn choice(&mut self, question: &Question, _options: &[String]) -> Result<usize> {
        Ok(self.next(question).parse::<usize>().unwrap() - 1)
    }

    fn done(&mut self, text: &str) -> Result<()> {
        self.said.push(text.to_string());
        Ok(())
    }

    fn trouble(&mut self, text: &str) -> Result<()> {
        self.said.push(format!("TROUBLE {text}"));
        Ok(())
    }

    fn note(&mut self, text: &str) -> Result<()> {
        self.said.push(text.to_string());
        Ok(())
    }
}

#[derive(Default)]
pub struct Pretend {
    pub installed: Vec<&'static str>,
    pub config: Option<Config>,
    pub kept: Vec<(String, String)>,
    pub service: Option<String>,
    pub too_old_for_checks: usize,
    pub connected: Vec<String>,
    pub servers: Vec<(String, Vec<String>)>,
    pub server_env: Vec<BTreeMap<String, String>>,
    pub acts_as: Vec<ActsAs>,
    pub waiters: Vec<(String, Waiter)>,
    pub profiles: Vec<&'static str>,
    pub nobody_logged_in: bool,
    /// She has no Claude login of her own yet; otherwise she has one
    /// and setup asks nothing about it
    pub new_to_claude: bool,
    pub claude_login: Option<String>,
    pub chefs_claude_login: Option<ClaudeAccount>,
    pub logged_in_with: Vec<Option<String>>,
    pub rotation: Vec<String>,
    pub github_login: Option<String>,
    pub git_identity: Option<(String, String)>,
}

impl Pretend {
    /// Everyone trusted on the waiters setup added, in order.
    pub fn trusted(&self) -> Vec<(String, String)> {
        self.waiters.iter().flat_map(|(_, waiter)| waiter.trusted.clone()).collect()
    }
}

impl Doing for Pretend {
    fn load_config(&mut self) -> Result<Config> {
        Ok(Config::default())
    }

    fn save_config(&mut self, config: &Config) -> Result<()> {
        let mut saved = Config::default();
        saved.name = config.name.clone();
        self.config = Some(saved);
        Ok(())
    }

    fn missing_programs(&mut self) -> Vec<&'static str> {
        Vec::new()
    }

    fn has_program(&mut self, program: &str) -> bool {
        self.installed.contains(&program)
    }

    fn has_secret(&mut self, _name: &str) -> bool {
        false
    }

    fn jev_key_works(&mut self, key: &str) -> bool {
        key == "good-key"
    }

    fn keep_secret(&mut self, name: &str, value: &str) -> Result<Kept> {
        self.kept.push((name.to_string(), value.to_string()));
        Ok(Kept::InKeyring)
    }

    fn install_service(&mut self, agent: &str) -> Result<()> {
        self.service = Some(agent.to_string());
        Ok(())
    }

    fn claude_login(&mut self) -> Option<String> {
        if self.new_to_claude {
            self.claude_login.clone()
        } else {
            Some("botten@example.com".to_string())
        }
    }

    fn chefs_claude_login(&mut self) -> Option<ClaudeAccount> {
        self.chefs_claude_login.clone()
    }

    fn log_in_to_claude(&mut self, email: Option<&str>) -> Result<String> {
        self.logged_in_with.push(email.map(str::to_string));
        let address = email.unwrap_or("botten@example.com").to_string();
        self.claude_login = Some(address.clone());
        Ok(address)
    }

    fn keep_claude_login(&mut self) -> Result<()> {
        self.rotation.extend(self.claude_login.clone());
        Ok(())
    }

    fn github_login(&mut self) -> Option<String> {
        self.github_login.clone()
    }

    fn git_email(&mut self) -> Option<String> {
        self.git_identity.as_ref().map(|(_, email)| email.clone())
    }

    fn log_in_to_github(&mut self) -> Result<String> {
        self.github_login = Some("botten-agent".to_string());
        Ok("botten-agent".to_string())
    }

    fn set_git_identity(&mut self, name: &str, email: &str) -> Result<()> {
        self.git_identity = Some((name.to_string(), email.to_string()));
        Ok(())
    }

    fn logged_in_as(&mut self, tool: &str) -> Vec<String> {
        match tool {
            _ if self.nobody_logged_in => Vec::new(),
            "github" => vec!["marta-k".to_string()],
            _ => vec![format!("me@{tool}.example.com")],
        }
    }

    fn profiles(&mut self, _tool: &str) -> Vec<Profile> {
        self.profiles.iter().map(|it| Profile { name: it.to_string() }).collect()
    }

    fn identity_of(&mut self, tool: &str, profile: Option<&str>) -> Option<String> {
        match profile {
            Some("bot") => Some("bot@basecamp.example.com".to_string()),
            _ => Some(format!("me@{tool}.example.com")),
        }
    }

    fn person_ids(&mut self, _tool: &str, address: &str) -> Vec<String> {
        match address {
            "me@basecamp.example.com" => vec!["1001".to_string()],
            "marta.k@example.com" => vec!["1002".to_string(), "2002".to_string()],
            _ => Vec::new(),
        }
    }

    fn accounts(&mut self, _tool: &str, _profile: Option<&str>) -> Result<Vec<Account>> {
        Ok(vec![
            Account { id: "111".to_string(), name: "Work".to_string() },
            Account { id: "222".to_string(), name: "Personal".to_string() },
        ])
    }

    fn can_connect_agents(&mut self, _tool: &str) -> bool {
        if self.too_old_for_checks == 0 {
            true
        } else {
            self.too_old_for_checks -= 1;
            false
        }
    }

    fn connect_agent(&mut self, tool: &str, profile: &str, agent: &str) -> Result<()> {
        self.connected.push(format!("{tool} {profile} {agent}"));
        Ok(())
    }

    fn add_server(&mut self, name: &str, command: &[String], env: BTreeMap<String, String>, acts_as: ActsAs) -> Result<()> {
        self.servers.push((name.to_string(), command.to_vec()));
        self.server_env.push(env);
        self.acts_as.push(acts_as);
        Ok(())
    }

    fn add_waiter(&mut self, name: &str, waiter: Waiter) -> Result<()> {
        self.waiters.push((name.to_string(), waiter));
        Ok(())
    }
}

/// The whole walk, in a config folder of its own that is returned for the
/// test to look in.
pub fn walk(asking: &mut Scripted, doing: &mut Pretend, name: &str) -> PathBuf {
    let config_dir = std::env::temp_dir().join(format!("sous-chef-setup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config_dir);
    Wizard { asking, doing, config_dir: config_dir.clone() }.walk_through().unwrap();
    config_dir
}
