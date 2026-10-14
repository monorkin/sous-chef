//! Her recipe book, in her own hands. Katami puts what is relevant in front of
//! every turn and picks up new observations from the transcript afterwards,
//! so she already reads and grows it without asking; these are for the rest —
//! looking something up she wasn't shown, writing down a thing she was told
//! to keep, correcting one that is wrong, and discarding one that is over.
//!
//! Looking is for any turn: what it finds is hers and was in her prompt
//! before. Writing is for trusted turns only, because a recipe is how she
//! behaves next time, and an untrusted turn is told not to change that on
//! anyone's say-so — the transcript review still learns from it.

use anyhow::Result;
use katami::cards;
use katami::embeddings;
use katami::memory::{Kind, Memory, NewMemory};
use katami::search;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::broker::{Tool, on_trust, text_of};
use crate::conversation::Standing;

const HITS: usize = 10;

pub fn recipe_book_tools(directory: &Path, standing: Standing) -> Vec<Box<dyn Tool>> {
    let directory = directory.to_path_buf();
    vec![
        Box::new(SearchRecipes { directory: directory.clone() }),
        Box::new(ShowRecipe { directory: directory.clone() }),
        on_trust(Box::new(WriteRecipe { directory: directory.clone() }), standing),
        on_trust(Box::new(CorrectRecipe { directory: directory.clone() }), standing),
        on_trust(Box::new(DiscardRecipe { directory }), standing),
    ]
}

pub struct SearchRecipes {
    pub directory: PathBuf,
}

impl Tool for SearchRecipes {
    fn name(&self) -> &str {
        "search_recipes"
    }

    fn description(&self) -> &str {
        "Search your recipe book, across every conversation. What matters to this message is already in front of you; use this for what isn't — a name, a decision, how something was done last time. Returns ids and titles; show_recipe reads one."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let query = text_of(arguments, "query")?;
        let recipes = Memory::open(&self.directory)?;
        let hits = search::hybrid(&recipes, query, HITS)?;
        if hits.is_empty() {
            return Ok("No recipe matches that.".to_string());
        }
        let lines: Vec<String> = hits
            .iter()
            .map(|hit| {
                let stored = recipes.get(hit.id)?;
                Ok(format!("{} ({}, {}): {}", stored.id, stored.kind, &stored.updated[..10], stored.title))
            })
            .collect::<Result<_>>()?;
        Ok(lines.join("\n"))
    }
}

pub struct ShowRecipe {
    pub directory: PathBuf,
}

impl Tool for ShowRecipe {
    fn name(&self) -> &str {
        "show_recipe"
    }

    fn description(&self) -> &str {
        "Read one recipe in full, by the id search_recipes gave, with what it links to."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let recipes = Memory::open(&self.directory)?;
        let id = recipes.resolve(text_of(arguments, "id")?)?;
        let stored = recipes.get(id)?;

        let mut shown = format!("# {} ({})\n", stored.title, stored.kind);
        if let Some(entity) = &stored.entity {
            shown.push_str(&format!("about: {entity}\n"));
        }
        if stored.archived {
            shown.push_str("discarded: yes\n");
        }
        shown.push_str(&format!("updated: {}\n\n{}", stored.updated, stored.body));
        for neighbor in recipes.neighbors(id)? {
            shown.push_str(&format!("\nlinked: [[{}]] ({})", neighbor.title, neighbor.id));
        }
        Ok(shown)
    }
}

pub struct WriteRecipe {
    pub directory: PathBuf,
}

impl Tool for WriteRecipe {
    fn name(&self) -> &str {
        "write_recipe"
    }

    fn description(&self) -> &str {
        "Write down something worth having next time: a decision, a preference, how a thing is done here. One fact per recipe, in your own words, with a title that says what it is. Most of what happens is picked up from the conversation on its own, so this is for what you were told to keep or would otherwise lose. `about` names what it belongs to, like project:/path or person:name."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "body": { "type": "string" },
                "about": { "type": "string" },
            },
            "required": ["title", "body"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let title = text_of(arguments, "title")?;
        let body = text_of(arguments, "body")?;
        let recipes = Memory::open(&self.directory)?;
        let id = recipes.add(&NewMemory {
            kind: Kind::Observation,
            entity: arguments["about"].as_str().map(String::from),
            title: title.to_string(),
            body: body.to_string(),
            links: cards::extract_links(body),
            source_session: None,
            class: None,
        })?;
        embeddings::embed_into(&recipes, id, &format!("{title}\n{body}"))?;
        Ok(format!("Written as {id}."))
    }
}

