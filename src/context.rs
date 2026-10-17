use std::fs;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use directories::UserDirs;

use crate::models::RuntimePaths;
use crate::storage::ensure_db;

pub(crate) struct AppContext {
    pub(crate) paths: RuntimePaths,
}

impl AppContext {
    pub(crate) fn load() -> Result<Self> {
        let home = if let Ok(value) = std::env::var("CTX_HOME") {
            PathBuf::from(value)
        } else {
            UserDirs::new()
                .ok_or_else(|| anyhow!("could not determine home directory"))?
                .home_dir()
                .join(".ctx")
        };
        let db_path = home.join("ctx.db");
        Ok(Self {
            paths: RuntimePaths { home, db_path },
        })
    }

    pub(crate) fn ensure_global_storage(&self) -> Result<()> {
        fs::create_dir_all(&self.paths.home)?;
        ensure_db(&self.paths.db_path)
    }
}
