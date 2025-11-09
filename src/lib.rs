use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use reflink_copy::reflink_or_copy;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::prelude::*;

#[derive(Clone, Debug)]
pub struct Client {
    root: PathBuf,
}

impl Client {
    pub fn new<P: AsRef<Path>>(parent: P) -> anyhow::Result<Self> {
        let root = parent.as_ref().join("root");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn read_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileReadGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_read_file_locks(&self.root, rpath)?;
        Ok(FileReadGaurd { path, lock })
    }

    pub fn read_dir<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<DirReadGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_read_file_locks(&self.root, rpath)?;
        Ok(DirReadGaurd { path, lock })
    }

    pub fn write_file<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<FileWriteGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_write_file_locks(&self.root, rpath)?;
        Ok(FileWriteGaurd { path, lock })
    }

    pub fn write_dir<P: AsRef<Path>>(&self, rpath: P) -> anyhow::Result<DirWriteGaurd> {
        let path = self.root.join(rpath.as_ref());
        let lock = create_write_file_locks(&self.root, rpath)?;
        Ok(DirWriteGaurd { path, lock })
    }

    pub fn tx(&self) -> TxBuilder {
        TxBuilder::new(self.root.clone())
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }
}

pub enum TxEntryKind {
    Read,
    Write,
}

pub struct TxEntry {
    kind: TxEntryKind,
    path: PathBuf,
}

pub struct TxBuilder {
    root: PathBuf,
    reads: HashSet<PathBuf>,
    writes: HashSet<PathBuf>,
}

impl TxBuilder {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            reads: HashSet::new(),
            writes: HashSet::new(),
        }
    }

    pub fn read<P: AsRef<Path>>(mut self, path: P) -> Self {
        for anscestor in path.as_ref().ancestors() {
            self.reads.insert(anscestor.to_path_buf());
        }
        self
    }

    pub fn write<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.writes.insert(path.as_ref().to_path_buf());
        for anscestor in path.as_ref().ancestors().skip(1) {
            self.reads.insert(anscestor.to_path_buf());
        }
        self
    }

    pub fn begin(mut self) -> anyhow::Result<Tx> {
        let mut remove_writes = Vec::new();
        for write in self.writes.iter() {
            for anscestor in write.ancestors().skip(1) {
                if self.writes.contains(anscestor) {
                    remove_writes.push(write.clone());
                    break;
                }
            }
        }
        for remove in remove_writes {
            self.writes.remove(&remove);
        }

        self.reads.retain(|p| {
            p.ancestors()
                .all(|anscestor| !self.writes.contains(anscestor))
        });

        let mut entries = Vec::new();

        for path in self.reads {
            entries.push(TxEntry {
                kind: TxEntryKind::Read,
                path,
            });
        }
        for path in self.writes {
            entries.push(TxEntry {
                kind: TxEntryKind::Write,
                path,
            });
        }

        entries.sort_by(|e1, e2| e1.path.cmp(&e2.path));

        let mut lock = Vec::with_capacity(entries.len());

        for e in entries {
            lock.push(match e.kind {
                TxEntryKind::Read => Lock::Read(ReadLock::new(self.root.join(e.path))?),
                TxEntryKind::Write => Lock::Write(WriteLock::new(self.root.join(e.path))?),
            });
        }

        lock.reverse();

        Ok(Tx {
            root: self.root.clone(),
            lock,
        })
    }
}

pub struct Tx {
    root: PathBuf,
    #[allow(dead_code)]
    lock: Vec<Lock>,
}

impl Tx {
    pub fn file_cp<P: AsRef<Path>>(&self, orig: P) -> anyhow::Result<CowFileGaurd> {
        file_cp(self.root.join(orig))
    }

    pub fn dir_cp<P: AsRef<Path>>(&self, orig: P) -> anyhow::Result<CowDirGaurd> {
        dir_cp(self.root.join(orig))
    }

    pub fn dir_cp_atomic<P: AsRef<Path>>(&self, orig: P) -> anyhow::Result<CowAtomicDirGaurd> {
        dir_cp_atomic(self.root.join(orig))
    }
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
    eprintln!("{:?}", path);
    result.push(Lock::Write(WriteLock::new(path)?));

