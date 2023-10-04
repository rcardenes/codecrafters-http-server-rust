use anyhow::Result;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Configuration {
    pub root_dir: Option<PathBuf>,
}

impl Configuration {
    pub fn get() -> Self {
        let mut directory: Option<PathBuf> = None;
        let args: Vec<String> = env::args().collect();

        if args.get(1) == Some(&"--directory".to_string()) {
            if let Some(path) = args.get(2) {
                directory = Some(PathBuf::from(path));
            }
        }

        Self {
            root_dir: directory,
        }
    }

    pub fn resolve_path(&self, path: &Path) -> Result<PathBuf> {
        let mut full_path = match &self.root_dir {
            Some(base_dir) => base_dir.clone(),
            None => env::current_dir()?,
        };
        full_path.push(path);

        Ok(full_path)
    }
}
