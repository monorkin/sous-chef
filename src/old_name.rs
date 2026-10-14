//! An install made when sous-chef was still called anna, moved over to the
//! new name the first time sous-chef starts: its folders, the Claude sessions
//! filed under them, the token in the keyring and the service.
//!
//! Only when the old folders are there and the new ones aren't, so it happens
//! once and never over an install already made under the new name. The old
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

const OLD: &str = "anna";

/// Stops here rather than going on without the old install: what would
/// start then is an empty one, set up from nothing.
pub fn move_over_if_left_behind() {
    if let Err(error) = move_over(OLD) {
        eprintln!("The install made as {OLD} couldn't be moved to sous-chef: {error:#}");
        std::process::exit(1);
    }
}

fn move_over(old: &str) -> Result<()> {
    let (old_config, old_data) = (paths::config_dir_named(old), paths::data_dir_named(old));
    let (config, data) = (paths::config_dir(), paths::data_dir());
    if !(left_behind(&old_config, &config) || left_behind(&old_data, &data)) {
        return Ok(());
    }
    if UnixStream::connect(paths::runtime_dir_named(old).join("control.sock")).is_ok() {
        bail!("{old} is still running; stop it first with `{old} stop`");
    }

    let was_a_service = service::remove_named(old)?;
    move_folder(&old_config, &config)?;
    move_folder(&old_data, &data)?;
    for suffix in ["", "-wal", "-shm"] {
        move_file(&data.join(format!("{old}.db{suffix}")), &paths::database().with_extension(format!("db{suffix}")))?;
    }
    move_file(&data.join(format!("{old}.out")), &paths::output_file())?;
    file_claude_sessions_anew(&paths::claude_config_home().join("projects"), &[(&old_config, &config), (&old_data, &data)])?;
    std::os::unix::fs::symlink(&data, &old_data).with_context(|| format!("could not link {} to {}", old_data.display(), data.display()))?;
    secrets::move_from(old, &old_config, config::JEV_API_KEY)?;
    if was_a_service {
        service::install(&Config::load()?.name, false)?;
    }

    println!("Moved the install made as {old} to sous-chef.");
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
/// thread whose folder moved would start over instead of resuming.
fn file_claude_sessions_anew(projects: &Path, moves: &[(&Path, &Path)]) -> Result<()> {
    let Ok(entries) = fs::read_dir(projects) else {
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
        let moves = [(old_config, config), (old_data, data)];

        assert_eq!(
            refiled("-home-someone--local-share-anna-threads-basecamp-123", &moves).as_deref(),
            Some("-home-someone--local-share-sous-chef-threads-basecamp-123")
        );
        assert_eq!(refiled("-home-someone--config-anna-katami", &moves).as_deref(), Some("-home-someone--config-sous-chef-katami"));
        assert_eq!(refiled("-home-someone--local-share-annabel-x", &moves), None, "only the folder itself, not one that starts the same");
        assert_eq!(refiled("-home-someone-Work-shop", &moves), None);
    }
}
