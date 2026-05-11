use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

fn fake_bin(temp: &TempDir, name: &str, body: &str) -> PathBuf {
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    std::fs::write(&path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    bin_dir
}

fn isolated_cmd(home: &Path, extra_bin: Option<&Path>) -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("tmx").unwrap();
    cmd.env("HOME", home);
    cmd.env_remove("TMUX");
    cmd.env_remove("XDG_CONFIG_HOME");
    if let Some(bin) = extra_bin {
        let orig = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{}", bin.display(), orig));
    }
    cmd
}

fn init_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("invoke git")
    };
    assert!(run(&["init", "-q", "-b", "main"]).success());
    assert!(run(&["config", "user.email", "test@example.com"]).success());
    assert!(run(&["config", "user.name", "Test"]).success());
    assert!(run(&["commit", "--allow-empty", "-m", "init"]).success());
}

#[test]
fn version_flag_prints_version() {
    let temp = TempDir::new().unwrap();
    isolated_cmd(temp.path(), None)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("tmx"));
}

#[test]
fn help_flag_lists_subcommands() {
    let temp = TempDir::new().unwrap();
    isolated_cmd(temp.path(), None)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("cleanup"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("switch"))
        .stdout(predicate::str::contains("worktree"))
        .stdout(predicate::str::contains("shell-init"));
}

#[test]
fn shell_init_zsh_emits_twt_function() {
    let temp = TempDir::new().unwrap();
    isolated_cmd(temp.path(), None)
        .args(["shell-init", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("twt()"))
        .stdout(predicate::str::contains("tmx worktree"));
}

#[test]
fn shell_init_unknown_shell_fails_with_message() {
    let temp = TempDir::new().unwrap();
    isolated_cmd(temp.path(), None)
        .args(["shell-init", "fish"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported shell"))
        .stderr(predicate::str::contains("fish"));
}

#[test]
fn shell_init_missing_argument_fails() {
    let temp = TempDir::new().unwrap();
    isolated_cmd(temp.path(), None)
        .arg("shell-init")
        .assert()
        .failure();
}

#[test]
fn list_without_tmux_server_prints_message() {
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(&temp, "tmux", "#!/usr/bin/env bash\nexit 1\n");
    isolated_cmd(temp.path(), Some(&bin))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No tmux server running"));
}

#[test]
fn list_with_empty_session_list_prints_message() {
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(&temp, "tmux", "#!/usr/bin/env bash\nexit 0\n");
    isolated_cmd(temp.path(), Some(&bin))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No active sessions"));
}

#[test]
fn list_renders_sessions_from_tmux_output() {
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(
        &temp,
        "tmux",
        "#!/usr/bin/env bash\nprintf 'main:3:1\\nside:1:0\\n'\n",
    );
    isolated_cmd(temp.path(), Some(&bin))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("3 window(s)"))
        .stdout(predicate::str::contains("(attached)"))
        .stdout(predicate::str::contains("side"));
}

#[test]
fn cleanup_with_no_sessions_prints_message() {
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(&temp, "tmux", "#!/usr/bin/env bash\nexit 0\n");
    isolated_cmd(temp.path(), Some(&bin))
        .arg("cleanup")
        .assert()
        .success()
        .stdout(predicate::str::contains("No unattached sessions"));
}

#[test]
fn cleanup_kills_unattached_sessions() {
    let temp = TempDir::new().unwrap();
    let killed_log = temp.path().join("killed.txt");
    // Fake tmux that lists 2 unattached + 1 attached and records kill-session args.
    let script = format!(
        "#!/usr/bin/env bash
case \"$1\" in
  list-sessions)
    printf 'a:1:0\\nb:1:1\\nc:1:0\\n'
    ;;
  kill-session)
    shift; printf '%s\\n' \"$@\" >> {log}
    ;;
  *)
    exit 0
    ;;
esac
",
        log = killed_log.to_string_lossy()
    );
    let bin = fake_bin(&temp, "tmux", &script);

    isolated_cmd(temp.path(), Some(&bin))
        .arg("cleanup")
        .assert()
        .success()
        .stdout(predicate::str::contains("Killed 2 session(s)"))
        .stdout(predicate::str::contains("a"))
        .stdout(predicate::str::contains("c"));

    let log = std::fs::read_to_string(&killed_log).unwrap();
    assert!(log.contains("a"));
    assert!(log.contains("c"));
    assert!(!log.contains("\nb\n"));
}

#[test]
fn worktree_outside_git_repo_fails() {
    let temp = TempDir::new().unwrap();
    let cwd = temp.path().join("plain");
    std::fs::create_dir_all(&cwd).unwrap();
    isolated_cmd(temp.path(), None)
        .current_dir(&cwd)
        .args(["worktree", "foo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a git repository"));
}

#[test]
fn worktree_creates_sibling_and_prints_path_to_stdout() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("myproj");
    init_repo(&repo);

    let assert = isolated_cmd(temp.path(), None)
        .current_dir(&repo)
        .args(["worktree", "feat/x"])
        .assert()
        .success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let expected = temp.path().join("myproj-feat-x");
    let stdout_trimmed = stdout.trim();
    assert_eq!(
        stdout_trimmed,
        expected.to_string_lossy().trim_end_matches('/')
    );
    assert!(
        expected.exists(),
        "expected worktree at {}",
        expected.display()
    );
    assert!(stderr.contains("Created worktree"));
}

#[test]
fn worktree_target_exists_fails() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("myproj");
    init_repo(&repo);
    std::fs::create_dir(temp.path().join("myproj-feat")).unwrap();

    isolated_cmd(temp.path(), None)
        .current_dir(&repo)
        .args(["worktree", "feat"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("target path already exists"));
}

#[test]
fn worktree_respects_naming_from_config() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("myproj");
    init_repo(&repo);

    let cfg_dir = temp.path().join(".config").join("tmx");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        "[worktree]\nnaming = \"wt_{repo}_{branch}\"\n",
    )
    .unwrap();

    let assert = isolated_cmd(temp.path(), None)
        .current_dir(&repo)
        .args(["worktree", "feat/x"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let expected = temp.path().join("wt_myproj_feat-x");
    assert_eq!(
        stdout.trim(),
        expected.to_string_lossy().trim_end_matches('/')
    );
    assert!(expected.exists());
}

#[test]
fn worktree_runs_post_create_script_when_configured() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("myproj");
    init_repo(&repo);

    let marker = temp.path().join("ran.txt");
    let script_path = repo.join("setup.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\necho ok > {}\n",
            marker.to_string_lossy()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let cfg_dir = temp.path().join(".config").join("tmx");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        format!(
            "[worktree]\nnaming = \"{{repo}}-{{branch}}\"\n\n[worktree.scripts]\n\"{}\" = \"{}\"\n",
            repo.to_string_lossy(),
            script_path.to_string_lossy()
        ),
    )
    .unwrap();

    isolated_cmd(temp.path(), None)
        .current_dir(&repo)
        .args(["worktree", "feat"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Ran post-create script"));

    assert!(
        marker.exists(),
        "post-create marker {} should exist",
        marker.display()
    );
}

#[test]
fn selector_with_empty_roots_prints_help_message() {
    let temp = TempDir::new().unwrap();
    let cfg_dir = temp.path().join(".config").join("tmx");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.toml"), "roots = []\n").unwrap();

    isolated_cmd(temp.path(), None)
        .assert()
        .success()
        .stdout(predicate::str::contains("No projects found"))
        .stdout(predicate::str::contains("config.toml"));
}
