use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum ViewName {
    #[default]
    Overview,
    Hierarchy,
    Ownership,
}

#[derive(Debug, Args)]
pub struct VizArgs {
    /// Projection rendered from the shared evidence graph.
    #[arg(long, value_enum, default_value_t)]
    pub(crate) view: ViewName,

    /// Restrict collection to one Cargo package.
    #[arg(long = "crate")]
    pub(crate) krate: Option<String>,

    /// Restrict the projection to one module path and its descendants.
    #[arg(long)]
    pub(crate) module: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::{Parser, Subcommand};

    use super::{ViewName, VizArgs};

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }

    #[derive(Debug, Subcommand)]
    enum Command {
        Viz(VizArgs),
    }

    #[test]
    fn viz_does_not_require_a_nested_subcommand() {
        let cli = Cli::try_parse_from(["xtask", "viz"]).expect("viz should parse without a view");
        let Command::Viz(args) = cli.command;
        assert_eq!(args.view, ViewName::Overview);
    }

    #[test]
    fn viz_keeps_filters_on_one_command() {
        let cli = Cli::try_parse_from([
            "xtask",
            "viz",
            "--view",
            "ownership",
            "--crate",
            "demo",
            "--module",
            "runtime",
        ])
        .expect("viz filters should parse");
        let Command::Viz(args) = cli.command;
        assert_eq!(args.view, ViewName::Ownership);
        assert_eq!(args.krate.as_deref(), Some("demo"));
        assert_eq!(args.module.as_deref(), Some("runtime"));
    }
}
