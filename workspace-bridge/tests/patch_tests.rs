use std::time::SystemTime;
use tempfile::tempdir;
use webbrain_workspace::files::{atomic_write, read_text_file, LineEnding};
use webbrain_workspace::patch::{apply_patch, PatchError};
use webbrain_workspace::session::WorkspaceSession;

#[test]
fn test_exact_patch_success() {
    let original = "function calculateTotal(items) {\n    let total = 0;\n    return total;\n}\n";
    let res = apply_patch(
        original,
        Some("let total = 0;"),
        Some("let total = 0;\n    for (const item of items) total += item.price;"),
        None,
        "calc.js",
    )
    .unwrap();

    assert!(res.new_content.contains("for (const item of items)"));
    assert_eq!(res.lines_removed, 1);
    assert_eq!(res.lines_added, 2);
    assert!(res.summary.contains("+2 / -1"));
}

#[test]
fn test_exact_patch_ambiguous_context_rejection() {
    let original = "console.log('hi');\nconsole.log('hi');\n";
    let err = apply_patch(
        original,
        Some("console.log('hi');"),
        Some("console.log('bye');"),
        None,
        "test.js",
    )
    .unwrap_err();

    match err {
        PatchError::AmbiguousContext { occurrences, .. } => assert_eq!(occurrences, 2),
        _ => panic!("Expected AmbiguousContext, got {err:?}"),
    }
}

#[test]
fn test_exact_patch_context_not_found() {
    let original = "const a = 1;\n";
    let err = apply_patch(
        original,
        Some("const missing = 2;"),
        Some("const a = 2;"),
        None,
        "test.js",
    )
    .unwrap_err();

    assert!(matches!(err, PatchError::ContextNotFound(_)));
}

#[test]
fn test_unified_diff_patch() {
    let original = "line1\nline2\nline3\nline4\n";
    let patch = "@@ -2,2 +2,2 @@\n-line2\n+line2_modified\n line3\n";
    let res = apply_patch(original, None, None, Some(patch), "test.txt").unwrap();
    assert_eq!(res.new_content, "line1\nline2_modified\nline3\nline4\n");
    assert_eq!(res.lines_added, 1);
    assert_eq!(res.lines_removed, 1);
}

#[test]
fn test_session_revision_tracking_and_conflict() {
    let temp = tempdir().unwrap();
    let session = WorkspaceSession::new(temp.path().to_path_buf(), true, false);
    let mtime = SystemTime::now();

    // 1. Initial read assigns revision 1
    let rev1 = session.record_read("src/app.js", "hash_v1", mtime, 500);
    assert_eq!(rev1, 1);

    // 2. Successful patch with expectedRevision = 1 updates to revision 2
    session.check_revision("src/app.js", 1, Some("hash_v1"), "hash_v1").unwrap();
    let (old_rev, new_rev) = session.commit_revision("src/app.js", "hash_v2", mtime, 520);
    assert_eq!(old_rev, 1);
    assert_eq!(new_rev, 2);

    // 3. Stale edit with expectedRevision = 1 is rejected
    let err = session
        .check_revision("src/app.js", 1, Some("hash_v1"), "hash_v2")
        .unwrap_err();

    match err {
        PatchError::RevisionConflict {
            expected_revision,
            current_revision,
            ..
        } => {
            assert_eq!(expected_revision, 1);
            assert_eq!(current_revision, 2);
        }
        _ => panic!("Expected RevisionConflict, got {err:?}"),
    }
}

#[test]
fn test_patch_atomic_write_preserves_crlf_and_bom() {
    let temp = tempdir().unwrap();
    let file_path = temp.path().join("crlf_bom.txt");

    // Write initial CRLF + BOM file
    atomic_write(
        &file_path,
        "Alpha\r\nBeta\r\n",
        true,
        Some(LineEnding::CrLf),
    )
    .unwrap();

    let initial = read_text_file(&file_path, None).unwrap();
    assert!(initial.has_bom);
    assert_eq!(initial.line_ending, LineEnding::CrLf);

    // Apply patch
    let patched = apply_patch(
        &initial.content,
        Some("Beta"),
        Some("BetaPatched"),
        None,
        "crlf_bom.txt",
    )
    .unwrap();

    // Write back atomically preserving BOM and CRLF
    let new_hash = atomic_write(
        &file_path,
        &patched.new_content,
        initial.has_bom,
        Some(initial.line_ending),
    )
    .unwrap();

    let updated = read_text_file(&file_path, None).unwrap();
    assert!(updated.has_bom);
    assert_eq!(updated.line_ending, LineEnding::CrLf);
    assert_eq!(updated.content, "Alpha\r\nBetaPatched\r\n");
    assert_eq!(updated.hash, new_hash);
}
