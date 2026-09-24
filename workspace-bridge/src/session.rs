use crate::patch::PatchError;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

/// Tracked state for an opened or read file.
#[derive(Debug, Clone)]
pub struct FileState {
    pub relative_path: String,
    pub revision: u64,
    pub hash: String,
    pub mtime: SystemTime,
    pub size: usize,
}

/// Active workspace session state.
#[derive(Debug, Clone)]
pub struct WorkspaceSession {
    pub session_id: String,
    pub root: PathBuf,
    pub root_name: String,
    pub allow_write: bool,
    pub allow_command: bool,
    opened_files: Arc<RwLock<HashMap<String, FileState>>>,
    pub created_at: DateTime<Utc>,
    last_activity: Arc<RwLock<DateTime<Utc>>>,
}

impl WorkspaceSession {
    pub fn new(root: PathBuf, allow_write: bool, allow_command: bool) -> Self {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string());

        let session_id = format!("sess_{}", Uuid::new_v4().simple());
        let now = Utc::now();

        Self {
            session_id,
            root,
            root_name,
            allow_write,
            allow_command,
            opened_files: Arc::new(RwLock::new(HashMap::new())),
            created_at: now,
            last_activity: Arc::new(RwLock::new(now)),
        }
    }

    pub fn touch_activity(&self) {
        *self.last_activity.write() = Utc::now();
    }

    /// Record a file read and return its tracked revision.
    pub fn record_read(
        &self,
        relative_path: &str,
        hash: &str,
        mtime: SystemTime,
        size: usize,
    ) -> u64 {
        self.touch_activity();
        let mut files = self.opened_files.write();

        if let Some(existing) = files.get_mut(relative_path) {
            // If the file hash changed on disk since last read, bump revision
            if existing.hash != hash {
                existing.revision += 1;
                existing.hash = hash.to_string();
                existing.mtime = mtime;
                existing.size = size;
            }
            existing.revision
        } else {
            let state = FileState {
                relative_path: relative_path.to_string(),
                revision: 1,
                hash: hash.to_string(),
                mtime,
                size,
            };
            files.insert(relative_path.to_string(), state);
            1
        }
    }

    /// Check whether expected_revision and expected_hash match current state and current disk hash.
    /// If disk hash differs from our recorded hash, an external modification occurred!
    /// In that case, we bump our tracked revision and return RevisionConflict.
    pub fn check_revision(
        &self,
        relative_path: &str,
        expected_revision: u64,
        expected_hash: Option<&str>,
        current_disk_hash: &str,
    ) -> Result<(), PatchError> {
        self.touch_activity();
        let mut files = self.opened_files.write();

        if let Some(existing) = files.get_mut(relative_path) {
            // Check if file was modified externally on disk since last recorded state
            if !existing.hash.eq_ignore_ascii_case(current_disk_hash) {
                existing.revision += 1;
                let _old_hash = std::mem::replace(&mut existing.hash, current_disk_hash.to_string());
                return Err(PatchError::RevisionConflict {
                    expected_revision,
                    current_revision: existing.revision,
                    expected_hash: expected_hash.map(|s| s.to_string()),
                    current_hash: current_disk_hash.to_string(),
                });
            }

            // Check if client supplied an outdated revision number
            if existing.revision != expected_revision {
                return Err(PatchError::RevisionConflict {
                    expected_revision,
                    current_revision: existing.revision,
                    expected_hash: expected_hash.map(|s| s.to_string()),
                    current_hash: existing.hash.clone(),
                });
            }

            // Check expected hash if supplied
            if let Some(exp_h) = expected_hash {
                if !existing.hash.eq_ignore_ascii_case(exp_h) {
                    return Err(PatchError::RevisionConflict {
                        expected_revision,
                        current_revision: existing.revision,
                        expected_hash: Some(exp_h.to_string()),
                        current_hash: existing.hash.clone(),
                    });
                }
            }

            Ok(())
        } else {
            // File not yet tracked in session
            if expected_revision > 1 {
                return Err(PatchError::RevisionConflict {
                    expected_revision,
                    current_revision: 1,
                    expected_hash: expected_hash.map(|s| s.to_string()),
                    current_hash: current_disk_hash.to_string(),
                });
            }

            if let Some(exp_h) = expected_hash {
                if !exp_h.eq_ignore_ascii_case(current_disk_hash) {
                    return Err(PatchError::RevisionConflict {
                        expected_revision,
                        current_revision: 1,
                        expected_hash: Some(exp_h.to_string()),
                        current_hash: current_disk_hash.to_string(),
                    });
                }
            }

            Ok(())
        }
    }

    /// Commit new revision after successful atomic write.
    pub fn commit_revision(
        &self,
        relative_path: &str,
        new_hash: &str,
        new_mtime: SystemTime,
        new_size: usize,
    ) -> (u64, u64) {
        self.touch_activity();
        let mut files = self.opened_files.write();

        if let Some(existing) = files.get_mut(relative_path) {
            let old_rev = existing.revision;
            let new_rev = old_rev + 1;
            existing.revision = new_rev;
            existing.hash = new_hash.to_string();
            existing.mtime = new_mtime;
            existing.size = new_size;
            (old_rev, new_rev)
        } else {
            let state = FileState {
                relative_path: relative_path.to_string(),
                revision: 2,
                hash: new_hash.to_string(),
                mtime: new_mtime,
                size: new_size,
            };
            files.insert(relative_path.to_string(), state);
            (1, 2)
        }
    }

    /// Commit revision 1 for a newly created file.
    pub fn commit_new_file(
        &self,
        relative_path: &str,
        new_hash: &str,
        new_mtime: SystemTime,
        new_size: usize,
    ) -> (u64, u64) {
        self.touch_activity();
        let mut files = self.opened_files.write();

        if let Some(existing) = files.get_mut(relative_path) {
            let old_rev = existing.revision;
            let new_rev = old_rev + 1;
            existing.revision = new_rev;
            existing.hash = new_hash.to_string();
            existing.mtime = new_mtime;
            existing.size = new_size;
            (old_rev, new_rev)
        } else {
            let state = FileState {
                relative_path: relative_path.to_string(),
                revision: 1,
                hash: new_hash.to_string(),
                mtime: new_mtime,
                size: new_size,
            };
            files.insert(relative_path.to_string(), state);
            (0, 1)
        }
    }

    /// Notify that an external change occurred on a file (e.g. from file watcher).
    pub fn invalidate_external_change(&self, relative_path: &str, new_hash: Option<&str>) {
        let mut files = self.opened_files.write();
        if let Some(existing) = files.get_mut(relative_path) {
            existing.revision += 1;
            if let Some(h) = new_hash {
                existing.hash = h.to_string();
            }
        }
    }

    pub fn get_file_state(&self, relative_path: &str) -> Option<FileState> {
        self.opened_files.read().get(relative_path).cloned()
    }

    pub fn opened_files_count(&self) -> usize {
        self.opened_files.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_read_and_revision_bump() {
        let session = WorkspaceSession::new(PathBuf::from("/test"), true, false);
        let mtime = SystemTime::now();

        let rev1 = session.record_read("src/lib.rs", "hash1", mtime, 100);
        assert_eq!(rev1, 1);

        // Same read with same hash preserves revision
        let rev1_again = session.record_read("src/lib.rs", "hash1", mtime, 100);
        assert_eq!(rev1_again, 1);

        // Read with changed hash increments revision
        let rev2 = session.record_read("src/lib.rs", "hash2", mtime, 120);
        assert_eq!(rev2, 2);
    }

    #[test]
    fn test_check_and_commit_revision() {
        let session = WorkspaceSession::new(PathBuf::from("/test"), true, false);
        let mtime = SystemTime::now();

        session.record_read("src/main.rs", "hashA", mtime, 200);

        // Stale revision: expected 0 when current is 1
        let err = session
            .check_revision("src/main.rs", 0, Some("hashA"), "hashA")
            .unwrap_err();
        assert!(matches!(
            err,
            PatchError::RevisionConflict {
                expected_revision: 0,
                current_revision: 1,
                ..
            }
        ));

        // External modification on disk: current disk hash is "hashExternal" while recorded was "hashA"
        let err_ext = session
            .check_revision("src/main.rs", 1, Some("hashA"), "hashExternal")
            .unwrap_err();
        assert!(matches!(
            err_ext,
            PatchError::RevisionConflict {
                expected_revision: 1,
                current_revision: 2, // bumped due to external change
                ..
            }
        ));

        // Valid update succeeds
        session.check_revision("src/main.rs", 2, Some("hashExternal"), "hashExternal").unwrap();
        let (old_rev, new_rev) = session.commit_revision("src/main.rs", "hashB", mtime, 250);
        assert_eq!(old_rev, 2);
        assert_eq!(new_rev, 3);
    }
}
