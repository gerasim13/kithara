use std::{
    fs,
    io::{ErrorKind, Write},
    path::PathBuf,
    process::ExitCode,
};

fn main() -> ExitCode {
    let Some(nextest_env) = std::env::var_os("NEXTEST_ENV").map(PathBuf::from) else {
        eprintln!("cold-fixture-cache: NEXTEST_ENV not set; refusing to run");
        return ExitCode::FAILURE;
    };

    let cache_dir = std::env::temp_dir().join("kithara-fixture-cold");

    // A separate root keeps cold runs from erasing the persistent fixture cache.
    match fs::remove_dir_all(&cache_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            eprintln!("cold-fixture-cache: remove {cache_dir:?}: {error}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = fs::create_dir_all(&cache_dir) {
        eprintln!("cold-fixture-cache: create {cache_dir:?}: {error}");
        return ExitCode::FAILURE;
    }

    let line = format!("KITHARA_FIXTURE_CACHE={}\n", cache_dir.display());
    match fs::OpenOptions::new().append(true).open(&nextest_env) {
        Ok(mut f) => {
            if let Err(error) = f.write_all(line.as_bytes()) {
                eprintln!("cold-fixture-cache: write NEXTEST_ENV: {error}");
                return ExitCode::FAILURE;
            }
        }
        Err(error) => {
            eprintln!("cold-fixture-cache: open NEXTEST_ENV: {error}");
            return ExitCode::FAILURE;
        }
    }

    println!("cold-fixture-cache: cache at {}", cache_dir.display());
    ExitCode::SUCCESS
}