    result.reverse();

    Ok(result)
}

pub struct FileReadGaurd {
    pub path: PathBuf,
    #[allow(dead_code)]
    lock: Vec<Lock>,
}

impl CowFileGaurd {
    pub fn commit(self) -> anyhow::Result<()> {
        fs::rename(&self.path, &self.orig)?;
        Ok(())
    }
}

pub struct FileWriteGaurd {
    pub path: PathBuf,
    #[allow(dead_code)]
    lock: Vec<Lock>,
}

impl FileWriteGaurd {
    pub fn cp(&self) -> anyhow::Result<CowFileGaurd> {
        file_cp(&self.path)
    }
}

pub fn file_cp<P: AsRef<Path>>(orig: P) -> anyhow::Result<CowFileGaurd> {
    let path = path_hidden_with_extension(&orig, ".tmp")?;
    reflink_or_copy(&orig, &path)?;
    Ok(CowFileGaurd {
        path,
        orig: orig.as_ref().to_path_buf(),
    })
}

pub struct CowFileGaurd {
    pub path: PathBuf,
    orig: PathBuf,
}

pub struct DirReadGaurd {
    pub path: PathBuf,
    #[allow(dead_code)]
    lock: Vec<Lock>,
}

pub struct DirWriteGaurd {
    pub path: PathBuf,
    #[allow(dead_code)]
    lock: Vec<Lock>,
}

impl DirWriteGaurd {
    pub fn cp(&self) -> anyhow::Result<CowDirGaurd> {
        dir_cp(&self.path)
    }

    /// platform specific behavior:
    ///
    /// This feature uses symbolic links, which windows supports, but only in developer mode
    /// or with escalated privlages. For that reason it should probably be avoided if you would
    /// like to have cross-platform support.
    pub fn cp_atomic(&self) -> anyhow::Result<CowAtomicDirGaurd> {
        dir_cp_atomic(&self.path)
    }
}

pub fn dir_cp<P: AsRef<Path>>(orig: P) -> anyhow::Result<CowDirGaurd> {
    let path = path_hidden_with_extension(&orig, ".tmp")?;
    copy_recursive(&orig, &path)?;
    Ok(CowDirGaurd {
        path,
        orig: orig.as_ref().to_path_buf(),
    })
}

pub fn dir_cp_atomic<P: AsRef<Path>>(parent: P) -> anyhow::Result<CowAtomicDirGaurd> {
    let parent = parent.as_ref().to_path_buf();
    let current = parent.join("current");
    let name = Uuid::new_v4().to_string();
    let path = parent.join(&name);
    if current.exists() {
        let orig = parent.join(fs::read_link(current)?);
        copy_recursive(&orig, &path)?;
        Ok(CowAtomicDirGaurd {
            parent,
            name,
            path,
            orig: Some(orig),
        })
    } else {
        fs::create_dir_all(&path)?;
        Ok(CowAtomicDirGaurd {
            parent,
            name,
            path,
            orig: None,
        })
    }
}

pub struct CowDirGaurd {
    pub path: PathBuf,
    orig: PathBuf,
}

impl CowDirGaurd {
    /// Directory commits are not strictly atomic because rename cannot be used to target a
    /// non-empty directory. This means commits are implemented as two rename operations, first
    /// the target is renamed as a backup, then the copy is renamed to place at the original
    /// location. The only way for the database to be left in an inconsistent state is if a
    /// catastrophic failure occurs between these two renames.
    pub fn commit(self) -> anyhow::Result<()> {
        let mut ext = ".bak".to_string();
        ext.push_str(Uuid::new_v4().to_string().as_str());
        let bak = path_hidden_with_extension(&self.path, ext.as_str())?;
        fs::rename(&self.orig, &bak)?;
        if let Err(e) = fs::rename(&self.path, &self.orig) {
            fs::rename(&bak, &self.orig)?;
            return Err(anyhow!(e));
        }
        if let Err(e) = fs::remove_dir_all(&bak) {
            // swallow error since it does not indicate failed commit
            eprintln!("failed to cleanup dir {:?}, error: {:?}", bak, e)
        }
        Ok(())
    }
}

