# Turning the Filesystem into a Database

This article is an introduction to and explanation of SubsidiaDB. Features and usage will not be covered thoroughly; instead, we will gradually explore the systems and intuitions behind how it works. It's important to know where we are headed, so to reiterate the tagline: SubsidiaDB is a transactional, concurrent, embedded database that utilizes the filesystem as its storage engine.

## File Locking

Let's start by considering how we can safely read and write a single file from multiple processes. Operating systems (like Windows and Linux) have file locking APIs for multi-reader/single-writer file locking. In the pseudocode of this article we will use the hypothetical functions `lock_shared/unlock_shared` and `lock_exclusive/unlock_exclusive`. It may seem sufficient to simply use these functions, but the problem is that operating systems do not guarantee that mixed reading and writing is fair. If, for instance, a file is constantly being read, the shared lock will always be taken and patiently waiting writers will block indefinitely. In a database, the possibility of writers never making forward progress is not acceptable.

In order for add fairness between readers and writers, we will introduce an adjacent file called _filename_.queue. Before either a reader or writer attempts to lock the file, it will first take an exclusive lock on the queue file, then immediately release it once it acquires the file lock (see pseudo-code below). By forcing an initial exclusive synchronization point, incoming concurrent readers and writers will both have an equal chance of making forward progress. While this does come with a performance penalty, in a database, well behaved concurrency with steady throughput is generally preferable over lopsided read/write throughput and exploding tail latencies.

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

## Directories

In treating the filesystem as a database, we additionally need to consider reading and writing directories (by writing I mean creating, deleting, or renaming a directories children). Since directories can't be locked, we will simply have an adjacent file next to each directory called `_dirname_.lock`. Read and write locking on a single directory will then work exactly the same way as files.

It's important to also note at this point that unlike most database systems, which are relatively flat, filesystems are hierarchical. When taking a read or write lock on a file or directory, concurrent modifications to the parent directory may cause problems. To remedy this, our lock procedure (read and write) will first take a shared lock on each parent directory starting from the root and going down to the target.

```rust
fn prepare_for_lock(root, target) {
    for ancestor from root to target (exclusive) {
        lock_read(ancestor)
    }
}
```

## Atomic Modification

Even if an application does have an exclusive write lock on a file, modifying that file is inherently risky because filesystems generally only protect their metadata and make little attempt at preventing file content corruption, in the case of a sudden shutdown or failure for instance. We can minimize this problem, by instead mutating a copy of the file, then performing an atomic rename operation that overwrites the original file with the new version; for those unfamiliar, this pattern is commonly referred to as copy on write.

There is an unfortunate caveat for entire directories. While the atomic rename operation can be used to replace a non-empty file, it cannot be used for replacing non-empty directories. Instead, we must use two sequential rename operations: first, rename the original directory with a backup name, second rename the new directory to replace the original. While this is not strictly atomic, it minimizes the chance of corruption, and is significantly safer than performing mutations on directory contents directly.

I'd also like to make note here that making copies of files and directories will typically add considerable overhead, but some modern filesystems like Btrfs, XFS, APFS and others, actually have support for copy on write operations. This means performing a copy is fast and does not duplicate data on disk; mutations are essentially stored as a diff of the original content.

## Transactions\*

This section header contains an asterisk because transactions in SubsidiaDB do not offer multi-entry rollback; while this is not uncommon in NoSQL databases, it is a notable limitation compared to transactions offered by many popular DBMSs. With that disclaimer out of the way, let's explore how they are implemented.

We have already established a robust locking mechanism, so the obvious algorithm for transactions would be some form of two-phase locking (2PL). For those unfamiliar, the idea is that all necessary locks are acquired (first phase) then released (second phase) strictly in that order. Standard 2PL, where locks are gradually acquired as needed during the transaction, is not a good fit here because it is susceptible to deadlocks, and unlike normal DBMSs, performing global deadlock detection would be very complicated, bordering on infeasible. Instead, SubsidiaDB uses conservative 2PL, so all possible reads and writes must be declared at the beginning of the transaction. This would still not entirely solve deadlocks if locks are acquired in random order, but we can guarantee a total ordering on locks by simply sorting the file paths lexicographically.

## Conclusion

I see this database as filling a neglected niche for applications that need persistent storage. As already expounded upon, filesystems by themselves do not offer the ACID guarantees needed for non-trivial resilient applications. Other embedded databases (like SQLite, RocksDB, LMDB, etc.) are more complex, and by using a single file, prevent truly concurrent multi-process writes. Full DBMSs are heavy on resources and incur higher operational burden; they are also not well suited for applications intending to be distributed and run by others as a single executable. SubsidiaDB is not better than these other solutions, but it brings a different set of tradeoffs to the table.

### Addendum on Hierarchical Databases

The tech nerd and history buff in me feels it necessary to mention that SubsidiaDB falls squarely into the now rarely mentioned category of hierarchical databases. These are the original NoSQL storage solutions, not because of modern trends, but because they predate relational algebra itself! IBM's IMS database is the most popular example and is still widely used in industries today.
