//! An install made under older names, moved over to the current ones the
//! first time sous-chef starts. First from when sous-chef was still called
//! anna: its folders, the Claude sessions filed under them, the token in the
//! keyring and the service. Then from before the kitchen: the folders of
//! threads, hands and sources become those of chefs de partie, commis and
//! ticket printers.
//!
//! And the Basecamp printers an older setup wrote are brought up to what this
//! one would write.
//!
//! Each move happens only when the old folder is there and the new one
//! isn't, so it happens once and never over what is already there. The old
//! data folder is left as a link to the new one: worktrees link their build
//! folders into it, and those links keep working.

use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::config::{self, Config};
use crate::paths;
use crate::secrets;
use crate::service;

const OLD_NAME: &str = "anna";
const OLD_FOLDERS: [(&str, &str); 3] = [("threads", paths::CHEFS_DE_PARTIE), ("hands", paths::COMMIS), ("sources", paths::PRINTERS)];

/// Stops here rather than going on without the old install: what would
/// start then is an empty one, set up from nothing.
pub fn move_over_if_left_behind() {
    if let Err(error) = move_from_the_old_name(OLD_NAME).and_then(|_| move_into_the_kitchen()) {
        eprintln!("The install made under older names couldn't be moved over: {error:#}");
        std::process::exit(1);
    }
    if let Err(error) = printers_up_to_date() {
        eprintln!("The Basecamp printers couldn't be brought up to date: {error:#}");
    }
}

/// What setup wrote into the config for Basecamp, as this release would
/// write it, so an install needn't run setup again to hear sooner.
fn printers_up_to_date() -> Result<()> {
    if !paths::config_dir().join("config.json").exists() {
        return Ok(());
    }
    let mut config = Config::load()?;
    let changed = config.printers.values_mut().map(crate::setup_basecamp::up_to_date).filter(|it| *it).count();
    if changed > 0 {
        config.save()?;
    }
    Ok(())
}

fn move_from_the_old_name(old: &str) -> Result<()> {
    let (old_config, old_data) = (paths::config_dir_named(old), paths::data_dir_named(old));
    let (config, data) = (paths::config_dir(), paths::data_dir());
    if !(left_behind(&old_config, &config) || left_behind(&old_data, &data)) {
        return Ok(());
    }
    refuse_while_running(&paths::runtime_dir_named(old), old)?;

    let service_with_path = service::remove_named(old)?;
    move_folder(&old_config, &config)?;
    move_folder(&old_data, &data)?;
    for suffix in ["", "-wal", "-shm"] {
        move_file(&data.join(format!("{old}.db{suffix}")), &paths::database().with_extension(format!("db{suffix}")))?;
    }
    move_file(&data.join(format!("{old}.out")), &paths::output_file())?;
    file_claude_sessions_anew(&[(&old_config, &config), (&old_data, &data)])?;
    std::os::unix::fs::symlink(&data, &old_data).with_context(|| format!("could not link {} to {}", old_data.display(), data.display()))?;
    secrets::move_from(old, &old_config, config::JEV_API_KEY)?;
    if let Some(search_path) = service_with_path {
        service::install_with_path(&Config::load()?.name, &search_path)?;
    }

    println!("Moved the install made as {old} to sous-chef.");
    Ok(())
}

fn move_into_the_kitchen() -> Result<()> {
    let data = paths::data_dir();
    let moves: Vec<_> = OLD_FOLDERS.iter().map(|(old, new)| (data.join(old), data.join(new))).filter(|(old, new)| left_behind(old, new)).collect();
    if moves.is_empty() {
        return Ok(());
    }
    refuse_while_running(&paths::runtime_dir(), "sous-chef")?;

    for (old, new) in &moves {
        move_folder(old, new)?;
    }
    let refiling: Vec<(&Path, &Path)> = moves.iter().map(|(old, new)| (old.as_path(), new.as_path())).collect();
    file_claude_sessions_anew(&refiling)?;

    println!("Moved the threads, hands and sources over to chefs de partie, commis and ticket printers.");
    Ok(())
}

fn refuse_while_running(runtime: &Path, name: &str) -> Result<()> {
    if UnixStream::connect(runtime.join("control.sock")).is_ok() {
        bail!("{name} is still running; stop it first with `{name} stop`");
    }
    Ok(())
}

fn left_behind(old: &Path, new: &Path) -> bool {
    fs::symlink_metadata(old).is_ok_and(|it| it.is_dir()) && !new.exists()
}

fn move_folder(from: &Path, to: &Path) -> Result<()> {
    if left_behind(from, to) {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, to).with_context(|| format!("could not move {} to {}", from.display(), to.display()))?;
    }
    Ok(())
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if from.exists() && !to.exists() {
        fs::rename(from, to).with_context(|| format!("could not move {} to {}", from.display(), to.display()))?;
    }
    Ok(())
}

/// Claude Code files a session under the folder it ran in, by name, so a
/// chef de partie whose folder moved would start over instead of resuming.
fn file_claude_sessions_anew(moves: &[(&Path, &Path)]) -> Result<()> {
    let projects = paths::claude_config_home().join("projects");
    let Ok(entries) = fs::read_dir(&projects) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(renamed) = refiled(&name, moves) {
            move_file(&entry.path(), &projects.join(renamed))?;
        }
    }
    Ok(())
}

fn refiled(name: &str, moves: &[(&Path, &Path)]) -> Option<String> {
    moves.iter().find_map(|(from, to)| {
        let rest = name.strip_prefix(&filed_as(from))?;
        (rest.is_empty() || rest.starts_with('-')).then(|| format!("{}{rest}", filed_as(to)))
    })
}

/// How Claude Code names a folder: every character but letters and digits
/// becomes a dash.
fn filed_as(folder: &Path) -> String {
    folder.to_string_lossy().chars().map(|it| if it.is_ascii_alphanumeric() { it } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_filed_under_the_old_folders_are_filed_under_the_new_ones() {
        let (old_data, data) = (Path::new("/home/someone/.local/share/anna"), Path::new("/home/someone/.local/share/sous-chef"));
        let (old_config, config) = (Path::new("/home/someone/.config/anna"), Path::new("/home/someone/.config/sous-chef"));
        let renamed = [(old_config, config), (old_data, data)];

        assert_eq!(
            refiled("-home-someone--local-share-anna-threads-basecamp-123", &renamed).as_deref(),
            Some("-home-someone--local-share-sous-chef-threads-basecamp-123")
        );
        assert_eq!(refiled("-home-someone--config-anna-katami", &renamed).as_deref(), Some("-home-someone--config-sous-chef-katami"));
        assert_eq!(refiled("-home-someone--local-share-annabel-x", &renamed), None, "only the folder itself, not one that starts the same");
        assert_eq!(refiled("-home-someone-Work-shop", &renamed), None);

        let (threads, chefs_de_partie) = (Path::new("/home/someone/.local/share/sous-chef/threads"), Path::new("/home/someone/.local/share/sous-chef/chefs_de_partie"));
        assert_eq!(
            refiled("-home-someone--local-share-sous-chef-threads-basecamp-123", &[(threads, chefs_de_partie)]).as_deref(),
            Some("-home-someone--local-share-sous-chef-chefs-de-partie-basecamp-123"),
            "and then again into the kitchen"
        );
    }
}
