//! Stations: the machines besides this one that a commis can work on.
//!
//! A station is reached over OpenSSH with a key of sous-chef's own, never the
//! chef's: commis run unattended, and nobody is there to answer a passphrase
//! or a Tailscale check halfway through one. Its host key is kept apart from
//! the chef's too, learnt the first time and held to after.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use crate::paths;

/// What a commis needs on a station, by the command each package gives.
pub const NEEDS: [&str; 4] = ["bwrap", "socat", "git", "curl"];

/// Looked for when a station is added, so whoever picks a station can see
/// what it has: what is there to build and test with, by the command that
/// gives it away.
pub const LOOKED_FOR: [(&str, &str); 8] = [
    ("rust", "cargo"),
    ("docker", "docker"),
    ("node", "node"),
    ("ruby", "ruby"),
    ("go", "go"),
    ("java", "java"),
    ("python", "python3"),
    ("xcode", "xcodebuild"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Station {
    /// Whatever `ssh` takes: `user@host`, or a name from an ssh config.
    pub host: String,
    pub os: Os,
    /// As `uname -m` says it: x86_64, aarch64, arm64.
    pub arch: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub has: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Arch,
    Debian,
    Fedora,
    #[serde(rename = "macos")]
    Mac,
}

impl Os {
    /// From what `uname -s` and /etc/os-release say. A distribution that is
    /// like one of these — Ubuntu like Debian, Manjaro like Arch — counts as
    /// it, since that is whose packages it takes.
    pub fn detect(uname: &str, os_release: &str) -> Result<Os> {
        if uname.trim().starts_with("Darwin") {
            return Ok(Os::Mac);
        }
        let field = |name: &str| {
            os_release
                .lines()
                .find_map(|it| it.strip_prefix(&format!("{name}=")))
                .map(|it| it.trim_matches('"').to_lowercase())
                .unwrap_or_default()
        };
        let kinds = format!("{} {}", field("ID"), field("ID_LIKE"));
        let os = kinds.split_whitespace().find_map(|kind| match kind {
            "arch" => Some(Os::Arch),
            "debian" | "ubuntu" => Some(Os::Debian),
            "fedora" | "rhel" | "centos" => Some(Os::Fedora),
            _ => None,
        });
        match os {
            Some(os) => Ok(os),
            None => bail!("this station runs {}, which sous-chef doesn't know how to set up", if kinds.trim().is_empty() { uname.trim() } else { kinds.trim() }),
        }
    }

    /// What installs the packages named in `$missing`. Debian's package
    /// lists are refreshed first: a fresh machine has none.
    fn installing(&self) -> &'static str {
        match self {
            Os::Arch => "sudo pacman -S --needed --noconfirm $missing",
            Os::Debian => "sudo apt-get update -qq && sudo apt-get install -y $missing",
            Os::Fedora => "sudo dnf install -y $missing",
            Os::Mac => "brew install $missing",
        }
    }

    /// What a package is called here, given the command it gives.
    fn package_of(&self, command: &str) -> &'static str {
        match (self, command) {
            (_, "bwrap") => "bubblewrap",
            (_, "socat") => "socat",
            (_, "git") => "git",
            _ => "curl",
        }
    }
}

/// The script that makes a station ready: the packages a commis needs, mise,
/// and Claude Code through mise. Run with a terminal, so sudo can ask for a
/// password the one time it is needed; whatever is already there is left.
pub fn setting_up(os: Os) -> String {
    let packages: Vec<String> = NEEDS.iter().map(|it| format!("{it}:{}", os.package_of(it))).collect();
    format!(
        r#"set -e
missing=""
for need in {packages}; do
  command -v "${{need%%:*}}" >/dev/null 2>&1 || missing="$missing ${{need#*:}}"
done
if [ -n "$missing" ]; then
  echo "Installing$missing"
  {installing}
fi
export PATH="$HOME/.local/bin:$PATH"
command -v mise >/dev/null 2>&1 || curl -fsSL https://mise.run | sh
mise use -g claude@latest
"#,
        packages = packages.join(" "),
        installing = os.installing(),
    )
}