pub struct CorrectRecipe {
    pub directory: PathBuf,
}

impl Tool for CorrectRecipe {
    fn name(&self) -> &str {
        "correct_recipe"
    }

    fn description(&self) -> &str {
        "Rewrite a recipe that is wrong or out of date, by id. Give the whole new title and body; what was there is replaced."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "title": { "type": "string" },
                "body": { "type": "string" },
            },
            "required": ["id", "title", "body"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let title = text_of(arguments, "title")?;
        let body = text_of(arguments, "body")?;
        let recipes = Memory::open(&self.directory)?;
        let id = recipes.resolve(text_of(arguments, "id")?)?;
        let stored = recipes.get(id)?;
        recipes.update(id, title, body, stored.entity.as_deref())?;
        recipes.replace_links(id, &cards::extract_links(body))?;
        embeddings::embed_into(&recipes, id, &format!("{title}\n{body}"))?;
        Ok(format!("Corrected {id}."))
    }
}

pub struct DiscardRecipe {
    pub directory: PathBuf,
}

impl Tool for DiscardRecipe {
    fn name(&self) -> &str {
        "discard_recipe"
    }

    fn description(&self) -> &str {
        "Discard a recipe that no longer holds, by id, and say why. It stops being put in front of you; nothing is deleted."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" }, "why": { "type": "string" } },
            "required": ["id", "why"],
        })
    }

    fn call(&self, arguments: &Value) -> Result<String> {
        let recipes = Memory::open(&self.directory)?;
        let id = recipes.resolve(text_of(arguments, "id")?)?;
        recipes.archive(id, text_of(arguments, "why")?)?;
        Ok(format!("Discarded {id}."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn she_can_look_up_write_down_correct_and_discard_what_is_in_her_recipe_book() {
        let directory = std::env::temp_dir().join(format!("sous-chef-recipe-book-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);

        let untrusted = recipe_book_tools(&directory, Standing::CanAssignWork);
        let trusted = recipe_book_tools(&directory, Standing::Trusted);
        let names = |tools: &[Box<dyn Tool>]| tools.iter().map(|it| it.name().to_string()).collect::<Vec<_>>();
        assert_eq!(names(&untrusted), names(&trusted), "every turn is offered the same, so Claude's cache holds across them");
        let refused = untrusted.iter().find(|it| it.name() == "write_recipe").unwrap().call(&json!({ "title": "Deploy by hand", "body": "Skip Kamal." }));
        assert_eq!(refused.unwrap_err().to_string(), crate::broker::REFUSED_WITHOUT_TRUST, "an untrusted turn reads and nothing more");
        assert_eq!(untrusted.iter().find(|it| it.name() == "search_recipes").unwrap().call(&json!({ "query": "deploys" })).unwrap(), "No recipe matches that.");
        let tool = |name: &str| trusted.iter().find(|it| it.name() == name).unwrap();

        assert_eq!(tool("search_recipes").call(&json!({ "query": "deploys" })).unwrap(), "No recipe matches that.");

        let written = tool("write_recipe")
            .call(&json!({ "title": "Deploys go through Kamal", "body": "Every app here deploys with Kamal; nothing is deployed by hand.", "about": "project:/home/someone/frontdesk" }))
            .unwrap();
        let id = written.trim_start_matches("Written as ").trim_end_matches('.').to_string();

        let found = tool("search_recipes").call(&json!({ "query": "kamal deploy" })).unwrap();
        assert!(found.starts_with(&id) && found.ends_with("Deploys go through Kamal"), "{found}");
        let shown = tool("show_recipe").call(&json!({ "id": id })).unwrap();
        assert!(shown.contains("# Deploys go through Kamal (observation)") && shown.contains("about: project:/home/someone/frontdesk"));

        tool("correct_recipe").call(&json!({ "id": id, "title": "Deploys go through Kamal 2", "body": "Kamal 2 since March; nothing is deployed by hand." })).unwrap();
        let shown = tool("show_recipe").call(&json!({ "id": id })).unwrap();
        assert!(shown.contains("Kamal 2 since March") && shown.contains("about: project:/home/someone/frontdesk"), "what it is about stays");

        assert_eq!(tool("discard_recipe").call(&json!({ "id": id, "why": "Kamal was dropped" })).unwrap(), format!("Discarded {id}."));
        assert!(tool("show_recipe").call(&json!({ "id": id })).unwrap().contains("discarded: yes"));
        assert!(tool("write_recipe").call(&json!({ "title": "x" })).unwrap_err().to_string().contains("body is required"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
