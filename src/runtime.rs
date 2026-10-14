//! What one `sous-chef` process builds once and every chef de partie shares: the
//! taster, the plating, the MCP servers, and the proxy commis reach Claude
//! through.
//!
//! Starting up also sweeps away what dead processes left behind. A commis's
//! profile holds a live access token, and a process that was killed never got
//! to delete it.

use anyhow::Result;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::at_work::AtWork;
use crate::cooling::Cooling;
use crate::claude;
use crate::config::{self, Config};
use crate::github;
use crate::kitchen::Brigade;
use crate::logs;
use crate::mailbox::Mailbox;
use crate::mcp::Catalog;
use crate::paths;
use crate::plating::Plating;
use crate::proxy::{self, Proxy};
use crate::sandbox::Outside;
use crate::store::Store;
use crate::taster::Taster;
use crate::toolchains::Toolchains;
use crate::turns::Turns;
use crate::held::HeldBack;

pub struct Runtime {
    pub config: Config,
    pub database: PathBuf,
    pub personality: Option<String>,
    /// How she writes: the plating holds her to it after the fact, and every
    /// chef de partie is told it up front, so most of what she writes needs no
    /// rewriting.
    pub style: Option<String>,
    pub taster: Arc<Taster>,
    pub plating: Arc<Plating>,
    pub catalog: Arc<Catalog>,
    /// What couldn't be checked yet, waiting to be read with read_held_back.
    pub held: Arc<HeldBack>,
    pub outside: Arc<Outside>,
    /// What is going on this minute, which nothing else keeps: the board
    /// outlives a turn and the log is already past.
    pub at_work: Arc<AtWork>,
    /// Every conversation's queue of turns.
    pub turns: Arc<Turns>,
    /// What is said to each conversation while its chef de partie is busy.
    pub mailbox: Arc<Mailbox>,
    /// When each quiet chef de partie is compacted, before its cache goes cold.
    pub cooling: Arc<Cooling>,
    /// Every chef de partie's commis, who work on between their chef de
    /// partie's turns.
    pub brigade: Arc<Brigade>,
    /// One connection open for as long as she runs. Everything else opens
    /// the database and closes it again, and when the last connection
    /// closes, SQLite folds the write-ahead log back in under an exclusive
    /// lock — on a busy disk, long enough to lock every other open out.
    _database_held_open: Mutex<Store>,
    _proxy: Proxy,
}

impl Runtime {
    pub fn start() -> Result<Runtime> {
        sweep(&paths::all_sessions_dir());
        paths::sweep_sockets(false);

        let config = Config::load()?;
        claude::write_settings(&paths::claude_config_home())?;
        let taster = Arc::new(Taster::with_whatever_is_set_up(&config)?);
        let style = config::style()?;
        let plating = Arc::new(Plating::new(style.clone(), taster.clone(), config.models.plating.clone()));
        let personality = config::personality()?;
        let held = Arc::new(HeldBack::default());
        let catalog = Arc::new(Catalog::open(&config, plating.clone(), taster.clone(), held.clone()));
        let proxy = Proxy::start(&paths::socket("proxy"), &[proxy::CLAUDE_API])?;
        let outside = Arc::new(Outside {
            proxy_socket: proxy.socket().to_path_buf(),
            toolchains: Toolchains::discover(),
            time_limit: Duration::from_secs(config.minutes_per_run * 60),
            gitconfig: github::is_set_up().then(github::gitconfig),
            scopes: Outside::can_have_scopes(),
            models: config.models.clone(),
        });
        if !outside.scopes {
            logs::event("sandbox.without_limits", json!({ "reason": "the user's systemd gave no scope" }));
        }

        let database = paths::database();
        let held_open = Mutex::new(Store::open_at(&database)?);
        let brigade = Arc::new(Brigade::new(config.commis_per_chef_de_partie));

        Ok(Runtime {
            config,
            database,
            personality,
            style,
            taster,
            plating,
            catalog,
            held,
            outside,
            at_work: Arc::new(AtWork::default()),
            turns: Arc::new(Turns::default()),
            mailbox: Arc::new(Mailbox::default()),
            cooling: Arc::new(Cooling::default()),
            brigade,
            _database_held_open: held_open,
            _proxy: proxy,
        })
    }

    /// For a process that runs one conversation and exits, like
    /// `sous-chef chat`: its commis and the turns their verdicts wake would die with it.
    pub fn wait_until_settled(&self) {
        while self.brigade.are_working() || self.turns.busy_conversations() > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn sweep(all_sessions: &Path) {
    let Ok(entries) = fs::read_dir(all_sessions) else {
        return;
    };

    for entry in entries.flatten() {
        let process = entry.file_name().to_string_lossy().into_owned();
        if !Path::new("/proc").join(&process).exists() {
            let _ = fs::remove_dir_all(entry.path());
            logs::event("sessions.swept", json!({ "process": process }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dead_processes_are_swept() {
        let all_sessions = std::env::temp_dir().join(format!("sous-chef-sweep-{}", std::process::id()));
        let mine = all_sessions.join(std::process::id().to_string());
        let dead = all_sessions.join("4194305");
        fs::create_dir_all(mine.join("h1/profile")).unwrap();
        fs::create_dir_all(dead.join("h2/profile")).unwrap();

        sweep(&all_sessions);

        assert!(mine.join("h1/profile").exists());
        assert!(!dead.exists());
        fs::remove_dir_all(all_sessions).unwrap();
    }
}
