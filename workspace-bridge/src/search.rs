use crate::paths::PathSandbox;
use crate::protocol::{SearchCodeParams, SearchCodeResult, SearchMatch};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub const DEFAULT_SEARCH_LIMIT: usize = 50;
pub const MAX_SEARCH_LIMIT: usize = 200;
pub const MAX_LINE_CHARS: usize = 250;
pub const MAX_SEARCH_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MB

#[derive(Debug, Clone)]
pub enum SearchError {
    InvalidRegex(String),
    Io(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRegex(msg) => write!(f, "Invalid regular expression: {msg}"),
            Self::Io(msg) => write!(f, "I/O error during search: {msg}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Search codebase under sandbox root according to params.
pub fn search_code(
    sandbox: &PathSandbox,
    params: &SearchCodeParams,
) -> Result<SearchCodeResult, SearchError> {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .min(MAX_SEARCH_LIMIT);
    let case_sensitive = params.case_sensitive.unwrap_or(false);
    let is_regex = params.is_regex.unwrap_or(false);

    // Prepare search matcher
    let regex_matcher = if is_regex {
        let re = RegexBuilder::new(&params.query)
            .case_insensitive(!case_sensitive)
            .multi_line(false)
            .dot_matches_new_line(false)
            .build()
            .map_err(|e| SearchError::InvalidRegex(e.to_string()))?;
        Some(re)
    } else {
        None
    };

    let query_lower = if !case_sensitive && !is_regex {
        params.query.to_lowercase()
    } else {
        params.query.clone()
    };

    // Configure ignore walker
    let root = sandbox.canonical_root();
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true) // Obey .gitignore, .ignore, etc.
        .require_git(false) // Obey gitignore even in standalone directories without .git
        .hidden(true) // Skip hidden files/dirs by default (.git, etc.)
        .max_filesize(Some(MAX_SEARCH_FILE_SIZE));

    // Handle include/exclude glob overrides
    let mut overrides = OverrideBuilder::new(root);
    if let Some(includes) = &params.include {
        for inc in includes {
            let glob_pat = if inc.starts_with('*') {
                inc.clone()
            } else {
                format!("*{inc}")
            };
            let _ = overrides.add(&glob_pat);
        }
    }
    if let Some(excludes) = &params.exclude {
        for exc in excludes {
            let glob_pat = if exc.starts_with('!') {
                exc.clone()
            } else {
                format!("!{exc}")
            };
            let _ = overrides.add(&glob_pat);
        }
    }
    if let Ok(built_overrides) = overrides.build() {
        builder.overrides(built_overrides);
    }

    let walker = builder.build();
    let mut matches = Vec::new();
    let mut total_matches = 0;
    let mut truncated = false;

    for entry in walker {
        let dir_entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let file_type = match dir_entry.file_type() {
            Some(ft) => ft,
            None => continue,
        };

        if !file_type.is_file() {
            continue;
        }

        let path = dir_entry.path();

        // Quick binary heuristic: check first 512 bytes for null bytes
        if is_likely_binary(path) {
            continue;
        }

        let rel_path = match sandbox.to_relative(path) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let reader = BufReader::new(file);
        let mut lines = Vec::new();
        for line_res in reader.lines() {
            match line_res {
                Ok(l) => lines.push(l),
                Err(_) => break,
            }
        }

        for (line_idx, line) in lines.iter().enumerate() {
            let is_match = if let Some(re) = &regex_matcher {
                re.is_match(line)
            } else if !case_sensitive {
                line.to_lowercase().contains(&query_lower)
            } else {
                line.contains(&query_lower)
            };

            if is_match {
                total_matches += 1;

                if matches.len() < limit {
                    let bounded_line = if line.chars().count() > MAX_LINE_CHARS {
                        format!(
                            "{}...",
                            line.chars().take(MAX_LINE_CHARS).collect::<String>()
                        )
                    } else {
                        line.clone()
                    };

                    let snippet = if let Some(c) = params.context_lines {
                        if c > 0 {
                            let start = line_idx.saturating_sub(c);
                            let end = std::cmp::min(lines.len() - 1, line_idx + c);
                            let snippet_lines: Vec<String> = lines[start..=end]
                                .iter()
                                .map(|l| {
                                    if l.chars().count() > MAX_LINE_CHARS {
                                        format!(
                                            "{}...",
                                            l.chars().take(MAX_LINE_CHARS).collect::<String>()
                                        )
                                    } else {
                                        l.clone()
                                    }
                                })
                                .collect();
                            Some(snippet_lines.join("\n"))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    matches.push(SearchMatch {
                        path: rel_path.clone(),
                        line: line_idx + 1, // 1-based line number
                        content: bounded_line,
                        snippet,
                    });
                } else {
                    truncated = true;
                }

                // If we've collected enough matches, we can stop walking
                if matches.len() >= limit && total_matches > limit + 50 {
                    break;
                }
            }
        }

        if truncated && total_matches >= limit + 100 {
            break;
        }
    }

    Ok(SearchCodeResult {
        matches,
        truncated,
        total_matches,
    })
}

fn is_likely_binary(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return true,
    };

    let mut buf = [0u8; 512];
    match file.read(&mut buf) {
        Ok(n) if n > 0 => buf[..n].contains(&0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_search_literal_and_case_insensitivity() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let src_dir = temp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            src_dir.join("main.rs"),
            "fn executeTool() {}\nfn other() {}\n",
        )
        .unwrap();
        fs::write(src_dir.join("test.rs"), "fn EXECUTETOOL_helper() {}\n").unwrap();

        let params = SearchCodeParams {
            query: "executetool".to_string(),
            limit: Some(10),
            is_regex: Some(false),
            case_sensitive: Some(false),
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
    fn test_search_regex() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let src_dir = temp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            src_dir.join("code.rs"),
            "let alpha = 42;\nlet beta = 100;\nlet gamma = 200;\n",
        )
        .unwrap();

        let params = SearchCodeParams {
            query: r"let (alpha|beta) = \d+;".to_string(),
            limit: Some(10),
            is_regex: Some(true),
            case_sensitive: Some(true),
            include: None,
            exclude: None,
            context_lines: None,
        };

        let res = search_code(&sandbox, &params).unwrap();
        assert_eq!(res.matches.len(), 2);
    }

    #[test]
    fn test_search_gitignore_respected() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let target_dir = temp.path().join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("hidden.rs"), "fn secretToken() {}\n").unwrap();
        fs::write(temp.path().join(".gitignore"), "target/\n").unwrap();

        let params = SearchCodeParams {
            query: "secretToken".to_string(),
            limit: Some(10),
            is_regex: Some(false),
            case_sensitive: Some(false),
            include: None,
            exclude: None,
            context_lines: None,
        };

        let res = search_code(&sandbox, &params).unwrap();
        assert_eq!(res.matches.len(), 0);
    }
}
