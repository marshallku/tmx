use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::{self, Config};
use crate::project::{self};
use crate::shell_init;
use crate::tmux;
use crate::ui;
use crate::worktree::{self, RemoveDecision};

#[derive(Parser, Debug)]
#[command(
    name = "tmx",
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

    /// Manage git worktrees for the current repo
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },

    /// Emit shell integration code (defines the 'twt' wrapper)
    #[command(
        name = "shell-init",
        long_about = "Emit shell initialization code that defines a 'twt' function\nwrapping 'tmx worktree create' to cd into the new worktree by default.\n\nAdd to your shell rc:\n\n    eval \"$(tmx shell-init zsh)\"\n\nThen:\n\n    twt feat-x           # create worktree and cd into it\n    twt feat-x -p        # create worktree and print path (no cd)\n    twt feat-x --tmux    # create worktree and switch to a new tmux session\n\nSupported shells: zsh."
    )]
    ShellInit {
        /// Shell name (currently only 'zsh')
        shell: String,
    },
}

#[derive(Subcommand, Debug)]
enum WorktreeCommand {
    /// Create a sibling git worktree for the current repo
    #[command(
        long_about = "Create a sibling git worktree, optionally run a post-create script,\nand optionally spawn a tmux session at the new worktree.\n\nWorktree is placed next to the source repo (sibling), named via the\n'worktree.naming' template in ~/.config/tmx/config.toml\n(default: '{repo}-{branch}'). Slashes in branch names are replaced\nwith dashes for the directory name."
    )]
    Create {
        /// New branch to create (also used to derive the directory name)
        branch: String,

        /// Create a tmux session at the new worktree and switch to it
        #[arg(short = 't', long = "tmux")]
        tmux: bool,

        /// Base ref for the new branch (default: HEAD)
        #[arg(long = "from")]
        from: Option<String>,
    },

    /// Remove a sibling git worktree (fuzzy picker if no target given)
    #[command(
        long_about = "Remove a worktree by exact path or short branch name (e.g. 'feat-x').\nIf no target is provided, an interactive picker is shown.\n\nSafety rules (apply even with --force):\n  - Locked worktrees are refused; run 'git worktree unlock' first.\n  - The worktree containing the current working directory is refused.\n  - The worktree containing the current tmux session is refused.\n\nWithout --force, any tmux session attached to the worktree blocks removal.\nWith --force, attached sessions are killed (by id) and 'git worktree remove --force'\nis used (covers dirty worktrees)."
    )]
    Rm {
        /// Worktree path or short branch name. Picker shown if omitted.
        target: Option<String>,

        /// Kill conflicting sessions and pass --force to git.
        #[arg(short = 'f', long = "force")]
        force: bool,
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
        Some(Command::Worktree { command }) => match command {
            WorktreeCommand::Create { branch, tmux, from } => {
                run_worktree_create(&branch, tmux, from.as_deref().unwrap_or(""))
            }
            WorktreeCommand::Rm { target, force } => run_worktree_rm(target.as_deref(), force),
        },
        Some(Command::ShellInit { shell }) => run_shell_init(&shell),
    }
}

fn run_selector() -> Result<()> {
    let cfg = Config::load();
    let projects = project::scan_projects(&cfg);
    if projects.is_empty() {
        println!("No projects found. Configure roots in ~/.config/tmx/config.toml");
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

fn run_worktree_create(branch: &str, with_tmux: bool, from: &str) -> Result<()> {
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

    // No --tmux: print the path on stdout so callers can `cd $(tmx worktree create ...)`.
    println!("{}", res.worktree_path.display());
    Ok(())
}

fn run_worktree_rm(target: Option<&str>, force: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve cwd")?;
    let entries = worktree::list_entries(&cwd)?;

    let entry: worktree::WorktreeEntry = match target {
        Some(t) => worktree::resolve_target(&entries, t)?.clone(),
        None => {
            let removable: Vec<worktree::WorktreeEntry> =
                entries.iter().filter(|e| !e.is_main).cloned().collect();
            if removable.is_empty() {
                println!("No removable worktrees.");
                return Ok(());
            }
            let Some(selected) = ui::run_worktree_picker(removable)? else {
                return Ok(());
            };
            selected
        }
    };

    rm_preflight_and_remove(&entry, force, &cwd)
}

fn rm_preflight_and_remove(entry: &worktree::WorktreeEntry, force: bool, cwd: &Path) -> Result<()> {
    // Step 1: locked preflight.
    if worktree::remove_decision(entry) == RemoveDecision::LockedRefuse {
        bail!(
            "worktree is locked: {}\nrun `git worktree unlock {}` first.",
            entry.path.display(),
            entry.path.display()
        );
    }

    // Step 2: missing-target shortcut. Skip cwd / tmux checks and hand to git.
    let target_canon = entry.path.canonicalize().ok();
    if target_canon.is_none() {
        return worktree::remove(&entry.path, force);
    }
    let target_canon = target_canon.unwrap();

    // Step 3: self-cwd guard.
    let cwd_canon = cwd.canonicalize().context("canonicalize cwd")?;
    if is_inside(&cwd_canon, &target_canon) {
        bail!(
            "refusing to remove worktree containing the current working directory.\ncd out of {} first, then re-run.",
            entry.path.display()
        );
    }

    // Step 4: self-session guard + conflict gate.
    let in_tmux = std::env::var_os("TMUX").is_some();
    let current_session = if in_tmux {
        match tmux::current_session_id() {
            Some(id) => Some(id),
            None => bail!("cannot determine current tmux session id; refusing to proceed"),
        }
    } else {
        None
    };

    let attached: Vec<worktree::AttachedSession> =
        match worktree::sessions_attached_to(&target_canon) {
            Ok(v) => v,
            Err(e) => {
                if in_tmux {
                    bail!(
                        "tmux query failed while inside a tmux session; refusing to proceed: {e}"
                    );
                } else {
                    Vec::new()
                }
            }
        };

    if let Some(cur) = current_session.as_deref()
        && attached.iter().any(|s| s.id == cur)
    {
        bail!(
            "refusing to remove worktree containing the current tmux session.\nswitch to a different session first, then re-run."
        );
    }

    if !attached.is_empty() {
        if !force {
            let names: Vec<String> = attached.iter().map(|s| s.name.clone()).collect();
            bail!(
                "worktree {} is in use by tmux session(s): {}\npass --force to kill them and proceed",
                entry.path.display(),
                names.join(", ")
            );
        }
        for s in &attached {
            tmux::kill_session_id(&s.id)
                .with_context(|| format!("kill-session {} ({})", s.id, s.name))?;
            eprintln!("killed tmux session: {} ({})", s.name, s.id);
        }
    }

    worktree::remove(&entry.path, force)
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    if child == parent {
        return true;
    }
    let c = child.to_string_lossy();
    let p = parent.to_string_lossy();
    let p = p.trim_end_matches('/');
    if let Some(rest) = c.strip_prefix(p)
        && rest.starts_with('/')
    {
        return true;
    }
    false
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
