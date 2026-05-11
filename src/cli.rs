use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::{self, Config};
use crate::project::{self};
use crate::shell_init;
use crate::tmux;
use crate::ui;
use crate::worktree;

#[derive(Parser, Debug)]
#[command(
    name = "tmux-powertools",
    about = "Project-aware tmux session manager",
    long_about = "Scan project directories, show git status, and manage tmux sessions with project-type layouts.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Kill all unattached tmux sessions
    Cleanup,

    /// List all tmux sessions
    List,

    /// Switch to a tmux session (interactive picker or by name)
    Switch {
        /// Optional session name. If provided, switch directly (creating if needed).
        session: Option<String>,
    },

    /// Create a sibling git worktree for the current repo
    #[command(
        long_about = "Create a sibling git worktree, optionally run a post-create script,\nand optionally spawn a tmux session at the new worktree.\n\nWorktree is placed next to the source repo (sibling), named via the\n'worktree.naming' template in ~/.config/tmux-powertools/config.toml\n(default: '{repo}-{branch}'). Slashes in branch names are replaced\nwith dashes for the directory name."
    )]
    Worktree {
        /// New branch to create (also used to derive the directory name)
        branch: String,

        /// Create a tmux session at the new worktree and switch to it
        #[arg(short = 't', long = "tmux")]
        tmux: bool,

        /// Base ref for the new branch (default: HEAD)
        #[arg(long = "from")]
        from: Option<String>,
    },

    /// Emit shell integration code (defines the 'twt' wrapper)
    #[command(
        name = "shell-init",
        long_about = "Emit shell initialization code that defines a 'twt' function\nwrapping 'tmux-powertools worktree' to cd into the new worktree by default.\n\nAdd to your shell rc:\n\n    eval \"$(tmux-powertools shell-init zsh)\"\n\nThen:\n\n    twt feat-x           # create worktree and cd into it\n    twt feat-x -p        # create worktree and print path (no cd)\n    twt feat-x --tmux    # create worktree and switch to a new tmux session\n\nSupported shells: zsh."
    )]
    ShellInit {
        /// Shell name (currently only 'zsh')
        shell: String,
    },
}

pub fn run() -> Result<()> {
    config::ensure_config_dir();
    let cli = Cli::parse();
    match cli.command {
        None => run_selector(),
        Some(Command::Cleanup) => run_cleanup(),
        Some(Command::List) => run_list(),
        Some(Command::Switch { session }) => run_switch(session.as_deref()),
        Some(Command::Worktree { branch, tmux, from }) => {
            run_worktree(&branch, tmux, from.as_deref().unwrap_or(""))
        }
        Some(Command::ShellInit { shell }) => run_shell_init(&shell),
    }
}

fn run_selector() -> Result<()> {
    let cfg = Config::load();
    let projects = project::scan_projects(&cfg);
    if projects.is_empty() {
        println!("No projects found. Configure roots in ~/.config/tmux-powertools/config.toml");
        println!("Example: roots = [\"~/dev\"]");
        return Ok(());
    }

    let selected = ui::run_project_selector(projects)?;
    let Some(project) = selected else {
        return Ok(());
    };
    ui::open_project(&project)
}

fn run_cleanup() -> Result<()> {
    let killed = tmux::cleanup_sessions().context("cleanup failed")?;
    if killed.is_empty() {
        println!("No unattached sessions to clean up.");
    } else {
        println!("Killed {} session(s): {}", killed.len(), killed.join(", "));
    }
    Ok(())
}

fn run_list() -> Result<()> {
    let sessions = match tmux::list_sessions() {
        Ok(s) => s,
        Err(_) => {
            println!("No tmux server running.");
            return Ok(());
        }
    };
    if sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }
    for s in sessions {
        let attached = if s.attached { " (attached)" } else { "" };
        println!("  {} — {} window(s){}", s.name, s.windows, attached);
    }
    Ok(())
}

fn run_switch(name: Option<&str>) -> Result<()> {
    if let Some(name) = name {
        if !tmux::session_exists(name) {
            tmux::create_session(name, "").context("failed to create session")?;
        }
        return tmux::switch_session(name).context("failed to switch session");
    }

    let sessions = match tmux::list_sessions() {
        Ok(s) if !s.is_empty() => s,
        _ => {
            println!("No active tmux sessions.");
            return Ok(());
        }
    };

    let Some(selected) = ui::run_session_switcher(sessions)? else {
        return Ok(());
    };
    tmux::switch_session(&selected.name).context("failed to switch session")
}

fn run_worktree(branch: &str, with_tmux: bool, from: &str) -> Result<()> {
    let cfg = Config::load();
    let res = worktree::create(
        worktree::Options {
            branch: branch.to_string(),
            from: from.to_string(),
            start_in: String::new(),
        },
        &cfg,
    )?;

    eprintln!("Created worktree: {}", res.worktree_path.display());
    if !res.script_ran.is_empty() {
        eprintln!("Ran post-create script: {}", res.script_ran);
    }

    if with_tmux {
        let session_name = tmux::safe_session_name(&res.repo_name, branch);
        if !tmux::session_exists(&session_name) {
            tmux::create_session(&session_name, &res.worktree_path.to_string_lossy())
                .context("create tmux session")?;
        }
        return tmux::switch_session(&session_name).context("switch tmux session");
    }

    // No --tmux: print the path on stdout so callers can `cd $(tmux-powertools worktree ...)`.
    println!("{}", res.worktree_path.display());
    Ok(())
}

fn run_shell_init(shell: &str) -> Result<()> {
    match shell_init::emit(shell) {
        Ok(text) => {
            print!("{text}");
            Ok(())
        }
        Err(e) => bail!("{e}"),
    }
}
