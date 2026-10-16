//! Home's side of a commis at a station: the work goes there by git, the
//! session's profile by tar, and every round runs over one ssh connection
//! that carries the sockets home keeps — the proxy to Claude, the broker
//! with the commis's tools, the registries proxy — so what a commis may
//! reach is decided here, as it is for one at home.
//!
//! The project goes as a branch of the commis's own, pushed to the station's
//! copy of the repository, and comes back the same way, as commits laid on
//! top of the worktree here. A worktree with changes nobody committed is
//! refused: those would stay behind.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::sandbox::Outside;
use crate::station::{self, Station};
use crate::station_side::Order;

/// Where on the station its sessions and projects live, from its home.
const SESSIONS: &str = ".local/share/sous-chef/station/sessions";
const PROJECTS: &str = ".local/share/sous-chef/station/projects";

pub struct AtStation {
    pub name: String,
    host: String,
    project: String,
    branch: String,
}

/// The sockets of home's a round needs on the station, by what they are.
pub struct Forwarded {
    pub proxy: PathBuf,
    pub broker: Option<PathBuf>,
    pub registries: Option<PathBuf>,
}

impl AtStation {
    pub fn new(name: &str, station: &Station, project: &Path, commis: &str) -> Result<AtStation> {
        let changed = Command::new("git").arg("-C").arg(project).args(["status", "--porcelain"]).output().context("could not run git")?;
        if !changed.status.success() {
            bail!("{} isn't a git repository, and a commis at a station gets its work by git", project.display());
        }
        if !changed.stdout.is_empty() {
            bail!("{} has changes nobody committed; commit them first, or they'd stay behind when the commis goes to {name}", project.display());
        }
        Ok(AtStation { name: name.to_string(), host: station.host.clone(), project: project_key(project), branch: format!("sous-chef/{commis}") })
    }

    /// The project's state here, as the branch the commis works on there.
    pub fn send_work(&self, project: &Path) -> Result<()> {
        let there = format!("{PROJECTS}/{}", self.project);
        asked(&self.host, &format!("[ -d {there}/.git ] || git init -q {there}"))?;
        let mut push = self.git(project);
        push.args(["push", "-q", "--force", &self.url()]).args(self.refspecs());
        done(&mut push, "pushing the work to the station")
    }

    /// The commis's branch, and the remotes as last fetched here: the
    /// station has no remote of its own to fetch from, and merging or
    /// porting against origin/main needs it there all the same.
    fn refspecs(&self) -> [String; 2] {
        [format!("HEAD:refs/sous-chef/in/{}", self.branch), "+refs/remotes/*:refs/remotes/*".to_string()]
    }

    /// What the commis committed there, laid on top of the worktree here.
    pub fn bring_back_work(&self, project: &Path) -> Result<()> {
        done(self.git(project).args(["fetch", "-q", &self.url(), &format!("refs/heads/{}", self.branch)]), "fetching the work back")?;
        done(self.git(project).args(["merge", "-q", "--ff-only", "FETCH_HEAD"]), "laying the work on the worktree")
    }

    /// A session folder here — its profile — copied to the station.
    pub fn send_session(&self, folder: &Path, session: &str) -> Result<()> {
        let mut packing = Command::new("tar").arg("-C").arg(folder).args(["-cf", "-", "profile"]).stdout(Stdio::piped()).spawn()?;
        let mut unpacking = station::ssh(&self.host)
            .arg(format!("mkdir -p {SESSIONS}/{session} && rm -rf {SESSIONS}/{session}/profile && tar -C {SESSIONS}/{session} -xf -"))
            .stdin(packing.stdout.take().context("tar has no stdout")?)
            .spawn()?;
        if !(packing.wait()?.success() && unpacking.wait()?.success()) {
            bail!("couldn't send the session to {}", self.name);
        }
        Ok(())
    }

    /// And back, after a round: the session went on there.
    pub fn bring_back_session(&self, folder: &Path, session: &str) -> Result<()> {
        let mut packing = station::ssh(&self.host).arg(format!("tar -C {SESSIONS}/{session} -cf - profile")).stdout(Stdio::piped()).spawn()?;
        let _ = std::fs::remove_dir_all(folder.join("profile"));
        let mut unpacking = Command::new("tar").arg("-C").arg(folder).args(["-xf", "-"]).stdin(packing.stdout.take().context("ssh has no stdout")?).spawn()?;
        if !(packing.wait()?.success() && unpacking.wait()?.success()) {
            bail!("couldn't bring the session back from {}", self.name);
        }
        Ok(())
    }

