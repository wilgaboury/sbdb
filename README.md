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
    let db = FsdbClient::from_str("/my/database/location");

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

tldr: conservative two-phase file locking

Let's learn how it works by building the systems and intuitions from the ground up. Ultimatley, the goal is to build a library that supports concurrent transactional operations on filesystems.

Most operating systems actually have file locking APIs that allow for multi-reader (shared lock) and single-writer (exclusive lock) locking. So lets start our design by assigning every file and directory in the database an associated lockfile called _filename_.lock. When a process want's to read it calls the lock shared function, which may block and wait for currently running write operations, and when a write happens it calls the lock exclusive function which will block and wait for currently running read operations.

So far so good, but there is a catch. Major operating systems typcially do not garuntee fair-locking. Meaning if a file is constantly being read it's possible that a pantiently waiting writer will never be given exclusive access. It's not acceptable for writers to never proceed in a concurrent multi-client database.

In order to cede precedence to writers, we need a way for prosective readers to know that there are writers waiting, so lets create a mechanism that does that. We'll have a file called _filename_.wwait (for writer wating). Writer's will first take an exclusive lock on the .wwait file before takeing an exclusive lock on the .lock file. Prospective readers, instead of greedily trying to take a shared lock, will first perform a non-blocking try shared lock on the .wwait file: if it succeeds it can be certain that there are no currently waiting writers, and go ahead reading. If not, it will call lock shared on .wwait, thus making it wait until writers have finished their work.

But now we have the opposite problem. If writers are constantly changeing a file, it's possible that readers will be permanantly starved. Thankfully, now that readers know about writers, they can simply choose to be less nice. By adding a wait timeout on readers taking a shared lock on .wwait, readers can choose to become greedy if they have been waiting around for too long.

We've now solved locking a single node but what about when we need to lock many nodes for a transaction. We need to make sure that we don't cause deadlocks when doing so: for instance transaction1 takes a write lock on file1 then file2 and transaction2 takes a write lock on file2 then file1, if these transactions interleve each one will be stuck waiting forever to lock their second file. If we put in place a global ordering on locks, then problem solved, and thankfully we can order locks by lexographically sorting the filename.

Here's the final problem. Filesystems are hierachical so when we lock a given file we don't want other processes modifying the parent directory (i.e. renaming/deleting the file currently being written too). So whenever a lock on a node is taken, first a shared lock is taken on each parent directory starting from the root and going down to the target.

And that's it! If you understood all that, you don't even need this library. Go ahead and write your own version!
