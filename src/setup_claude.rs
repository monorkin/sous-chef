//! The part of the setup walk that gives the agent a Claude login of its own,
//! without which it can't do anything.

use anyhow::Result;

use crate::prompt::Question;
use crate::setup::Wizard;

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeAccount {
    pub email: String,
    pub organization: Option<String>,
}

impl ClaudeAccount {
    /// How the account is named when the chef is asked to hand it over.
    pub fn described(&self) -> String {
        match &self.organization {
            Some(organization) => format!("{} ({organization})", self.email),
            None => self.email.clone(),
        }
    }
}

impl Wizard<'_> {
    /// The chef's own account is offered first, by name, so they know what
    /// they agree to. It is never copied: she logs in to it herself, in her
    /// own folder, because a login two folders share is refreshed out from
    /// under one of them by the other.
    pub fn claude(&mut self, agent: &str) -> Result<()> {
        if let Some(address) = self.doing.claude_login() {
            return self.asking.done(&format!("{agent} works in Claude as {address}."));
        }

        if let Some(yours) = self.doing.chefs_claude_login() {
            let question = format!("Should {agent} use your Claude account, {}?", yours.described());
            let hint = format!("Claude opens in the browser with that account filled in; approve it there. {agent} gets a login of its own to the account, so neither of you logs the other out.");
            if self.asking.yes(&Question { title: "Claude", question: &question, hint: &hint }, true)? {
                return self.log_in_to_claude(agent, Some(&yours.email));
            }
        }

        let question = format!("{agent} needs a Claude account to work. Do you want to log it in now?");
        let hint = "Claude opens in the browser; log in with the account it should use.";
        if self.asking.yes(&Question { title: "Claude", question: &question, hint }, true)? {
            self.log_in_to_claude(agent, None)
        } else {
            self.asking.trouble(&format!("{agent} can't work without a Claude account. Add one whenever you like with `sous-chef claude login` and `sous-chef claude account add`."))
        }
    }

    fn log_in_to_claude(&mut self, agent: &str, email: Option<&str>) -> Result<()> {
        self.asking.note("\n  Claude Code takes it from here: approve the login in the browser and come back.\n")?;
        let address = match self.doing.log_in_to_claude(email) {
            Ok(address) => address,
            Err(error) => return self.asking.trouble(&format!("Couldn't log in: {error:#}. {agent} can't work without it; `sous-chef claude login` and `sous-chef claude account add` try again.")),
        };
        match self.doing.keep_claude_login() {
            Ok(()) => self.asking.done(&format!("{agent} works in Claude as {address}.")),
            Err(error) => self.asking.trouble(&format!("{agent} works in Claude as {address}, but it couldn't be added to the rotation: {error:#}. `sous-chef claude account add` tries again.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup_rehearsal::{Pretend, Scripted, walk};

    #[test]
    fn the_chefs_own_claude_account_is_offered_by_name_and_logged_in_to_afresh() {
        let mut asking = Scripted::answering(&[
            "Botten",                // Name
            "y",                     // use your Claude account
            "", "", "n", "n",        // about, style, no service, no Jev
        ]);
        let chefs = ClaudeAccount { email: "marta@example.com".to_string(), organization: Some("Shop".to_string()) };
        let mut doing = Pretend { new_to_claude: true, chefs_claude_login: Some(chefs), ..Pretend::default() };
        walk(&mut asking, &mut doing, "claude-chefs");

        assert!(asking.asked.iter().any(|it| it == "Claude: Should Botten use your Claude account, marta@example.com (Shop)?"), "{:?}", asking.asked);
        assert_eq!(doing.logged_in_with, [Some("marta@example.com".to_string())], "she logs in to that account herself, never copies the chef's login");
        assert_eq!(doing.rotation, ["marta@example.com"]);
        assert!(asking.said.iter().any(|it| it == "Botten works in Claude as marta@example.com."));

        let mut again = Scripted::answering(&["Botten", "", "", "n", "n"]);
        walk(&mut again, &mut doing, "claude-chefs-again");
        assert_eq!(doing.logged_in_with.len(), 1, "a login already there is kept, and nothing is asked");
        assert!(!again.asked.iter().any(|it| it.starts_with("Claude:")));
    }

    #[test]
    fn turning_down_the_chefs_claude_account_offers_a_login_of_any_account() {
        let mut asking = Scripted::answering(&["Botten", "n", "y", "", "", "n", "n"]);
        let chefs = ClaudeAccount { email: "marta@example.com".to_string(), organization: None };
        let mut doing = Pretend { new_to_claude: true, chefs_claude_login: Some(chefs), ..Pretend::default() };
        walk(&mut asking, &mut doing, "claude-another");

        assert!(asking.asked.iter().any(|it| it == "Claude: Should Botten use your Claude account, marta@example.com?"));
        assert!(asking.asked.iter().any(|it| it == "Claude: Botten needs a Claude account to work. Do you want to log it in now?"));
        assert_eq!(doing.logged_in_with, [None], "whichever account the chef logs in with");
        assert_eq!(doing.rotation, ["botten@example.com"]);
    }

    #[test]
    fn without_a_claude_account_setup_says_how_to_add_one_later() {
        let mut asking = Scripted::answering(&["Botten", "n", "", "", "n", "n"]);
        let mut doing = Pretend { new_to_claude: true, ..Pretend::default() };
        walk(&mut asking, &mut doing, "claude-none");

        assert!(!asking.asked.iter().any(|it| it.contains("your Claude account")), "nobody is logged in to offer");
        assert!(doing.logged_in_with.is_empty());
        assert!(doing.rotation.is_empty());
        assert!(
            asking.said.iter().any(|it| it.starts_with("TROUBLE") && it.contains("`sous-chef claude login` and `sous-chef claude account add`")),
            "{:?}",
            asking.said
        );
    }
}
