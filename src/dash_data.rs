//! What `sous-chef dash` shows, gathered in one place: her Claude accounts and
//! Jev's bill, what is going on now, the end of her log, and her memories.
//! Gathering asks the running sous-chef, ax, katami and the log; turning their
//! answers into rows touches none of them, and that is the part the tests
//! cover.

use anyhow::Result;
use ax::usage::Report;
use katami::memory::{ListFilter, Listing, Memory, OverviewRow};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::config;
use crate::control;
use crate::taster::JEV_DOLLARS_PER_REQUEST_ESTIMATE;
use crate::lifecycle::text;
use crate::logs::{self, short_id};
use crate::paths;
use crate::secrets;

const LOG_LINES: usize = 500;

/// The station of a commis that works on this machine.
pub const HERE: &str = "here";

/// Everything on the dashboard at one moment.
pub struct Snapshot {
    pub accounts: Vec<Account>,
    /// Why there are no accounts to show, when ax couldn't say.
    pub accounts_unknown: Option<String>,
    /// Jev's requests and what they cost, as rows, when it is set up.
    pub jev: Option<Vec<[String; 2]>>,
    /// The newest lines of the log, as `sous-chef log` colours them.
    pub log: Vec<String>,
    /// Or why it can't be had: she isn't running, or didn't answer.
    pub now: Result<Now, String>,
    /// Every memory, archived ones too, the newest first; or why they
    /// couldn't be read.
    pub memories: Result<Vec<MemoryRow>, String>,
}

/// One memory as the dashboard lists and shows it.
#[derive(Debug, PartialEq, Clone)]
pub struct MemoryRow {
    pub id: String,
    pub kind: String,
    pub entity: Option<String>,
    pub title: String,
    pub body: String,
    pub archived: bool,
    /// How many times it was put in front of a session, and when last.
    pub uses: i64,
    pub last_used: Option<String>,
    pub updated: String,
}

/// One Claude account and how full its limits are.
#[derive(Debug, PartialEq)]
pub struct Account {
    /// Its alias, or its email up to the @.
    pub name: String,
    pub in_use: bool,
    /// Session, week and the model-scoped one, always three and in that
    /// order; a limit ax has nothing on is None.
    pub limits: [Option<Limit>; 3],
}

#[derive(Debug, PartialEq)]
pub struct Limit {
    pub name: String,
    pub percent: f64,
    /// How long until it resets, or empty when that isn't known.
    pub resets_in: String,
}

/// What `sous-chef status` knows, as rows.
#[derive(Default, Debug, PartialEq)]
pub struct Now {
    /// Chef de partie, how long its turn has run, messages waiting, title,
    /// and where its commis are: one row for every chef de partie with a
    /// turn running, commis working or work claimed, in the order of
    /// `chefs_de_partie`.
    pub rows: Vec<[String; 5]>,
    /// Where commis work: here first, then every station, idle ones too.
    pub stations: Vec<StationNow>,
    /// Every chef de partie that can be told something, by name, so the
    /// list holds still while their turns come and go.
    pub chefs_de_partie: Vec<String>,
    /// What is known about each of those chefs de partie, by its full name.
    pub about: HashMap<String, ChefDePartieNow>,
}

/// One chef de partie's part of what is going on, for the head of its chat.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct ChefDePartieNow {
    pub title: String,
    /// How long its turn has run; none between turns.
    pub turn_for: Option<String>,
    pub waiting: String,
    /// Each commis at work for it: the commis, its station, its worktree,
    /// and how long.
    pub commis: Vec<[String; 4]>,
}

/// One machine commis work on, and who is at work there.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct StationNow {
    pub name: String,
    /// The commis, its chef de partie, its worktree, and how long.
    pub commis: Vec<[String; 4]>,
}

/// Asked once: the key lives in the keyring, which isn't something to knock
/// on every few seconds.
pub fn jev_is_set_up() -> bool {
    secrets::load(config::JEV_API_KEY).is_some()
}

