use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use uuid::Uuid;

pub const DEFAULT_MAX_FILE_BYTES: usize = 5 * 1024 * 1024; // 5 MB
pub const DEFAULT_MAX_READ_CHARS: usize = 100_000; // 100k chars
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    NotFound(String),
    NotTextFile(String),
    FileTooLarge {
        size: usize,
        limit: usize,
    },
    InvalidRange {
        start: usize,
        end: usize,
        total: usize,
    },
    Io(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "File not found: {p}"),
            Self::NotTextFile(p) => write!(f, "File is binary or not valid UTF-8: {p}"),
            Self::FileTooLarge { size, limit } => {
                write!(
                    f,
                    "File size ({size} bytes) exceeds maximum limit ({limit} bytes)"
                )
            }
            Self::InvalidRange { start, end, total } => {
                write!(
                    f,
                    "Invalid line range: start {start}, end {end} (file has {total} lines)"
                )
            }
            Self::Io(e) => write!(f, "File I/O error: {e}"),
        }
    }
}

impl std::error::Error for FileError {}

/// Detected line ending style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

/// Metadata and content returned from a text read operation
#[derive(Debug, Clone)]
pub struct TextReadResult {
    pub content: String,
    pub hash: String,
    pub size: usize,
    pub total_lines: usize,
    pub truncated: bool,
    pub has_bom: bool,
    pub line_ending: LineEnding,
}

/// Inspect if bytes are valid UTF-8 and contain no null bytes anywhere.
pub fn is_text(bytes: &[u8]) -> bool {
    // Reject null bytes anywhere in file (binary marker)
    if bytes.contains(&0) {
        return false;
    }

    // Verify valid UTF-8 across entire file (excluding optional UTF-8 BOM)
    let bytes_without_bom = if bytes.starts_with(UTF8_BOM) {
        &bytes[UTF8_BOM.len()..]
    } else {
        bytes
    };

    std::str::from_utf8(bytes_without_bom).is_ok()
}

