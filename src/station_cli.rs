//! `sous-chef station add`, `list` and `remove`: setting up a machine a commis
//! can work on, and forgetting one.
//!
//! Adding is one command for the chef to run: it lets sous-chef's key in,
//! finds out what the machine runs, installs what a commis needs and puts
//! this sous-chef there, then checks each of those before the station is
//! written down, and last sends a commis there that has to come back with
//! its work. Removing only forgets it; what was installed stays.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::claude::Started;
use crate::clock;
use crate::commis::Commis;
use crate::config::Config;
use crate::pass;
use crate::paths;
use crate::proxy::{self, Proxy};
use crate::sandbox::Outside;
use crate::station::{self, Os, Station};
use crate::toolchains::Toolchains;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn add(name: &str, host: &str) -> Result<()> {
    let key = ensure_key()?;
    let_the_key_in(host, &key)?;

    let (uname, os_release) = asked(host, "uname -sm; echo ---; cat /etc/os-release 2>/dev/null || true")?
        .split_once("---")
        .map(|(uname, release)| (uname.trim().to_string(), release.to_string()))
        .context("the station didn't say what it runs")?;
    let os = Os::detect(&uname, &os_release)?;
    if os == Os::Mac {
        bail!("{name} is a Mac; a commis can't be sandboxed there yet");
    }
    let arch = uname.split_whitespace().last().unwrap_or_default().to_string();
    println!("{name} runs {os:?} on {arch}.");

    println!("Installing what a commis needs; sudo may ask for your password on {name}.");
    run(station::ssh_with_a_terminal(host).arg(station::setting_up(os)), "setting it up")?;
    put_sous_chef_there(host, &arch)?;
    check(host)?;

    let has: Vec<String> = asked(host, &station::looking_around())?.lines().map(str::to_string).collect();
    let mut config = Config::load()?;
    config.stations.insert(name.to_string(), Station { host: host.to_string(), os, arch, has });
    config.save()?;
    println!("Added {name}.");
    list()?;
    check_with_a_commis(name)
}

/// A commis that has to answer, the way any commis there will: in a
/// repository made for it, it writes a file and commits it, the commit has
/// to come back here, and the pass, there too, has to accept it.
pub fn check_with_a_commis(name: &str) -> Result<()> {
    let config = Config::load()?;
    let station = config.stations.get(name).with_context(|| format!("there is no station called {name}"))?;
    let folder = std::env::temp_dir().join(format!("sous-chef-check-{:x}", clock::nanos()));
    std::fs::create_dir_all(&folder)?;
    let checked = with_a_commis_in(&folder, name, station, &config);
    let _ = std::fs::remove_dir_all(&folder);
    checked?;
    println!("A commis worked at {name}, its work came back, and the pass there accepted it.");
    Ok(())
}

fn with_a_commis_in(folder: &std::path::Path, name: &str, station: &Station, config: &Config) -> Result<()> {
    let git = |arguments: &[&str]| run(Command::new("git").arg("-C").arg(folder).args(["-c", "user.name=sous-chef", "-c", "user.email=sous-chef@localhost"]).args(arguments).stdout(Stdio::null()), "preparing the check's repository");
    git(&["init", "-q"])?;
    std::fs::write(folder.join("README.md"), "A repository a commis checks a station with.\n")?;
    git(&["add", "README.md"])?;
    git(&["commit", "-q", "-m", "Start"])?;

    let proxy = Proxy::start(&paths::socket("proxy"), &[proxy::CLAUDE_API])?;
    let outside = Outside {
        proxy_socket: proxy.socket().to_path_buf(),
        toolchains: Toolchains::default(),
        time_limit: Duration::from_secs(10 * 60),
        gitconfig: None,
        scopes: false,
        models: config.models.clone(),
    };
    println!("Sending a commis to {name}…");
    let brief = "Write the word ready into a file called check.txt in this folder and commit it with git. Reply with one short line.";
    let started = Started::default();
    let mut commis = Commis::start(folder, Vec::new(), &[], false, Some((name, station)))?;
    let checked = commis.work(brief, &outside, &started).and_then(|report| {
        println!("It said: {}", report.trim());
        if std::fs::read_to_string(folder.join("check.txt")).unwrap_or_default().trim() != "ready" {
            bail!("the commis answered, but check.txt didn't come back with its work");
        }
        println!("Its work came back. The pass checks it at {name}…");
        pass::check(&commis, brief, &report, &outside, &started)
    });
    commis.discard();
    let verdict = checked?;
    if !verdict.accepted {
        bail!("the pass didn't accept the commis's work: {}", verdict.notes);
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let config = Config::load()?;
    if config.stations.is_empty() {
        println!("No stations; every commis works here. Add one with: sous-chef station add <name> <user@host>");
    }
    for (name, station) in &config.stations {
        let has = if station.has.is_empty() { String::new() } else { format!(", has {}", station.has.join(", ")) };
        println!("{name}  {}  ({:?}, {}{has})", station.host, station.os, station.arch);
    }
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    let mut config = Config::load()?;
    if config.stations.remove(name).is_none() {
        bail!("there is no station called {name}");
    }
    config.save()?;
    println!("Removed {name}. What was installed there stays.");
    Ok(())
}

/// Made once, with no passphrase: nobody is there to type one when a commis
/// starts.
fn ensure_key() -> Result<String> {
    let key = station::key();
    if !key.exists() {
        std::fs::create_dir_all(key.parent().context("the key has no folder")?)?;
        run(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "sous-chef"]).arg("-f").arg(&key), "making sous-chef's key")?;
    }
    Ok(std::fs::read_to_string(key.with_extension("pub"))?.trim().to_string())
}

