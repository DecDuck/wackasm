use std::{
    env,
    error::Error,
    fs::{self, FileTimes, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime},
};

fn ensure(condition: bool, message: &str) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args().collect::<Vec<_>>();
    ensure(args.len() == 2, "unexpected argument count")?;
    ensure(args[1] == "guest-arg", "argument was not passed through")?;
    ensure(
        env::var("WACKASM_WASI_TEST")? == "enabled",
        "environment variable was not passed through",
    )?;
    println!("args and environment ok");

    let original = Path::new("guest-file.txt");
    let hard_link = Path::new("hard-link.txt");
    let renamed = Path::new("renamed-file.txt");
    let contents = b"preview1 through std";
    let mut file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&original)?;
    println!("preopen and file open ok");
    file.write_all(contents)?;
    file.sync_data()?;
    file.sync_all()?;

    file.set_len(contents.len() as u64)?;
    let metadata = file.metadata()?;
    ensure(metadata.is_file(), "opened path should be a regular file")?;
    ensure(metadata.len() == contents.len() as u64, "file size mismatch")?;
    let now = SystemTime::now();
    file.set_times(FileTimes::new().set_accessed(now).set_modified(now))?;

    file.seek(SeekFrom::Start(0))?;
    let mut read_back = vec![0; contents.len()];
    file.read_exact(&mut read_back)?;
    ensure(read_back == contents, "file contents did not round-trip")?;
    ensure(
        file.seek(SeekFrom::End(0))? == contents.len() as u64,
        "file seek position mismatch",
    )?;
    drop(file);

    fs::hard_link(&original, &hard_link)?;
    fs::rename(&original, &renamed)?;
    ensure(
        fs::metadata(&hard_link)?.len() == contents.len() as u64,
        "hard link stat failed",
    )?;
    fs::remove_file(hard_link)?;
    fs::remove_file(renamed)?;
    println!("file, metadata, and path APIs ok");

    let mut random = [0; 32];
    getrandom::fill(&mut random)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let start = Instant::now();
    thread::sleep(Duration::from_millis(1));
    ensure(start.elapsed() > Duration::ZERO, "monotonic clock did not advance")?;
    ensure(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos()
            > 0,
        "realtime clock was unavailable",
    )?;
    println!("random and clock APIs ok");
    println!("wasip1 std API tests passed");
    Ok(())
}
