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

#[test]
fn test_sandbox_root_name_alias_resolution() {
    let temp = tempdir().unwrap();
    let root_path = temp.path();
    let root_name = root_path
        .file_name()
        .and_then(|n| n.to_str())
        .expect("tempdir has valid name");

    let sandbox = PathSandbox::new(root_path).unwrap();

    // 1. sandbox.resolve("test") when root name is "test" returns canonical root
    let resolved_root = sandbox.resolve(root_name).unwrap();
    assert_eq!(resolved_root, *sandbox.canonical_root());

    // Leading/trailing slashes or dot-slash
    let resolved_dot = sandbox.resolve(&format!("./{root_name}")).unwrap();
    assert_eq!(resolved_dot, *sandbox.canonical_root());

    let resolved_slash = sandbox.resolve(&format!("{root_name}/")).unwrap();
    assert_eq!(resolved_slash, *sandbox.canonical_root());

    // 2. sandbox.resolve("test/subfile.txt") resolves to canonical_root.join("subfile.txt")
    let subfile = root_path.join("subfile.txt");
    fs::write(&subfile, "content").unwrap();

    let resolved_sub = sandbox.resolve(&format!("{root_name}/subfile.txt")).unwrap();
    assert_eq!(
        resolved_sub,
        dunce::canonicalize(&subfile).unwrap_or(subfile.clone())
    );

    // Backslash variant
    let resolved_bs = sandbox.resolve(&format!(r"{root_name}\subfile.txt")).unwrap();
    assert_eq!(
        resolved_bs,
        dunce::canonicalize(&subfile).unwrap_or(subfile)
    );

    // Resolving a non-existent file for writing with root prefix
    let new_file_rel = format!("{root_name}/new_file.txt");
    let resolved_new = sandbox.resolve(&new_file_rel).unwrap();
    assert_eq!(resolved_new, sandbox.canonical_root().join("new_file.txt"));
}
