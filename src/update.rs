use std::fmt;
#[cfg(unix)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::Stdio;

const BUILD_REVISION: &str = env!("NEOLOVE_GIT_REVISION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AvailableUpdate {
    pub current_revision: String,
    pub latest_revision: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    UpToDate,
    Updated {
        revision: String,
    },
    #[cfg(windows)]
    Scheduled {
        revision: String,
    },
}

impl fmt::Display for UpdateOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UpToDate => write!(formatter, "NeoLOVE is already up to date."),
            Self::Updated { revision } => write!(
                formatter,
                "NeoLOVE updated successfully to {}. Restart it to use the new version.",
                short_revision(revision)
            ),
            #[cfg(windows)]
            Self::Scheduled { revision } => write!(
                formatter,
                "NeoLOVE {} was built. The executable will be replaced after this process exits.",
                short_revision(revision)
            ),
        }
    }
}

fn source_root() -> Result<PathBuf, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if !root.join("Cargo.toml").is_file() || !root.join(".git").exists() {
        return Err(format!(
            "the engine source checkout is unavailable at {}; reinstall NeoLOVE from its Git repository",
            root.display()
        ));
    }
    Ok(root)
}

fn command_output(mut command: Command, description: &str) -> Result<String, String> {
    command.env("GIT_TERMINAL_PROMPT", "0");
    let rendered = format!("{command:?}");
    let output = command
        .output()
        .map_err(|error| format!("failed while {description}: {error}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{description} failed with {}: {rendered}\n{}",
            output.status,
            details.trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("{description} returned non-UTF-8 output: {error}"))
}

fn git_output(root: &Path, arguments: &[&str], description: &str) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(arguments);
    command_output(command, description)
}

fn tracking_target(root: &Path) -> Result<(String, String, String), String> {
    let branch = git_output(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "resolving the current branch",
    )?;
    let remote_key = format!("branch.{branch}.remote");
    let merge_key = format!("branch.{branch}.merge");
    let remote = git_output(
        root,
        &["config", "--get", &remote_key],
        "resolving the update remote",
    )?;
    let remote_ref = git_output(
        root,
        &["config", "--get", &merge_key],
        "resolving the upstream branch",
    )?;
    if remote.is_empty() || remote == "." || remote_ref.is_empty() {
        return Err(format!(
            "branch '{branch}' does not track a remote Git branch"
        ));
    }
    Ok((branch, remote, remote_ref))
}

fn parse_ls_remote(output: &str, remote_ref: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (revision, reference) = line.split_once(char::is_whitespace)?;
        (reference.trim() == remote_ref).then(|| revision.trim().to_string())
    })
}

pub(crate) fn check_for_update() -> Result<Option<AvailableUpdate>, String> {
    let root = source_root()?;
    let (branch, remote, remote_ref) = tracking_target(&root)?;
    let output = git_output(
        &root,
        &["ls-remote", "--exit-code", &remote, &remote_ref],
        "checking for NeoLOVE updates",
    )?;
    let latest_revision = parse_ls_remote(&output, &remote_ref)
        .ok_or_else(|| format!("upstream ref '{remote_ref}' was not found on '{remote}'"))?;
    if BUILD_REVISION == "unknown" || latest_revision == BUILD_REVISION {
        return Ok(None);
    }
    Ok(Some(AvailableUpdate {
        current_revision: BUILD_REVISION.to_string(),
        latest_revision,
        branch,
    }))
}

fn run_inherited(command: &mut Command, description: &str) -> Result<(), String> {
    let rendered = format!("{command:?}");
    let status = command
        .status()
        .map_err(|error| format!("failed while {description}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{description} failed with {status}: {rendered}"))
    }
}