    /// Her git identity goes along only for a commis, which commits; the pass
    /// only reads.
    pub fn order(&self, session: &str, writable: bool, ports: &[u16], forwarded: &Forwarded, outside: &Outside, args: Vec<String>) -> Result<Order> {
        let gitconfig = if writable { outside.gitconfig.as_deref() } else { None };
        Ok(Order {
            session: session.to_string(),
            project: self.project.clone(),
            branch: self.branch.clone(),
            writable,
            ports: ports.to_vec(),
            proxy: there(session, "proxy"),
            broker: forwarded.broker.as_ref().map(|_| there(session, "broker")),
            registries: forwarded.registries.as_ref().map(|_| there(session, "registries")),
            gitconfig: gitconfig.map(std::fs::read_to_string).transpose()?,
            minutes: outside.time_limit.as_secs() / 60,
            args,
        })
    }

    /// The round, as a command that answers the way claude does at home:
    /// ssh, carrying home's sockets to where the order says they are.
    pub fn command(&self, order: &Order, forwarded: &Forwarded) -> Result<Command> {
        let mut command = station::ssh(&self.host);
        command.args(["-o", "StreamLocalBindUnlink=yes", "-o", "StreamLocalBindMask=0177", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30"]);
        let mut forward = |there: &Path, here: &Path| {
            command.arg("-R").arg(format!("{}:{}", there.display(), here.display()));
        };
        forward(&order.proxy, &forwarded.proxy);
        if let (Some(there), Some(here)) = (&order.broker, &forwarded.broker) {
            forward(there, here);
        }
        if let (Some(there), Some(here)) = (&order.registries, &forwarded.registries) {
            forward(there, here);
        }
        // What the caller adds for claude comes after the --
        command.args(["~/.local/bin/sous-chef", "station", "run", &order.encoded()?, "--"]);
        Ok(command)
    }

    /// The session's folder there, once its commis or pass is done with.
    pub fn forget_session(&self, session: &str) {
        let _ = station::ssh(&self.host).arg(format!("rm -rf {SESSIONS}/{session}")).stdin(Stdio::null()).status();
    }

    fn url(&self) -> String {
        match self.host.strip_prefix("ssh://") {
            Some(_) => format!("{}/~/{PROJECTS}/{}", self.host, self.project),
            None => format!("{}:{PROJECTS}/{}", self.host, self.project),
        }
    }

    /// git, reaching the station the way everything else does.
    fn git(&self, project: &Path) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(project).env("GIT_SSH_COMMAND", ssh_for_git());
        command
    }
}

/// Where a forwarded socket sits on the station. /tmp, because a unix socket
/// path has to be short; only its owner can use it.
fn there(session: &str, what: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/sous-chef-{session}-{what}.sock"))
}

/// The station's copy of a project: the folder's name, so it can be told
/// apart, and a hash of where it is here, so two of the same name aren't one.
pub fn project_key(project: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in project.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let name: String = project.file_name().unwrap_or_default().to_string_lossy().chars().filter(|it| it.is_ascii_alphanumeric() || *it == '-' || *it == '_').collect();
    format!("{name}-{:08x}", hash as u32)
}

fn ssh_for_git() -> String {
    let command = station::ssh("");
    let mut words = vec!["ssh".to_string()];
    words.extend(command.get_args().map(|it| it.to_string_lossy().into_owned()).filter(|it| !it.is_empty()).map(|it| format!("'{it}'")));
    words.join(" ")
}

fn asked(host: &str, script: &str) -> Result<()> {
    let output = station::ssh(host).arg(script).stdin(Stdio::null()).output()?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim())
    }
}

