//! What runs on a station: `sous-chef station run`, one round of a commis or
//! a pass, the way it would have run at home.
//!
//! The order says which project and branch, which session folder holds the
//! profile, and where the sockets forwarded from home are: the proxy, the
//! broker, the registries. Everything else is the station's own — its
//! toolchains, its build store, the services on its loopback — so a station
//! carries the load of the work and home carries only the talking. The
//! sandbox is the same one a commis gets at home.
//!
//! Claude's answer goes out on stdout, as it would to a caller at home. When
//! the ssh connection that carries it goes, nobody is left to read it, so the
//! session is stopped; it is stopped too when its time is up, in case home
//! never noticed.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::claude;
use crate::config::Models;
use crate::paths;
use crate::proxy::Bridge;
use crate::sandbox::{self, Outside, Sandbox, Service};
use crate::toolchains::Toolchains;

/// One round of work, as home hands it to a station.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Order {
    /// The session's folder under the station's sessions: its profile, which
    /// home sent ahead, and its scratch.
    pub session: String,
    /// The project's folder under the station's projects.
    pub project: String,
    /// The commis's branch. Home pushes it to `refs/sous-chef/in/<branch>`,
    /// and the branch is set to that before a commis's round; the pass reads
    /// the branch as the commis left it.
    pub branch: String,
    pub writable: bool,
    pub ports: Vec<u16>,
    pub proxy: PathBuf,
    pub broker: Option<PathBuf>,
    pub registries: Option<PathBuf>,
    /// Her git identity, when she has one.
    pub gitconfig: Option<String>,
    pub minutes: u64,
    /// For claude, after the sandbox's own.
    pub args: Vec<String>,
}

impl Order {
    /// For the command line: hex, so no shell on the way can change it.
    pub fn encoded(&self) -> Result<String> {
        Ok(serde_json::to_vec(self)?.iter().map(|it| format!("{it:02x}")).collect())
    }

