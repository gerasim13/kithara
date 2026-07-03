mod metrics;
mod pump;

use pump::{Abr, Mode};

fn main() {
    let mut args = std::env::args().skip(1);
    let input = match args.next() {
        Some(input) => input,
        None => {
            usage();
            std::process::exit(2);
        }
    };
    let mut paced = false;
    let mut mode = Mode::Hls;
    let mut abr = Abr::Manual(0);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--paced" => paced = true,
            "--mode" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        usage();
                        std::process::exit(2);
                    }
                };
                mode = match value.as_str() {
                    "hls" => Mode::Hls,
                    "progressive" => Mode::Progressive,
                    "local" => Mode::Local,
                    _ => {
                        usage();
                        std::process::exit(2);
                    }
                };
            }
            "--abr" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        usage();
                        std::process::exit(2);
                    }
                };
                abr = match value.as_str() {
                    "auto" => Abr::Auto,
                    idx => match idx.parse() {
                        Ok(idx) => Abr::Manual(idx),
                        Err(_) => {
                            usage();
                            std::process::exit(2);
                        }
                    },
                };
            }
            _ => {
                usage();
                std::process::exit(2);
            }
        }
    }

    match pump::run(&input, paced, mode, abr) {
        Ok(report) => println!("{}", report.to_json_line()),
        Err(err) => {
            eprintln!("kit-bench failed: {err}");
            std::process::exit(1);
        }
    }
}

fn usage() {
    eprintln!(
        "usage: kit-bench <url-or-path> [--paced] [--mode hls|progressive|local] [--abr auto|<index>]"
    );
}
