use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[clap(version, about)]

pub struct Cli {
    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Send {
        file: String,
        #[clap(long, default_value = "tcp")]
        protocol: Protocol,
    },
    Receive {
        #[clap(long, default_value = "tcp")]
        protocol: Protocol,
    },
}

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
pub enum Protocol {
    Tcp,
    Quic,
}
