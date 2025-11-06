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

This article is an introduction to and explination of FSDB. Complex features and usage will not be covered thoroughly; instead, we will gradually explore the systems and intuitions behind how it works. It's important to know where we are headed, so to reiterate and expand on the tagline: FSDB is a transactional, concurrent, embedded, key/value database that utilyzes the filesystem as it's storage engine.

Before getting too complicated, let's consider how we can safley read and write a single file from multiple processes. Operating systems (like Windows and Linux) have file locking APIs for multi-reader/single-writer file locking. In the pseudocode of this article we will use the hypothetical functions `lock_shared/unlock_shared` and `lock_exclusive/unlock_exclusive`. It may seem sufficent to simply use these functions, but the problem is that operating systems do not typically garuntee that these locks are fair. Meaning, if a file is constantly being read, the shared lock will always be taken and patiently waiting writers will block indefitely. In a concurrent database, the possibility of writers never making forward progress is not acceptable.

In order for readers to occationally cede precedence to writers, there needs to be a way for prospective readers to know that there are writers waiting before taking a shared lock. We'll say that adjacent to our file in question, there is a new file called _filename_.wwait, shorthand for "writer wating". Writer's will first take an exclusive lock on the writer waiting file before taking an exclusive lock on the file itself. Prospective readers, instead of greedily trying to take a shared lock on the file before doing work, will first take and immediatley release a shared lock on the writer waiting file.

But now we have the opposite problem. If writers are constantly changeing a file, it's possible that readers will be permanantly starved. Thankfully, now that readers know about writers, they can simply choose to be less nice. By adding a wait timeout on readers taking a shared lock on .wwait, readers can choose to become greedy if they have been waiting around for too long.

We've now solved locking a single node but what about when we need to lock many nodes for a transaction. We need to make sure that we don't cause deadlocks when doing so: for instance transaction1 takes a write lock on file1 then file2 and transaction2 takes a write lock on file2 then file1, if these transactions interleve each one will be stuck waiting forever to lock their second file. If we put in place a global ordering on locks, then problem solved, and thankfully we can order locks by lexographically sorting the filename.

Here's the final problem. Filesystems are hierachical so when we lock a given file we don't want other processes modifying the parent directory (i.e. renaming/deleting the file currently being written too). So whenever a lock on a node is taken, first a shared lock is taken on each parent directory starting from the root and going down to the target.

And that's it! If you understood all that, you don't even need this library. Go ahead and write your own version!
