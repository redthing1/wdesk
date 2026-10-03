use crate::{
    client::Client,
    lifecycle, media,
    protocol::Action,
    state::{self, VmConfig},
};
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "A small Windows desktop for agents and people")]
struct Cli {
    #[arg(long, global = true, default_value = "default")]
    session: String,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, env = "WDESK_DESCRIPTOR")]
    descriptor: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check KVM, QEMU, engines and image tooling
    Doctor,
    /// Create, resume or reconnect to a Windows desktop
    Open {
        #[arg(long, default_value = "windows-lite")]
        image: String,
        #[arg(long,default_value="native",value_parser=["native","podman","docker"])]
        engine: String,
        #[arg(long, default_value_t = 4096)]
        memory: u32,
        #[arg(long, default_value_t = 2)]
        cpus: u32,
        #[arg(long)]
        offline: bool,
        #[arg(long, default_value_t = 180)]
        timeout: u64,
        #[arg(long)]
        no_wait: bool,
    },
    /// Report VM, helper and automation readiness
    Status,
    /// Wait for an observable interactive desktop
    Wait {
        #[arg(long, default_value_t = 180)]
        timeout: u64,
    },
    /// Stop the VM while keeping its overlay
    Stop,
    /// Cold boot a fresh overlay and rotate session credentials
    Reset {
        #[arg(long, default_value_t = 180)]
        timeout: u64,
        #[arg(long)]
        no_wait: bool,
    },
    /// Stop this session and move its files to private recovery trash
    Delete,
    /// Print the shared interactive browser URL
    View {
        #[arg(long)]
        browser: bool,
    },
    Capabilities,
    /// Save an exact native framebuffer PNG
    See {
        #[arg(long, default_value = "desktop.png")]
        output: PathBuf,
    },
    Click {
        x: u32,
        y: u32,
        #[arg(long, default_value = "left")]
        button: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Move {
        x: u32,
        y: u32,
    },
    Drag {
        x: u32,
        y: u32,
        to_x: u32,
        to_y: u32,
    },
    /// Type Unicode through the interactive Windows helper
    Type {
        text: String,
        /// Type ASCII physical keys with the guest's US layout, without the helper
        #[arg(long)]
        console: bool,
        #[arg(long)]
        generation: Option<u64>,
    },
    /// Send physical keys, e.g. CTRL+S or WIN+R
    Key {
        chord: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Scroll {
        #[arg(value_parser=["up","down","left","right"])]
        direction: String,
        #[arg(default_value_t = 3)]
        steps: u32,
    },
    /// Submit a versioned, idempotent action batch JSON file
    Batch {
        file: PathBuf,
    },
    Windows {
        #[arg(long)]
        focus: Option<String>,
    },
    A11y {
        #[arg(long, default_value_t = 256)]
        max_nodes: u32,
        #[arg(long, default_value_t = 6)]
        max_depth: u32,
    },
    Clipboard {
        #[command(subcommand)]
        command: Clipboard,
    },
    /// Launch a Windows application with explicit arguments
    Launch {
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    Process {
        #[command(subcommand)]
        command: Process,
    },
    Import {
        local: PathBuf,
        remote: String,
    },
    Export {
        remote: String,
        local: PathBuf,
    },
    /// Emit a data-only descriptor for another agent
    Connect {
        output: PathBuf,
    },
    Image {
        #[command(subcommand)]
        command: Image,
    },
    /// Run a foreground VM and its API; normally managed by open
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: String,
        #[arg(long)]
        owner: Option<String>,
    },
}

#[derive(Subcommand)]
enum Clipboard {
    Get,
    Set { text: String },
}
#[derive(Subcommand)]
enum Process {
    Start {
        #[arg(long, default_value = "workspace")]
        cwd: String,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    Status {
        id: String,
    },
    Output {
        id: String,
    },
    Kill {
        id: String,
    },
    Wait {
        id: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
}
#[derive(Subcommand)]
enum Image {
    List,
    /// List optional, pinned Windows installation media
    Media,
    /// Build the optional OCI QEMU runner from the source checkout
    Build {
        #[arg(long,default_value="podman",value_parser=["podman","docker"])]
        engine: String,
        /// Source checkout to use as the build context
        #[arg(long, default_value = ".")]
        source: PathBuf,
    },
    /// Fetch media with a required published SHA-256
    Fetch {
        #[arg(required_unless_present = "media", conflicts_with = "media")]
        url: Option<String>,
        #[arg(
            long,
            required_unless_present = "media",
            requires = "url",
            conflicts_with = "media"
        )]
        sha256: Option<String>,
        /// Select optional pinned media; downloads only on this explicit request
        #[arg(long, conflicts_with_all = ["url", "sha256"])]
        media: Option<String>,
        /// Destination; named media defaults to the private media cache
        #[arg(long, required_unless_present = "media")]
        output: Option<PathBuf>,
    },
    /// Install Windows unattended, wait for the helper, then seal a reusable base
    Install {
        #[arg(long, required_unless_present = "media", conflicts_with = "media")]
        iso: Option<PathBuf>,
        /// Explicitly fetch/cache a pinned preset before installation
        #[arg(long, conflicts_with = "iso")]
        media: Option<String>,
        #[arg(long, default_value = "windows-lite")]
        name: String,
        /// Defaults to the selected media's profile, or lite for local ISO media
        #[arg(long,value_parser=["reference","lite","core"])]
        profile: Option<String>,
        /// Compress the immutable base; overlays remain ordinary writable qcow2
        #[arg(long)]
        compress: bool,
        /// Install Core media's OS files compressed, preserving their functionality
        #[arg(long)]
        compact_os: bool,
        #[arg(long, default_value_t = 1)]
        index: u32,
        /// Edition selection key only; the default selects Pro and does not activate Windows
        #[arg(long, default_value = "VK7JG-NPHTM-C97JM-9MPGT-3V66T")]
        install_key: String,
        #[arg(long, default_value_t = 64)]
        disk_gb: u32,
        #[arg(long, default_value_t = 1800)]
        timeout: u64,
        #[arg(long,default_value="native",value_parser=["native","podman","docker"])]
        engine: String,
        #[arg(long)]
        no_wait: bool,
    },
    /// Import an already provisioned qcow2 image (helper must be installed)
    Import {
        disk: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "reference")]
        profile: String,
        #[arg(long)]
        compress: bool,
    },
    /// Seal a stopped or running provisioned session as a base image
    Seal {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "reference")]
        profile: String,
        #[arg(long)]
        compress: bool,
    },
}

