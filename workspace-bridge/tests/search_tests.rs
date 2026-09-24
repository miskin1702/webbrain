use std::fs;
use tempfile::tempdir;
use webbrain_workspace::paths::PathSandbox;
use webbrain_workspace::protocol::SearchCodeParams;
use webbrain_workspace::search::search_code;

#[test]
fn test_search_code_literal_multi_file() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let dir1 = temp.path().join("src");
    let dir2 = temp.path().join("lib");
    fs::create_dir_all(&dir1).unwrap();
    fs::create_dir_all(&dir2).unwrap();

    fs::write(dir1.join("a.js"), "const token = 'SECRET_VAL';\n").unwrap();
    fs::write(
        dir2.join("b.js"),
        "const token = 'SECRET_VAL';\nconst other = 1;\n",
    )
    .unwrap();
    fs::write(dir2.join("c.js"), "const publicInfo = true;\n").unwrap();

    let params = SearchCodeParams {
        query: "SECRET_VAL".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(true),
        include: None,
        exclude: None,
        context_lines: None,
    };

    let res = search_code(&sandbox, &params).unwrap();
    assert_eq!(res.matches.len(), 2);
    assert_eq!(res.total_matches, 2);
    assert!(!res.truncated);
}

#[test]
fn test_search_code_limit_truncation() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let dir = temp.path().join("items");
    fs::create_dir_all(&dir).unwrap();

    for i in 1..=10 {
        fs::write(dir.join(format!("item_{i}.txt")), "target_symbol_to_find\n").unwrap();
    }

    let params = SearchCodeParams {
        query: "target_symbol_to_find".to_string(),
        limit: Some(4),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: None,
    };

    let res = search_code(&sandbox, &params).unwrap();
    assert_eq!(res.matches.len(), 4);
    assert!(res.total_matches >= 4);
    assert!(res.truncated);
}

#[test]
fn test_search_code_gitignore_and_hidden() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    fs::write(temp.path().join(".gitignore"), "ignored_dir/\n*.log\n").unwrap();

    let ignored_dir = temp.path().join("ignored_dir");
    let tracked_dir = temp.path().join("tracked");
    fs::create_dir_all(&ignored_dir).unwrap();
    fs::create_dir_all(&tracked_dir).unwrap();

    fs::write(ignored_dir.join("find_me.txt"), "find_me_needle\n").unwrap();
    fs::write(tracked_dir.join("app.log"), "find_me_needle\n").unwrap();
    fs::write(tracked_dir.join("valid.txt"), "find_me_needle\n").unwrap();

    let params = SearchCodeParams {
        query: "find_me_needle".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: None,
    };

    let res = search_code(&sandbox, &params).unwrap();
    // Only valid.txt should be matched
    assert_eq!(res.matches.len(), 1);
    assert_eq!(res.matches[0].path.replace('\\', "/"), "tracked/valid.txt");
}

#[test]
fn test_search_skips_binary_files() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let bin_path = temp.path().join("binary.dat");
    let text_path = temp.path().join("text.txt");

    // Write binary file with null byte
    let mut bin_data = b"find_marker".to_vec();
    bin_data.push(0x00);
    bin_data.extend_from_slice(b"binary_payload");
    fs::write(&bin_path, bin_data).unwrap();

    fs::write(&text_path, "find_marker in plain text\n").unwrap();

    let params = SearchCodeParams {
        query: "find_marker".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: None,
    };

    let res = search_code(&sandbox, &params).unwrap();
    assert_eq!(res.matches.len(), 1);
    assert_eq!(res.matches[0].path, "text.txt");
    assert!(res.matches[0].snippet.is_none());
}

#[test]
fn test_search_code_with_context_lines() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    let file_path = temp.path().join("code.rs");
    fs::write(
        &file_path,
        "line 1 before\nline 2 before\ntarget_function()\nline 2 after\nline 3 after\n",
    )
    .unwrap();

    let params = SearchCodeParams {
        query: "target_function".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: Some(2),
    };

    let res = search_code(&sandbox, &params).unwrap();
    assert_eq!(res.matches.len(), 1);
    let snippet = res.matches[0].snippet.as_ref().unwrap();
    assert!(snippet.contains("line 1 before"));
    assert!(snippet.contains("line 2 before"));
    assert!(snippet.contains("target_function()"));
    assert!(snippet.contains("line 2 after"));
}
