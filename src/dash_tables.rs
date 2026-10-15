//! The dashboard's tables: Claude accounts and Jev, each station's commis,
//! and every chef de partie that is busy or has work claimed. Every column gets a width
//! here, and text too long for its column is cut with an ellipsis rather
//! than stopped mid-word at the edge.

use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};

use crate::dash_data::{Account, HERE, Limit, Now, StationNow};

const SPACING: u16 = 1;
const PERCENT: u16 = 7;
const RESETS: u16 = 7;
const CHEF_DE_PARTIE: u16 = 18;
const RUNNING: u16 = 7;
const WAITING: u16 = 7;
/// A commis by the end of its id, which tells it apart from the others on
/// its station well enough and leaves the worktree its room.
const COMMIS: u16 = 8;
const WORKTREE: u16 = 12;
/// Where a chef de partie's commis are: `1 here · 2 at server`.
const WHERE: u16 = 28;

/// The narrowest a station's box gets, borders and all, before the next one
/// goes on a row below rather than beside it.
pub const STATION_WIDTH: u16 = COMMIS + CHEF_DE_PARTIE + WORKTREE + RUNNING + 3 * SPACING + 2;

/// Room for the Jev table beside the accounts.
pub const JEV_WIDTH: u16 = 36;
const LONGEST_NAME: u16 = 24;

pub fn accounts(accounts: &[Account], width: u16) -> Table<'static> {
    let name_width = width.saturating_sub(3 * (PERCENT + RESETS) + 6 * SPACING).clamp(4, LONGEST_NAME);
    let scoped = accounts.iter().find_map(|it| it.limits[2].as_ref()).map(|it| it.name.clone()).unwrap_or_else(|| "Fable".to_string());
    let header = ["Account".to_string(), "Session".to_string(), "resets".to_string(), "Week".to_string(), "resets".to_string(), scoped, "resets".to_string()];

    let rows = accounts.iter().map(|account| {
        let mut cells = vec![name_cell(account, name_width)];
        for limit in &account.limits {
            cells.extend(limit_cells(limit.as_ref()));
        }
        Row::new(cells)
    });
    let mut widths = vec![Constraint::Length(name_width)];
    widths.extend([PERCENT, RESETS].repeat(3).into_iter().map(Constraint::Length));
    Table::new(rows, widths).header(bold_row(header)).column_spacing(SPACING)
}

pub fn jev(rows: &[[String; 2]]) -> Table<'static> {
    Table::new(rows.iter().map(|it| Row::new(it.clone())), [Constraint::Length(24), Constraint::Length(JEV_WIDTH - 24 - SPACING)])
        .block(Block::new().title(heading("Jev")))
        .column_spacing(SPACING)
}

/// Every chef de partie that is busy or has work claimed, one row each, with
/// how long its turn has run left empty between turns.
pub fn chefs_de_partie(now: &Now, width: u16) -> Table<'static> {
    let title = width.saturating_sub(CHEF_DE_PARTIE + RUNNING + WAITING + WHERE + 4 * SPACING).max(8);
    let widths = [CHEF_DE_PARTIE, RUNNING, WAITING, title, WHERE];

    let rows: Vec<Row> = if now.rows.is_empty() {
        vec![Row::new(["Nothing going on or claimed."])]
    } else {
        now.rows.iter().map(|row| styled_row(row, widths, CHEF_DE_PARTIE_STYLES)).collect()
    };
    Table::new(rows, widths.map(Constraint::Length))
        .header(bold_row(["Chef de partie", "Turn", "Waiting", "Title", "Commis"].map(String::from)))
        .column_spacing(SPACING)
}

/// Who is at work on one station: its commis, whose they are, where, and
/// for how long, in as many lines as `height`, the last one saying how many
/// more there are when they don't all fit.
pub fn station(station: &StationNow, width: u16, height: u16) -> Table<'static> {
    let worktree = width.saturating_sub(COMMIS + CHEF_DE_PARTIE + RUNNING + 3 * SPACING).max(6);
    let widths = [COMMIS, CHEF_DE_PARTIE, worktree, RUNNING];

    let rows: Vec<Row> = station_lines(&station.commis, height as usize)
        .into_iter()
        .map(|line| match line {
            StationLine::Idle => Row::new(["Idle"]).style(Style::new().add_modifier(Modifier::DIM)),
            StationLine::Commis([commis, chef_de_partie, worktree, taken]) => {
                styled_row(&[last_of(&commis, COMMIS as usize), chef_de_partie, worktree, taken], widths, STATION_STYLES)
            }
            StationLine::More(more) => Row::new([format!("+{more} more")]).style(Style::new().add_modifier(Modifier::DIM)),
        })
        .collect();
    Table::new(rows, widths.map(Constraint::Length)).column_spacing(SPACING)
}

