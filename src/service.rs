//! sous-chef as a systemd user service, for the people who want it to come
//! back after a reboot without being asked.
//!
//! It is a user service, not a system one: it runs as the person the agent
//! works for, with their Claude login and their keyring. The unit carries the
//! PATH setup was run with, because a user service starts with next to none
//! and sous-chef needs claude, bwrap, socat and whatever the MCP servers are.
//! `KillMode=control-group` is systemd's version of what `sous-chef stop`
//! does itself: nothing it started outlives it. It takes a while to go — the
//! threads it stopped mid-turn are compacted first — so systemd waits.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fsutil;

const NAME: &str = "sous-chef.service";

pub fn installed() -> bool {
    unit_path().is_some_and(|it| it.exists())
}

pub fn install(agent: &str, start_at_boot: bool) -> Result<()> {
    install_with_path(agent, &std::env::var("PATH").unwrap_or_default())?;
    if start_at_boot {
        linger()?;
    }
    Ok(())
}

/// Installed to run with the PATH given rather than the one this process
/// has: a moved install's, which whoever moved it — over ssh, say — may not
/// have.
pub fn install_with_path(agent: &str, search_path: &str) -> Result<()> {
    let executable = std::env::current_exe().context("could not determine sous-chef's own path")?;
    write_unit(agent, &executable, search_path)?;
    systemctl(&["enable", NAME])
}

/// The unit names the binary it runs by its path, and a mise install's path
/// carries its version: after an upgrade the unit would start the old one
/// until it is pointed at the new. The PATH stays the one setup was run with:
/// an upgrade run from a shell with less on it would leave the service
/// without claude.
pub fn point_at(agent: &str, executable: &Path) -> Result<()> {
    let kept = unit_path().and_then(|it| std::fs::read_to_string(it).ok()).and_then(|it| search_path_of(&it));
    let search_path = kept.unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    write_unit(agent, executable, &search_path)
}

fn write_unit(agent: &str, executable: &Path, search_path: &str) -> Result<()> {
    let path = unit_path().context("could not determine the home directory")?;
    fsutil::write_private(&path, &unit(agent, executable, search_path))?;
    systemctl(&["daemon-reload"])
}

pub fn start() -> Result<()> {
    systemctl(&["start", NAME])
}

pub fn stop() -> Result<()> {
    systemctl(&["stop", NAME])
}

pub fn active() -> bool {
    Command::new("systemctl")
        .args(["--user", "--quiet", "is-active", NAME])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|it| it.success())
}

fn unit(agent: &str, executable: &Path, search_path: &str) -> String {
    let description: String = agent.chars().filter(|it| !it.is_control()).collect();
    format!(
        "[Unit]\n\
         Description={description}\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={} run\n\
         Environment=PATH={search_path}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         KillMode=control-group\n\
         TimeoutStopSec=5min\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        executable.display()
    )
}

/// Takes away the unit of an install made under another name, so the old
/// binary isn't started next to this one after a reboot. When it was
/// enabled, the PATH it ran with is the answer: the new unit should be
/// enabled too, and needs that PATH.
pub fn remove_named(name: &str) -> Result<Option<String>> {
    let unit = format!("{name}.service");
    match unit_path_named(&unit) {
        Some(path) if path.exists() => {
            let enabled = Command::new("systemctl")
                .args(["--user", "--quiet", "is-enabled", &unit])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|it| it.success());
            let search_path = search_path_of(&std::fs::read_to_string(&path)?).unwrap_or_default();
            let _ = systemctl(&["disable", &unit]);
            std::fs::remove_file(&path)?;
            systemctl(&["daemon-reload"])?;
            Ok(enabled.then_some(search_path))
        }
        _ => Ok(None),
    }
}

fn search_path_of(unit: &str) -> Option<String> {
    unit.lines().find_map(|it| it.strip_prefix("Environment=PATH=")).map(str::to_string)
}

fn unit_path() -> Option<PathBuf> {
    unit_path_named(NAME)
}

fn unit_path_named(unit: &str) -> Option<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|it| !it.is_empty()) {
        Some(directory) => PathBuf::from(directory),
        None => dirs::home_dir()?.join(".config"),
    };
    Some(config.join("systemd/user").join(unit))
}

/// Without lingering, a user's services only run while they are logged in.
fn linger() -> Result<()> {
    let user = std::env::var("USER").context("USER is not set")?;
    let status = Command::new("loginctl").args(["enable-linger", &user]).status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("could not enable lingering; run `loginctl enable-linger {user}` yourself")
    }
}

fn systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .context("could not run systemctl")?;
    if status.success() {
        Ok(())
    } else {
        bail!("systemctl --user {} failed", arguments.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_sous_chef_with_the_path_it_was_set_up_with() {
        let unit = unit("Botten\nExecStartPre=/bin/evil", Path::new("/opt/sous-chef/bin/sous-chef"), "/opt/tools/bin:/usr/bin");

        assert!(unit.contains("Description=BottenExecStartPre=/bin/evil\n"));
        assert!(unit.contains("ExecStart=/opt/sous-chef/bin/sous-chef run\n"));
        assert!(unit.contains("Environment=PATH=/opt/tools/bin:/usr/bin\n"));
        assert!(unit.contains("KillMode=control-group\n"));
        assert!(unit.contains("TimeoutStopSec=5min\n"), "stopping waits for the threads it stopped to be compacted");
        assert!(unit.contains("WantedBy=default.target\n"));
        assert_eq!(search_path_of(&unit).as_deref(), Some("/opt/tools/bin:/usr/bin"), "a unit rewritten later keeps the PATH it was set up with");
    }
}
