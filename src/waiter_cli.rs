//! `sous-chef waiter …`: looking at a waiter without acting on it.

use anyhow::{Context, Result};
use std::sync::Arc;

use crate::config::{self, Config};
use crate::held::HeldBack;
use crate::mcp::Catalog;
use crate::plating::Plating;
use crate::waiter::{self, Position};
use crate::taster::Taster;

const LONGEST_SHOWN: usize = 70;

/// Calls the waiter's watch tool once and shows what she would make of the
/// answer: which messages she finds, who she thinks sent them, and whether
/// she listens to that person. Nothing is remembered and nobody is woken, so
/// it is safe to run while the pointers are still being worked out.
pub fn check(name: &str) -> Result<()> {
    let config = Config::load()?;
    let waiter = config
        .waiters
        .get(name)
        .with_context(|| format!("there is no waiter called {name}"))?;

    let taster = Arc::new(Taster::with_whatever_is_set_up(&config)?);
    let plating = Arc::new(Plating::new(config::style()?, taster.clone(), config.models.plating.clone()));
    let catalog = Catalog::open(&config, plating, taster, Arc::new(HeldBack::default()));

    // Read from where she has got to, and the position left where it is
    let arguments = match Position::of(name, waiter) {
        Some(position) => position.in_arguments(&waiter.watch.arguments),
        None => waiter.watch.arguments.clone(),
    };
    let answer = catalog.call(&waiter.server, &waiter.watch.tool, &arguments)?;
    let messages = waiter::messages_in(waiter, &answer)?;

    println!("{} messages found in {} bytes of answer.", messages.len(), answer.len());
    if waiter.cursor.is_some() && messages.is_empty() {
        println!("This waiter is read from a position, so an empty answer is the usual one: it holds what arrived since it last looked.");
    }
    for message in &messages {
        // Asked the way she asks it, since who spoke last is whose word it is
        let sender = match &waiter.latest {
            Some(latest) => match waiter::newest_spoken(&catalog, waiter, latest, message) {
                Ok(Some(spoken)) => spoken.sender,
                Ok(None) => message.sender.clone(),
                Err(error) => {
                    println!("\n  couldn't tell who spoke last: {error:#}");
                    message.sender.clone()
                }
            },
            None => message.sender.clone(),
        };
        let listened_to = match waiter.trusted.get(&sender) {
            _ if sender.is_empty() => "ignored: nobody could be found to have said it".to_string(),
            Some(person) => format!("listens to: {person}"),
            None if waiter.anyone => "listens to: anyone here".to_string(),
            None => "ignored: not trusted on this waiter".to_string(),
        };
        println!();
        println!("  id            {}", shortened(&message.id));
        println!("  conversation  {}", shortened(&message.conversation));
        println!("  sender        {sender} ({listened_to})");
        println!("  text          {}", shortened(message.text.lines().next().unwrap_or_default()));
    }
    Ok(())
}

fn shortened(text: &str) -> String {
    if text.chars().count() > LONGEST_SHOWN {
        format!("{}…", text.chars().take(LONGEST_SHOWN).collect::<String>())
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_cut_on_a_character_not_a_byte() {
        assert_eq!(shortened("short"), "short");
        assert_eq!(shortened(&"š".repeat(80)), format!("{}…", "š".repeat(70)));
    }
}
