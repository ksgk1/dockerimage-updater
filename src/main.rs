use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use crate::utils::{handle_file, handle_input, handle_multi};

mod cli;
mod container_image;
mod registries;
mod tag;
mod utils;

fn main() {
    // Needs to be initialised so that ureq can use rustls and not be dependendant
    // on openssl. This makes building for musl a lot easier.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    let cli = cli::Cli::parse();
    let debug = match &cli.mode {
        cli::Mode::File(file_mode) => file_mode.common.debug,
        cli::Mode::Input(input_mode) => input_mode.common.debug,
        cli::Mode::Multi(multi_file_mode) => multi_file_mode.common.debug,
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(if debug { "debug" } else { "info" }));
    let custom_format = fmt::format()
        .with_target(false)
        .with_file(true)
        .with_level(true)
        .with_line_number(true)
        .compact();
    let fmt_layer = fmt::layer().event_format(custom_format);

    // If quiet flag is set, we do not initialise and use the tracing_subscriber.
    // Only (e)print(ln) will be printed.
    if let cli::Mode::Input(input_mode) = &cli.mode {
        if !input_mode.common.quiet {
            tracing_subscriber::registry().with(env_filter).with(fmt_layer).init();
        }
    } else {
        tracing_subscriber::registry().with(env_filter).with(fmt_layer).init();
    }

    match cli.mode {
        cli::Mode::Input(input_mode) => {
            handle_input(&input_mode);
        }
        cli::Mode::File(file_mode) => {
            handle_file(&file_mode);
        }
        cli::Mode::Multi(multi_mode) => {
            handle_multi(&multi_mode);
        }
    }
}
