use crate::paths::PathSandbox;
use crate::protocol::{GitDiffParams, GitDiffResult};
use std::path::Path;
use std::process::Command;

pub const DEFAULT_MAX_DIFF_BYTES: usize = 50_000; // 50 KB
pub const ABSOLUTE_MAX_DIFF_BYTES: usize = 200_000; // 200 KB

#[derive(Debug, Clone)]
pub enum GitError {
    NotGitRepository,
    ExecutionFailed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotGitRepository => write!(f, "Workspace root is not a git repository"),
            Self::ExecutionFailed(msg) => write!(f, "Git execution failed: {msg}"),
        }
    }
}

impl std::error::Error for GitError {}

/// Check if the directory is inside a Git repository.
pub fn is_git_repository(root: &Path) -> bool {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .current_dir(root)
        .output();

    match output {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

/// Run `git diff` inside the workspace root, returning a bounded diff suitable for model review.
pub fn get_git_diff(
    sandbox: &PathSandbox,
    params: &GitDiffParams,
) -> Result<GitDiffResult, GitError> {
    let root = sandbox.canonical_root();

    if !is_git_repository(root) {
        return Ok(GitDiffResult {
            diff: String::new(),
            files_changed: Vec::new(),
            truncated: false,
        });
    }

    let max_bytes = params
        .max_bytes
        .unwrap_or(DEFAULT_MAX_DIFF_BYTES)
        .min(ABSOLUTE_MAX_DIFF_BYTES);

    // 1. Get list of changed files via `git status --porcelain`
    let mut files_changed = Vec::new();
    if let Ok(status_out) = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(root)
        .output()
    {
        if status_out.status.success() {
            let out_str = String::from_utf8_lossy(&status_out.stdout);
            for line in out_str.lines() {
                if line.len() > 3 {
                    let file_name = line[3..].trim().trim_matches('"');
                    files_changed.push(file_name.replace('\\', "/"));
                }
            }
        }
    }

    // 2. Run `git diff HEAD` (captures both staged and unstaged working tree changes)
    let mut cmd = Command::new("git");
    cmd.arg("diff").arg("HEAD").current_dir(root);

    if let Some(target_paths) = &params.paths {
        if !target_paths.is_empty() {
            cmd.arg("--");
            for p in target_paths {
                cmd.arg(p);
            }
        }
    }

    let diff_output = cmd
        .output()
        .map_err(|e| GitError::ExecutionFailed(e.to_string()))?;

    // If `git diff HEAD` fails (e.g. initial commit doesn't exist yet), fallback to plain `git diff`
    let raw_diff = if diff_output.status.success() {
        String::from_utf8_lossy(&diff_output.stdout).to_string()
    } else {
        let fallback = Command::new("git")
            .arg("diff")
            .current_dir(root)
            .output()
            .map_err(|e| GitError::ExecutionFailed(e.to_string()))?;
        String::from_utf8_lossy(&fallback.stdout).to_string()
    };

    let (diff, truncated) = if raw_diff.len() > max_bytes {
        // Truncate safely at line boundary
        let truncated_str: String = raw_diff.chars().take(max_bytes).collect();
        let last_nl = truncated_str.rfind('\n').unwrap_or(truncated_str.len());
        let mut clean_truncated = truncated_str[..last_nl].to_string();
        clean_truncated.push_str("\n\n[Diff truncated: exceeds byte limit]");
        (clean_truncated, true)
    } else {
        (raw_diff, false)
    };

    Ok(GitDiffResult {
        diff,
        files_changed,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_non_git_repo_diff() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();
        let res = get_git_diff(
            &sandbox,
            &GitDiffParams {
                paths: None,
                max_bytes: None,
            },
        )
        .unwrap();
        assert_eq!(res.diff, "");
        assert_eq!(res.files_changed.len(), 0);
        assert!(!res.truncated);
    }
}
