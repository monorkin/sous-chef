mod at_work;
mod backup;
mod breaker;
mod broker;
mod chef_de_partie;
mod chef_de_partie_tools;
mod claude;
mod clock;
mod commis;
mod config;
mod control;
mod conversation;
mod cooling;
mod dash;
mod dash_ansi;
mod dash_chat;
mod dash_data;
mod dash_recipes;
mod dash_tables;
mod deadline;
mod expediter;
mod fsutil;
mod github;
mod held;
mod kitchen;
mod lifecycle;
mod logs;
mod mailbox;
mod mcp;
mod mcp_cli;
mod mcp_server;
mod old_install;
mod pass;
mod paths;
mod plating;
mod printer;
mod printer_cli;
mod prompt;
mod proxy;
mod recipe_book_tools;
mod runtime;
mod sandbox;
mod schedule_tools;
mod secrets;
mod service;
mod setup;
mod setup_basecamp;
mod setup_machine;
mod setup_tools;
mod skills;
mod store;
mod suppliers;
mod switching;
mod taster;
mod timekeeper;
mod toolchains;
mod transcripts;
mod triggers;
mod turns;
mod upgrade;
mod work_tools;

use anyhow::Result;
use std::sync::Arc;
use usage::{Cli, Subcommands};

use crate::conversation::{Standing, Terminal};
use crate::runtime::Runtime;

/// The harness for a self-governing agent you work with like a colleague
#[derive(Cli)]
#[usage(bin = "sous-chef", version, unknown_flags = "error")]
struct Cli {
    #[usage(subcommand)]
    command: Command,
}

#[derive(Subcommands)]
enum Command {
    /// Walk through the one-time setup; safe to run again
    Setup,
    /// Upgrade a mise install to the latest release, and point her service at it
    Upgrade {
        /// Target a specific release instead of the latest
        version: Option<String>,
    },
    /// Bring sous-chef up in the background, through systemd when the service is installed
    Start,
    /// Stop sous-chef and everything it started
    Stop,
    /// Stop and start it again; she picks up every turn she was in the middle of
    Restart,
    /// Show whether sous-chef is running and what it's listening on
    Status,
    /// A dashboard in this terminal: her suppliers, recipes and work, what's going on, and a line to tell a chef de partie something
    Dash,
    /// Have sous-chef check a printer now instead of at the next tick; all of them when none is named
    Poke { printer: Option<String> },
    /// Run sous-chef in this terminal: listen on every printer until stopped
    Run,
    /// Write everything that makes this sous-chef what it is to a zip; safe while it runs
    Backup {
        /// Where to write the zip; defaults to sous-chef-backup-<time>.zip here
        #[usage(long)]
        to: Option<std::path::PathBuf>,
        /// Leave her tokens and logins out of the zip: Jev's key, the Claude suppliers, her tools' own config
        #[usage(long)]
        without_secrets: bool,
    },
    /// Bring a sous-chef back from a backup zip; it has to be stopped
    Restore {
        path: std::path::PathBuf,
        /// Replace the sous-chef that is already set up here
        #[usage(long)]
        force: bool,
    },
    /// Talk to the agent from this terminal
    Chat {
        message: String,
        /// Which terminal conversation this belongs to
        #[usage(long, default = "main")]
        conversation: String,
    },
    /// Say something straight to one of her chefs de partie, with your standing; any part of its name will do
    Tell { chef_de_partie: String, message: String },
    /// Show what the agent has been doing
    Log {
        /// Keep printing as things happen
        #[usage(short = 'f', long)]
        follow: bool,
        /// Only the lines of one chef de partie, commis or conversation; any part of its id
        #[usage(long)]
        only: Option<String>,
    },
    /// Read what was said in a chef de partie or a commis; lists them when no id is given
    Transcript { id: Option<String> },
    /// Manage the MCP servers the agent works through
    Mcp {
        #[usage(subcommand)]
        command: McpCommand,
    },
    /// Manage what she knows about using a tool before she calls it
    Skill {
        #[usage(subcommand)]
        command: SkillCommand,
    },
    /// Look at the places the agent listens
    Printer {
        #[usage(subcommand)]
        command: PrinterCommand,
    },
    /// Inspect and manage what the agent has learned
    Recipes {
        #[usage(subcommand)]
        command: katami::cli::MemoryCommand,
    },
    /// Manage the Claude subscriptions the agent works with
    Claude {
        #[usage(subcommand)]
        command: ClaudeCommand,
    },
    /// The agent's own GitHub login, kept apart from yours
    Github {
        #[usage(subcommand)]
        command: GithubCommand,
    },
}

#[derive(Subcommands)]
enum PrinterCommand {
    /// Read a printer once and show what the agent would make of it, without acting on anything
    Check { name: String },
}

#[derive(Subcommands)]
enum GithubCommand {
    /// Log the agent's own GitHub folder in, in the browser; her turns push and open pull requests as that account
    Login {
        /// The email that account uses, so commits are credited to it; kept from last time when left out
        #[usage(long)]
        email: Option<String>,
    },
}

#[derive(Subcommands)]
enum ClaudeCommand {
    /// Log the agent's own Claude folder in, in the browser; then `supplier add` keeps that login
    Login,
    /// Manage the suppliers in the rotation
    Supplier {
        #[usage(subcommand)]
        command: ax::cli::AccountCommand,
    },
    /// Make a stored supplier the one the agent works as
    Switch {
        /// Supplier number, email, or alias
        supplier: String,
    },
    /// Who the agent is logged in as right now
    Whoami,
}

