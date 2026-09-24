#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    RevisionConflict {
        expected_revision: u64,
        current_revision: u64,
        expected_hash: Option<String>,
        current_hash: String,
    },
    AmbiguousContext {
        occurrences: usize,
        message: String,
    },
    ContextNotFound(String),
    MalformedPatch(String),
    Io(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionConflict {
                expected_revision,
                current_revision,
                ..
            } => {
                write!(
                    f,
                    "Revision conflict: expected revision {expected_revision}, but current revision is {current_revision}. File was modified externally."
                )
            }
            Self::AmbiguousContext {
                occurrences,
                message,
            } => {
                write!(
                    f,
                    "Ambiguous patch context ({occurrences} matches found): {message}. Provide more surrounding context."
                )
            }
            Self::ContextNotFound(msg) => write!(f, "Patch context not found: {msg}"),
            Self::MalformedPatch(msg) => write!(f, "Malformed patch format: {msg}"),
            Self::Io(msg) => write!(f, "I/O error during patch: {msg}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Result of a patch application
#[derive(Debug, Clone)]
pub struct PatchResult {
    pub new_content: String,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub summary: String,
}

/// Apply targeted patch using either exact old_text/new_text replacement or unified diff.
pub fn apply_patch(
    current_content: &str,
    old_text: Option<&str>,
    new_text: Option<&str>,
    unified_patch: Option<&str>,
    path_display: &str,
) -> Result<PatchResult, PatchError> {
    // Mode 1: Exact old_text -> new_text replacement
    if let (Some(old_t), Some(new_t)) = (old_text, new_text) {
        return apply_exact_replacement(current_content, old_t, new_t, path_display);
    }

    // Mode 2: Unified diff
    if let Some(patch_str) = unified_patch {
        return apply_unified_diff(current_content, patch_str, path_display);
    }

    Err(PatchError::MalformedPatch(
        "Either (oldText, newText) or patch must be provided".to_string(),
    ))
}

/// Exact search-and-replace with uniqueness check.
fn apply_exact_replacement(
    current_content: &str,
    old_text: &str,
    new_text: &str,
    path_display: &str,
) -> Result<PatchResult, PatchError> {
    if old_text.is_empty() {
        return Err(PatchError::MalformedPatch(
            "oldText cannot be empty".to_string(),
        ));
    }

    let matches: Vec<_> = current_content.match_indices(old_text).collect();

    if matches.is_empty() {
        // Try normalized line ending match
        let norm_current = current_content.replace("\r\n", "\n");
        let norm_old = old_text.replace("\r\n", "\n");
        let norm_matches: Vec<_> = norm_current.match_indices(&norm_old).collect();

        if norm_matches.is_empty() {
            return Err(PatchError::ContextNotFound(
                "Specified oldText was not found in the file".to_string(),
            ));
        }

        if norm_matches.len() > 1 {
            return Err(PatchError::AmbiguousContext {
                occurrences: norm_matches.len(),
                message: "oldText matches multiple locations".to_string(),
            });
        }

        let norm_new = new_text.replace("\r\n", "\n");
        let (idx, _) = norm_matches[0];
        let mut new_content = String::with_capacity(norm_current.len() + norm_new.len());
        new_content.push_str(&norm_current[..idx]);
        new_content.push_str(&norm_new);
        new_content.push_str(&norm_current[idx + norm_old.len()..]);

        let lines_removed = norm_old.lines().count();
        let lines_added = norm_new.lines().count();
        let summary = format!("+{lines_added} / -{lines_removed} lines in {path_display}");

        return Ok(PatchResult {
            new_content,
            lines_added,
            lines_removed,
            summary,
        });
    }

    if matches.len() > 1 {
        return Err(PatchError::AmbiguousContext {
            occurrences: matches.len(),
            message: "oldText matches multiple locations".to_string(),
        });
    }

    let (idx, _) = matches[0];
    let mut new_content = String::with_capacity(current_content.len() + new_text.len());
    new_content.push_str(&current_content[..idx]);
    new_content.push_str(new_text);
    new_content.push_str(&current_content[idx + old_text.len()..]);

    let lines_removed = old_text.lines().count();
    let lines_added = new_text.lines().count();
    let summary = format!("+{lines_added} / -{lines_removed} lines in {path_display}");

    Ok(PatchResult {
        new_content,
        lines_added,
        lines_removed,
        summary,
    })
}

/// Apply a simple unified diff patch.
fn apply_unified_diff(
    current_content: &str,
    patch_str: &str,
    path_display: &str,
) -> Result<PatchResult, PatchError> {
    let current_lines: Vec<&str> = current_content.lines().collect();
    let mut result_lines = current_lines.clone();
    let mut total_added = 0;
    let mut total_removed = 0;

    let patch_lines: Vec<&str> = patch_str.lines().collect();
    let mut i = 0;
    let mut line_offset: isize = 0;
    let mut min_match_idx: usize = 0;

    while i < patch_lines.len() {
        let line = patch_lines[i];
        if line.starts_with("@@") {
            // Parse hunk header: @@ -orig_start,orig_count +new_start,new_count @@
            let (orig_start, _) = parse_hunk_header(line)?;
            i += 1;

            // Collect hunk lines
            let mut hunk_old = Vec::new();
            let mut hunk_new = Vec::new();

            while i < patch_lines.len() && !patch_lines[i].starts_with("@@") {
                let hline = patch_lines[i];
                if let Some(stripped) = hline.strip_prefix(' ') {
                    hunk_old.push(stripped);
                    hunk_new.push(stripped);
                } else if let Some(stripped) = hline.strip_prefix('-') {
                    hunk_old.push(stripped);
                    total_removed += 1;
                } else if let Some(stripped) = hline.strip_prefix('+') {
                    hunk_new.push(stripped);
                    total_added += 1;
                } else if hline.is_empty() {
                    // Empty context line in unified diff
                    hunk_old.push("");
                    hunk_new.push("");
                } else if hline.starts_with('\\') {
                    // Ignore "\ No newline at end of file"
                }
                i += 1;
            }

            // Expected index adjusted for cumulative line offset from previous hunks
            let expected_idx = if orig_start > 0 {
                let shifted = (orig_start as isize - 1) + line_offset;
                shifted.max(0) as usize
            } else {
                0
            };

            let match_idx = find_hunk_match(&result_lines, &hunk_old, min_match_idx, expected_idx)?;

            // Replace hunk_old lines with hunk_new lines
            let mut next_result = Vec::with_capacity(
                result_lines
                    .len()
                    .saturating_add(hunk_new.len())
                    .saturating_sub(hunk_old.len()),
            );
            next_result.extend_from_slice(&result_lines[..match_idx]);
            for &nl in &hunk_new {
                next_result.push(nl);
            }
            next_result.extend_from_slice(&result_lines[match_idx + hunk_old.len()..]);
            result_lines = next_result;

            let hunk_delta = hunk_new.len() as isize - hunk_old.len() as isize;
            line_offset += hunk_delta;
            min_match_idx = match_idx + hunk_new.len();
        } else {
            i += 1;
        }
    }

    let mut new_content = result_lines.join("\n");
    if current_content.ends_with('\n') {
        new_content.push('\n');
    }

    let summary = format!("+{total_added} / -{total_removed} lines in {path_display}");

    Ok(PatchResult {
        new_content,
        lines_added: total_added,
        lines_removed: total_removed,
        summary,
    })
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize), PatchError> {
    let trimmed = header.trim();
    if !trimmed.starts_with("@@") {
        return Err(PatchError::MalformedPatch(format!(
            "Invalid hunk header: {header}"
        )));
    }

    let after_at = trimmed[2..].trim_start();
    let parts: Vec<&str> = after_at.split_whitespace().collect();
    if parts.is_empty() || !parts[0].starts_with('-') {
        return Err(PatchError::MalformedPatch(format!(
            "Invalid hunk header: {header}"
        )));
    }

    let old_range = parts[0].trim_start_matches('-');
    let mut subparts = old_range.split(',');
    let start: usize = subparts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PatchError::MalformedPatch(format!("Invalid hunk start line: {header}")))?;
    let count: usize = subparts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    Ok((start, count))
}

fn find_hunk_match(
    lines: &[&str],
    hunk_old: &[&str],
    min_idx: usize,
    hint_idx: usize,
) -> Result<usize, PatchError> {
    if hunk_old.is_empty() {
        return Ok(hint_idx.max(min_idx).min(lines.len()));
    }

    let mut matches = Vec::new();
    let max_idx = lines.len().saturating_sub(hunk_old.len());
    if min_idx <= max_idx {
        for idx in min_idx..=max_idx {
            if &lines[idx..idx + hunk_old.len()] == hunk_old {
                matches.push(idx);
            }
        }
    }

    if matches.is_empty() {
        return Err(PatchError::ContextNotFound(
            "Unified diff context lines do not match target file".to_string(),
        ));
    }

    if matches.len() > 1 {
        return Err(PatchError::AmbiguousContext {
            occurrences: matches.len(),
            message: "Hunk context matches multiple file positions".to_string(),
        });
    }

    Ok(matches[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_replacement_unique() {
        let content = "fn main() {\n    println!(\"Hello\");\n}\n";
        let res = apply_patch(
            content,
            Some("println!(\"Hello\");"),
            Some("println!(\"World!\");"),
            None,
            "main.rs",
        )
        .unwrap();

        assert_eq!(
            res.new_content,
            "fn main() {\n    println!(\"World!\");\n}\n"
        );
        assert_eq!(res.lines_added, 1);
        assert_eq!(res.lines_removed, 1);
    }

    #[test]
    fn test_exact_replacement_not_found() {
        let content = "fn main() {}\n";
        let err =
            apply_patch(content, Some("missing text"), Some("new"), None, "main.rs").unwrap_err();
        assert!(matches!(err, PatchError::ContextNotFound(_)));
    }

    #[test]
    fn test_exact_replacement_ambiguous() {
        let content = "let x = 1;\nlet x = 1;\n";
        let err = apply_patch(
            content,
            Some("let x = 1;"),
            Some("let x = 2;"),
            None,
            "main.rs",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PatchError::AmbiguousContext { occurrences: 2, .. }
        ));
    }

    #[test]
    fn test_unified_diff_application() {
        let content = "first\nsecond\nthird\nfourth\n";
        let patch = "@@ -2,2 +2,2 @@\n-second\n+modified_second\n third\n";
        let res = apply_patch(content, None, None, Some(patch), "test.txt").unwrap();
        assert_eq!(res.new_content, "first\nmodified_second\nthird\nfourth\n");
    }

    #[test]
    fn test_unified_diff_empty_context_lines() {
        let content = "head\n\nmiddle\n\ntail\n";
        let patch = "@@ -1,5 +1,5 @@\n head\n\n-middle\n+middle_edited\n\n tail\n";
        let res = apply_patch(content, None, None, Some(patch), "test.txt").unwrap();
        assert_eq!(res.new_content, "head\n\nmiddle_edited\n\ntail\n");
    }

    #[test]
    fn test_unified_diff_multi_hunk_line_shifts() {
        let content = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
        // Hunk 1 inserts 2 lines at line 2. Hunk 2 modifies line 8.
        let patch = "@@ -2,2 +2,4 @@\n-l2\n+l2_a\n+l2_b\n+l2_c\n l3\n@@ -8,2 +10,2 @@\n-l8\n+l8_modified\n l9\n";
        let res = apply_patch(content, None, None, Some(patch), "test.txt").unwrap();
        assert_eq!(
            res.new_content,
            "l1\nl2_a\nl2_b\nl2_c\nl3\nl4\nl5\nl6\nl7\nl8_modified\nl9\nl10\n"
        );
        assert_eq!(res.lines_added, 4);
        assert_eq!(res.lines_removed, 2);
    }

    #[test]
    fn test_unified_diff_rejects_ambiguous_hunk() {
        let content = "item\nitem\n";
        let patch = "@@ -1,1 +1,1 @@\n-item\n+replaced\n";
        let err = apply_patch(content, None, None, Some(patch), "test.txt").unwrap_err();
        match err {
            PatchError::AmbiguousContext { occurrences, .. } => assert_eq!(occurrences, 2),
            _ => panic!("Expected AmbiguousContext, got {err:?}"),
        }
    }
}
