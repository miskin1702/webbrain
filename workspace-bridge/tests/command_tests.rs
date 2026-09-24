use tempfile::tempdir;
use webbrain_workspace::command::{run_command, CommandError};
use webbrain_workspace::paths::PathSandbox;
use webbrain_workspace::protocol::RunCommandParams;

#[tokio::test]
async fn test_command_execution_disabled_by_default() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let params = RunCommandParams {
        command: "node".to_string(),
        args: Some(vec!["--version".to_string()]),
        timeout_ms: Some(5000),
    };

    let err = run_command(&sandbox, &params, false).await.unwrap_err();
    assert!(matches!(err, CommandError::NotAllowed));
}

#[tokio::test]
async fn test_command_execution_runs_in_workspace_root() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    // Create a marker file in workspace root
    std::fs::write(temp.path().join("marker.txt"), "found_marker").unwrap();

    #[cfg(windows)]
    let (cmd, args) = (
        "cmd.exe",
        vec![
            "/c".to_string(),
            "type".to_string(),
            "marker.txt".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = ("cat", vec!["marker.txt".to_string()]);

    let params = RunCommandParams {
        command: cmd.to_string(),
        args: Some(args),
        timeout_ms: Some(5000),
    };

    let res = run_command(&sandbox, &params, true).await.unwrap();
    assert_eq!(res.exit_code, Some(0));
    assert!(res.stdout.contains("found_marker"));
    assert!(!res.timed_out);
}

#[tokio::test]
async fn test_command_timeout_and_process_cleanup() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    // Run a command that takes longer than the timeout
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-Command".to_string(),
            "Start-Sleep -Seconds 10".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = ("sleep", vec!["10".to_string()]);

    let params = RunCommandParams {
        command: cmd.to_string(),
        args: Some(args),
        timeout_ms: Some(500), // 500 ms timeout
    };

    let res = run_command(&sandbox, &params, true).await.unwrap();
    assert!(res.timed_out);
    assert_eq!(res.exit_code, None);
    assert!(res.stderr.contains("timed out"));
}

#[tokio::test]
async fn test_command_execution_shell_builtin_dir() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    std::fs::write(temp.path().join("shell_marker.txt"), "shell_content").unwrap();

    #[cfg(windows)]
    let command = "dir".to_string();
    #[cfg(not(windows))]
    let command = "ls".to_string();

    let params = RunCommandParams {
        command,
        args: None,
        timeout_ms: Some(5000),
    };

    let res = run_command(&sandbox, &params, true).await.unwrap();
    assert_eq!(res.exit_code, Some(0));
    assert!(res.stdout.contains("shell_marker.txt"));
    assert!(!res.timed_out);
}

#[tokio::test]
async fn test_command_execution_cmd_c_dir() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    std::fs::write(temp.path().join("cmd_marker.txt"), "cmd_content").unwrap();

    #[cfg(windows)]
    let command = "cmd /c dir".to_string();
    #[cfg(not(windows))]
    let command = "sh -c ls".to_string();

    let params = RunCommandParams {
        command,
        args: None,
        timeout_ms: Some(5000),
    };

    let res = run_command(&sandbox, &params, true).await.unwrap();
    assert_eq!(res.exit_code, Some(0));
    assert!(res.stdout.contains("cmd_marker.txt"));
    assert!(!res.timed_out);
}
