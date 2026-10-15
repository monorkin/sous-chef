//! The part of the setup walk that gives the agent GitHub: a login of its own
//! through gh, the identity its commits carry, and listening there — GitHub's
//! own MCP server, signed in as that login, asked for its notifications.

use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;

use crate::config::{self, Call, Latest, Listing, Opening, Pointers, Waiter, When};
use crate::prompt::Question;
use crate::setup::{ActsAs, RecognizedBy, Tool, Wizard, words};

pub const TOOL: Tool = Tool { name: "github", title: "GitHub", program: "gh", can_be_its_own_agent: false };

pub const SERVER: &str = "github-mcp-server";

/// GitHub asks for notifications no more than once a minute.
pub const EVERY_SECONDS: u64 = 60;

const NOTE: &str = "That is a GitHub notification, not what they wrote. Its lines are, in order: why it reached you (mention, assign, review_requested, comment…), the repository, what it is about (Issue, PullRequest…), its title, and its API URL; the newest comment there follows when there is one. Read the issue or pull request with your GitHub tools before you do anything — its comments, and for a pull request its reviews and review comments too — and answer there, with those tools, in GitHub Markdown.";

impl Wizard<'_> {
    /// A GitHub account of her own, so what she pushes and opens is hers
    /// and not the chef's. The login is gh's to do; setup then writes the
    /// identity her commits carry, which needs the account's email.
    pub fn github(&mut self, agent: &str) -> Result<()> {
        let already = self.doing.github_login();
        let question = format!("Do you want {agent} to have a GitHub login of its own?");
        let hint = format!("{agent} then pushes and opens pull requests as that account instead of as you. Make the account first, and give it access to the repositories {agent} works in.");
        if !self.asking.yes(&Question { title: "GitHub", question: &question, hint: &hint }, already.is_some())? {
            return self.asking.done(&format!("{agent} pushes nothing on its own; when you tell it to use your login, it does."));
        }

        let login = match already {
            Some(login) => {
                self.asking.done(&format!("Already logged in as {login}."))?;
                login
            }
            None => {
                self.asking.note("\n  gh takes it from here: approve the login in the browser and come back.\n")?;
                match self.doing.log_in_to_github() {
                    Ok(login) => login,
                    Err(error) => return self.asking.trouble(&format!("Couldn't log in: {error:#}. Nothing was changed; run setup again to retry.")),
                }
            }
        };
        self.commit_as(agent, &login)?;
        self.listen_on_github(agent, &login)
    }

    fn commit_as(&mut self, agent: &str, login: &str) -> Result<()> {
        let current = self.doing.git_email();
        let email_question = Question {
            title: "GitHub email",
            question: "Which email address does that account use?",
            hint: "GitHub credits commits to the account by it.",
        };
        match self.asking.string(&email_question, current.as_deref())?.or(current) {
            Some(email) => {
                self.doing.set_git_identity(agent, &email)?;
                self.asking.done(&format!("{agent} pushes and opens pull requests as {login}, with commits by {email}."))
            }
            None => self.asking.trouble(&format!("Without an email, {agent}'s commits are credited to nobody. Run setup again, or `sous-chef github login --email <address>`.")),
        }
    }

    /// Only once she has a GitHub login of her own: listening as the chef
    /// would wake her on everything addressed to them.
    fn listen_on_github(&mut self, agent: &str, login: &str) -> Result<()> {
        let question = format!("Should {agent} listen on GitHub?");
        let hint = format!("Mentions, assignments and review requests for {login} wake {agent}, and it answers there as {login}.");
        if !self.asking.yes(&Question { title: "Listen", question: &question, hint: &hint }, true)? {
            return self.asking.done(&format!("{agent} doesn't listen on GitHub."));
        }
        if !self.doing.has_program(SERVER) {
            return self.asking.trouble(&format!("{agent} listens through GitHub's MCP server, which isn't installed. Install it with `mise use -g aqua:github/github-mcp-server` and run setup again."));
        }

        if let Err(error) = self.doing.add_server(TOOL.name, &server_command(), server_env(), ActsAs::Agent) {
            return self.asking.trouble(&format!("Couldn't add the GitHub server: {error:#}"));
        }
        let trusted = self.trusted_people(&TOOL, agent, RecognizedBy::Login)?;
        let anyone = self.anyone_may_assign_work(&TOOL, agent)?;
        let heard = trusted.len();
        self.doing.add_waiter(TOOL.name, Waiter { trusted, ..notifications_waiter(anyone) })?;
        self.asking.done(&format!("{agent} listens on GitHub as {login}."))?;
        self.say_who_is_heard(&TOOL, agent, heard, anyone, EVERY_SECONDS)
    }
}

/// Her token is read from her own gh login every time the server starts,
/// so it is never written down anywhere else and follows a login gh renews.
pub fn server_command() -> Vec<String> {
    words(&["sh", "-c", "GITHUB_PERSONAL_ACCESS_TOKEN=\"$(gh auth token)\" exec github-mcp-server stdio --toolsets=default,notifications"])
}

