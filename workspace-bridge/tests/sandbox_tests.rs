use std::fs;
use tempfile::tempdir;
use webbrain_workspace::paths::{is_subpath, PathSandbox, SandboxError};

#[test]
fn test_sandbox_normal_paths() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let resolved = sandbox.resolve("src/index.js").unwrap();
    assert!(is_subpath(&resolved, sandbox.canonical_root()));
    assert!(resolved.ends_with("src\\index.js") || resolved.ends_with("src/index.js"));

    // Subdirectory resolution
    let sub = sandbox.resolve("deep/nested/folder/file.txt").unwrap();
    assert!(is_subpath(&sub, sandbox.canonical_root()));
}

#[test]
fn test_sandbox_traversal_rejection() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    // Direct parent traversal
    let err1 = sandbox.resolve("../secret.txt").unwrap_err();
    assert!(matches!(err1, SandboxError::TraversalNotAllowed(_)));

    // Obfuscated traversal
    let err2 = sandbox.resolve("sub/dir/../../../secret.txt").unwrap_err();
    assert!(matches!(err2, SandboxError::TraversalNotAllowed(_)));

    // Windows backslash traversal
    let err3 = sandbox.resolve(r"..\secret.txt").unwrap_err();
    assert!(matches!(err3, SandboxError::TraversalNotAllowed(_)));
}

#[test]
fn test_sandbox_absolute_path_escape_rejection() {
    let temp1 = tempdir().unwrap();
    let temp2 = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp1.path()).unwrap();

    // Absolute path pointing to outside directory
    let outside_file = temp2.path().join("outside.txt");
    fs::write(&outside_file, "forbidden").unwrap();

    let err = sandbox
        .resolve(&outside_file.to_string_lossy())
        .unwrap_err();
    assert!(matches!(err, SandboxError::PathOutsideWorkspace(_)));
}

#[test]
fn test_sandbox_reserved_device_names() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let reserved = ["con", "prn", "aux", "nul", "com1", "com9", "lpt1", "lpt9"];
    for name in &reserved {
        let err = sandbox.resolve(name).unwrap_err();
        assert!(
            matches!(err, SandboxError::ReservedDeviceName(_)),
            "Failed for {name}"
        );

        let with_ext = format!("{name}.txt");
        let err2 = sandbox.resolve(&with_ext).unwrap_err();
        assert!(
            matches!(err2, SandboxError::ReservedDeviceName(_)),
            "Failed for {with_ext}"
        );

        let in_dir = format!("sub/{name}.js");
        let err3 = sandbox.resolve(&in_dir).unwrap_err();
        assert!(
            matches!(err3, SandboxError::ReservedDeviceName(_)),
            "Failed for {in_dir}"
        );
    }
}

#[test]
fn test_sandbox_symlink_escape_rejection() {
    let inside_temp = tempdir().unwrap();
    let outside_temp = tempdir().unwrap();

    let outside_file = outside_temp.path().join("target.txt");
    fs::write(&outside_file, "secret outside content").unwrap();

    let sandbox = PathSandbox::new(inside_temp.path()).unwrap();
    let link_path = inside_temp.path().join("escape_link.txt");

    // Try creating a symlink inside pointing outside
    #[cfg(windows)]
    let symlink_created = std::os::windows::fs::symlink_file(&outside_file, &link_path).is_ok();
    #[cfg(unix)]
    let symlink_created = std::os::unix::fs::symlink(&outside_file, &link_path).is_ok();

    if symlink_created {
        // Attempting to resolve the symlink must be detected as escaping the sandbox
        let err = sandbox.resolve("escape_link.txt").unwrap_err();
        assert!(matches!(err, SandboxError::PathOutsideWorkspace(_)));
    }
}