/// When sous-chef's key isn't let in yet, the chef's own ssh puts it in
/// authorized_keys, so it may ask for their password this once.
fn let_the_key_in(host: &str, key: &str) -> Result<()> {
    if station::ssh(host).arg("true").stderr(Stdio::null()).status()?.success() {
        return Ok(());
    }
    println!("Letting sous-chef's key into {host}; ssh may ask for your password.");
    let mut chefs_ssh = Command::new("ssh")
        .arg(host)
        .arg("umask 077; mkdir -p ~/.ssh; cat >> ~/.ssh/authorized_keys")
        .stdin(Stdio::piped())
        .spawn()
        .context("could not run ssh")?;
    writeln!(chefs_ssh.stdin.take().context("ssh has no stdin")?, "{key}")?;
    if !chefs_ssh.wait()?.success() {
        bail!("couldn't let sous-chef's key into {host}");
    }
    if !station::ssh(host).arg("true").status()?.success() {
        bail!(
            "{host} still refuses sous-chef's key. If it is reached through Tailscale SSH, that doesn't read authorized_keys: give the station's OpenSSH address instead"
        );
    }
    Ok(())
}

/// The same version as here. A build for this machine's architecture is
/// copied as it is, so a build that was never released works too; any other
/// is downloaded from the release by the station itself.
fn put_sous_chef_there(host: &str, arch: &str) -> Result<()> {
    let install = "mkdir -p ~/.local/bin && cat > ~/.local/bin/sous-chef.new && chmod +x ~/.local/bin/sous-chef.new && mv ~/.local/bin/sous-chef.new ~/.local/bin/sous-chef";
    if station::release_binary(arch) == station::release_binary(std::env::consts::ARCH) {
        let binary = std::fs::File::open(std::env::current_exe()?)?;
        run(station::ssh(host).arg(install).stdin(binary), "copying sous-chef over")
    } else {
        let Some(binary) = station::release_binary(arch) else {
            bail!("there is no sous-chef built for {arch}");
        };
        let url = format!("https://github.com/monorkin/sous-chef/releases/download/v{VERSION}/{binary}");
        run(station::ssh(host).arg(format!("curl -fsSL {url} | sh -c '{install}'")), "downloading sous-chef there")
    }
}

/// Each thing a commis needs, asked for the way it will be: sous-chef's
/// version, a sandbox, socat, and Claude Code.
fn check(host: &str) -> Result<()> {
    let version = asked(host, "~/.local/bin/sous-chef --version")?;
    if !version.trim().ends_with(VERSION) {
        bail!("the station runs {}, not {VERSION}", version.trim());
    }
    asked(host, "bwrap --unshare-all --die-with-parent --ro-bind / / true").context("a sandbox can't be made there; user namespaces may be turned off")?;
    asked(host, "socat -V >/dev/null").context("socat doesn't run there")?;
    asked(host, "export PATH=\"$HOME/.local/bin:$HOME/.local/share/mise/shims:$PATH\"; claude --version").context("Claude Code doesn't run there")?;
    Ok(())
}

fn asked(host: &str, script: &str) -> Result<String> {
    let output = station::ssh(host).arg(script).stdin(Stdio::null()).output().context("could not run ssh")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim())
    }
}

fn run(command: &mut Command, doing: &str) -> Result<()> {
    if command.status().with_context(|| format!("could not start {doing}"))?.success() {
        Ok(())
    } else {
        bail!("{doing} failed")
    }
}
