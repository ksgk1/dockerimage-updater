use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::utils::Strategy;

#[derive(Parser, Debug)]
#[command(version)]
#[command(long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) mode: Mode,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Mode {
    /// Input mode: Enter a docker image string via stdin and receive the
    /// updated version for a given strategy.
    #[command(alias = "i")]
    Input(InputArguments),

    /// Overview mode: Enter a docker image string via stdin and receive the
    /// all possible upgrades for each available strategy
    #[command(alias = "o")]
    Overview(OverviewArguments),

    /// File mode: Choose a dockerfile and update all images based on a given
    /// strategy.
    #[command(alias = "s")]
    File(SingleFileArguments),

    /// Multi file mode: Enter a folder path, the program will find all
    /// dockerfiles. Specific files can be excluded.
    #[command(alias = "m")]
    Multi(MultiFileArguments),
}

#[derive(Args, Debug, Clone)]
pub struct SingleFileArguments {
    // Using positional argument instead of named argument
    #[arg(value_name = "FILE", help = "Path to the file.")]
    pub(crate) file: PathBuf,

    #[arg(long, help = "Which strategy should be used.", default_value = Strategy::Latest)]
    pub(crate) strat: Strategy,

    #[arg(long, short = 'n', help = "If set will output the new file contents for inspection.")]
    pub(crate) dry_run: bool,

    #[command(flatten)]
    pub(crate) common: CommonOptions,
}

#[derive(Args, Debug, Clone)]
pub struct InputArguments {
    // Using positional argument instead of named argument
    #[arg(value_name = "IMAGE", help = "The full docker image including the tag, that shall be updated.")]
    pub(crate) input: String,

    #[arg(long, help = "Which strategy should be used.", default_value = Strategy::Latest)]
    pub(crate) strat: Strategy,

    #[command(flatten)]
    pub(crate) common: CommonOptions,
}

#[derive(Args, Debug, Clone)]
pub struct OverviewArguments {
    // Using positional argument instead of named argument
    #[arg(value_name = "IMAGE", help = "The full docker image including the tag, that shall be updated.")]
    pub(crate) input: String,

    #[command(flatten)]
    pub(crate) common: CommonOptions,
}

#[derive(Args, Debug, Clone)]
pub struct CommonOptions {
    #[arg(long, short, help = "Will filter out tags only for the given architecture.")]
    pub(crate) arch: Option<String>,

    #[arg(long, help = "Limit the amount of tags to be searched on Docker Hub.")]
    pub(crate) tag_search_limit: Option<u16>,

    #[arg(long, short, help = "Activates debug logging.")]
    pub(crate) debug: bool,

    #[arg(
        long,
        short,
        help = "Will print out only the result or an empty string if no match was found when used in input mode."
    )]
    pub(crate) quiet: bool,
}

#[derive(Args, Debug, Clone)]
pub struct MultiFileArguments {
    // Using positional argument instead of named argument
    #[arg(value_name = "FOLDER", help = "Path to the folder.")]
    pub(crate) folder: PathBuf,

    #[arg(long, help = "Which strategy should be used.", default_value = Strategy::Latest)]
    pub(crate) strat: Strategy,

    #[arg(long, short = 'n', help = "If set will output the new file contents for inspection.")]
    pub(crate) dry_run: bool,

    /// Allows the user to exclude certain files in the folder and its
    /// subfolders.
    #[arg(long, short, help = "The list of files to exclude", required = false, num_args = 0..)]
    pub(crate) exclude_file: Vec<String>,

    /// Allows to ignore certain versions to not be updated, in case of needed
    /// legacy compatibility. This ignore applies globally for all found
    /// files that will be processed.
    #[arg(long, short, help = "The list of versions to ignore (they will not be updated), e.g.: alpine:3.12", required = false, num_args = 0..)]
    pub(crate) ignore_versions: Vec<String>,

    #[command(flatten)]
    pub(crate) common: CommonOptions,
}
