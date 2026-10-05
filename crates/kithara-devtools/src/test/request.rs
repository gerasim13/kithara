use anyhow::{Result, bail};

#[derive(Debug)]
pub(super) struct TestRequest {
    pub(super) flash: Option<bool>,
    pub(super) loom: Option<bool>,
    pub(super) net_backend: Option<String>,
    pub(super) no_block: Option<bool>,
    /// Lanes named with `--lane`. One picks the lane to run; with `--touched`
    /// they name the lanes the touched paths may run, the default lane when
    /// none is named.
    pub(super) lanes: Vec<String>,
    pub(super) passthrough: Vec<String>,
    pub(super) touched: bool,
}

impl TestRequest {
    pub(super) fn parse(args: &[String]) -> Result<Self> {
        let mut request = Self {
            lanes: Vec::new(),
            net_backend: None,
            no_block: None,
            loom: None,
            passthrough: Vec::new(),
            flash: None,
            touched: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--flash=off" | "--flash=false" | "--no-flash" => request.flash = Some(false),
                "--flash=on" | "--flash=true" => request.flash = Some(true),
                "--flash" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--flash requires a value"))?;
                    request.flash = Some(parse_toggle("flash", value)?);
                }
                "--no-block=off" | "--no-block=false" => request.no_block = Some(false),
                "--no-block=on" | "--no-block=true" => request.no_block = Some(true),
                "--no-block" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--no-block requires a value"))?;
                    request.no_block = Some(parse_toggle("no-block", value)?);
                }
                "--touched" => request.touched = true,
                "--loom=off" | "--loom=false" | "--no-loom" => request.loom = Some(false),
                "--loom=on" | "--loom=true" => request.loom = Some(true),
                "--loom" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--loom requires a value"))?;
                    request.loom = Some(parse_toggle("loom", value)?);
                }
                "--lane" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--lane requires a value"))?;
                    request.lanes.push(value.clone());
                }
                "--net-backend" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--net-backend requires a value"))?;
                    request.net_backend = Some(value.clone());
                }
                _ if arg.starts_with("--flash=") => {
                    let value = arg.trim_start_matches("--flash=");
                    request.flash = Some(parse_toggle("flash", value)?);
                }
                _ if arg.starts_with("--no-block=") => {
                    let value = arg.trim_start_matches("--no-block=");
                    request.no_block = Some(parse_toggle("no-block", value)?);
                }
                _ if arg.starts_with("--loom=") => {
                    let value = arg.trim_start_matches("--loom=");
                    request.loom = Some(parse_toggle("loom", value)?);
                }
                _ if arg.starts_with("--lane=") => {
                    let value = arg.trim_start_matches("--lane=");
                    request.lanes.push(value.to_owned());
                }
                _ if arg.starts_with("--net-backend=") => {
                    let value = arg.trim_start_matches("--net-backend=");
                    request.net_backend = Some(value.to_owned());
                }
                _ => request.passthrough.push(arg.clone()),
            }
        }
        Ok(request)
    }
}

fn parse_toggle(name: &str, value: &str) -> Result<bool> {
    match value {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        _ => bail!("unsupported {name} mode: {value}"),
    }
}
