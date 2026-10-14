//! The dashboard's recipes tab: every recipe she has, archived ones dimmed,
//! the picked one read in full beside the list, and archiving and bringing
//! back from there the way `sous-chef recipes archive` and `unarchive` do.

use anyhow::Result;
use katami::memory::Memory;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::dash::Effect;
use crate::dash_data::RecipeRow;

#[derive(Default)]
pub struct Recipes {
    picked: usize,
    /// Lines the picked recipe is scrolled down by.
    down: u16,
    pub said: String,
}

impl Recipes {
    pub fn press(&mut self, code: KeyCode, rows: &[RecipeRow]) -> Effect {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.pick(self.picked.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.pick((self.picked + 1).min(rows.len().saturating_sub(1))),
            KeyCode::Home => self.pick(0),
            KeyCode::End => self.pick(rows.len().saturating_sub(1)),
            KeyCode::PageDown => self.down = self.down.saturating_add(10),
            KeyCode::PageUp => self.down = self.down.saturating_sub(10),
            KeyCode::Char('a') => match rows.get(self.picked) {
                Some(row) if !row.archived => return Effect::Archive { id: row.id.clone() },
                Some(_) => self.said = "That one is archived already; u brings it back.".to_string(),
                None => {}
            },
            KeyCode::Char('u') => match rows.get(self.picked) {
                Some(row) if row.archived => return Effect::Unarchive { id: row.id.clone() },
                Some(_) => self.said = "That one isn't archived.".to_string(),
                None => {}
            },
            _ => {}
        }
        Effect::Nothing
    }

    fn pick(&mut self, index: usize) {
        if index != self.picked {
            self.picked = index;
            self.down = 0;
        }
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, rows: &Result<Vec<RecipeRow>, String>) {
        let rows = match rows {
            Ok(rows) if rows.is_empty() => return frame.render_widget(Paragraph::new("She has no recipes yet."), area),
            Ok(rows) => rows,
            Err(why) => return frame.render_widget(Paragraph::new(why.as_str()), area),
        };
        let picked = self.picked.min(rows.len() - 1);
        let [list, _, shown] = Layout::horizontal([Constraint::Percentage(38), Constraint::Length(1), Constraint::Fill(1)]).areas(area);

        let block = Block::bordered().border_style(Style::new().fg(Color::DarkGray)).title(Line::styled(
            format!(" {} recipes, {} archived ", rows.len(), rows.iter().filter(|it| it.archived).count()),
            Style::new().fg(Color::Cyan),
        ));
        let inner = block.inner(list);
        frame.render_widget(block, list);
        let height = inner.height as usize;
        let first = picked.saturating_sub(height.saturating_sub(1));
        let lines: Vec<Line> = rows.iter().enumerate().skip(first).take(height).map(|(index, row)| listed(row, index == picked)).collect();
        frame.render_widget(Paragraph::new(lines), inner);

        let [recipe, message] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(shown);
        frame.render_widget(Paragraph::new(read(&rows[picked])).wrap(Wrap { trim: false }).scroll((self.down, 0)), recipe);
        frame.render_widget(Paragraph::new(self.said.as_str()).style(Style::new().add_modifier(Modifier::DIM)), message);
    }
}

pub fn archive(id: &str) -> Result<String> {
    let memory = Memory::open(&katami::paths::memory_dir())?;
    let id = memory.resolve(id)?;
    memory.archive(id, "archived from sous-chef dash")?;
    Ok(format!("Archived “{}”.", memory.get(id)?.title))
}

pub fn unarchive(id: &str) -> Result<String> {
    let memory = Memory::open(&katami::paths::memory_dir())?;
    let id = memory.resolve(id)?;
    memory.unarchive(id)?;
    Ok(format!("Brought back “{}”.", memory.get(id)?.title))
}