/// What the station has from LOOKED_FOR, one name per line it prints, with
/// mise's tools on the PATH the way a commis would have them.
pub fn looking_around() -> String {
    let probes: Vec<String> = LOOKED_FOR.iter().map(|(name, command)| format!("command -v {command} >/dev/null 2>&1 && echo {name}")).collect();
    format!("export PATH=\"$HOME/.local/bin:$HOME/.local/share/mise/shims:$PATH\"\n{}\ntrue\n", probes.join("\n"))
}

/// sous-chef's own key for its stations.
pub fn key() -> PathBuf {
    paths::config_dir().join("ssh/id_ed25519")
}

fn known_hosts() -> PathBuf {
    paths::config_dir().join("ssh/known_hosts")
}

/// ssh to a station as sous-chef: its key and nothing else, never a prompt.
pub fn ssh(host: &str) -> Command {
    let mut command = Command::new("ssh");
    command.args(ssh_options()).arg(host);
    command
}

/// The same, with a terminal, for the one time a person is there to answer
/// sudo.
pub fn ssh_with_a_terminal(host: &str) -> Command {
    let mut command = Command::new("ssh");
    command.arg("-t").args(ssh_options()).arg(host);
    command
}

fn ssh_options() -> Vec<String> {
    vec![
        "-i".to_string(),
        key().to_string_lossy().into_owned(),
        "-o".to_string(),
        "IdentitiesOnly=yes".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        format!("UserKnownHostsFile={}", known_hosts().display()),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
    ]
}

/// The release binary a station of this architecture runs, by the name it
/// is published under.
pub fn release_binary(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "amd64" => Some("sous-chef-linux-amd64"),
        "aarch64" | "arm64" => Some("sous-chef-linux-arm64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_station_is_known_by_its_distribution_or_the_one_it_is_like() {
        assert_eq!(Os::detect("Linux x86_64", "NAME=\"Arch Linux\"\nID=arch\n").unwrap(), Os::Arch);
        assert_eq!(Os::detect("Linux x86_64", "ID=ubuntu\nID_LIKE=debian\n").unwrap(), Os::Debian);
        assert_eq!(Os::detect("Linux aarch64", "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n").unwrap(), Os::Fedora);
        assert_eq!(Os::detect("Linux x86_64", "ID=manjaro\nID_LIKE=arch\n").unwrap(), Os::Arch);
        assert_eq!(Os::detect("Darwin arm64", "").unwrap(), Os::Mac);
        assert!(Os::detect("Linux x86_64", "ID=alpine\n").unwrap_err().to_string().contains("alpine"));
    }

    #[test]
    fn setting_up_installs_only_what_is_missing_with_the_stations_own_packages() {
        let debian = setting_up(Os::Debian);
        assert!(debian.contains("bwrap:bubblewrap socat:socat git:git curl:curl"));
        assert!(debian.contains("sudo apt-get update -qq && sudo apt-get install -y $missing"), "a fresh Debian has no package lists");
        assert!(debian.contains("mise use -g claude@latest"));
        assert!(setting_up(Os::Arch).contains("sudo pacman -S --needed --noconfirm $missing"));
    }

    #[test]
    fn a_station_is_reached_with_sous_chefs_own_key_and_never_asks() {
        let command = ssh("someone@station.example.com");
        let arguments: Vec<String> = command.get_args().map(|it| it.to_string_lossy().into_owned()).collect();
        assert!(arguments.windows(2).any(|it| it[0] == "-i" && it[1].ends_with("sous-chef/ssh/id_ed25519")));
        assert!(arguments.contains(&"BatchMode=yes".to_string()), "a commis runs unattended");
        assert!(arguments.contains(&"IdentitiesOnly=yes".to_string()), "never the chef's keys");
        assert!(arguments.iter().any(|it| it.starts_with("UserKnownHostsFile=") && it.ends_with("sous-chef/ssh/known_hosts")));
        assert_eq!(arguments.last().unwrap(), "someone@station.example.com");
    }

    #[test]
    fn a_station_runs_the_release_built_for_its_architecture() {
        assert_eq!(release_binary("x86_64"), Some("sous-chef-linux-amd64"));
        assert_eq!(release_binary("aarch64"), Some("sous-chef-linux-arm64"));
        assert_eq!(release_binary("riscv64"), None);
    }
}
