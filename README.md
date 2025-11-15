# SubsidiaDB

[![Build Status](https://github.com/wilgaboury/sbdb/workflows/build/badge.svg)](https://github.com/wilgaboury/sbdb/actions)
[![codecov](https://codecov.io/github/wilgaboury/sbdb/graph/badge.svg?token=9H65L60DGZ)](https://codecov.io/github/wilgaboury/sbdb)
[![Casual Maintenance Intended](https://casuallymaintained.tech/badge.svg)](https://casuallymaintained.tech/)

A transactional, concurrent, embedded database that utilyzes the filesystem as it's storage engine.

Personal Note: I could spend the time in the world perfecting this library, but I don't currently have a concrete use case so that would be a waste of time. For now this was just an interesting thought.

If you are looking for a explination of how SubsidiaDB works, try reading the included article: [Turning the Filesystem into a Database](./explain.md).

Documentation, examples, and more thorough testing are a WIP.

## Example

```rust
fn main() -> anyhow::Result<()> {
    let db = Client::new("/my/db/path")?;

    {
        let gaurd = db.read_dir(path!("some" | "dir"))?;
        let metadata = fs::metadata(gaurd.path).context("could not get metadata")?;
    }

    {
        let gaurd = db.write_dir(path!("some" | "dir"))?;
        let cp = gaurd.cp()?;
        fs::create_dir(cp.path.join("new_dir"))?;
        File::create(cp.path.join("new_file"))?;
        cp.commit()?;
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
```

### TODO

- get rid of weird bimodal cow_atomic behavior, should have never added that
- make kv store on top of library for performance testing against others
- use fsync everywhere for durable storage