fn listed(row: &RecipeRow, picked: bool) -> Line<'static> {
    let mut style = if row.archived { Style::new().fg(Color::DarkGray) } else { Style::new() };
    if picked {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(vec![Span::styled(format!("{:<12} ", row.kind), style.add_modifier(Modifier::DIM)), Span::styled(row.title.clone(), style)])
}

/// The recipe in full: its title, what it is and how much it's used, and
/// what it says.
fn read(row: &RecipeRow) -> Vec<Line<'static>> {
    let mut about = vec![row.kind.clone()];
    about.extend(row.entity.clone());
    about.push(match (row.uses, &row.last_used) {
        (0, _) => "never used".to_string(),
        (uses, Some(last)) => format!("used {uses} times, last {}", day_of(last)),
        (uses, None) => format!("used {uses} times"),
    });
    about.push(format!("updated {}", day_of(&row.updated)));

    let mut head = vec![Span::styled(row.title.clone(), Style::new().add_modifier(Modifier::BOLD))];
    if row.archived {
        head.push(Span::styled("  archived", Style::new().fg(Color::Yellow)));
    }
    let mut lines = vec![
        Line::from(head),
        Line::styled(format!("{} · {}", row.id, about.join(" · ")), Style::new().add_modifier(Modifier::DIM)),
        Line::from(""),
    ];
    lines.extend(row.body.lines().map(|it| Line::from(it.to_string())));
    lines
}

/// `2026-10-02T07:30:12Z` as `2026-10-02 07:30`.
fn day_of(timestamp: &str) -> String {
    timestamp.get(..16).unwrap_or(timestamp).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, archived: bool) -> RecipeRow {
        RecipeRow {
            id: id.to_string(),
            kind: "observation".to_string(),
            entity: Some("project:shop".to_string()),
            title: format!("Recipe {id}"),
            body: "First line\nSecond line".to_string(),
            archived,
            uses: 3,
            last_used: Some("2026-10-01T09:15:00Z".to_string()),
            updated: "2026-09-30T18:02:44Z".to_string(),
        }
    }

    #[test]
    fn recipes_are_picked_read_and_archived_or_brought_back() {
        let rows = [row("aaaa1111", false), row("bbbb2222", true)];
        let mut recipes = Recipes::default();

        assert_eq!(recipes.press(KeyCode::Char('u'), &rows), Effect::Nothing, "the first isn't archived");
        assert_eq!(recipes.said, "That one isn't archived.");
        assert_eq!(recipes.press(KeyCode::Char('a'), &rows), Effect::Archive { id: "aaaa1111".to_string() });

        recipes.press(KeyCode::PageDown, &rows);
        assert_eq!(recipes.down, 10);
        recipes.press(KeyCode::Down, &rows);
        assert_eq!((recipes.picked, recipes.down), (1, 0), "another recipe is read from its top");
        recipes.press(KeyCode::Down, &rows);
        assert_eq!(recipes.picked, 1, "the last stays picked");

        assert_eq!(recipes.press(KeyCode::Char('a'), &rows), Effect::Nothing, "it's archived already");
        assert_eq!(recipes.press(KeyCode::Char('u'), &rows), Effect::Unarchive { id: "bbbb2222".to_string() });
        recipes.press(KeyCode::Home, &rows);
        assert_eq!(recipes.picked, 0);
    }

    #[test]
    fn a_recipe_is_read_with_what_it_is_and_how_much_it_is_used() {
        let text: Vec<String> = read(&row("bbbb2222", true)).iter().map(|it| it.to_string()).collect();
        assert_eq!(
            text,
            [
                "Recipe bbbb2222  archived",
                "bbbb2222 · observation · project:shop · used 3 times, last 2026-10-01 09:15 · updated 2026-09-30 18:02",
                "",
                "First line",
                "Second line",
            ]
        );
        let unused = RecipeRow { uses: 0, last_used: None, entity: None, ..row("aaaa1111", false) };
        assert_eq!(read(&unused)[1].to_string(), "aaaa1111 · observation · never used · updated 2026-09-30 18:02");
    }
}
