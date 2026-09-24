use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    PathOutsideWorkspace(String),
    TraversalNotAllowed(String),
    ReservedDeviceName(String),
    RootNotFound(String),
    NotADirectory(String),
    InvalidPath(String),
    Io(String),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathOutsideWorkspace(p) => {
                write!(f, "Path is outside the authorized workspace: {p}")
            }
            Self::TraversalNotAllowed(p) => {
                write!(f, "Directory traversal (..) is strictly prohibited: {p}")
            }
            Self::ReservedDeviceName(d) => {
                write!(f, "Path contains a reserved Windows device name: {d}")
            }
            Self::RootNotFound(p) => write!(f, "Authorized workspace root does not exist: {p}"),
            Self::NotADirectory(p) => {
                write!(f, "Authorized workspace root is not a directory: {p}")
            }
            Self::InvalidPath(p) => write!(f, "Invalid path: {p}"),
            Self::Io(e) => write!(f, "Filesystem error: {e}"),
        }
    }
}

impl std::error::Error for SandboxError {}

/// Path sandbox that enforces confinement to an authorized root directory.
#[derive(Debug, Clone)]
pub struct PathSandbox {
    root: PathBuf,
    canonical_root: PathBuf,
}

impl PathSandbox {
    pub fn new(root_path: impl AsRef<Path>) -> Result<Self, SandboxError> {
        let root = root_path.as_ref().to_path_buf();
        if !root.exists() {
            return Err(SandboxError::RootNotFound(root.display().to_string()));
        }

        let canonical_root = dunce::canonicalize(&root)
            .map_err(|e| SandboxError::Io(format!("Failed to canonicalize root: {e}")))?;

        if !canonical_root.is_dir() {
            return Err(SandboxError::NotADirectory(
                canonical_root.display().to_string(),
            ));
        }

        Ok(Self {
            root,
            canonical_root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Resolve and validate a path relative to the authorized root.
    ///
    /// Enforces:
    /// - Rejection of reserved Windows device names
    /// - Rejection of `..` directory traversal
    /// - Stripping of root prefix if client sends full path inside root
    /// - Confinement: canonicalized target (resolving symlinks/junctions) MUST stay inside canonical_root.
    pub fn resolve(&self, req_path: &str) -> Result<PathBuf, SandboxError> {
        let trimmed = req_path.trim();
        if trimmed.is_empty() {
            return Ok(self.canonical_root.clone());
        }

        // Check for reserved Windows device names anywhere in the path
        if let Some(device) = contains_reserved_device_name(trimmed) {
            return Err(SandboxError::ReservedDeviceName(device));
        }

        // Normalize slashes
        let normalized = trimmed.replace('/', std::path::MAIN_SEPARATOR_STR);
        let path_obj = Path::new(&normalized);

        // Lexical traversal check
        for comp in path_obj.components() {
            if comp == Component::ParentDir {
                return Err(SandboxError::TraversalNotAllowed(req_path.to_string()));
            }
        }

        // If path is absolute, check if it starts with the authorized root
        let full_target = if path_obj.is_absolute() {
            let canon_candidate =
                dunce::canonicalize(path_obj).unwrap_or_else(|_| path_obj.to_path_buf());
            if !is_subpath(&canon_candidate, &self.canonical_root) {
                return Err(SandboxError::PathOutsideWorkspace(req_path.to_string()));
            }
            canon_candidate
        } else {
            // Treat as relative to canonical root
            // Strip any leading slashes or prefixes like .\
            let rel = strip_leading_slashes(path_obj);
            let mut target = self.canonical_root.join(&rel);

            if !target.exists() {
                let root_name = self
                    .canonical_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                if !root_name.is_empty() {
                    let rel_str = rel.to_string_lossy();
                    let trimmed_rel = rel_str.trim_matches(['/', '\\']);

                    if trimmed_rel.eq_ignore_ascii_case(root_name) {
                        return Ok(self.canonical_root.clone());
                    }

                    if trimmed_rel.len() > root_name.len()
                        && trimmed_rel[..root_name.len()].eq_ignore_ascii_case(root_name)
                    {
                        let next_char = trimmed_rel.as_bytes()[root_name.len()];
                        if next_char == b'/' || next_char == b'\\' {
                            let stripped = &trimmed_rel[root_name.len() + 1..];
                            let stripped_target = self.canonical_root.join(stripped);
                            let parent_ok = stripped_target
                                .parent()
                                .map(|p| p.exists() && is_subpath(p, &self.canonical_root))
                                .unwrap_or(false);
                            if stripped_target.exists() || parent_ok {
                                target = stripped_target;
                            }
                        }
                    }
                }
            }

            target
        };

        // If the target exists, canonicalize it to resolve all symlinks and junctions
        if full_target.exists() {
            let canon = dunce::canonicalize(&full_target)
                .map_err(|e| SandboxError::Io(format!("Failed to canonicalize target: {e}")))?;

            if !is_subpath(&canon, &self.canonical_root) {
                return Err(SandboxError::PathOutsideWorkspace(req_path.to_string()));
            }
            return Ok(canon);
        }

        // If the target doesn't exist yet (e.g. for write), verify the parent chain
        // resolves inside the canonical root without symlink escape.
        let mut ancestor = full_target.parent();
        while let Some(anc) = ancestor {
            if anc.exists() {
                let canon_anc = dunce::canonicalize(anc).map_err(|e| {
                    SandboxError::Io(format!("Failed to canonicalize ancestor: {e}"))
                })?;

                if !is_subpath(&canon_anc, &self.canonical_root) {
                    return Err(SandboxError::PathOutsideWorkspace(req_path.to_string()));
                }
                break;
            }
            ancestor = anc.parent();
        }

        Ok(full_target)
    }

    /// Convert an absolute path inside the workspace into a relative workspace path.
    pub fn to_relative(&self, abs_path: &Path) -> Result<String, SandboxError> {
        let canon = dunce::canonicalize(abs_path).unwrap_or_else(|_| abs_path.to_path_buf());
        if !is_subpath(&canon, &self.canonical_root) {
            return Err(SandboxError::PathOutsideWorkspace(
                abs_path.display().to_string(),
            ));
        }

        match canon.strip_prefix(&self.canonical_root) {
            Ok(rel) => {
                let s = rel.to_string_lossy().replace('\\', "/");
                Ok(s)
            }
            Err(_) => {
                // Fallback for case-insensitive matches on Windows
                let root_str = self.canonical_root.to_string_lossy();
                let path_str = canon.to_string_lossy();
                if path_str.len() >= root_str.len() {
                    let suffix = &path_str[root_str.len()..];
                    let trimmed = suffix.trim_start_matches(['/', '\\']);
                    Ok(trimmed.replace('\\', "/"))
                } else {
                    Ok(String::new())
                }
            }
        }
    }
}

/// Checks if candidate path is equal to or a subpath of base.
/// Handles case-insensitivity on Windows.
pub fn is_subpath(candidate: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let cand_str = candidate.to_string_lossy().to_lowercase();
        let base_str = base.to_string_lossy().to_lowercase();

        if cand_str == base_str {
            return true;
        }

        let prefix_with_sep = if base_str.ends_with('\\') || base_str.ends_with('/') {
            base_str
        } else {
            format!("{base_str}\\",)
        };

        cand_str.starts_with(&prefix_with_sep)
    }

    #[cfg(not(windows))]
    {
        candidate == base || candidate.starts_with(base)
    }
}

fn strip_leading_slashes(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => {}
        }
    }
    out
}

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Returns the matching reserved device name if found.
fn contains_reserved_device_name(path_str: &str) -> Option<String> {
    let path = Path::new(path_str);
    for comp in path.components() {
        if let Component::Normal(os_str) = comp {
            let s = os_str.to_string_lossy();
            let stem = match s.split('.').next() {
                Some(st) => st.trim(),
                None => s.trim(),
            };
            for &reserved in RESERVED_DEVICE_NAMES {
                if stem.eq_ignore_ascii_case(reserved) {
                    return Some(reserved.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sandbox_normal_resolution() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let resolved = sandbox.resolve("src/main.rs").unwrap();
        assert!(is_subpath(&resolved, sandbox.canonical_root()));
    }

    #[test]
    fn test_sandbox_traversal_rejection() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let err = sandbox.resolve("../outside.txt").unwrap_err();
        match err {
            SandboxError::TraversalNotAllowed(_) => {}
            _ => panic!("Expected TraversalNotAllowed, got {err:?}"),
        }

        let err2 = sandbox.resolve("foo/../../outside.txt").unwrap_err();
        match err2 {
            SandboxError::TraversalNotAllowed(_) => {}
            _ => panic!("Expected TraversalNotAllowed, got {err2:?}"),
        }
    }

    #[test]
    fn test_sandbox_reserved_device_names() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        assert!(matches!(
            sandbox.resolve("nul").unwrap_err(),
            SandboxError::ReservedDeviceName(_)
        ));
        assert!(matches!(
            sandbox.resolve("con.txt").unwrap_err(),
            SandboxError::ReservedDeviceName(_)
        ));
        assert!(matches!(
            sandbox.resolve("subdir/aux.js").unwrap_err(),
            SandboxError::ReservedDeviceName(_)
        ));
        assert!(matches!(
            sandbox.resolve("com1").unwrap_err(),
            SandboxError::ReservedDeviceName(_)
        ));
    }

    #[test]
    fn test_to_relative() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();

        let file_path = temp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "hello").unwrap();

        let rel = sandbox.to_relative(&file_path).unwrap();
        assert_eq!(rel, "src/lib.rs");
    }
}