    pub fn decoded(hex: &str) -> Result<Order> {
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(hex.get(at..at + 2).unwrap_or("zz"), 16))
            .collect::<Result<Vec<u8>, _>>()
            .context("the order isn't hex")?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub fn sessions_dir() -> PathBuf {
    paths::data_dir().join("station/sessions")
}

pub fn projects_dir() -> PathBuf {
    paths::data_dir().join("station/projects")
}

/// Runs the round and leaves with claude's exit code.
pub fn run(order: &str, for_claude: &[String]) -> Result<()> {
    let order = Order::decoded(order)?;
    with_mises_tools_on_the_path();
    let project = projects_dir().join(&order.project);
    let session = sessions_dir().join(&order.session);
    // A commis starts from what home sent; the pass reads what it left
    if order.writable {
        git(&project, &["checkout", "-q", "--force", "-B", &order.branch, &format!("refs/sous-chef/in/{}", order.branch)])?;
        git(&project, &["clean", "-q", "-f", "-d"])?;
    } else {
        git(&project, &["checkout", "-q", "--force", &order.branch])?;
    }

    sandbox::make_scratch(&session)?;
    let gitconfig = match &order.gitconfig {
        Some(contents) => {
            std::fs::write(session.join("gitconfig"), contents)?;
            Some(session.join("gitconfig"))
        }
        None => None,
    };
    let (services, _bridges) = bridged(&order.session, &order.ports)?;
    let outside = Outside {
        proxy_socket: order.proxy.clone(),
        toolchains: Toolchains::discover(),
        time_limit: Duration::from_secs(order.minutes * 60),
        gitconfig,
        scopes: Outside::can_have_scopes(),
        models: Models::default(),
    };
    let sandbox = Sandbox {
        project: project.clone(),
        profile: session.join("profile"),
        outside: &outside,
        broker_socket: order.broker.clone(),
        writable: order.writable,
        build_dir: sandbox::build_dir_of(&project)?,
        services,
        registries: order.registries.clone(),
        scratch: session.clone(),
    };

    let mut command = sandbox.claude(&claude::binary()?);
    command.args(["--dangerously-skip-permissions", "--strict-mcp-config", "--tools", sandbox.tools()]);
    command.args(&order.args).args(for_claude);
    let status = watched(&mut command, outside.time_limit)?;
    if order.writable {
        keep_what_was_left(&project, outside.gitconfig.as_deref())?;
    }
    // ssh leaves the sockets it forwarded behind when it goes
    for socket in [Some(&order.proxy), order.broker.as_ref(), order.registries.as_ref()].into_iter().flatten() {
        let _ = std::fs::remove_file(socket);
    }
    std::process::exit(status.code().unwrap_or(1));
}

/// A non-interactive ssh session starts with next to no PATH, and claude and
/// the project's tools are mise's.
fn with_mises_tools_on_the_path() {
    let home = paths::home();
    let path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: set before any thread of this process is started
    unsafe { std::env::set_var("PATH", format!("{}:{}:{path}", home.join(".local/bin").display(), home.join(".local/share/mise/shims").display())) };
}

fn bridged(session: &str, ports: &[u16]) -> Result<(Vec<Service>, Vec<Bridge>)> {
    let mut services = Vec::new();
    let mut bridges = Vec::new();
    for (n, port) in ports.iter().enumerate() {
        let socket = paths::socket(&format!("service-{session}-{n}"));
        bridges.push(Bridge::start(&socket, *port).with_context(|| format!("could not bridge port {port} on the station"))?);
        services.push(Service { port: *port, socket });
    }
    Ok((services, bridges))
}

/// Stopped when nobody is reading any more — stdout goes back over the ssh
/// connection, and a write end whose reader is gone polls as an error — or
/// when its time is up.
fn watched(command: &mut Command, limit: Duration) -> Result<std::process::ExitStatus> {
    let mut child = command.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn().context("could not run claude")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if nobody_reads_stdout() || started.elapsed() > limit {
            let _ = child.kill();
            return Ok(child.wait()?);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn nobody_reads_stdout() -> bool {
    let mut stdout = libc::pollfd { fd: 1, events: 0, revents: 0 };
    // SAFETY: one pollfd that lives for the call
    let ready = unsafe { libc::poll(&mut stdout, 1, 0) };
    ready > 0 && stdout.revents & (libc::POLLERR | libc::POLLHUP) != 0
}

/// What the commis didn't commit comes home as one commit of its own, so
/// nothing it did is lost on the way.
fn keep_what_was_left(project: &Path, gitconfig: Option<&Path>) -> Result<()> {
    let changed = Command::new("git").arg("-C").arg(project).args(["status", "--porcelain"]).output()?;
    if changed.stdout.is_empty() {
        return Ok(());
    }
    let identity = |command: &mut Command| {
        match gitconfig {
            Some(file) => command.env("GIT_CONFIG_GLOBAL", file),
            None => command.args(["-c", "user.name=commis", "-c", "user.email=commis@localhost"]),
        };
    };
    git(project, &["add", "-A"])?;
    let mut commit = Command::new("git");
    identity(&mut commit);
    let status = commit.arg("-C").arg(project).args(["commit", "-q", "-m", "What the commis left uncommitted"]).status()?;
    if !status.success() {
        bail!("couldn't commit what the commis left");
    }
    Ok(())
}

fn git(project: &Path, arguments: &[&str]) -> Result<()> {
    let output = Command::new("git").arg("-C").arg(project).args(arguments).output().context("could not run git")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("git {} failed: {}", arguments.join(" "), String::from_utf8_lossy(&output.stderr).trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_order_survives_the_command_line_unchanged() {
        let order = Order {
            session: "h1".to_string(),
            project: "shop-0a1b".to_string(),
            branch: "sous-chef/h1".to_string(),
            writable: true,
            ports: vec![33380],
            proxy: PathBuf::from("/tmp/sous-chef-h1-proxy.sock"),
            broker: Some(PathBuf::from("/tmp/sous-chef-h1-broker.sock")),
            registries: None,
            gitconfig: Some("[user]\n\tname = Botten\n".to_string()),
            minutes: 180,
            args: vec!["--model".to_string(), "opus".to_string(), "--append-system-prompt".to_string(), "it's \"quoted\"; $(and) `this`".to_string()],
        };
        let encoded = order.encoded().unwrap();

        assert!(encoded.chars().all(|it| it.is_ascii_hexdigit()), "nothing a shell would read anything into");
        assert_eq!(Order::decoded(&encoded).unwrap(), order);
        assert!(Order::decoded("zz").is_err());
    }
}