fn print(value: Value, json_output: bool) {
    if json_output {
        println!("{}", serde_json::to_string(&value).expect("JSON output"));
    } else if let Some(text) = value.as_str() {
        println!("{text}");
    } else if let Some(text) = value.get("text").and_then(Value::as_str) {
        println!("{text}");
    } else if let Some(completed) = value.get("completed").and_then(Value::as_u64) {
        println!(
            "Delivered {completed} action(s) · input {}",
            value["input_generation"]
        );
    } else if let Some(windows) = value.get("windows").and_then(Value::as_array) {
        for window in windows {
            println!(
                "{}  {}{}",
                window["id"].as_str().unwrap_or("?"),
                if window["focused"] == true {
                    "* "
                } else {
                    "  "
                },
                window["title"].as_str().unwrap_or("")
            );
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON output")
        );
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    state::validate_name(&cli.session)?;
    let result = match cli.command {
        Command::Doctor => lifecycle::doctor().await?,
        Command::Serve {
            state_dir,
            bind,
            owner: _,
        } => return crate::runtime::serve(state_dir, bind).await,
        Command::Open {
            image,
            engine,
            memory,
            cpus,
            offline,
            timeout,
            no_wait,
        } => {
            lifecycle::open(
                &cli.session,
                &image,
                &engine,
                VmConfig {
                    memory_mb: memory,
                    cpus,
                    offline,
                    ..Default::default()
                },
            )
            .await?;
            if !no_wait {
                lifecycle::wait(&cli.session, timeout).await?
            } else {
                json!({"session":cli.session,"state":"vm_running"})
            }
        }
        Command::Stop => {
            lifecycle::stop(&cli.session).await?;
            json!({"session":cli.session,"state":"stopped"})
        }
        Command::Delete => {
            lifecycle::delete(&cli.session).await?;
            json!({"session":cli.session,"state":"deleted"})
        }
        Command::Reset { timeout, no_wait } => {
            lifecycle::reset(&cli.session).await?;
            if !no_wait {
                lifecycle::wait(&cli.session, timeout).await?
            } else {
                json!({"session":cli.session,"state":"vm_running"})
            }
        }
        Command::Wait { timeout } => lifecycle::wait(&cli.session, timeout).await?,
        Command::View { browser } => {
            let url = lifecycle::viewer_url(&cli.session)?;
            if browser {
                tokio::process::Command::new("xdg-open")
                    .arg(&url)
                    .spawn()
                    .context("opening browser")?;
            }
            json!(url)
        }
        Command::Image { command } => match command {
            Image::List => lifecycle::images()?,
            Image::Media => serde_json::to_value(media::PRESETS)?,
            Image::Build { engine, source } => {
                lifecycle::build(&engine, &source).await?;
                json!({"image":lifecycle::RUNNER_IMAGE,"engine":engine})
            }
            Image::Fetch {
                url,
                sha256,
                output,
                media,
            } => {
                if let Some(id) = media {
                    media::fetch(&id, output.as_deref()).await?
                } else {
                    lifecycle::fetch(
                        &url.context("URL required")?,
                        &sha256.context("SHA-256 required")?,
                        &output.context("output required")?,
                    )
                    .await?
                }
            }
            Image::Import {
                disk,
                name,
                profile,
                compress,
            } => lifecycle::import_image(&disk, &name, &profile, None, json!({}), compress).await?,
            Image::Seal {
                name,
                profile,
                compress,
            } => lifecycle::seal(&cli.session, &name, &profile, None, compress).await?,
            Image::Install {
                iso,
                media,
                name,
                profile,
                compress,
                compact_os,
                index,
                install_key,
                disk_gb,
                timeout,
                engine,
                no_wait,
            } => {
                state::validate_name(&name)?;
                ensure!(
                    !state::root().join("images").join(&name).exists(),
                    "image already exists"
                );
                let (iso, default_profile) = if let Some(id) = media {
                    let preset = media::preset(&id)?;
                    lifecycle::validate_compact_profile(preset.profile, compact_os)?;
                    let fetched = media::fetch(&id, None).await?;
                    (
                        PathBuf::from(fetched["path"].as_str().context("media path missing")?),
                        preset.profile,
                    )
                } else {
                    (iso.context("ISO required")?, "lite")
                };
                let profile = profile.unwrap_or_else(|| default_profile.into());
                lifecycle::validate_compact_profile(&profile, compact_os)?;
                lifecycle::install(
                    &iso,
                    &name,
                    lifecycle::InstallOptions {
                        profile: &profile,
                        index,
                        disk_gb,
                        engine: &engine,
                        install_key: &install_key,
                        compact_os,
                    },
                )
                .await?;
                let session = format!("build-{name}");
                if no_wait {
                    json!({"session":session,"state":"installing","next":format!("wdesk --session {session} wait --timeout {timeout}")})
                } else {
                    lifecycle::wait(&session, timeout).await?;
                    lifecycle::seal(&session, &name, &profile, Some(&iso), compress).await?
                }
            }
        },
        command => {
            let client = Client::discover(&cli.session, cli.descriptor.as_deref())?;
            match command {
                Command::Status => client.get("/v1/health").await?,
                Command::Capabilities => client.get("/v1/capabilities").await?,
                Command::See { output } => client.screenshot(&output).await?,
                Command::Click {
                    x,
                    y,
                    button,
                    generation,
                } => {
                    client
                        .action(vec![Action::Click { x, y, button }], generation)
                        .await?
                }
                Command::Move { x, y } => client.action(vec![Action::Move { x, y }], None).await?,
                Command::Drag { x, y, to_x, to_y } => {
                    client
                        .action(vec![Action::Drag { x, y, to_x, to_y }], None)
                        .await?
                }
                Command::Type {
                    text,
                    console,
                    generation,
                } => {
                    client
                        .action(
                            vec![if console {
                                Action::TypeAscii { text }
                            } else {
                                Action::TypeText { text }
                            }],
                            generation,
                        )
                        .await?
                }
                Command::Key { chord, generation } => {
                    client
                        .action(
                            vec![Action::Key {
                                keys: chord.split('+').map(String::from).collect(),
                            }],
                            generation,
                        )
                        .await?
                }
                Command::Scroll { direction, steps } => {
                    client
                        .action(vec![Action::Scroll { direction, steps }], None)
                        .await?
                }
                Command::Batch { file } => {
                    let batch: crate::protocol::Batch = state::read_json(&file)?;
                    let value = client
                        .post("/v1/batch", serde_json::to_value(batch)?)
                        .await?;
                    print(value.clone(), cli.json);
                    ensure!(
                        value["results"]
                            .as_array()
                            .is_some_and(|r| r.iter().all(|r| r["delivered"] == true)),
                        "partial batch delivery"
                    );
                    return Ok(());
                }
                Command::Windows { focus } => {
                    if let Some(id) = focus {
                        client.guest("focus", json!({"id":id})).await?
                    } else {
                        client.guest("windows", json!({})).await?
                    }
                }
                Command::A11y {
                    max_nodes,
                    max_depth,
                } => {
                    client
                        .guest("a11y", json!({"max_nodes":max_nodes,"max_depth":max_depth}))
                        .await?
                }
                Command::Clipboard { command } => match command {
                    Clipboard::Get => client.guest("clipboard_get", json!({})).await?,
                    Clipboard::Set { text } => {
                        client.guest("clipboard_set", json!({"text":text})).await?
                    }
                },
                Command::Launch { argv } => client.guest("launch", json!({"argv":argv})).await?,
                Command::Process { command } => match command {
                    Process::Start { cwd, timeout, argv } => {
                        client
                            .guest(
                                "process_start",
                                json!({"argv":argv,"cwd":cwd,"timeout_seconds":timeout}),
                            )
                            .await?
                    }
                    Process::Status { id } | Process::Output { id } => {
                        client.guest("process_status", json!({"id":id})).await?
                    }
                    Process::Kill { id } => client.guest("process_kill", json!({"id":id})).await?,
                    Process::Wait { id, timeout } => {
                        let deadline =
                            tokio::time::Instant::now() + Duration::from_secs(timeout.min(3600));
                        loop {
                            let result = client.guest("process_status", json!({"id":id})).await?;
                            if result["running"] == false {
                                break result;
                            }
                            ensure!(
                                tokio::time::Instant::now() < deadline,
                                "process still running after wait deadline (not killed)"
                            );
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                    }
                },
                Command::Import { local, remote } => client.import(&local, &remote).await?,
                Command::Export { remote, local } => client.export(&remote, &local).await?,
                Command::Connect { output } => {
                    state::write_json(&output, &client.descriptor)?;
                    json!({"descriptor":output,"authority":"data_plane_only"})
                }
                _ => unreachable!(),
            }
        }
    };
    print(result, cli.json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_source_is_portable_and_explicit() {
        for (args, expected) in [
            (vec!["wdesk", "image", "build"], "."),
            (
                vec!["wdesk", "image", "build", "--source", "checkout"],
                "checkout",
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Image {
                command: Image::Build { source, .. },
            } = cli.command
            else {
                panic!("image build expected")
            };
            assert_eq!(source, PathBuf::from(expected));
        }
    }

    #[test]
    fn guest_and_host_compression_are_independent() {
        for (flag, compact, compressed) in
            [("--compact-os", true, false), ("--compress", false, true)]
        {
            let cli =
                Cli::try_parse_from(["wdesk", "image", "install", "--iso", "windows.iso", flag])
                    .unwrap();
            let Command::Image {
                command:
                    Image::Install {
                        compact_os,
                        compress,
                        ..
                    },
            } = cli.command
            else {
                panic!("image install expected")
            };
            assert_eq!(compact_os, compact);
            assert_eq!(compress, compressed);
        }
    }

    #[test]
    fn media_selection_is_explicit_and_exclusive() {
        for args in [
            vec!["wdesk", "image", "media"],
            vec!["wdesk", "image", "fetch", "--media", "tiny11-25h2"],
            vec!["wdesk", "image", "install", "--media", "tiny11-core-25h2"],
            vec!["wdesk", "image", "install", "--iso", "windows.iso"],
            vec![
                "wdesk",
                "image",
                "fetch",
                "https://example.invalid/windows.iso",
                "--sha256",
                "HASH",
                "--output",
                "windows.iso",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec!["wdesk", "image", "install"],
            vec![
                "wdesk",
                "image",
                "install",
                "--iso",
                "windows.iso",
                "--media",
                "tiny11-25h2",
            ],
            vec![
                "wdesk",
                "image",
                "fetch",
                "https://example.invalid/windows.iso",
                "--output",
                "windows.iso",
            ],
            vec![
                "wdesk",
                "image",
                "fetch",
                "--media",
                "tiny11-25h2",
                "--sha256",
                "HASH",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
