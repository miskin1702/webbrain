use crate::paths::PathSandbox;
use crate::protocol::{RunCommandParams, RunCommandResult};
use std::collections::VecDeque;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[cfg(windows)]
use std::path::{Path, PathBuf};

pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000; // 30 seconds
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 300_000; // 5 minutes
pub const MAX_OUTPUT_BYTES: usize = 50_000; // 50 KB per stream
const OUTPUT_HEAD_BYTES: usize = 10_000;
const OUTPUT_TAIL_BYTES: usize = MAX_OUTPUT_BYTES - OUTPUT_HEAD_BYTES;

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

#[derive(Debug)]
struct BoundedOutput {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

impl BoundedOutput {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(OUTPUT_HEAD_BYTES),
            tail: VecDeque::with_capacity(OUTPUT_TAIL_BYTES),
            total_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());

        let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(self.head.len());
        let head_len = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_len]);

        let remaining = &bytes[head_len..];
        if remaining.len() >= OUTPUT_TAIL_BYTES {
            self.tail.clear();
            self.tail
                .extend(&remaining[remaining.len() - OUTPUT_TAIL_BYTES..]);
            return;
        }

        let overflow = self
            .tail
            .len()
            .saturating_add(remaining.len())
            .saturating_sub(OUTPUT_TAIL_BYTES);
        if overflow > 0 {
            self.tail.drain(..overflow);
        }
        self.tail.extend(remaining);
    }
}

async fn collect_output<R>(mut pipe: Option<R>) -> BoundedOutput
where
    R: AsyncRead + Unpin,
{
    let mut output = BoundedOutput::new();
    if let Some(pipe) = &mut pipe {
        let mut buf = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut buf).await {
            if n == 0 {
                break;
            }
            output.push(&buf[..n]);
        }
    }
    output
}

#[cfg(windows)]
fn resolve_windows_command(command: &str, working_dir: &Path) -> PathBuf {
    let input = Path::new(command);
    if input.extension().is_some() {
        return input.to_path_buf();
    }

    if input.components().count() > 1 {
        let base = if input.is_absolute() {
            input.to_path_buf()
        } else {
            working_dir.join(input)
        };
        for extension in ["exe", "com", "cmd", "bat"] {
            let candidate = base.with_extension(extension);
            if candidate.is_file() {
                return candidate;
            }
        }
        return input.to_path_buf();
    }

    let mut search_dirs = vec![working_dir.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        search_dirs.extend(std::env::split_paths(&path));
    }

    for directory in search_dirs {
        for extension in ["exe", "com", "cmd", "bat"] {
            let candidate = directory.join(command).with_extension(extension);
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    input.to_path_buf()
}

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
    let start_time = Instant::now();

    let mut cmd = match &params.args {
        Some(args) if !args.is_empty() => {
            #[cfg(windows)]
            let program = resolve_windows_command(&params.command, root);
            #[cfg(not(windows))]
            let program = &params.command;

            let mut c = Command::new(program);
            c.args(args);
            c
        }
        _ => {
            #[cfg(windows)]
            {
                let mut c = Command::new("cmd.exe");
                c.args(["/d", "/s", "/c", &params.command]);
                c
            }
            #[cfg(not(windows))]
            {
                let mut c = Command::new("sh");
                c.args(["-c", &params.command]);
                c
            }
        }
    };
    cmd.current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Spawn child
    let mut child = cmd
        .spawn()
        .map_err(|e| CommandError::SpawnFailed(format!("{}: {e}", params.command)))?;

    let child_pid = child.id();

    // Read stdout and stderr asynchronously while retaining bounded head/tail output.
    let stdout_reader = collect_output(child.stdout.take());
    let stderr_reader = collect_output(child.stderr.take());

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

fn format_output(mut output: BoundedOutput) -> (String, bool) {
    let truncated = output.total_bytes > MAX_OUTPUT_BYTES;
    if !truncated {
        output.head.extend(output.tail);
        return (String::from_utf8_lossy(&output.head).to_string(), false);
    }

    let retained_bytes = output.head.len() + output.tail.len();
    let omitted_bytes = output.total_bytes.saturating_sub(retained_bytes);
    let head = String::from_utf8_lossy(&output.head);
    let tail: Vec<u8> = output.tail.into_iter().collect();
    let tail = String::from_utf8_lossy(&tail);
    (
        format!("{head}\n\n[Output truncated: omitted {omitted_bytes} bytes]\n\n{tail}"),
        true,
    )
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

    #[tokio::test]
    async fn test_command_stdin_is_closed() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        #[cfg(windows)]
        let (cmd, args) = (
            "powershell.exe",
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "[Console]::In.ReadToEnd() | Out-Null; Write-Output 'stdin-closed'".to_string(),
            ],
        );
        #[cfg(not(windows))]
        let (cmd, args) = (
            "sh",
            vec![
                "-c".to_string(),
                "cat >/dev/null; printf stdin-closed".to_string(),
            ],
        );

        let params = RunCommandParams {
            command: cmd.to_string(),
            args: Some(args),
            timeout_ms: Some(5000),
        };

        let res = run_command(&sandbox, &params, true).await.unwrap();
        assert_eq!(res.exit_code, Some(0));
        assert!(!res.timed_out);
        assert!(res.stdout.contains("stdin-closed"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_command_resolves_cmd_tool_without_shell_concatenation() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("echo-tool.cmd"),
            "@echo off\r\necho %~1\r\n",
        )
        .unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();
        let params = RunCommandParams {
            command: "echo-tool".to_string(),
            args: Some(vec!["hello world".to_string()]),
            timeout_ms: Some(5000),
        };

        let res = run_command(&sandbox, &params, true).await.unwrap();
        assert_eq!(res.exit_code, Some(0));
        assert_eq!(res.stdout.trim(), "hello world");
    }

    #[test]
    fn test_bounded_output_preserves_tail_and_signals_truncation() {
        let mut output = BoundedOutput::new();
        let mut bytes = vec![b'h'; MAX_OUTPUT_BYTES + 1234];
        bytes[..4].copy_from_slice(b"HEAD");
        let end = bytes.len();
        bytes[end - 4..].copy_from_slice(b"TAIL");
        for chunk in bytes.chunks(8192) {
            output.push(chunk);
        }

        let (formatted, truncated) = format_output(output);
        assert!(truncated);
        assert!(formatted.starts_with("HEAD"));
        assert!(formatted.ends_with("TAIL"));
        assert!(formatted.contains("[Output truncated: omitted 1234 bytes]"));
    }

    #[test]
    fn test_bounded_output_exact_limit_is_not_truncated() {
        let mut output = BoundedOutput::new();
        output.push(&vec![b'x'; MAX_OUTPUT_BYTES]);

        let (formatted, truncated) = format_output(output);
        assert!(!truncated);
        assert_eq!(formatted.len(), MAX_OUTPUT_BYTES);
    }
}
