mod metrics;
mod pump;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = match args.next() {
        Some(url) => url,
        None => {
            eprintln!("usage: kit-bench <media-playlist-url> [--paced]");
            std::process::exit(2);
        }
    };
    let paced = args.any(|arg| arg == "--paced");

    match pump::run(&url, paced) {
        Ok(report) => println!("{}", report.to_json_line()),
        Err(err) => {
            eprintln!("kit-bench failed: {err}");
            std::process::exit(1);
        }
    }
}
