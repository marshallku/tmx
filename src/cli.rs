use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::agents;
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
    /// Show a live dashboard of agents running in tmux panes
    #[command(
        long_about = "Open a terminal dashboard listing every tmux pane and any Claude/Codex agent\nrunning inside it, plus codex-companion background jobs. Reads tmux + local\nstate (~/.claude/state/), no daemon required. Works the same in GUI tmux and\nover SSH.\n\nKeys:\n  j/k or ↑/↓   navigate\n  enter        switch tmux client to the selected pane\n  q / esc      quit\n\nWith --json: skip the TUI, emit one snapshot to stdout as JSON and exit.\nUseful for agent-driven consumers (jq, scripts, other LLMs)."
    )]
    Agents {
        /// Emit one snapshot to stdout as JSON instead of opening the TUI.
        #[arg(long)]
        json: bool,
    },

    /// Kill all unattached tmux sessions
    Cleanup,

    /// List all tmux sessions
    List {
        /// Emit the session list to stdout as JSON.
        #[arg(long)]
        json: bool,
    },

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
        long_about = "Emit shell initialization code that defines a 'twt' function\nrouting to 'tmx worktree …'.\n\nAdd to your shell rc:\n\n    eval \"$(tmx shell-init zsh)\"\n\nThen:\n\n    twt feat-x                 # create worktree + spawn/switch tmux session (default)\n    twt feat-x --keep-current  # create only; cd into the new worktree (no tmux switch)\n    twt feat-x -p              # create only; print path (no cd, no tmux switch)\n    twt rm [target]            # remove a worktree (picker if no target)\n    twt list                   # picker → cd into the selected worktree\n    twt list <target>          # cd into the named worktree (path or short branch)\n    twt list --plain           # dump all worktrees as plain text (no cd)\n\nSupported shells: zsh."
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
        long_about = "Create a sibling git worktree, optionally run a post-create command,\nand by default spawn a tmux session at the new worktree and switch to it.\n\nWorktree is placed next to the source repo (sibling), named via the\n'worktree.naming' template in ~/.config/tmx/config.toml\n(default: '{repo}-{branch}'). Slashes in branch names are replaced\nwith dashes for the directory name.\n\nWith --keep-current, no tmux session is created and the worktree path is\nprinted on stdout (useful for shell wrappers that cd into the new path)."
    )]
    Create {
        /// New branch to create (also used to derive the directory name)
        branch: String,

        /// Do not spawn / switch to a tmux session for the new worktree;
        /// just create it and print the path on stdout.
        #[arg(long = "keep-current")]
        keep_current: bool,

        /// Base ref for the new branch (default: HEAD)
        #[arg(long = "from")]
        from: Option<String>,
    },

    /// List worktrees and print the chosen path on stdout
    #[command(
        long_about = "Print a worktree path on stdout, intended for shell wrappers (`cd \"$(tmx worktree list)\"`).\n\nWith no target, an interactive picker is shown on stderr (so stdout stays clean for `$(...)` capture).\nWith a target (path or short branch), the resolved path is printed directly.\nWith --plain, all worktrees are dumped as 'path  branch  [flags]' lines on stdout.\nWith --json, all worktrees (or just the resolved target) are emitted as a JSON array."
    )]
    List {
        /// Worktree path or short branch name. Picker shown if omitted.
        target: Option<String>,

        /// Dump all worktrees as plain text instead of running the picker.
        #[arg(long = "plain", conflicts_with = "json")]
        plain: bool,

        /// Emit worktrees to stdout as a JSON array.
        #[arg(long)]
        json: bool,
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
        Some(Command::Agents { json }) => run_agents(json),
        Some(Command::Cleanup) => run_cleanup(),
        Some(Command::List { json }) => run_list(json),
        Some(Command::Switch { session }) => run_switch(session.as_deref()),
        Some(Command::Worktree { command }) => match command {
            WorktreeCommand::Create {
                branch,
                keep_current,
                from,
            } => run_worktree_create(&branch, keep_current, from.as_deref().unwrap_or("")),
            WorktreeCommand::List {
                target,
                plain,
                json,
            } => run_worktree_list(target.as_deref(), plain, json),
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

fn run_agents(json: bool) -> Result<()> {
    if json {
        let mut proc = agents::proc::ProcSnapshot::new();
        proc.refresh();
        let snapshot = agents::collector::collect(&proc);
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        serde_json::to_writer_pretty(&mut handle, &snapshot).context("serialise snapshot")?;
        writeln!(handle).ok();
        return Ok(());
    }
    agents::run()
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

fn run_list(json: bool) -> Result<()> {
    // In JSON mode we always emit a (possibly empty) array — even when tmux
    // isn't running. Consumers parse a single shape regardless of state.
    if json {
        let sessions = tmux::list_sessions().unwrap_or_default();
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        serde_json::to_writer_pretty(&mut handle, &sessions).context("serialise sessions")?;
        writeln!(handle).ok();
        return Ok(());
    }

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
        Ok(_) => {
            println!("No active tmux sessions.");
            return Ok(());
        }
        Err(_) => {
            // No server running. Boot it so tmux-continuum auto-restores the
            // previously saved environment, then pick from what came back.
            // Drop the bootstrap session unconditionally — its kill-session
            // cleanup is best-effort, so filtering here is what guarantees it
            // never reaches the switcher.
            tmux::boot_and_restore().context("failed to boot tmux server")?;
            let mut restored =
                tmux::list_sessions().context("failed to list sessions after restore")?;
            restored.retain(|s| s.name != tmux::BOOTSTRAP_SESSION);
            if restored.is_empty() {
                println!("No sessions to restore.");
                return Ok(());
            }
            restored
        }
    };

    let Some(selected) = ui::run_session_switcher(sessions)? else {
        return Ok(());
    };
    tmux::switch_session(&selected.name).context("failed to switch session")
}

fn run_worktree_create(branch: &str, keep_current: bool, from: &str) -> Result<()> {
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
        eprintln!("Ran post-create command: {}", res.script_ran);
    }

    if keep_current {
        // Print the path on stdout so callers can `cd "$(tmx worktree create … --keep-current)"`.
        println!("{}", res.worktree_path.display());
        return Ok(());
    }

    let session_name = tmux::safe_session_name(&res.repo_name, branch);
    if !tmux::session_exists(&session_name) {
        tmux::create_session(&session_name, &res.worktree_path.to_string_lossy())
            .context("create tmux session")?;
    }
    tmux::switch_session(&session_name).context("switch tmux session")
}

fn run_worktree_list(target: Option<&str>, plain: bool, json: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve cwd")?;
    let entries = worktree::list_entries(&cwd)?;

    // `--json` always emits a JSON array, even with a target (1-element)
    // and even when the repo has only its main worktree (single entry).
    // Consumers can rely on the shape without branching on count.
    if json {
        let payload: Vec<&worktree::WorktreeEntry> = if let Some(t) = target {
            vec![worktree::resolve_target_any(&entries, t)?]
        } else {
            entries.iter().collect()
        };
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        serde_json::to_writer_pretty(&mut handle, &payload).context("serialise worktrees")?;
        writeln!(handle).ok();
        return Ok(());
    }

    if plain {
        for e in &entries {
            let branch = e
                .branch
                .as_deref()
                .map(worktree::short_branch)
                .unwrap_or("(detached)");
            let mut flags: Vec<&str> = Vec::new();
            if e.is_main {
                flags.push("main");
            }
            if e.locked {
                flags.push("locked");
            }
            if e.prunable {
                flags.push("prunable");
            }
            let suffix = if flags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", flags.join(","))
            };
            println!("{}\t{}{}", e.path.display(), branch, suffix);
        }
        return Ok(());
    }

    if let Some(t) = target {
        // `list` is non-destructive, so the main worktree is a valid target.
        let entry = worktree::resolve_target_any(&entries, t)?;
        println!("{}", entry.path.display());
        return Ok(());
    }

    if entries.is_empty() {
        return Ok(());
    }

    let Some(selected) = ui::run_worktree_picker(entries)? else {
        return Ok(());
    };
    println!("{}", selected.path.display());
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
