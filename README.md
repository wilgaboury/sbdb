# SubsidiaDB

A transactional, concurrent, embedded database that utilyzes the filesystem as it's storage engine.

If you are looking for a explination of how SubsidiaDB works, try reading the included article: [Turning the Filesystem into a Database](./explain.md).

## Use

TODO: crates.io link

## Example

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