pub fn server_env() -> BTreeMap<String, String> {
    BTreeMap::from([("GH_CONFIG_DIR".to_string(), format!("{}/gh", config::HER_TOOLS))])
}

/// Her notifications, the ones she takes part in. A notification is a
/// thread, bumped for every new comment, so what makes one new is its id and
/// when it was last updated. It names nobody, so who spoke is always read
/// off the thread: its newest comment or, on a pull request, its newest
/// review — every line comment and reply goes in as one — or whoever opened
/// it when nobody has answered. A notification that can't be told, a release
/// or a check run, is nobody's word. Each is marked read once handled, so
/// her GitHub inbox shows what she hasn't got to.
pub fn notifications_waiter(anyone: bool) -> Waiter {
    let thread = json!({ "owner": "{/repository/owner/login}", "repo": "{/repository/name}" });
    let mut comments = thread.clone();
    comments["method"] = json!("get_comments");
    comments["issue_number"] = json!("{recording}");
    comments["page"] = json!("{page}");
    comments["perPage"] = json!(100);
    let mut reviews = thread.clone();
    reviews["method"] = json!("get_reviews");
    reviews["pullNumber"] = json!("{recording}");
    reviews["page"] = json!("{page}");
    reviews["perPage"] = json!(100);
    let mut opened = thread;
    opened["method"] = json!("get");
    opened["issue_number"] = json!("{recording}");

    Waiter {
        server: TOOL.name.to_string(),
        watch: Call { tool: "list_notifications".to_string(), arguments: json!({ "filter": "only_participating", "perPage": 50 }) },
        every_seconds: EVERY_SECONDS,
        items: String::new(),
        id: Pointers::Several(words(&["/id", "/updated_at"])),
        conversation: "/subject/url".to_string(),
        sender: None,
        sender_name: None,
        anyone,
        trusted: BTreeMap::new(),
        text: Pointers::Several(words(&["/reason", "/repository/full_name", "/subject/type", "/subject/title", "/subject/url"])),
        reply: None,
        handled: Some(Call { tool: "dismiss_notification".to_string(), arguments: json!({ "threadID": "{/id}", "state": "read" }) }),
        trigger: None,
        cursor: None,
        note: Some(NOTE.to_string()),
        latest: Some(Latest {
            listing: Listing {
                call: Call { tool: "issue_read".to_string(), arguments: comments },
                items: Pointers::One(String::new()),
                sender: "/user/login".to_string(),
                sender_name: None,
                text: "/body".to_string(),
                at: Some("/created_at".to_string()),
                when: None,
            },
            also: vec![Listing {
                call: Call { tool: "pull_request_read".to_string(), arguments: reviews },
                items: Pointers::One(String::new()),
                sender: "/user/login".to_string(),
                sender_name: None,
                text: "/body".to_string(),
                at: Some("/submitted_at".to_string()),
                when: Some(When { pointer: "/subject/type".to_string(), is: "PullRequest".to_string() }),
            }],
            opening: Some(Opening {
                call: Call { tool: "issue_read".to_string(), arguments: opened },
                sender: "/user/login".to_string(),
                sender_name: None,
                text: "/body".to_string(),
            }),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup_rehearsal::{Pretend, Scripted, walk};
    use crate::waiter;

    #[test]
    fn a_github_login_of_her_own_is_made_at_setup_and_kept_after() {
        let mut asking = Scripted::answering(&[
            "Botten", "", "", "n", "n",   // name, about, style, no service, no Jev
            "y", "botten@example.com",    // GitHub, and the account's email
            "n",                          // not listening there
        ]);
        let mut doing = Pretend { installed: vec!["gh"], ..Pretend::default() };
        walk(&mut asking, &mut doing, "github");
        assert_eq!(doing.github_login.as_deref(), Some("botten-agent"));
        assert_eq!(doing.git_identity, Some(("Botten".to_string(), "botten@example.com".to_string())));
        assert!(asking.said.iter().any(|it| it.contains("as botten-agent, with commits by botten@example.com")), "{:?}", asking.said);
        assert!(doing.waiters.is_empty());

        let mut again = Scripted::answering(&["Botten", "", "", "n", "n", "", "", "n"]);
        walk(&mut again, &mut doing, "github-again");
        assert_eq!(doing.git_identity, Some(("Botten".to_string(), "botten@example.com".to_string())), "enter keeps the login and the email");
        assert!(again.said.iter().any(|it| it.contains("Already logged in as botten-agent")));

        let mut without_gh = Scripted::answering(&["Botten", "", "", "n", "n"]);
        let mut nothing = Pretend::default();
        walk(&mut without_gh, &mut nothing, "github-without");
        assert!(!without_gh.asked.iter().any(|it| it.contains("GitHub")), "no gh, no question");
    }

    #[test]
    fn with_a_github_login_of_her_own_she_listens_there_through_githubs_server() {
        let mut asking = Scripted::answering(&[
            "Botten", "", "", "n", "n",    // name, about, style, no service, no Jev
            "y", "botten@example.com",     // GitHub, and the account's email
            "y",                           // listen on GitHub
            "y", "",                       // trust whoever runs setup, under the suggested name
            "ivo", "Ivo", "",              // and one more person
            "n",                           // nobody else may assign work
        ]);
        let mut doing = Pretend { installed: vec!["gh", "github-mcp-server"], ..Pretend::default() };
        walk(&mut asking, &mut doing, "github-listens");

        assert_eq!(doing.servers, [("github".to_string(), server_command())]);
        assert_eq!(doing.acts_as, [ActsAs::Agent], "she answers there as herself");
        assert_eq!(doing.server_env, [server_env()]);
        assert!(asking.asked.iter().any(|it| it == "Trusted person: Should Botten trust marta-k?"), "the chef's own GitHub login is offered first: {:?}", asking.asked);
        assert!(asking.asked.iter().any(|it| it == "Trusted person: Who else should Botten trust in GitHub? Their username."));
        assert_eq!(doing.trusted(), [("ivo".to_string(), "Ivo".to_string()), ("marta-k".to_string(), "Marta".to_string())], "GitHub reports people by username");
        let (name, waiter) = &doing.waiters[0];
        assert_eq!((name.as_str(), waiter.every_seconds, waiter.anyone), ("github", 60, false));
        assert_eq!(waiter.sender, None, "a notification names nobody, so who spoke is read off the thread");
        assert!(asking.said.iter().any(|it| it == "Botten listens on GitHub as botten-agent."));
    }

    #[test]
    fn she_listens_on_github_even_with_no_email_for_her_commits() {
        let mut asking = Scripted::answering(&["Botten", "", "", "n", "n", "y", "", "y", "y", "", "", "n"]);
        let mut doing = Pretend { installed: vec!["gh", "github-mcp-server"], ..Pretend::default() };
        walk(&mut asking, &mut doing, "github-no-email");

        assert!(asking.said.iter().any(|it| it.starts_with("TROUBLE Without an email")));
        assert_eq!(doing.waiters.len(), 1);
    }

    #[test]
    fn without_githubs_server_she_is_told_how_to_get_it_and_doesnt_listen() {
        let mut asking = Scripted::answering(&["Botten", "", "", "n", "n", "y", "botten@example.com", "y"]);
        let mut doing = Pretend { installed: vec!["gh"], ..Pretend::default() };
        walk(&mut asking, &mut doing, "github-no-server");

        assert!(doing.servers.is_empty() && doing.waiters.is_empty());
        assert!(asking.said.iter().any(|it| it.starts_with("TROUBLE") && it.contains("mise use -g aqua:github/github-mcp-server")), "{:?}", asking.said);
    }

    #[test]
    fn a_notification_is_read_as_its_thread_and_new_again_with_every_update() {
        let answer = json!([
            { "id": "26211607271", "reason": "mention", "updated_at": "2026-10-15T09:00:00Z",
              "subject": { "title": "Fix the build", "url": "https://api.github.com/repos/shop/web/issues/42", "type": "Issue" },
              "repository": { "name": "web", "full_name": "shop/web", "owner": { "login": "shop" } } },
            { "id": "26211607272", "reason": "ci_activity", "updated_at": "2026-10-15T09:01:00Z",
              "subject": { "title": "CI", "url": null, "type": "CheckSuite" },
              "repository": { "name": "web", "full_name": "shop/web", "owner": { "login": "shop" } } },
        ]);

        let messages = waiter::messages_in(&notifications_waiter(false), &answer.to_string()).unwrap();
        assert_eq!(messages.len(), 1, "a notification about no thread is no conversation");
        let message = &messages[0];
        assert_eq!(message.id, "26211607271:2026-10-15T09:00:00Z");
        assert_eq!(message.conversation, "https://api.github.com/repos/shop/web/issues/42");
        assert_eq!(message.sender, "", "a notification names nobody");
        assert_eq!(message.text, "mention\n\nshop/web\n\nIssue\n\nFix the build\n\nhttps://api.github.com/repos/shop/web/issues/42");

        let waiter = notifications_waiter(false);
        let latest = waiter.latest.unwrap();
        assert_eq!(latest.also[0].call.arguments["method"], "get_reviews", "a pull request's reviews are weighed beside its comments");
        assert_eq!(latest.also[0].when, Some(When { pointer: "/subject/type".to_string(), is: "PullRequest".to_string() }));
        assert_eq!(waiter.handled, Some(Call { tool: "dismiss_notification".to_string(), arguments: json!({ "threadID": "{/id}", "state": "read" }) }), "and each is marked read once handled");
    }

    #[test]
    fn the_server_reads_her_own_gh_login_and_never_keeps_the_token() {
        let command = server_command();
        assert_eq!(command[..2], words(&["sh", "-c"]));
        assert!(command[2].contains("$(gh auth token)") && command[2].contains("notifications"), "{}", command[2]);
        assert!(config::environment(&server_env())["GH_CONFIG_DIR"].ends_with("/sous-chef/tools/gh"), "her login, not the chef's");
    }
}
