mod cli;
mod client;
mod lifecycle;
mod media;
mod protocol;
mod qmp;
mod runtime;
mod state;

#[tokio::main]
async fn main() {
    if let Err(error) = cli::run().await {
        eprintln!("wdesk: {error:#}");
        std::process::exit(1);
    }
}
