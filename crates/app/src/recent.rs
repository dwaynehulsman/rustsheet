//! Recently opened files, kept in Application Support.

use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 10;

pub struct RecentFiles {
    files: Vec<PathBuf>,
    store: Option<PathBuf>,
}

fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/nl.infodatek.rustsheet/recent.json"))
}

impl RecentFiles {
    pub fn load() -> RecentFiles {
        let store = store_path();
        let files = store
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.exists())
            .take(MAX_RECENT)
            .collect();
        RecentFiles { files, store }
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Moves `path` to the top of the list.
    pub fn add(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.files.retain(|p| p != &path);
        self.files.insert(0, path);
        self.files.truncate(MAX_RECENT);
        self.save();
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.retain(|p| p != path);
        self.save();
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.save();
    }

    fn save(&self) {
        let Some(store) = &self.store else { return };
        if let Some(dir) = store.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.files) {
            let _ = std::fs::write(store, json);
        }
    }
}
