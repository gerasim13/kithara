use std::{
    env,
    fs::{self, OpenOptions},
    io,
    process::{self, Command, ExitStatus},
};

fn main() {
    match run() {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("failed to hold the CI build target: {error}");
            process::exit(1);
        }
    }
}

fn run() -> io::Result<ExitStatus> {
    let mut args = env::args_os().skip(1);
    let lease = args
        .next()
        .ok_or_else(|| io::Error::other("missing lease path"))?;
    let command = args
        .next()
        .ok_or_else(|| io::Error::other("missing command"))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lease)?;
    file.lock_shared()?;
    let _ = env::current_exe().and_then(fs::remove_file);
    Command::new(command).args(args).status()
}