#[derive(Debug, PartialEq)]
enum StationLine {
    Idle,
    Commis([String; 4]),
    More(usize),
}

fn station_lines(commis: &[[String; 4]], height: usize) -> Vec<StationLine> {
    let height = height.max(1);
    if commis.is_empty() {
        vec![StationLine::Idle]
    } else if commis.len() <= height {
        commis.iter().cloned().map(StationLine::Commis).collect()
    } else {
        let shown = height - 1;
        let mut lines: Vec<StationLine> = commis.iter().take(shown).cloned().map(StationLine::Commis).collect();
        lines.push(StationLine::More(commis.len() - shown));
        lines
    }
}

/// The last `count` characters of `text`.
fn last_of(text: &str, count: usize) -> String {
    let skip = text.chars().count().saturating_sub(count);
    text.chars().skip(skip).collect()
}

/// A station's box: its name in the colour of where it is, and how many
/// commis are at work on it.
pub fn station_block(station: &StationNow) -> Block<'static> {
    let busy = match station.commis.len() {
        0 => "idle".to_string(),
        1 => "1 commis".to_string(),
        many => format!("{many} commis"),
    };
    let border = if station.commis.is_empty() {
        Style::new().fg(Color::DarkGray)
    } else {
        station_style(&station.name)
    };
    Block::bordered()
        .border_style(border)
        .title(Line::from(vec![Span::styled(format!(" {} ", station.name), station_style(&station.name)), Span::styled(format!("· {busy} "), Style::new().add_modifier(Modifier::DIM))]))
}

fn styled_row<const N: usize>(cells: &[String; N], widths: [u16; N], styles: [fn(&str) -> Style; N]) -> Row<'static> {
    Row::new(cells.iter().zip(widths).zip(styles).map(|((text, width), style)| Cell::from(ellipsized(text, width as usize)).style(style(text))))
}

/// How each column of the chefs de partie is coloured, by what is in it.
const CHEF_DE_PARTIE_STYLES: [fn(&str) -> Style; 5] = [chef_de_partie_style, age_style, waiting_style, plain_style, where_style];

/// And each column of a station's box.
const STATION_STYLES: [fn(&str) -> Style; 4] = [commis_style, chef_de_partie_style, worktree_style, age_style];

fn chef_de_partie_style(_: &str) -> Style {
    Style::new().fg(Color::Cyan)
}

/// Green while it's fresh, yellow past half an hour, red past two.
fn age_style(span: &str) -> Style {
    match minutes_in(span) {
        Some(minutes) if minutes < 30 => Style::new().fg(Color::Green),
        Some(minutes) if minutes < 120 => Style::new().fg(Color::Yellow),
        Some(_) => Style::new().fg(Color::Red),
        None => Style::new(),
    }
}

fn waiting_style(count: &str) -> Style {
    if count.parse::<u64>().is_ok_and(|it| it > 0) {
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    }
}

fn plain_style(_: &str) -> Style {
    Style::new()
}

fn commis_style(_: &str) -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// A commis away at a station stands out; one working here doesn't.
fn station_style(station: &str) -> Style {
    if station == HERE {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
    }
}

/// Commis that are all here stay quiet; any away at a station stand out.
fn where_style(commis: &str) -> Style {
    if commis.contains(" at ") {
        Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::DIM)
    }
}

fn worktree_style(_: &str) -> Style {
    Style::new().fg(Color::Magenta)
}

/// A span as a running sous-chef gives it — "16s", "58m", "1h 17m", "2d 3h" —
/// in whole minutes; None for anything else.
fn minutes_in(span: &str) -> Option<u64> {
    if span.trim().is_empty() {
        return None;
    }

    let mut minutes = 0;
    for part in span.split_whitespace() {
        let (number, unit) = part.split_at(part.find(|it: char| !it.is_ascii_digit())?);
        let number: u64 = number.parse().ok()?;
        minutes += match unit {
            "d" => number * 24 * 60,
            "h" => number * 60,
            "m" => number,
            "s" => 0,
            _ => return None,
        };
    }
    Some(minutes)
}

