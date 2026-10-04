mod cli;
mod client;
mod graphics;
mod lifecycle;
mod media;
mod protocol;
mod qmp;
mod runtime;
mod shares;
mod state;
mod storage;
mod transfers;

#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(error) = cli::run().await {
        eprintln!("wdesk: {error:#}");
        std::process::exit(1);
    }
}
