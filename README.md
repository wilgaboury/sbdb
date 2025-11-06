# FSDB - FileSystem DataBase

Turn your filesystem into a transactional database.

## Why?

- just a filesystem - provides massive flexibility and enormous amount of existing tooling for the underlying storage engine
- lightweight - essentially just a handful of functions for structured sychronization of file operations
- embedded - all operations occur in-process
- concurrent - multiple processes can safely operate on the same database
- because I thought it was a cool idea 😎

## Why not?

- operations on file contents are not non-atomic by default; althought, the library provides utilities for helping with that at the cost of performance
- High tail latencies caused by lock contention
- Slow list/scan operations
- Minimal/unsafe - the database makes little effort in trying to protect users from themselves.

## Show me Code

```rust
fn main() {
    let db = Client::new("/my/database/location");

    let gaurd = db.read_dir("/some/dir");
    let dir = gaurd.open();
    drop(gaurd);

    let gaurd = db.write_file("/some/dir");
    let file = gaurd.open();
    drop(gaurd);

    let tx = db.tx()
        .reads("/file1")
        .writes("/file2");
    let n = file_read_int(tx, "/file1");
    if (n > 1) {
        let n = if n % 2 == 0 { n/2 } else { 3*n + 1 };
        file_write_int(tx, "/file1", n);
    }
    drop(tx);
}
```

## How Does It Work

This article is an introduction to and explination of FSDB. Features and usage will not be covered thoroughly; instead, we will gradually explore the systems and intuitions behind how it works. It's important to know where we are headed, so to reiterate and expand on the tagline: FSDB is a transactional, concurrent, embedded database that utilyzes the filesystem as it's storage engine.

### File Locking

Let's start by considering how we can safely read and write a single file from multiple processes. Operating systems (like Windows and Linux) have file locking APIs for multi-reader/single-writer file locking. In the pseudocode of this article we will use the hypothetical functions `lock_shared/unlock_shared` and `lock_exclusive/unlock_exclusive`. It may seem sufficent to simply use these functions, but the problem is that operating systems do not garuntee that mixed reading and writing is fair. If, for instance, a file is constantly being read, the shared lock will always be taken and patiently waiting writers will block indefitely. In a database, the possibility of writers never making forward progress is not acceptable.

In order for add fairness between readers and writers, we will introduce an adjacent file called `_filename_.queue`. Before either a reader or writer attempts to lock the file, it will first take an exclusive lock on the queue file, then immediatley release it once it aquires the file lock (see pseudo-code below). By forcing an initial exclusive synchronization point, incoming concurrent readers and writers will both have an equal chance of making forward progress. While this does come with a performance penalty, in a database, well behaved concurrency with steady throughput is generally preferable over lopsided read/write throughput and exploding tail latencies.

```rust
fn lock_read(file) {
    queue = file + ".queue"
    lock_exclusive(queue)
    lock_shared(file)
    unlock_exclusive(queue)
}

fn lock_write(file) {
    queue = file + ".queue"
    lock_exclusive(queue)
    lock_exclusive(file)
    unlock_exclusive(queue)
}
```

### Directories

In treating the filesystem as a database, we additionally need to consider reading and writing directories. Since directories can't be locked directly, we will simply have an adjacent file next to each directory called `_dirname_.lock`. Read and write locking on a single directory will then work exactly the same way as files.

It's important to also note at this point that unlike most database systems, which are reletively flat, filesystems are hierarchical. When taking a read or write lock on a file or directory, concurrent modifications to the parent directory may cause problems. To remedy this, our lock procedure (read and write) will first take a shared lock on each parent directory starting from the root and going down to the target.

```rust
fn prepare_for_lock(root, target) {
    for ancestor from root to target (exclusive) {
        lock_read(ancestor)
    }
}
```

### Atomic Modification

Even if an application does have an exclusive write lock on a file, modifying that file is inherently risky because filesystems generally only protect their metadata and make little attempt at preventing file content corruption, in the case of a sudden shutdown or failure for instance. We can minimize this problem, by instead mutating a copy of the file, then performing an atomic rename operation that overwrites the original file with the new version; for those unfimilar, this pattern is commonly referred to as copy on write.

There is an unfortunate caviate for entire directories. While the atomic rename operation can be used to replace a non-empty file, it cannot be used for replacing non-empty directories. Instead, we must use two sequential rename operations: first, rename the original directory with a backup name, second rename the new directory to replace the original. While this is not strictly atomic, it minimizes the chance of corruption, and is significantly safer than performing a mutations on directory contents directly.

I'd also like to make note here that making copies of files and directories will typically add considerable overhead, but some modern filesystems like Btrfs, XFS, APFS and others, actually use copy on write algorithms internally. This means that copy operations are fast and do not duplicate data on disk; modifications are essentially stored as a diff of the original contents.

### Transactions