pub fn gather(with_jev: bool) -> Snapshot {
    let (accounts, accounts_unknown) = match ax::usage::of_every_account() {
        Ok(reports) => (accounts_of(&reports, ax::clock::now_seconds()), None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    };

    Snapshot {
        accounts,
        accounts_unknown,
        jev: with_jev.then(|| jev_rows(&JevRequests::in_log(&fs::read_to_string(paths::log_file()).unwrap_or_default(), &a_day_ago()))),
        log: logs::newest_styled(LOG_LINES),
        now: control::ask(json!({ "command": "status" })).map(|it| now_of(&it["status"])).map_err(|error| format!("{error}.")),
        memories: memories().map_err(|error| format!("The agent's memories couldn't be read: {error:#}.")),
    }
}

fn memories() -> Result<Vec<MemoryRow>> {
    let memory = Memory::open(&katami::paths::memory_dir())?;
    let listing = Listing { filter: ListFilter::All, kinds: Vec::new(), order: Vec::new() };
    Ok(memory.overview(&listing)?.iter().map(memory_row).collect())
}

fn memory_row(row: &OverviewRow) -> MemoryRow {
    let stored = &row.stored;
    MemoryRow {
        id: stored.id.to_string(),
        kind: stored.kind.to_string(),
        entity: stored.entity.clone(),
        title: stored.title.clone(),
        body: stored.body.clone(),
        archived: stored.archived,
        uses: row.uses,
        last_used: row.last_used.clone(),
        updated: stored.updated.clone(),
    }
}

fn a_day_ago() -> String {
    (chrono::Utc::now() - chrono::Duration::hours(24)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// How many requests went to Jev, from the log.
#[derive(Default, Debug, PartialEq)]
struct JevRequests {
    since: u64,
    ever: u64,
}

impl JevRequests {
    /// From `since` on, and over the whole log. Lines that aren't JSON are
    /// skipped.
    fn in_log(log: &str, since: &str) -> JevRequests {
        let mut requests = JevRequests::default();
        let asked = log.lines().filter(|it| it.contains("taster.jev_asked")).filter_map(|it| serde_json::from_str::<Value>(it).ok());
        for entry in asked.filter(|it| it["event"] == "taster.jev_asked") {
            requests.ever += 1;
            if entry["at"].as_str().unwrap_or_default() >= since {
                requests.since += 1;
            }
        }
        requests
    }
}

/// Each account's limits, in the order and under the names ax gives them.
fn accounts_of(reports: &[Report], now: i64) -> Vec<Account> {
    reports
        .iter()
        .map(|report| {
            let mut limits = [None, None, None];
            if let Some(reading) = &report.reading {
                for (slot, (name, window)) in limits.iter_mut().zip(ax::usage::limits_of(&reading.usage)) {
                    *slot = window.map(|window| Limit {
                        name: name.to_string(),
                        percent: window.utilization,
                        resets_in: window
                            .resets_at
                            .as_deref()
                            .and_then(ax::clock::epoch_seconds_of)
                            .map(|at| ax::clock::span(at - now))
                            .unwrap_or_default(),
                    });
                }
            }
            Account { name: name_of(&report.account), in_use: report.active, limits }
        })
        .collect()
}

fn name_of(account: &ax::store::Account) -> String {
    match &account.alias {
        Some(alias) => alias.clone(),
        None => account.email.split('@').next().unwrap_or_default().to_string(),
    }
}

fn jev_rows(requests: &JevRequests) -> Vec<[String; 2]> {
    vec![
        ["Requests, last day".to_string(), requests.since.to_string()],
        ["Requests, whole log".to_string(), requests.ever.to_string()],
        ["Cost, last day (est.)".to_string(), dollars(requests.since)],
        ["Cost, whole log (est.)".to_string(), dollars(requests.ever)],
    ]
}

fn dollars(requests: u64) -> String {
    format!("${:.2}", requests as f64 * JEV_DOLLARS_PER_REQUEST_ESTIMATE)
}

/// The `status` answer of a running sous-chef, as rows. Everything is in
/// the order of the chefs de partie's names, which nothing that happens
/// moves.
fn now_of(status: &Value) -> Now {
    let mut about: BTreeMap<String, ChefDePartieNow> = BTreeMap::new();
    for turn in listed(&status["going_on"]) {
        let commis = listed(&turn["commis"]).iter().map(commis_of).collect();
        let chef_de_partie = ChefDePartieNow {
            title: text(&turn["doing"]).to_string(),
            turn_for: turn["for"].as_str().map(String::from),
            waiting: waiting(&turn["waiting"]),
            commis,
        };
        about.insert(text(&turn["conversation"]).to_string(), chef_de_partie);
    }
    for work in listed(&status["board"]) {
        about.insert(text(&work["conversation"]).to_string(), ChefDePartieNow { title: text(&work["doing"]).to_string(), ..ChefDePartieNow::default() });
    }

    let mut stations: Vec<StationNow> = std::iter::once(HERE)
        .chain(listed(&status["stations"]).iter().map(text))
        .map(|name| StationNow { name: name.to_string(), commis: Vec::new() })
        .collect();
    for (chef_de_partie, it) in &about {
        for [commis, station, worktree, taken] in &it.commis {
            let at = match stations.iter().position(|it| &it.name == station) {
                Some(at) => at,
                None => {
                    stations.push(StationNow { name: station.clone(), commis: Vec::new() });
                    stations.len() - 1
                }
            };
            stations[at].commis.push([commis.clone(), short_id(chef_de_partie), worktree.clone(), taken.clone()]);
        }
    }

    Now {
        rows: about
            .iter()
            .map(|(chef_de_partie, it)| [short_id(chef_de_partie), it.turn_for.clone().unwrap_or_default(), it.waiting.clone(), it.title.clone(), where_its_commis_are(&it.commis)])
            .collect(),
        stations,
        chefs_de_partie: about.keys().cloned().collect(),
        about: about.into_iter().collect(),
    }
}

fn commis_of(commis: &Value) -> [String; 4] {
    let worktree = Path::new(text(&commis["project"])).file_name().map(|it| it.to_string_lossy().into_owned()).unwrap_or_default();
    let station = commis["station"].as_str().unwrap_or(HERE).to_string();
    [text(&commis["commis"]).to_string(), station, worktree, text(&commis["for"]).to_string()]
}

/// `1 here`, `2 at server`, `1 here · 1 at server`: how many commis a chef de
/// partie has out, and where, in the order they were sent.
fn where_its_commis_are(commis: &[[String; 4]]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for [_, station, _, _] in commis {
        match counted.iter_mut().find(|(it, _)| it == station) {
            Some((_, count)) => *count += 1,
            None => counted.push((station, 1)),
        }
    }
    counted
        .into_iter()
        .map(|(station, count)| {
            if station == HERE {
                format!("{count} here")
            } else {
                format!("{count} at {station}")
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn listed(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn waiting(value: &Value) -> String {
    match value.as_u64() {
        Some(0) | None => String::new(),
        Some(count) => count.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jev_requests_are_counted_from_a_moment_on_and_over_the_whole_log() {
        let log = [
            r#"{"at":"2026-09-26T09:00:00Z","event":"taster.jev_asked","details":{"bytes":10}}"#,
            r#"{"at":"2026-09-27T09:00:00Z","event":"taster.jev_asked","details":{"bytes":10}}"#,
            r#"{"at":"2026-09-27T09:00:01Z","event":"chef_de_partie.woken","details":{"said":"taster.jev_asked"}}"#,
            "taster.jev_asked, not json",
        ]
        .join("\n");

        let requests = JevRequests::in_log(&log, "2026-09-27T00:00:00Z");
        assert_eq!(requests, JevRequests { since: 1, ever: 2 });
        let rows = jev_rows(&requests);
        assert_eq!(rows[0], ["Requests, last day".to_string(), "1".to_string()]);
        assert_eq!(rows[1][1], "2");
        assert_eq!(rows[3][1], dollars(2));
        assert_eq!(dollars(2500), format!("${:.2}", 2500.0 * JEV_DOLLARS_PER_REQUEST_ESTIMATE));
    }

    #[test]
    fn accounts_show_each_limit_and_which_one_is_in_use() {
        let account = |email: &str| serde_json::from_value(json!({ "number": 1, "email": email, "added": "2026-09-01" })).unwrap();
        let usage = serde_json::from_value(json!({
            "five_hour": { "utilization": 40.0, "resets_at": "2026-09-27T12:00:00Z" },
            "seven_day": { "utilization": 10.0, "resets_at": null },
        }))
        .unwrap();
        let mut aliased: ax::store::Account = account("work@example.com");
        aliased.alias = Some("Work".to_string());
        let reports = [
            Report { account: account("marta@example.com"), active: true, reading: Some(ax::usage::Reading { usage, taken_at: 1_000 }), failed: None },
            Report { account: aliased, active: false, reading: None, failed: Some("asked too often".to_string()) },
        ];
        let now = ax::clock::epoch_seconds_of("2026-09-27T10:20:00Z").unwrap();

        let accounts = accounts_of(&reports, now);
        assert_eq!(
            accounts[0],
            Account {
                name: "marta".to_string(),
                in_use: true,
                limits: [
                    Some(Limit { name: "session".to_string(), percent: 40.0, resets_in: "1h 40m".to_string() }),
                    Some(Limit { name: "week".to_string(), percent: 10.0, resets_in: String::new() }),
                    None,
                ],
            }
        );
        assert_eq!(accounts[1], Account { name: "Work".to_string(), in_use: false, limits: [None, None, None] });
    }

    #[test]
    fn status_becomes_one_row_per_chef_de_partie_by_name_and_its_commis_under_their_stations() {
        let status = json!({
            "going_on": [{
                "conversation": "basecamp-card-3",
                "for": "1m",
                "waiting": 0,
                "doing": null,
                "commis": [],
            }, {
                "conversation": "basecamp-card-1",
                "for": "14m",
                "waiting": 3,
                "doing": "Fixing the login",
                "commis": [
                    { "commis": "h18d9", "project": "/srv/worktrees/login-fix", "station": null, "for": "13m" },
                    { "commis": "h18da", "project": "/srv/worktrees/login-docs", "station": "server", "for": "2m" },
                ],
            }],
            "board": [{ "conversation": "basecamp-card-2", "doing": "Waiting on review" }],
            "stations": ["build-box", "server"],
        });

        let row = |cells: [&str; 5]| cells.map(String::from);
        let now = now_of(&status);
        assert_eq!(
            now.rows,
            vec![
                row(["basecamp-card-1", "14m", "3", "Fixing the login", "1 here · 1 at server"]),
                row(["basecamp-card-2", "", "", "Waiting on review", ""]),
                row(["basecamp-card-3", "1m", "", "-", ""]),
            ],
            "running and claimed alike, by name, so a turn starting or ending moves nothing"
        );
        assert_eq!(now.chefs_de_partie, ["basecamp-card-1", "basecamp-card-2", "basecamp-card-3"]);
        let station = |name: &str, commis: Vec<[&str; 4]>| StationNow { name: name.to_string(), commis: commis.into_iter().map(|it| it.map(String::from)).collect() };
        assert_eq!(
            now.stations,
            vec![
                station("here", vec![["h18d9", "basecamp-card-1", "login-fix", "13m"]]),
                station("build-box", vec![]),
                station("server", vec![["h18da", "basecamp-card-1", "login-docs", "2m"]]),
            ],
            "here first, then every station, the idle one too"
        );
        assert_eq!(
            now.about["basecamp-card-1"],
            ChefDePartieNow {
                title: "Fixing the login".to_string(),
                turn_for: Some("14m".to_string()),
                waiting: "3".to_string(),
                commis: vec![["h18d9", "here", "login-fix", "13m"].map(String::from), ["h18da", "server", "login-docs", "2m"].map(String::from)],
            }
        );
        assert_eq!(now.about["basecamp-card-2"], ChefDePartieNow { title: "Waiting on review".to_string(), ..ChefDePartieNow::default() }, "a chef de partie between turns has no turn running");
        assert_eq!(now_of(&json!({})).stations, vec![station("here", vec![])], "with nothing going on, here is still there");
    }

    #[test]
    fn a_station_a_commis_names_that_is_not_listed_still_gets_shown() {
        let status = json!({
            "going_on": [{ "conversation": "basecamp-card-1", "commis": [{ "commis": "h1", "project": "/srv/shop", "station": "old-box", "for": "5m" }] }],
        });
        let names: Vec<String> = now_of(&status).stations.into_iter().map(|it| it.name).collect();
        assert_eq!(names, ["here", "old-box"]);
    }
}
