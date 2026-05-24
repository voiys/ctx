use std::fs;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use directories::UserDirs;

use crate::models::RuntimePaths;
use crate::storage::ensure_db;

pub(crate) struct AppContext {
    pub(crate) paths: RuntimePaths,
}

impl AppContext {
    pub(crate) fn load(cwd: Option<PathBuf>) -> Result<Self> {
        let project_root = cwd.unwrap_or(std::env::current_dir()?).canonicalize()?;
        let ctx_dir = project_root.join(".ctx");
        let manifest_path = ctx_dir.join("ctx.json");
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
            paths: RuntimePaths {
                project_root,
                manifest_path,
                ctx_dir,
                home,
                db_path,
            },
        })
    }

    pub(crate) fn init_storage(&self) -> Result<()> {
        fs::create_dir_all(&self.paths.ctx_dir)?;
        self.ensure_global_storage()
    }

    pub(crate) fn ensure_global_storage(&self) -> Result<()> {
        fs::create_dir_all(&self.paths.home)?;
        ensure_db(&self.paths.db_path)
    }

    pub(crate) fn ensure_project(&self) -> Result<()> {
        if !self.paths.manifest_path.exists() {
            bail!(
                "no ctx project found at {}; run `ctx init` first",
                self.paths.manifest_path.display()
            );
        }
        fs::create_dir_all(&self.paths.home)?;
        ensure_db(&self.paths.db_path)
    }
}
