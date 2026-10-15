//! `sous-chef start`, `stop`, `status` and `poke`: the commands that talk to
//! a running sous-chef, or bring one up.
//!
//! When the systemd service is installed, starting and stopping go through
//! it, so systemd's idea of whether it is running stays true. Otherwise
//! `start` puts `sous-chef run` in the background itself, and `stop` asks it
//! over the control socket.

use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::control;
use crate::paths;
use crate::service;

const PATIENCE: Duration = Duration::from_secs(20);

pub fn start() -> Result<()> {
    start_as(&std::env::current_exe().context("could not determine sous-chef's own path")?)
}

/// Starts `binary` rather than the one running this command, for an upgrade
/// that has just put a new one in place. The service already starts whatever
/// it was pointed at.
pub fn start_as(binary: &Path) -> Result<()> {
    if control::is_running() {
        println!("sous-chef is already running.");
        return Ok(());
    }

    if service::installed() {
        service::start()?;
    } else {
        start_in_the_background(binary)?;
    }

    if wait_until(control::is_running) {
        println!("sous-chef is running. `sous-chef log -f` shows what it's doing.");
        Ok(())
    } else {
        bail!("sous-chef did not come up; see {}", paths::output_file().display())
    }
}

pub fn stop() -> Result<()> {
    if !control::is_running() {
        println!("sous-chef isn't running.");
        return Ok(());
    }

    if service::active() {
        service::stop()?;
    } else {
        control::ask(json!({ "command": "stop" }))?;
    }

    if wait_until(|| !control::is_running()) {
        println!("sous-chef stopped.");
        Ok(())
    } else {
        bail!("sous-chef is still running")
    }
}

/// Stop, then start. Nothing is lost in between: every turn that was going
/// or waiting is kept in the store and asked again when it is back.
pub fn restart() -> Result<()> {
    stop()?;
    start()
}

pub fn restart_as(binary: &Path) -> Result<()> {
    stop()?;
    start_as(binary)
}

pub fn status() -> Result<()> {
    match control::ask(json!({ "command": "status" })) {
        Ok(answer) => {
            let status = &answer["status"];
            println!("sous-chef has been running since {}, as process {}.", text(&status["since"]), status["process"]);
            println!("Working as: {}", text(&status["claude_login"]));
            if let Some(login) = status["github_login"].as_str() {
                println!("On GitHub as: {login}");
            }
            println!("Listening on: {}", names(&status["waiters"]));
            match status["going_on"].as_array() {
                Some(turns) => {
                    print_going_on(turns);
                    print_board(&status["board"]);
                }
                // A sous-chef started before this build knows the count and
                // nothing else; saying nothing is going on would be a guess
                None => println!("Conversations going on: {}", status["conversations"]),
            }
        }
        Err(error) => println!("{error}."),
    }
    Ok(())
}

pub fn poke(waiter: Option<&str>) -> Result<()> {
    answered(control::ask(json!({ "command": "poke", "waiter": waiter }))?)
}

pub fn tell(chef_de_partie: &str, message: &str) -> Result<()> {
    println!("{}", told(chef_de_partie, message)?);
    Ok(())
}

/// What the chef de partie was told, in her words, for a caller that shows it
/// somewhere other than stdout.
pub fn told(chef_de_partie: &str, message: &str) -> Result<String> {
    said_back(control::ask(json!({ "command": "tell", "chef_de_partie": chef_de_partie, "message": message }))?)
}

fn answered(answer: serde_json::Value) -> Result<()> {
    println!("{}", said_back(answer)?);
    Ok(())
}

/// What a running sous-chef said back when it did it, an error when it
/// couldn't.
fn said_back(answer: serde_json::Value) -> Result<String> {
    if answer["ok"].as_bool().unwrap_or(false) {
        Ok(text(&answer["message"]).to_string())
    } else {
        bail!("{}", text(&answer["message"]))
    }
}

/// `sous-chef run` in a session of its own, so it survives the terminal that
/// started it, with its output kept where `start` can point at it.
fn start_in_the_background(binary: &Path) -> Result<()> {
    let output_path = paths::output_file();
    if let Some(directory) = output_path.parent() {
        fs::create_dir_all(directory)?;
    }
    let output = OpenOptions::new().create(true).append(true).open(&output_path)?;

    let mut command = Command::new(binary);
    command
        .arg("run")
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().context("could not start sous-chef")?;
    Ok(())
}

fn wait_until(condition: impl Fn() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < PATIENCE {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    condition()
}

fn print_going_on(turns: &[serde_json::Value]) {
    if turns.is_empty() {
        println!("\nNothing going on right now.");
        return;
    }

    println!("\nGoing on right now:");
    for turn in turns {
        match turn["for"].as_str() {
            Some(taken) => println!("  {} for {taken}{}", text(&turn["conversation"]), behind(&turn["waiting"])),
            None => println!("  {}, between turns", text(&turn["conversation"])),
        }
        if let Some(doing) = turn["doing"].as_str() {
            println!("      {doing}");
        }
        for commis in turn["commis"].as_array().map(Vec::as_slice).unwrap_or_default() {
            println!(
                "      commis {} in {}, {}",
                text(&commis["commis"]),
                text(&commis["project"]),
                text(&commis["for"])
            );
        }
    }
}

fn print_board(board: &serde_json::Value) {
    let claimed = board.as_array().map(Vec::as_slice).unwrap_or_default();
    if claimed.is_empty() {
        return;
    }

    println!("\nClaimed, between turns:");
    for work in claimed {
        println!("  {}", text(&work["conversation"]));
        println!("      {}", text(&work["doing"]));
    }
}

fn behind(waiting: &serde_json::Value) -> String {
    match waiting.as_u64().unwrap_or(0) {
        0 => String::new(),
        1 => ", 1 message waiting".to_string(),
        many => format!(", {many} messages waiting"),
    }
}

pub fn text(value: &serde_json::Value) -> &str {
    value.as_str().unwrap_or("-")
}

pub fn names(value: &serde_json::Value) -> String {
    value
        .as_array()
        .map(|items| items.iter().filter_map(|it| it.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}
