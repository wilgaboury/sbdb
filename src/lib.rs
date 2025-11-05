use std::{ffi::OsString, fs::{self, File, OpenOptions, TryLockError}, path::{Path, PathBuf}};

use anyhow::Context;
use reflink_copy::reflink_or_copy;

pub struct Client {
    root: PathBuf
}

impl Client {
    pub fn open<P: AsRef<Path>>(root: P) -> anyhow::Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf()
        })
    }

    pub fn read_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileReadGaurd> {
        // TODO: this is wrong, have to lock all parents
        let path= self.root.join(rpath.as_ref());
        let lock = create_read_file_locks(&self.root, rpath)?;
        Ok(FileReadGaurd { path, lock })
    }

    // TODO
    // pub fn read_dir();

    pub fn write_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileWriteGaurd> {
        // TODO: this is wrong, have to lock all parents
        let path= self.root.join(rpath.as_ref());
        let lock = create_write_file_locks(&self.root, rpath)?;
        Ok(FileWriteGaurd { path, lock })
    }

    // TODO
    // pub fn write_dir();

    // TODO
    // pub fn tx();
}

fn create_read_file_locks<P: AsRef<Path>>(root: &PathBuf, rpath: P) -> anyhow::Result<Vec<Lock>> {
    let mut result = Vec::new();

    for anc in rpath.as_ref().ancestors().collect::<Vec<_>>().into_iter().rev() {
        let path = root.join(anc);
        result.push(Lock::Read(ReadLock::new(path)?))
    }

    result.reverse();

    Ok(result)
}

fn create_write_file_locks<P: AsRef<Path>>(root: &PathBuf, rpath: P) -> anyhow::Result<Vec<Lock>> {
    let mut result = Vec::new();

    for anc in rpath.as_ref().ancestors().skip(1).collect::<Vec<_>>().into_iter().rev() {
        let path = root.join(anc);
        result.push(Lock::Read(ReadLock::new(path)?))
    }

    let path = root.join(rpath);
    result.push(Lock::Write(WriteLock::new(path)?));

    result.reverse();

    Ok(result)
}

pub struct FileReadGaurd {
    path: PathBuf,
    lock: Vec<Lock>
}

impl FileReadGaurd {
    pub fn open(&self) -> anyhow::Result<File> {
        OpenOptions::new()
            .read(true)
            .create(true)
            .open(&self.path)
            .context("failed to open")
    }
}

pub struct CowFileGaurd {
    path: PathBuf,
    tmp: PathBuf,
    pub file: File
}

impl CowFileGaurd {
    pub fn commit(self) -> anyhow::Result<()> {
        fs::rename(&self.tmp, &self.path)?;
        drop(self);
        Ok(())
    }
}

pub struct FileWriteGaurd {
    path: PathBuf,
    lock: Vec<Lock>
}

impl FileWriteGaurd {
    pub fn open(&self) -> anyhow::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.path)
            .context("failed to open")
    }

    pub fn open_cp(&self) -> anyhow::Result<CowFileGaurd> {
        let tmp = path_with_extension(&self.path, ".tmp")?;
        reflink_or_copy(&self.path, &tmp)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&tmp)
            .context("failed to open")?;
        Ok(CowFileGaurd { 
            path: self.path.clone(), 
            tmp, 
            file
        })
    }
}

fn path_with_extension<P: AsRef<Path>>(path: P, ext: &str) -> anyhow::Result<PathBuf> {
    let mut name = path.as_ref().file_name().context("not a valid path")?.to_os_string();
    name.push(ext);
    let parent = path.as_ref().parent().context("needs a parent")?;
    Ok(parent.join(name))
}

pub fn get_lock_and_wwrite<P: AsRef<Path>>(path: P) -> anyhow::Result<(File, File)> {


        let path_lock = path_with_extension(&path, ".lock")?;
        let path_wwrite = path_with_extension(&path, ".wwrite")?;

        let lock = OpenOptions::new()
            .read(true)
            .create(true)
            .open(path_lock)
            .context("could not open lock file")?;

        let wwrite = OpenOptions::new()
            .read(true)
            .create(true)
            .open(path_wwrite)
            .context("could not open wwrite file")?;

        Ok((lock, wwrite))
}

pub enum Lock {
    Read(ReadLock),
    Write(WriteLock)
}


pub struct ReadLock {
    lock: File
}

impl ReadLock {
    fn new<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let (lock, wwrite) = get_lock_and_wwrite(path)?;

        match wwrite.try_lock_shared() {
            Ok(()) => {
                wwrite.unlock()?;
            },
            Err(TryLockError::WouldBlock) => {
                wwrite.lock_shared()?;
            },
            e => {
                e.context("failed to try lock")?
            }
        }

        lock.lock_shared()?;

        Ok(Self { lock })
    }
}

impl Drop for ReadLock {
    fn drop(&mut self) {
        if let Err(e) = self.lock.unlock().context("failed to unlock") {
            eprint!("{:?}", e);
        }
    }
}

pub struct WriteLock {
    lock: File
}

impl WriteLock {
    fn new<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let (lock, wwrite) = get_lock_and_wwrite(path)?;

        match lock.try_lock() {
            Ok(()) => {},
            Err(TryLockError::WouldBlock) => {
                wwrite.lock()?;
                lock.lock()?;
                wwrite.unlock()?;
            },
            e => {
                e.context("failed to try lock")?
            }
        }

        Ok(Self { lock })
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        if let Err(e) = self.lock.unlock().context("failed to unlock") {
            eprint!("{:?}", e);
        }
    }
}