pub struct CowAtomicDirGaurd {
    parent: PathBuf,
    name: String,
    pub path: PathBuf,
    orig: Option<PathBuf>,
}

impl CowAtomicDirGaurd {
    pub fn commit(self) -> anyhow::Result<()> {
        let current = self.parent.join("current");
        let current_tmp = self.parent.join(".current.tmp");

        let current_rel = PathBuf::from(self.name);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&current_rel, &current_tmp)?;
        }

        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(self.path, &current_rel)?;
        }

        // atomic commit
        fs::rename(&current_tmp, current)?;

        if let Some(orig) = self.orig {
            if let Err(e) = fs::remove_dir_all(&orig) {
                // swallow error since it does not indicate failed commit
                eprintln!("failed to cleanup dir {:?}, error: {:?}", orig, e)
            }
        }
        Ok(())
    }
}

fn path_hidden_with_extension<P: AsRef<Path>>(path: P, ext: &str) -> anyhow::Result<PathBuf> {
    path_modify_filename(path, |name| {
        let mut result = OsString::new();
        result.push(".");
        result.push(&name);
        result.push(ext);
        name.clear();
        name.push(result);
    })
}

fn path_modify_filename<P: AsRef<Path>, F: FnOnce(&mut OsString)>(
    path: P,
    modify: F,
) -> anyhow::Result<PathBuf> {
    let mut name = path
        .as_ref()
        .file_name()
        .context("not a valid path")?
        .to_os_string();
    modify(&mut name);
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
    let path_lock = path_hidden_with_extension(&path, ".lock")?;
    let path_queue = path_hidden_with_extension(&path, ".queue")?;

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
    let path_lock = path_hidden_with_extension(&path, ".lock")?;
    let path_queue = path_hidden_with_extension(&path, ".queue")?;

    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .open(path_lock)
        .context("could not open lock file")?;

    let queue = OpenOptions::new()
        .write(true)
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
            eprintln!("{:?}", e);
        }
    }
}

