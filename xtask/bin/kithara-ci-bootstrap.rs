use std::{env, process};

fn main() {
    match xtask::bootstrap::run(env::args_os().skip(1)) {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("CI bootstrap failed: {error}");
            process::exit(1);
        }
    }
}