fn updated_artifact(root: &Path) -> Result<PathBuf, String> {
    let target_dir = root.join("target").join("neolove-self-update");
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(root)
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--bin")
        .arg("neolove")
        .arg("--target-dir")
        .arg(&target_dir);
    if cfg!(feature = "vulkan") {
        cargo.args(["--features", "vulkan"]);
    }
    run_inherited(&mut cargo, "building the updated NeoLOVE executable")?;
    let executable = if cfg!(windows) {
        "neolove.exe"
    } else {
        "neolove"
    };
    let artifact = target_dir.join("release").join(executable);
    if !artifact.is_file() {
        return Err(format!(
            "the updated executable was not produced at {}",
            artifact.display()
        ));
    }
    Ok(artifact)
}

#[cfg(unix)]
fn replace_executable(artifact: &Path, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension(format!("update-{}", std::process::id()));
    fs::copy(artifact, &temporary).map_err(|error| {
        format!(
            "failed to copy the update to {}: {error}",
            temporary.display()
        )
    })?;
    let permissions = fs::metadata(artifact)
        .map_err(|error| format!("failed to inspect {}: {error}", artifact.display()))?
        .permissions();
    fs::set_permissions(&temporary, permissions)
        .map_err(|error| format!("failed to set update permissions: {error}"))?;
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "failed to replace {}: {error}. Check that the executable is writable",
            destination.display()
        )
    })
}

#[cfg(windows)]
fn powershell_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(windows)]
fn schedule_executable_replacement(artifact: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let source = powershell_literal(artifact);
    let destination = powershell_literal(destination);
    let parent = std::process::id();
    let script = format!(
        "$ErrorActionPreference='Stop'; Wait-Process -Id {parent} -ErrorAction SilentlyContinue; \
         $deadline=(Get-Date).AddMinutes(2); do {{ try {{ Copy-Item -LiteralPath '{source}' \
         -Destination '{destination}' -Force; exit 0 }} catch {{ Start-Sleep -Milliseconds 250 }} }} \
         while ((Get-Date) -lt $deadline); exit 1"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to schedule the executable replacement: {error}"))
}

pub(crate) fn update_engine() -> Result<UpdateOutcome, String> {
    let root = source_root()?;
    let changes = git_output(
        &root,
        &["status", "--porcelain"],
        "checking the engine source checkout",
    )?;
    if !changes.is_empty() {
        return Err(format!(
            "the engine source checkout at {} has local changes; commit or stash them before updating",
            root.display()
        ));
    }
    let _ = tracking_target(&root)?;

    let mut pull = Command::new("git");
    pull.arg("-C")
        .arg(&root)
        .args(["pull", "--ff-only"])
        .env("GIT_TERMINAL_PROMPT", "0");
    run_inherited(&mut pull, "updating the NeoLOVE source checkout")?;

    let revision = git_output(
        &root,
        &["rev-parse", "HEAD"],
        "reading the updated revision",
    )?;
    if revision == BUILD_REVISION {
        return Ok(UpdateOutcome::UpToDate);
    }

    let artifact = updated_artifact(&root)?;
    let destination = std::env::current_exe()
        .map_err(|error| format!("failed to locate the running executable: {error}"))?;

    #[cfg(unix)]
    {
        replace_executable(&artifact, &destination)?;
        Ok(UpdateOutcome::Updated { revision })
    }

    #[cfg(windows)]
    {
        schedule_executable_replacement(&artifact, &destination)?;
        Ok(UpdateOutcome::Scheduled { revision })
    }
}

fn short_revision(revision: &str) -> &str {
    revision.get(..revision.len().min(8)).unwrap_or(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requested_remote_ref() {
        let output = "aaaaaaaa\trefs/heads/main\nbbbbbbbb\trefs/heads/dev\n";
        assert_eq!(
            parse_ls_remote(output, "refs/heads/dev"),
            Some("bbbbbbbb".to_string())
        );
        assert_eq!(parse_ls_remote(output, "refs/heads/missing"), None);
    }

    #[test]
    fn update_messages_use_short_revisions() {
        assert_eq!(
            UpdateOutcome::Updated {
                revision: "1234567890abcdef".to_string()
            }
            .to_string(),
            "NeoLOVE updated successfully to 12345678. Restart it to use the new version."
        );
    }
}