fn copy_recursive(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> anyhow::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    // Create destination directory if it doesn't exist
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(file_name);

        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_recursive(&entry_path, &dest_path)?;
        } else if file_type.is_file() {
            reflink_or_copy(&entry_path, &dest_path)?;
        } else if file_type.is_symlink() {
            let link_target = fs::read_link(&entry_path)?;

            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&link_target, &dest_path)?;
            }

            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(&link_target, &dest_path)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::{
        fs::{self, File},
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::Duration,
    };

    use anyhow::Context;
    use path_dsl::path;
    use rand::Rng;
    use uuid::Uuid;

    use crate::{Client, ReadLock, WriteLock};

    struct TestClient {
        pub client: Client,
        root: PathBuf,
    }

    impl TestClient {
        pub fn new(name: &str) -> anyhow::Result<Self> {
            let root = std::env::temp_dir()
                .join(name.to_string() + "-" + Uuid::new_v4().to_string().as_str());
            Ok(TestClient {
                client: Client::new(&root)?,
                root,
            })
        }
    }

    impl Drop for TestClient {
        fn drop(&mut self) {
            if let Err(e) = fs::remove_dir_all(&self.root) {
                eprintln!("failed to delete test db: {:?}", e);
            }
        }
    }

    #[test]
    #[allow(unused_variables)]
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
                    // rec.lock().unwrap().push('r');
                    rcnt.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    if wcnt.load(Ordering::Acquire) > 0 {
                        panic!("can't have readers and writers")
                    }
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=10)));
                    rcnt.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                } else {
                    thread::sleep(Duration::from_millis(rng.gen_range(1..=10)));
                    let _gaurd = WriteLock::new(tmp_file_path).unwrap();
                    // rec.lock().unwrap().push('w');
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

    #[test]
    fn test_readme_example() -> anyhow::Result<()> {
        let test_client = TestClient::new("test_readme_example")?;
        let db = &test_client.client;

        let some_dir = db.root().join(path!("some" | "dir"));
        fs::create_dir_all(&some_dir)?;
        let created = fs::metadata(some_dir)
            .context("could not get metadata")?
            .created()?;
        eprint!("{:?}", db.root().join("test_write.txt"));
        File::create(db.root().join("test_write.txt"))?;
        File::create(db.root().join("collatz_in.txt"))?;
        fs::write(db.root().join("collatz_in.txt"), "500")?;
        File::create(db.root().join("collatz_out.txt"))?;

        {
            let gaurd = db.read_dir(path!("some" | "dir"))?;
            let metadata = fs::metadata(gaurd.path).context("could not get metadata")?;
            assert_eq!(created, metadata.created()?);
        }

        {
            let _gaurd = db.write_dir(path!("some" | "dir"));
            fs::create_dir(db.root.join(path!("some" | "dir" | "new")))?;
        }

        {
            let gaurd = db.write_file("test_write.txt")?;
            let cp = gaurd.cp()?;
            fs::write(&cp.path, "some content")?;
            cp.commit()?;
        }

        {
            let tx = db
                .tx()
                .read("collatz_in.txt")
                .write("collatz_out.txt")
                .begin()?;
            let n = fs::read_to_string(db.root.join("collatz_in.txt"))?
                .trim()
                .parse::<i64>()?;
            if n > 1 {
                let n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
                let cp = tx.file_cp("collatz_out.txt")?;
                fs::write(&cp.path, n.to_string())?;
                cp.commit()?;
            }
        }

        Ok(())
    }

    #[test]
    fn test_dir_cp() -> anyhow::Result<()> {
        let test_client = TestClient::new("test_dir_cp")?;
        let db = &test_client.client;

        {
            let gaurd = db.write_dir("")?;
            let dir = gaurd.cp()?;
            let dir1 = dir.path.join("dir1");
            let dir2 = dir.path.join("dir2");
            let test1 = dir1.join("test.txt");
            let test2 = dir2.join("test.txt");
            fs::create_dir(&dir1)?;
            fs::create_dir(&dir2)?;
            File::create(&test1)?;
            File::create(&test2)?;
            fs::write(&test1, "content1")?;
            fs::write(&test2, "content1")?;
            dir.commit()?;
        }

        {
            let gaurd = db.read_file("dir1/test.txt")?;
            assert_eq!("content1", fs::read_to_string(gaurd.path)?);
        }

        {
            let gaurd = db.read_file("dir2/test.txt")?;
            assert_eq!("content1", fs::read_to_string(gaurd.path)?);
        }

        {
            let gaurd = db.write_dir("")?;
            let dir = gaurd.cp()?;
            let dir1 = dir.path.join("dir1");
            let dir2 = dir.path.join("dir2");
            let dir3 = dir.path.join("dir3");
            let test1 = dir1.join("test.txt");
            let test3 = dir3.join("test.txt");
            fs::remove_dir_all(dir2)?;
            fs::create_dir(&dir3)?;
            File::create(&test3)?;
            fs::write(&test1, "content2")?;
            fs::write(&test3, "content2")?;
            dir.commit()?;
        }

        {
            let gaurd = db.read_file("dir1/test.txt")?;
            assert_eq!("content2", fs::read_to_string(gaurd.path)?);
        }

        {
            let gaurd = db.read_file("dir3/test.txt")?;
            assert_eq!("content2", fs::read_to_string(gaurd.path)?);
        }

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_dir_cp_atomic() -> anyhow::Result<()> {
        use crate::dir_cp_atomic;

        let test_client = TestClient::new("test_dir_cp_atomic")?;
        let db = &test_client.client;

        {
            let gaurd = db.write_dir("")?;
            let dir = gaurd.cp_atomic()?;
            let nested_path = dir.path.join("nested");
            fs::create_dir(&nested_path)?;
            dir_cp_atomic(&nested_path)?.commit()?;
            let nested_cur_path = nested_path.join("current");
            let test_path = nested_cur_path.join("test.txt");
            File::create(&test_path)?;
            fs::write(&test_path, "test1")?;
            dir.commit()?;
        }

        {
            let gaurd = db.read_file("current/nested/current/test.txt")?;
            assert_eq!("test1", fs::read_to_string(gaurd.path)?);
        }

        {
            let gaurd = db.write_dir("")?;
            let dir = gaurd.cp_atomic()?;
            let test_path = dir.path.join("nested/current/test.txt");
            fs::write(&test_path, "test2")?;
            dir.commit()?;
        }

        {
            let gaurd = db.read_file("current/nested/current/test.txt")?;
            assert_eq!("test2", fs::read_to_string(gaurd.path)?);
        }

        Ok(())
    }

    #[test]
    fn test_tx_operations() -> anyhow::Result<()> {
        let test_client = TestClient::new("test_tx_operations")?;
        let db = &test_client.client;

        {
            let gaurd = db.write_dir("")?;
            let cp = gaurd.cp()?;
            let nested = cp.path.join("nested");
            let read = nested.join("read.txt");
            let writes = nested.join("writes");
            let write1 = writes.join("write1.txt");
            let write2 = writes.join("write2.txt");
            fs::create_dir_all(&nested)?;
            File::create(&read)?;
            fs::create_dir(&writes)?;
            File::create(&write1)?;
            File::create(&write2)?;
            fs::write(&read, "1")?;
            fs::write(&write1, "0")?;
            fs::write(&write2, "0")?;
            cp.commit()?;
        }

        {
            let tx = db
                .tx()
                .read("nested/read.txt")
                .write("nested/writes/write1.txt")
                .write("nested/writes/write2.txt")
                .write("nested/writes") // purposefully add more write protection than neccessary
                .begin()?;
            let cp = tx.dir_cp("nested/writes")?;
            let write1 = cp.path.join("write1.txt");
            let write2 = cp.path.join("write2.txt");

            let n = fs::read_to_string(db.root().join("nested/read.txt"))?
                .trim()
                .parse::<i64>()?;

            fs::write(write1, (n + 1).to_string())?;
            fs::write(write2, (n + 2).to_string())?;

            cp.commit()?;
        }

        {
            let gaurd = db.read_file("nested/writes/write1.txt")?;
            let n = fs::read_to_string(gaurd.path)?.trim().parse::<i64>()?;
            assert_eq!(2, n);
        }

        {
            let gaurd = db.read_file("nested/writes/write2.txt")?;
            let n = fs::read_to_string(gaurd.path)?.trim().parse::<i64>()?;
            assert_eq!(3, n);
        }

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_tx_operations_atomic_cp() -> anyhow::Result<()> {
        use crate::dir_cp_atomic;

        let test_client = TestClient::new("test_tx_operations_atomic_cp")?;
        let db = &test_client.client;

        {
            let gaurd = db.write_dir("")?;
            let cp = gaurd.cp()?;
            let nested = cp.path.join("nested");
            let read = nested.join("read.txt");
            let writes = nested.join("writes");
            let write1 = writes.join("current/write1.txt");
            let write2 = writes.join("current/write2.txt");
            fs::create_dir_all(&nested)?;
            File::create(&read)?;
            fs::create_dir(&writes)?;
            dir_cp_atomic(writes)?.commit()?;
            File::create(&write1)?;
            File::create(&write2)?;
            fs::write(&read, "1")?;
            fs::write(&write1, "0")?;
            fs::write(&write2, "0")?;
            cp.commit()?;
        }

        {
            let tx = db
                .tx()
                .read("nested/read.txt")
                .write("nested/writes/current/write1.txt")
                .write("nested/writes/current/write2.txt")
                .write("nested/writes") // purposefully add more write protection than neccessary
                .begin()?;
            let cp = tx.dir_cp_atomic("nested/writes")?;
            let write1 = cp.path.join("write1.txt");
            let write2 = cp.path.join("write2.txt");

            let n = fs::read_to_string(db.root().join("nested/read.txt"))?
                .trim()
                .parse::<i64>()?;

            fs::write(write1, (n + 1).to_string())?;
            fs::write(write2, (n + 2).to_string())?;

            cp.commit()?;
        }

        {
            let gaurd = db.read_file("nested/writes/current/write1.txt")?;
            let n = fs::read_to_string(gaurd.path)?.trim().parse::<i64>()?;
            assert_eq!(2, n);
        }

        {
            let gaurd = db.read_file("nested/writes/current/write2.txt")?;
            let n = fs::read_to_string(gaurd.path)?.trim().parse::<i64>()?;
            assert_eq!(3, n);
        }

        Ok(())
    }
}
