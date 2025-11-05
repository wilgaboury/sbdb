use std::{fs::{self, File, OpenOptions, TryLockError}, path::{Path, PathBuf}};

use anyhow::Context;

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

    pub fn read_file<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<FileReadGaurd> {
        // TODO: this is wrong, have to lock all parents
        let path= self.root.join(path.as_ref());
        let lock = ReadLock::new(&path)?;
        Ok(FileReadGaurd { path, lock })
    }

    // TODO
    // pub fn read_dir();

    pub fn write_file<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<FileWriteGaurd> {
        // TODO: this is wrong, have to lock all parents
        let path= self.root.join(path.as_ref());
        let lock = WriteLock::new(&path)?;
        Ok(FileWriteGaurd { path, lock })
    }

    // TODO
    // pub fn write_dir();

    // TODO
    // pub fn tx();
}

pub struct FileReadGaurd {
    path: PathBuf,
    lock: ReadLock
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

pub struct FileWriteGaurd {
    path: PathBuf,
    lock: WriteLock
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
}


pub struct ReadLock {
    lock: File
}

pub fn get_lock_and_wwrite<P: AsRef<Path>>(path: P) -> anyhow::Result<(File, File)> {
        let name = path.as_ref().file_name().context("not a valid path")?.to_os_string();
        let mut name_lock = name.clone();
        name_lock.push(".lock");
        let mut name_wwrite = name.clone();
        name_wwrite.push(".wwrite");

        let parent = path.as_ref().parent().context("needs a parent")?;
        let path_lock = parent.join(name_lock);
        let path_wwrite = parent.join(name_wwrite);

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

pub enum Lock {
    Read(ReadLock),
    Write(WriteLock)
}

pub struct MultiLock {
    locks: Vec<Lock>
}

impl MultiLock {

}