fn name_cell(account: &Account, width: u16) -> Cell<'static> {
    let name = ellipsized(&account.name, width.saturating_sub(2) as usize);
    if account.in_use {
        Cell::from(Line::from(vec![Span::raw("● "), Span::raw(name)])).style(Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Cell::from(format!("  {name}"))
    }
}

fn limit_cells(limit: Option<&Limit>) -> [Cell<'static>; 2] {
    match limit {
        Some(limit) => [
            Cell::from(format!("{:>4.0}%", limit.percent)).style(Style::new().fg(level_colour(limit.percent))),
            Cell::from(limit.resets_in.clone()).style(Style::new().add_modifier(Modifier::DIM)),
        ],
        None => [Cell::from("    —").style(Style::new().add_modifier(Modifier::DIM)), Cell::from("")],
    }
}

/// The same thresholds ax colours its bars by.
fn level_colour(percent: f64) -> Color {
    if percent >= 90.0 {
        Color::Red
    } else if percent >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn bold_row<const N: usize>(cells: [String; N]) -> Row<'static> {
    Row::new(cells).style(Style::new().add_modifier(Modifier::BOLD))
}

fn heading(text: &'static str) -> Line<'static> {
    Line::styled(text, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
}

/// `text` as it fits in `width` characters: whole, or cut with an ellipsis
/// as its last one.
pub fn ellipsized(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else if width == 0 {
        String::new()
    } else {
        format!("{}…", text.chars().take(width - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_with_an_ellipsis_to_its_width() {
        assert_eq!(ellipsized("Fixing the login", 20), "Fixing the login");
        assert_eq!(ellipsized("Fixing the login", 16), "Fixing the login");
        assert_eq!(ellipsized("Fixing the login", 10), "Fixing th…");
        assert_eq!(ellipsized("Fixing", 1), "…");
        assert_eq!(ellipsized("Fixing", 0), "");
    }

    #[test]
    fn how_long_something_has_run_is_coloured_by_its_age() {
        assert_eq!(minutes_in("16s"), Some(0));
        assert_eq!(minutes_in("1h 17m"), Some(77));
        assert_eq!(minutes_in("2d 3h"), Some(2 * 24 * 60 + 180));
        assert_eq!(minutes_in("-"), None);
        assert_eq!(minutes_in(""), None);
        assert_eq!(minutes_in("5 minutes"), None);

        assert_eq!(age_style("29m").fg, Some(Color::Green));
        assert_eq!(age_style("30m").fg, Some(Color::Yellow));
        assert_eq!(age_style("1h 59m").fg, Some(Color::Yellow));
        assert_eq!(age_style("2h 0m").fg, Some(Color::Red));
        assert_eq!(age_style("-"), Style::new());
    }

    #[test]
    fn only_a_waiting_count_above_nothing_stands_out() {
        assert_eq!(waiting_style("3").fg, Some(Color::Yellow));
        assert_eq!(waiting_style("0"), Style::new());
        assert_eq!(waiting_style(""), Style::new());
    }

    #[test]
    fn a_station_shows_what_fits_and_says_how_many_more_there_are() {
        let commis: Vec<[String; 4]> = (1..=4).map(|it| [format!("h{it}"), "basecamp-card-1".to_string(), "shop".to_string(), "2m".to_string()]).collect();
        assert_eq!(station_lines(&[], 3), [StationLine::Idle]);
        assert_eq!(station_lines(&commis[..2], 3).len(), 2, "all of them, when they fit");
        let lines = station_lines(&commis, 3);
        assert_eq!(lines[..2], [StationLine::Commis(commis[0].clone()), StationLine::Commis(commis[1].clone())]);
        assert_eq!(lines[2], StationLine::More(2), "the last line counts the rest");
        assert_eq!(last_of("h18deb1c1c846bd63", 8), "c846bd63");
        assert_eq!(last_of("h1", 8), "h1");
    }

    #[test]
    fn a_commis_away_at_a_station_stands_out_and_one_here_does_not() {
        assert_eq!(station_style("server").fg, Some(Color::Blue));
        assert_eq!(station_style(HERE).fg, None);
        assert_eq!(where_style("1 here · 1 at server").fg, Some(Color::Blue), "a chef de partie with any commis away stands out");
        assert_eq!(where_style("2 here").fg, None);
    }

    #[test]
    fn a_limit_is_green_then_yellow_then_red_as_it_fills() {
        assert_eq!(level_colour(0.0), Color::Green);
        assert_eq!(level_colour(69.9), Color::Green);
        assert_eq!(level_colour(70.0), Color::Yellow);
        assert_eq!(level_colour(90.0), Color::Red);
    }
}
