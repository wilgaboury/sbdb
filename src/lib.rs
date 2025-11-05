use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use anyhow::Context;
use reflink_copy::reflink_or_copy;

#[cfg(windows)]
use std::os::windows::prelude::*;

pub struct Client {
    root: PathBuf,
}

impl Client {
    pub fn open<P: AsRef<Path>>(root: P) -> anyhow::Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    pub fn read_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileReadGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_read_file_locks(&self.root, rpath)?;
        Ok(FileReadGaurd { path, lock })
    }

    pub fn read_dir<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<DirGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_read_file_locks(&self.root, rpath)?;
        Ok(DirGaurd { path, lock })
    }

    pub fn write_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileWriteGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_write_file_locks(&self.root, rpath)?;
        Ok(FileWriteGaurd { path, lock })
    }

    pub fn write_dir<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<DirGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_write_file_locks(&self.root, rpath)?;
        Ok(DirGaurd { path, lock })
    }

    // TODO
    // pub fn tx();
}

fn create_read_file_locks<P: AsRef<Path>>(root: &PathBuf, rpath: P) -> anyhow::Result<Vec<Lock>> {
    let mut result = Vec::new();

    for anc in rpath
        .as_ref()
        .ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let path = root.join(anc);
        result.push(Lock::Read(ReadLock::new(path)?))
    }

    result.reverse();

    Ok(result)
}

fn create_write_file_locks<P: AsRef<Path>>(root: &PathBuf, rpath: P) -> anyhow::Result<Vec<Lock>> {
    let mut result = Vec::new();

    for anc in rpath
        .as_ref()
        .ancestors()
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let path = root.join(anc);
        result.push(Lock::Read(ReadLock::new(path)?))
    }

    let path = root.join(rpath);
    result.push(Lock::Write(WriteLock::new(path)?));

    result.reverse();

    Ok(result)
}

pub struct FileReadGaurd {
    pub path: PathBuf,
    lock: Vec<Lock>,
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
    pub path: PathBuf,
    tmp: PathBuf,
    pub file: File,
}

impl CowFileGaurd {
    pub fn commit(self) -> anyhow::Result<()> {
        fs::rename(&self.tmp, &self.path)?;
        drop(self);
        Ok(())
    }
}

pub struct FileWriteGaurd {
    pub path: PathBuf,
    lock: Vec<Lock>,
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
            file,
        })
    }
}

pub struct DirGaurd {
    pub path: PathBuf,
    lock: Vec<Lock>,
}

fn path_with_extension<P: AsRef<Path>>(path: P, ext: &str) -> anyhow::Result<PathBuf> {
    let mut name = path
        .as_ref()
        .file_name()
        .context("not a valid path")?
        .to_os_string();
    name.push(ext);
    let parent = path.as_ref().parent().context("needs a parent")?;
    Ok(parent.join(name))
}
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x00000001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x00000002;
#[cfg(windows)]
const FILE_SHARE_DELETE: u32 = 0x00000004;

#[cfg(windows)]
pub fn get_lock_and_queue<P: AsRef<Path>>(path: P) -> anyhow::Result<(File, File)> {
    let path_lock = path_with_extension(&path, ".lock")?;
    let path_queue = path_with_extension(&path, ".queue")?;

    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path_lock)
        .context("could not open lock file")?;

    let queue = OpenOptions::new()
        .write(true)
        .create(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path_queue)
        .context("could not open queue file")?;

    Ok((lock, queue))
}

#[cfg(not(windows))]
pub fn get_lock_and_queue<P: AsRef<Path>>(path: P) -> anyhow::Result<(File, File)> {
    let path_lock = path_with_extension(&path, ".lock")?;
    let path_wwrite = path_with_extension(&path, ".queue")?;

    let lock = OpenOptions::new()
        .read(true)
        .create(true)
        .open(path_lock)
        .context("could not open lock file")?;

    let queue = OpenOptions::new()
        .read(true)
        .create(true)
        .open(path_queue)
        .context("could not open queue file")?;

    Ok((lock, queue))
}

pub enum Lock {
    Read(ReadLock),
    Write(WriteLock),
}

pub struct ReadLock {
    lock: File,
}

impl ReadLock {
    fn new<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let (lock, queue) = get_lock_and_queue(path)?;

        queue.lock()?;
        lock.lock_shared()?;
        queue.unlock()?;

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
    lock: File,
}

impl WriteLock {
    fn new<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let (lock, queue) = get_lock_and_queue(path)?;

        queue.lock()?;
        lock.lock()?;
        queue.unlock()?;

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

#[cfg(test)]
mod test {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::Duration,
    };

    use rand::Rng;

    use crate::{ReadLock, WriteLock};

    #[test]
    fn fuzz_test_mixed_locking() {
        let mut threads = Vec::new();
        let tmp_dir = std::env::temp_dir();
        let tmp_file_path_orig = tmp_dir.join("my_temp_file.txt");
        let rcnt_orig = Arc::new(AtomicU64::new(0));
        let wcnt_orig = Arc::new(AtomicU64::new(0));
        let rec_orig = Arc::new(Mutex::new(String::new()));

        for _ in 0..1000 {
            let tmp_file_path = tmp_file_path_orig.clone();
            let rcnt = rcnt_orig.clone();
            let wcnt = wcnt_orig.clone();
            let rec = rec_orig.clone();
            threads.push(thread::spawn(move || {
                let mut rng = rand::thread_rng();
                if rng.gen_bool(0.5) {
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=10)));
                    let _gaurd = ReadLock::new(tmp_file_path).unwrap();
                    rec.lock().unwrap().push('r');
                    rcnt.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    if wcnt.load(Ordering::Acquire) > 0 {
                        panic!("can't have readers and writers")
                    }
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=10)));
                    rcnt.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                } else {
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=10)));
                    let _gaurd = WriteLock::new(tmp_file_path).unwrap();
                    rec.lock().unwrap().push('w');
                    let wcnt_sn = wcnt.fetch_add(1, Ordering::AcqRel);
                    if wcnt_sn > 0 {
                        panic!("can't have multiple concurrent writers, num: {}", wcnt_sn);
                    }
                    let rcnt_sn = rcnt.load(Ordering::Acquire);
                    if rcnt_sn > 0 {
                        panic!("can't have readers and writers, num: {}", rcnt_sn);
                    }
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=50)));
                    wcnt.fetch_sub(1, Ordering::AcqRel);
                }
            }));
        }

        for thread in threads {
            match thread.join() {
                Ok(_) => (),
                Err(_) => panic!("Thread had error!"),
            }
        }

        println!("{}", rec_orig.lock().unwrap().as_str());
    }
}
