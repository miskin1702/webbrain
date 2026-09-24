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

#[test]
fn test_search_code_context_lines_capping_and_boundaries() {
    let temp = tempdir().unwrap();
    let sandbox = PathSandbox::new(temp.path()).unwrap();

    // Create a 25-line file
    let mut file_content = String::new();
    for i in 1..=25 {
        file_content.push_str(&format!("line {}\n", i));
    }
    file_content.push_str("target_marker_func()\n");
    for i in 27..=30 {
        file_content.push_str(&format!("line {}\n", i));
    }

    let file_path = temp.path().join("boundary.rs");
    fs::write(&file_path, &file_content).unwrap();

    // 1. Test request with context_lines: Some(50) (much larger than MAX_CONTEXT_LINES = 10)
    let params_capped = SearchCodeParams {
        query: "target_marker_func".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: Some(50),
    };

    let res_capped = search_code(&sandbox, &params_capped).unwrap();
    assert_eq!(res_capped.matches.len(), 1);
    let snippet_capped = res_capped.matches[0].snippet.as_ref().unwrap();
    let snippet_lines: Vec<&str> = snippet_capped.lines().collect();
    // MAX_CONTEXT_LINES is 10, so max lines = 2 * 10 + 1 = 21 lines max.
    assert!(snippet_lines.len() <= 21);
    assert!(snippet_lines.contains(&"target_marker_func()"));

    // 2. Test start-of-file boundary (match at line 2, context 5)
    let file_path_start = temp.path().join("start.rs");
    fs::write(&file_path_start, "line 1 start\nmatch_start_line\nline 3 after\n").unwrap();
    let params_start = SearchCodeParams {
        query: "match_start_line".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: Some(5),
    };
    let res_start = search_code(&sandbox, &params_start).unwrap();
    assert_eq!(res_start.matches.len(), 1);
    let snippet_start = res_start.matches[0].snippet.as_ref().unwrap();
    let start_lines: Vec<&str> = snippet_start.lines().collect();
    // Should start at line 1 without underflowing or including negative lines
    assert_eq!(start_lines[0], "line 1 start");
    assert_eq!(start_lines[1], "match_start_line");

    // 3. Test end-of-file boundary (match near end of file)
    let file_path_end = temp.path().join("end.rs");
    fs::write(&file_path_end, "line 1 before\nmatch_end_line\nline 3 end\n").unwrap();
    let params_end = SearchCodeParams {
        query: "match_end_line".to_string(),
        limit: Some(10),
        is_regex: Some(false),
        case_sensitive: Some(false),
        include: None,
        exclude: None,
        context_lines: Some(5),
    };
    let res_end = search_code(&sandbox, &params_end).unwrap();
    assert_eq!(res_end.matches.len(), 1);
    let snippet_end = res_end.matches[0].snippet.as_ref().unwrap();
    let end_lines: Vec<&str> = snippet_end.lines().collect();
    // Should terminate at end of file without overflowing
    assert_eq!(end_lines.last().unwrap(), &"line 3 end");
}
