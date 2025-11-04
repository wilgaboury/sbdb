use std::{fs, path::{Path, PathBuf}};

use fs2::FileExt;

pub struct FsdbClient {
    loc: PathBuf
}

impl FsdbClient {
    pub fn open(loc: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(loc)?;

        
        
        Ok(Self {
            loc: loc.to_path_buf()
        })
    }
}