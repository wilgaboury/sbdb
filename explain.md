# Turning the Filesystem into a Database

This article is an introduction to and explanation of SubsidiaDB. Usage and precise implementation details will not be covered; instead, we will gradually explore the systems and intuitions behind how it works. It's important to know where we are headed, so to reiterate the tagline: SubsidiaDB is a transactional, concurrent, embedded database that utilizes the filesystem as its storage engine.

## File Locking

Let's start by considering how we can safely read and write a single file from multiple processes. All of the major desktop operating systems (Windows, MacOS, and Linux) have file locking APIs supporting multi-reader/single-writer. In pseudocode we will use the hypothetical functions `lock_shared/unlock_shared` and `lock_exclusive/unlock_exclusive`. It may seem sufficient to simply use these functions, but the problem is that none of these operating systems guarantee that mixed reading and writing is fair. If, for instance, a file is constantly being read, the shared lock will always be taken and patiently waiting writers will block indefinitely. In a database, the possibility of writers never making forward progress is not acceptable.

In order for add fairness between readers and writers, we will introduce an adjacent file called `_filename_.queue`. Before either a reader or writer attempts to lock the file, it will first take an exclusive lock on the queue file, then immediately release it once it acquires the file lock (see pseudo-code below). By forcing an initial exclusive synchronization point, incoming concurrent readers and writers will both have an equal chance of making forward progress. While this does come with a performance penalty, in a database, well behaved concurrency with steady throughput is generally preferable over lopsided read/write throughput and exploding tail latencies.

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

In treating the filesystem as a database, we additionally need to consider reading and writing directories (by writing a directory I mean CRUD operations on a directory's children). Since directories can't be locked, we will simply have an adjacent file next to each directory called `_dirname_.lock`. Read and write locking on a single directory will then work exactly the same way as files.

It's important to also note at this point that unlike most database systems, which are relatively flat, filesystems are hierarchical. When taking a read or write lock on a file or directory, concurrent modifications to the parent directory may cause problems. To remedy this, our lock procedure (read and write) will first take a shared lock on each parent directory starting from the root and going down to the target.

```rust
fn prepare_for_lock(root, target) {
    for ancestor from root to target (exclusive) {
        lock_read(ancestor)
    }
}
```

## Atomic Modification

Even if an application does have an exclusive write lock on a file, modifying that file is inherently risky because filesystems generally only protect their metadata and make little attempt at preventing file content corruption. A sudden shutdown or failure during ongoing file writing will leave it in an incomplete state. We can minimize this problem, by instead mutating a copy of the file, then performing an atomic rename that overwrites the original file with the new version; for those unfamiliar, this pattern is commonly referred to as copy-on-write (CoW).

There is an unfortunate caveat for entire directories. While the atomic rename operation can be used to replace a non-empty file, it cannot be used for replacing non-empty directories. Instead, we must use two sequential rename operations: first, rename the original directory with a backup name; second, rename the new directory to replace the original. While this is not strictly atomic, it significatly minimizes the possiblity of a partial commit compared to performing a series of mutations directly.

I'd also like to note here that making copies of files and directories will typically add considerable overhead, but some modern filesystems like Btrfs, XFS, APFS and others, actually have support for CoW operations. This means performing a copy is fast and does not duplicate data on disk; mutations are essentially stored as a diff of the original content.

## Transactions\*

This section header contains an asterisk because transactions in SubsidiaDB do not offer multi-entry rollback; while this is not uncommon in NoSQL databases, it is a notable limitation compared to transactions offered by many popular DBMSs. With that disclaimer out of the way, let's explore how they are implemented.

We have already established a robust locking mechanism, so the obvious algorithm for transactions would be some form of [two-phase locking](https://en.wikipedia.org/wiki/Two-phase_locking) (2PL). For those unfamiliar, the idea is that all necessary locks are acquired (first phase) then released (second phase) strictly in that order. Standard 2PL, where locks are gradually acquired as needed during the transaction, is not a good fit here because it is susceptible to deadlocks, and unlike normal DBMSs, performing global deadlock detection would be very complicated, bordering on infeasible. Instead, SubsidiaDB uses conservative 2PL, so all possible reads and writes must be declared at the beginning of the transaction. This would still not entirely solve deadlocks if locks are acquired in random order, but we can guarantee a total ordering on locks by simply sorting the file paths lexicographically.

Without getting too wrapped up in an analysis of different concurrency control solutions, 2PL is great because it guarantees strict serializability: every transaction is applied as if it was run in order on a single thread. I find that weaker guarantees can be hard to reason about and lead to subtle logic bugs. While many praises have been sung about the performance benefits of MVCC, I think it's validating that Google Spanner, the company's primary OLTP datastore, uses 2PL on write transactions for exactly this reason.

## Atomic Directory Modifications Revisited

So far, I have mentioned two major shortcomings, non-atomic directory commit and lack of multi-entry rollback. SubsidiaDB does offer a solution to these problems, but it comes with drawbacks.

As mentioned before, write commits are atomic for files, so by representing our directory with a symbolic link, which is just a file, we actually can perform atomic commit for an entire directory. Let's demonstrate how it works with a hypothetical directory called `target`. Inside target will exist a symbolic link `target/current` which points to an actual directory with our database content `target/<uuid-1>`. First, we take a write lock on `target`, then copy `target/<uuid-1>` to `target/<uuid-2>` and perform mutations on the copy. To commit, we make a new symbolic link `target/current.tmp` and atomically rename it to `target/current`. Finally, we can safely delete `target/<uuid-1>` to cleanup. Symbolic links aren't an ideal solution in many cases though, as they have problems on Windows and can complicate programmatic traversal. This scheme also adds additional nesting to the database structure, which is harmful to locking performance. For these reasons, it is left to the discretion of users to what extent they employ this feature.

For multi-entry rollback, this may have already been apparent to some readers, but it can be achieved by performing CoW + commit on the highest encompassing parent directory for all the data a transaction touches. This approach can be very heavy handed, and requires the foresight to structure data in accordance with transactions that will be performed.

## Conclusion

This database came about from wrestling with a simple question: how could one create an embedded database that supports truly concurrent multi-process writes? From this starting point, it felt to me like the entire design fell into place as a natural logical progression. I see this system as filling a neglected niche, small applications that want resilient storage with ACID guarantees but don’t want the large leap in complexity of standard embedded databases (or full DBMSs for that matter). Working with files is one of the first proramming topics people learn about, so it’s a huge benefit that this design is simply providing additional safety to a persistence interface that everyone is already familiar with.

### Addendum on Hierarchical Databases

The tech nerd and history buff in me feels it necessary to mention that SubsidiaDB falls squarely into the now rarely mentioned category of [hierarchical databases](https://en.wikipedia.org/wiki/Hierarchical_database_model). These are the original NoSQL storage solutions because they came about in the 1960s and actually predate relational algebra itself! [IBM IMS](https://en.wikipedia.org/wiki/IBM_Information_Management_System) (Information Management System) is the most popular example and is still widely used in industries today.
