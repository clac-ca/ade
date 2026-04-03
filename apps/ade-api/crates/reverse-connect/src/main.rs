use clap::{Args, Parser, Subcommand};
use reverse_connect::{ConnectOptions, DEFAULT_IDLE_TIMEOUT_SECONDS, connect};

#[derive(Debug, Parser)]
#[command(name = "reverse-connect")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Connect(ConnectArgs),
}

#[derive(Debug, Args)]
struct ConnectArgs {
    #[arg(long)]
    bearer_token: String,
    #[arg(long, default_value_t = DEFAULT_IDLE_TIMEOUT_SECONDS)]
    idle_timeout_seconds: u64,
    #[arg(long)]
    url: String,
}

#[tokio::main]
async fn main() {
    let result = match Cli::parse().command {
        Commands::Connect(args) => {
            connect(ConnectOptions {
                bearer_token: args.bearer_token,
                idle_timeout_seconds: args.idle_timeout_seconds,
                url: args.url,
            })
            .await
        }
    };

    if let Err(error) = result {
        eprintln!("reverse-connect failed: {error}");
        std::process::exit(1);
    }
}