fn done(command: &mut Command, doing: &str) -> Result<()> {
    let output = command.stdin(Stdio::null()).output().with_context(|| format!("could not start {doing}"))?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("{doing} failed: {}", String::from_utf8_lossy(&output.stderr).trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_copy_on_a_station_is_named_for_it_and_told_apart_from_its_namesakes() {
        let shop = project_key(Path::new("/home/someone/Work/shop/.claude/worktrees/fix"));
        assert!(shop.starts_with("fix-"), "{shop}");
        assert_ne!(shop, project_key(Path::new("/home/someone/Work/blog/.claude/worktrees/fix")));
        assert_eq!(shop, project_key(Path::new("/home/someone/Work/shop/.claude/worktrees/fix")));
    }

    #[test]
    fn a_round_at_a_station_carries_homes_sockets_and_nothing_more() {
        let at = AtStation { name: "server".to_string(), host: "someone@station.example.com".to_string(), project: "shop-1".to_string(), branch: "sous-chef/h1".to_string() };
        let forwarded = Forwarded { proxy: PathBuf::from("/run/sous-chef/proxy.sock"), broker: Some(PathBuf::from("/run/sous-chef/commis-h1.sock")), registries: None };
        let outside = Outside {
            proxy_socket: forwarded.proxy.clone(),
            toolchains: crate::toolchains::Toolchains::default(),
            time_limit: std::time::Duration::from_secs(3 * 60 * 60),
            gitconfig: None,
            scopes: false,
            models: crate::config::Models::default(),
        };
        let order = at.order("h1", true, &[33380], &forwarded, &outside, vec!["--model".to_string(), "opus".to_string()]).unwrap();
        assert_eq!(order.minutes, 180);
        let command = at.command(&order, &forwarded).unwrap();
        let arguments: Vec<String> = command.get_args().map(|it| it.to_string_lossy().into_owned()).collect();

        assert!(arguments.windows(2).any(|it| it[0] == "-R" && it[1] == "/tmp/sous-chef-h1-proxy.sock:/run/sous-chef/proxy.sock"));
        assert!(arguments.windows(2).any(|it| it[0] == "-R" && it[1] == "/tmp/sous-chef-h1-broker.sock:/run/sous-chef/commis-h1.sock"));
        assert_eq!(arguments.iter().filter(|it| *it == "-R").count(), 2, "no registries asked for, none carried");
        assert!(arguments.contains(&"StreamLocalBindMask=0177".to_string()), "only its owner can use a socket there");
        assert_eq!(Order::decoded(&arguments[arguments.len() - 2]).unwrap(), order);
        assert_eq!(arguments.last().unwrap(), "--", "claude's own arguments follow, as the caller adds them");
    }

    #[test]
    fn the_stations_copy_is_reached_by_git_the_way_ssh_reaches_it() {
        let at = |host: &str| AtStation { name: "s".to_string(), host: host.to_string(), project: "shop-1".to_string(), branch: "b".to_string() };
        assert_eq!(at("someone@station").url(), "someone@station:.local/share/sous-chef/station/projects/shop-1");
        assert_eq!(at("ssh://someone@station:2222").url(), "ssh://someone@station:2222/~/.local/share/sous-chef/station/projects/shop-1");
        assert!(ssh_for_git().contains("'BatchMode=yes'"));
    }

    #[test]
    fn a_commis_at_a_station_sees_the_remotes_as_they_were_last_fetched_here() {
        let scratch = std::env::temp_dir().join(format!("sous-chef-refs-{}", std::process::id()));
        let (here, there) = (scratch.join("here"), scratch.join("there"));
        for repository in [&here, &there] {
            assert!(Command::new("git").args(["init", "-q"]).arg(repository).status().unwrap().success());
        }
        let git = |arguments: &[&str]| assert!(Command::new("git").arg("-C").arg(&here).args(arguments).status().unwrap().success());
        git(&["-c", "user.name=Marta", "-c", "user.email=marta@example.com", "commit", "-q", "--allow-empty", "-m", "Start"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

        let at = AtStation { name: "s".to_string(), host: "unused".to_string(), project: "shop-1".to_string(), branch: "sous-chef/h1".to_string() };
        git(&[&["push", "-q", "--force", there.to_str().unwrap()][..], &at.refspecs().each_ref().map(String::as_str)[..]].concat());

        let refs = Command::new("git").arg("-C").arg(&there).arg("show-ref").output().unwrap();
        let refs = String::from_utf8_lossy(&refs.stdout);
        assert!(refs.contains("refs/sous-chef/in/sous-chef/h1"), "{refs}");
        assert!(refs.contains("refs/remotes/origin/main"), "{refs}");
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