#[derive(Subcommands)]
enum SkillCommand {
    /// Give her a skill of your own: the folder with its SKILL.md in it
    Add { folder: std::path::PathBuf },
    /// What she knows
    List,
    /// Take one away
    Remove { name: String },
}

#[derive(Subcommands)]
enum McpCommand {
    /// Add a server, or accept the changed tools of one already added
    Add {
        name: String,
        /// The server acts as you, not as the agent: only turns on a trusted person's word get it
        #[usage(long)]
        as_me: bool,
        /// The command that starts the server
        #[usage(double_dash = "required")]
        server_command: Vec<String>,
    },
    /// List servers and their tools
    List,
    /// Remove a server
    Remove { name: String },
    /// Mark a tool's argument as prose for people, so the plating restyles it
    Prose {
        name: String,
        tool: String,
        argument: String,
    },
}

fn main() {
    // Before anything makes the new folders, or the old ones would be left
    // behind for good
    old_install::move_over_if_left_behind();
    paths::claim_own_state();

    // Rust ignores SIGPIPE, which turns `sous-chef log | head` into a panic,
    // so the commands that print and leave get the usual behaviour back. The
    // ones that talk to MCP servers don't: there a closed pipe has to come
    // back as an error, or a server that died takes the whole of sous-chef
    // with it at the next write.
    let talks_to_servers = matches!(std::env::args().nth(1).as_deref(), Some("run" | "chat"));
    if !talks_to_servers {
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }

    let result = match std::env::args().nth(1).as_deref() {
        Some(command) if KATAMI_RUNS_ITSELF_AS.contains(&command) => katami::cli::run(katami::cli::Cli::parse()),
        _ => run(Cli::parse()),
    };
    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

/// katami runs its own executable for these: the hooks it registers with
/// claude, and the reviews and curation it spawns in the background. Inside
/// sous-chef that executable is sous-chef, so it hands them straight to
/// katami.
const KATAMI_RUNS_ITSELF_AS: [&str; 3] = ["hook", "review", "curate"];

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Setup => setup::run(),
        Command::Upgrade { version } => upgrade::run(version.as_deref()),
        Command::Start => lifecycle::start(),
        Command::Stop => lifecycle::stop(),
        Command::Restart => lifecycle::restart(),
        Command::Status => lifecycle::status(),
        Command::Dash => dash::run(),
        Command::Poke { printer } => lifecycle::poke(printer.as_deref()),
        Command::Run => expediter::run(),
        Command::Backup { to, without_secrets } => backup::backup(to, without_secrets),
        Command::Restore { path, force } => backup::restore(&path, force),
        Command::Chat { message, conversation } => {
            // Whoever is at this terminal can already do anything the agent can
            let runtime = Arc::new(Runtime::start()?);
            chef_de_partie::wake(&runtime, Arc::new(Terminal::new(&conversation)), Standing::Trusted, &message)?;
            runtime.wait_until_settled();
            Ok(())
        }
        Command::Tell { chef_de_partie, message } => lifecycle::tell(&chef_de_partie, &message),
        Command::Log { follow, only } => logs::print(follow, only.as_deref()),
        Command::Transcript { id: Some(id) } => transcripts::show(&id),
        Command::Transcript { id: None } => transcripts::list(),
        Command::Mcp { command } => match command {
            McpCommand::Add { name, as_me, server_command } => mcp_cli::add(&name, &server_command, as_me),
            McpCommand::List => mcp_cli::list(),
            McpCommand::Remove { name } => mcp_cli::remove(&name),
            McpCommand::Prose { name, tool, argument } => mcp_cli::mark_prose(&name, &tool, &argument),
        },
        Command::Skill { command } => match command {
            SkillCommand::Add { folder } => {
                let name = skills::add(&folder)?;
                println!("Added {name}. Her chefs de partie read it when they need it.");
                Ok(())
            }
            SkillCommand::List => skills::list(),
            SkillCommand::Remove { name } => skills::remove(&name),
        },
        Command::Printer { command } => match command {
            PrinterCommand::Check { name } => printer_cli::check(&name),
        },
        Command::Recipes { command } => katami::cli::run(katami::cli::Cli {
            command: katami::cli::Command::Memory { command },
        }),
        Command::Github { command } => match command {
            GithubCommand::Login { email } => github_login(email),
        },
        Command::Claude { command } => match command {
            ClaudeCommand::Login => claude::log_in(),
            ClaudeCommand::Supplier { command } => ax::cli::run(ax::cli::Cli {
                command: ax::cli::Command::Account { command },
            }),
            ClaudeCommand::Switch { supplier } => ax::cli::run(ax::cli::Cli {
                command: ax::cli::Command::Switch { account: supplier },
            }),
            ClaudeCommand::Whoami => {
                println!("{}", claude::login());
                Ok(())
            }
        },
    }
}

/// Her commits are credited to the account by its email, so the login isn't
/// finished without one: given here, or kept from the last time.
fn github_login(email: Option<String>) -> Result<()> {
    let login = github::log_in()?;
    match email.or_else(github::email) {
        Some(email) => {
            github::set_identity(&config::Config::load()?.name, &email)?;
            println!("Logged in as {login}. Her turns push and open pull requests as that account, with commits by {email}.");
        }
        None => println!("Logged in as {login}, but without an email its commits are credited to nobody: run this again with --email <the account's address>."),
    }
    Ok(())
}
