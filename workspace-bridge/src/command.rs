use crate::paths::PathSandbox;
use crate::protocol::{RunCommandParams, RunCommandResult};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000; // 30 seconds
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 300_000; // 5 minutes
pub const MAX_OUTPUT_BYTES: usize = 50_000; // 50 KB per stream

#[derive(Debug, Clone)]
pub enum CommandError {
    NotAllowed,
    SpawnFailed(String),
    ExecutionFailed(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAllowed => write!(
                f,
                "Command execution is disabled. Enable --allow-command in daemon flags or Settings."
            ),
            Self::SpawnFailed(msg) => write!(f, "Failed to spawn command: {msg}"),
            Self::ExecutionFailed(msg) => write!(f, "Command execution error: {msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Run a command inside the sandbox root with timeout and output bounds.
pub async fn run_command(
    sandbox: &PathSandbox,
    params: &RunCommandParams,
    allow_command: bool,
) -> Result<RunCommandResult, CommandError> {
    if !allow_command {
        return Err(CommandError::NotAllowed);
    }

    let timeout_ms = params
        .timeout_ms
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS)
        .min(MAX_COMMAND_TIMEOUT_MS);
    let timeout_duration = Duration::from_millis(timeout_ms);

    let root = sandbox.canonical_root();
    let args = params.args.clone().unwrap_or_default();

    let start_time = Instant::now();

    let mut cmd = Command::new(&params.command);
    cmd.args(&args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Spawn child
    let mut child = cmd
        .spawn()
        .map_err(|e| CommandError::SpawnFailed(format!("{}: {e}", params.command)))?;

    let child_pid = child.id();

    // Read stdout and stderr asynchronously
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_reader = async {
        let mut out = Vec::new();
        if let Some(ref mut pipe) = stdout_handle {
            let mut buf = [0u8; 1024];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if out.len() < MAX_OUTPUT_BYTES {
                    out.extend_from_slice(&buf[..n]);
                }
            }
        }
        out
    };

    let stderr_reader = async {
        let mut err = Vec::new();
        if let Some(ref mut pipe) = stderr_handle {
            let mut buf = [0u8; 1024];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if err.len() < MAX_OUTPUT_BYTES {
                    err.extend_from_slice(&buf[..n]);
                }
            }
        }
        err
    };

    // Wait for command completion or timeout
    let execution_result = tokio::time::timeout(timeout_duration, async {
        let (status_res, stdout_bytes, stderr_bytes) =
            tokio::join!(child.wait(), stdout_reader, stderr_reader);
        (status_res, stdout_bytes, stderr_bytes)
    })
    .await;

    let duration_ms = start_time.elapsed().as_millis() as u64;

    match execution_result {
        Ok((status_res, stdout_bytes, stderr_bytes)) => {
            let status = status_res.map_err(|e| CommandError::ExecutionFailed(e.to_string()))?;
            let (stdout, stdout_trunc) = format_output(stdout_bytes);
            let (stderr, stderr_trunc) = format_output(stderr_bytes);

            Ok(RunCommandResult {
                exit_code: status.code(),
                stdout,
                stderr,
                duration_ms,
                timed_out: false,
                truncated: stdout_trunc || stderr_trunc,
            })
        }
        Err(_) => {
            // Timed out: kill child process tree on Windows
            if let Some(pid) = child_pid {
                kill_process_tree(pid);
            }
            let _ = child.kill().await;

            Ok(RunCommandResult {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Command timed out after {timeout_ms} ms"),
                duration_ms,
                timed_out: true,
                truncated: false,
            })
        }
    }
}

fn format_output(bytes: Vec<u8>) -> (String, bool) {
    let truncated = bytes.len() >= MAX_OUTPUT_BYTES;
    let s = String::from_utf8_lossy(&bytes).to_string();
    if truncated {
        (
            format!("{s}\n\n[Output truncated: exceeded size limit]"),
            true,
        )
    } else {
        (s, false)
    }
}

/// Terminate the process tree on Windows to prevent orphaned child processes.
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(not(windows))]
    {
        // On Unix, kill process group or PID
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_command_permission_denied() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();
        let params = RunCommandParams {
            command: "echo".to_string(),
            args: Some(vec!["hello".to_string()]),
            timeout_ms: Some(5000),
        };

        let err = run_command(&sandbox, &params, false).await.unwrap_err();
        assert!(matches!(err, CommandError::NotAllowed));
    }

    #[tokio::test]
    async fn test_command_execution_success() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        #[cfg(windows)]
        let (cmd, args) = (
            "cmd.exe",
            vec!["/c".to_string(), "echo".to_string(), "hello".to_string()],
        );
        #[cfg(not(windows))]
        let (cmd, args) = ("echo", vec!["hello".to_string()]);

        let params = RunCommandParams {
            command: cmd.to_string(),
            args: Some(args),
            timeout_ms: Some(5000),
        };

        let res = run_command(&sandbox, &params, true).await.unwrap();
        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.contains("hello"));
        assert!(!res.timed_out);
    }
}