/// Compute BLAKE3 content hash as hex string.
pub fn compute_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Detect whether the string predominantly uses CRLF or LF line endings.
pub fn detect_line_ending(text: &str) -> LineEnding {
    let mut crlf_count = 0usize;
    let mut lf_count = 0usize;

    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                crlf_count += 1;
            } else {
                lf_count += 1;
            }
        }
    }

    if crlf_count > lf_count {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

/// Read an entire file as text, enforcing binary checks, size limits, and optional char truncation.
pub fn read_text_file(path: &Path, max_chars: Option<usize>) -> Result<TextReadResult, FileError> {
    if !path.exists() {
        return Err(FileError::NotFound(path.display().to_string()));
    }

    let metadata = fs::metadata(path).map_err(|e| FileError::Io(e.to_string()))?;
    let file_size = metadata.len() as usize;

    if file_size > DEFAULT_MAX_FILE_BYTES {
        return Err(FileError::FileTooLarge {
            size: file_size,
            limit: DEFAULT_MAX_FILE_BYTES,
        });
    }

    let raw_bytes = fs::read(path).map_err(|e| FileError::Io(e.to_string()))?;
    if !is_text(&raw_bytes) {
        return Err(FileError::NotTextFile(path.display().to_string()));
    }

    let hash = compute_hash(&raw_bytes);
    let has_bom = raw_bytes.starts_with(UTF8_BOM);

    let text_slice = if has_bom {
        &raw_bytes[UTF8_BOM.len()..]
    } else {
        &raw_bytes[..]
    };

    let full_text = match std::str::from_utf8(text_slice) {
        Ok(s) => s.to_string(),
        Err(_) => return Err(FileError::NotTextFile(path.display().to_string())),
    };
    let line_ending = detect_line_ending(&full_text);
    let total_lines = full_text.lines().count().max(1);

    let char_limit = max_chars.unwrap_or(DEFAULT_MAX_READ_CHARS);
    let (content, truncated) = if full_text.chars().count() > char_limit {
        let truncated_str: String = full_text.chars().take(char_limit).collect();
        (truncated_str, true)
    } else {
        (full_text, false)
    };

    Ok(TextReadResult {
        content,
        hash,
        size: file_size,
        total_lines,
        truncated,
        has_bom,
        line_ending,
    })
}

/// Read a line range (1-based, inclusive) from a text file.
pub fn read_range(
    path: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<(String, String, usize), FileError> {
    if start_line == 0 || end_line < start_line {
        return Err(FileError::InvalidRange {
            start: start_line,
            end: end_line,
            total: 0,
        });
    }

    let read_result = read_text_file(path, None)?;
    let lines: Vec<&str> = read_result.content.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return Ok((String::new(), read_result.hash, 0));
    }

    if start_line > total_lines {
        return Err(FileError::InvalidRange {
            start: start_line,
            end: end_line,
            total: total_lines,
        });
    }

    let bounded_end = end_line.min(total_lines);
    // 1-based indexing
    let slice = &lines[start_line - 1..bounded_end];
    let delimiter = read_result.line_ending.as_str();
    let mut range_content = slice.join(delimiter);
    if bounded_end == total_lines && read_result.content.ends_with('\n') {
        range_content.push_str(delimiter);
    }

    Ok((range_content, read_result.hash, total_lines))
}

/// Atomically write new content to a target file.
///
/// Steps:
/// 1. Create temporary file in the same directory (`.wb-tmp-<uuid>`)
/// 2. Write BOM if requested, then content
/// 3. Flush & sync OS buffers
/// 4. Atomic rename with bounded retry for Windows file locks
pub fn atomic_write(
    target_path: &Path,
    content: &str,
    has_bom: bool,
    line_ending: Option<LineEnding>,
) -> Result<String, FileError> {
    let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|e| FileError::Io(e.to_string()))?;
    }

    // Temporary file in same directory ensures same filesystem volume for atomic rename
    let tmp_name = format!(".wb-tmp-{}", Uuid::new_v4().simple());
    let tmp_path = parent.join(tmp_name);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| FileError::Io(format!("Failed to create temporary file: {e}")))?;

    // Prepare normalized content with requested line ending
    let final_content = if let Some(le) = line_ending {
        normalize_line_endings(content, le)
    } else {
        content.to_string()
    };

    let mut written_bytes = Vec::new();
    if has_bom {
        written_bytes.extend_from_slice(UTF8_BOM);
    }
    written_bytes.extend_from_slice(final_content.as_bytes());

    file.write_all(&written_bytes)
        .map_err(|e| FileError::Io(format!("Failed to write to temporary file: {e}")))?;
    file.sync_data()
        .map_err(|e| FileError::Io(format!("Failed to sync temporary file: {e}")))?;
    drop(file);

    // Atomic replacement with retry on Windows
    let mut last_err = None;
    for attempt in 0..5 {
        match fs::rename(&tmp_path, target_path) {
            Ok(()) => {
                let hash = compute_hash(&written_bytes);
                return Ok(hash);
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(25 * (attempt + 1)));
            }
        }
    }

    // Clean up temporary file on failure
    let _ = fs::remove_file(&tmp_path);

    Err(FileError::Io(format!(
        "Atomic rename failed after retries: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// Convert all line endings in string to requested style.
pub fn normalize_line_endings(text: &str, line_ending: LineEnding) -> String {
    let target_sep = line_ending.as_str();
    let mut out = String::with_capacity(text.len());
    let mut prev_cr = false;

    for ch in text.chars() {
        if ch == '\r' {
            prev_cr = true;
            continue;
        }
        if ch == '\n' {
            out.push_str(target_sep);
            prev_cr = false;
            continue;
        }
        if prev_cr {
            out.push_str(target_sep);
            prev_cr = false;
        }
        out.push(ch);
    }
    if prev_cr {
        out.push_str(target_sep);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_text_detection() {
        assert!(is_text(b"Hello world\nThis is text."));
        assert!(is_text("Unicode: \u{1F9E0} WebBrain".as_bytes()));
        // Binary with null byte
        assert!(!is_text(&[0x48, 0x65, 0x00, 0x6C, 0x6F]));
    }

    #[test]
    fn test_atomic_write_and_read() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("test.txt");

        let hash = atomic_write(
            &target,
            "Line 1\nLine 2\nLine 3",
            false,
            Some(LineEnding::Lf),
        )
        .unwrap();
        assert!(!hash.is_empty());

        let read = read_text_file(&target, None).unwrap();
        assert_eq!(read.content, "Line 1\nLine 2\nLine 3");
        assert_eq!(read.total_lines, 3);
        assert_eq!(read.hash, hash);
        assert!(!read.has_bom);
    }

    #[test]
    fn test_read_range() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("range.txt");

        atomic_write(
            &target,
            "Alpha\nBeta\nGamma\nDelta\nEpsilon",
            false,
            Some(LineEnding::Lf),
        )
        .unwrap();

        let (range, hash, total) = read_range(&target, 2, 4).unwrap();
        assert_eq!(range, "Beta\nGamma\nDelta");
        assert_eq!(total, 5);
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_bom_preservation() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("bom.txt");

        let hash = atomic_write(&target, "BOM test", true, Some(LineEnding::Lf)).unwrap();
        let read = read_text_file(&target, None).unwrap();
        assert!(read.has_bom);
        assert_eq!(read.content, "BOM test");
        assert_eq!(read.hash, hash);
    }

    #[test]
    fn test_crlf_normalization() {
        let text = "Line 1\nLine 2\r\nLine 3\n";
        let normalized = normalize_line_endings(text, LineEnding::CrLf);
        assert_eq!(normalized, "Line 1\r\nLine 2\r\nLine 3\r\n");
    }

    #[test]
    fn test_rejects_invalid_utf8() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("invalid_utf8.bin");

        // Write valid UTF-8 preamble followed by invalid UTF-8 sequence past 8 KB
        let mut data = vec![b'a'; 8200];
        data.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // Invalid UTF-8 bytes
        std::fs::write(&target, &data).unwrap();

        let err = read_text_file(&target, None).unwrap_err();
        assert!(matches!(err, FileError::NotTextFile(_)));
    }

    #[test]
    fn test_rejects_null_byte_past_sample_window() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("late_null.bin");

        let mut data = vec![b'x'; 9000];
        data[8500] = 0x00; // Null byte past 8 KB
        std::fs::write(&target, &data).unwrap();

        let err = read_text_file(&target, None).unwrap_err();
        assert!(matches!(err, FileError::NotTextFile(_)));
    }
}
