//! The server-side working directory owned by the native library.
//!
//! Layout:
//!
//! ```text
//! <work_dir>/
//!   boot.wasm            optional fallback image when the EEPROM isn't wasm
//!   machines/<key>.state suspended executor snapshots
//!   logs/<key>.log       output of the guest's `oc.log` import
//! ```
//!
//! Java never reads or writes these files: NBT only stores the machine key,
//! and every file under the directory goes through this module.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};

#[derive(Clone, Debug)]
pub struct WorkDir {
    root: PathBuf,
}

/// The longest log file kept before it is rotated to `<key>.log.old`.
const MAX_LOG_BYTES: u64 = 1 << 20;

impl WorkDir {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        for dir in [root.join("machines"), root.join("logs")] {
            fs::create_dir_all(&dir)
                .with_context(|| format!("creating work dir {}", dir.display()))?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Machine keys become file names, so they must not contain path syntax.
    /// OpenComputers addresses and the UUIDs Java generates both pass this check.
    pub fn validate_key(key: &str) -> Result<()> {
        if key.is_empty()
            || key.len() > 64
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid machine key {key:?}");
        }
        Ok(())
    }

    fn state_path(&self, key: &str) -> PathBuf {
        self.root.join("machines").join(format!("{key}.state"))
    }

    fn log_path(&self, key: &str) -> PathBuf {
        self.root.join("logs").join(format!("{key}.log"))
    }

    /// Writes through a temporary file then renames it, so a crash mid-save
    /// leaves the previous snapshot intact.
    pub fn write_state(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.state_path(key);
        let tmp = path.with_extension("state.tmp");
        {
            let mut file = fs::File::create(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn read_state(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.state_path(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("reading machine state"),
        }
    }

    pub fn remove_state(&self, key: &str) -> Result<()> {
        match fs::remove_file(self.state_path(key)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn append_log(&self, key: &str, line: &str) -> Result<()> {
        let path = self.log_path(key);
        if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = fs::rename(&path, path.with_extension("log.old"));
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    pub fn default_boot_image(&self) -> Result<Option<Vec<u8>>> {
        match fs::read(self.root.join("boot.wasm")) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("reading boot.wasm"),
        }
    }
}
