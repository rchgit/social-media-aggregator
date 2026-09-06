use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use social_media_aggregator::config::Config;
use social_media_aggregator::models::{Network, Post};
use social_media_aggregator::storage::Storage;
use social_media_aggregator::{Error, Result};

#[derive(Parser)]
#[command(
    name = "sma",
    version,
    about = "Aggregate posts across social networks into a topic feed"
)]
struct Cli {
    /// Path to the config file (defaults to ./sma.toml or $SMA_CONFIG).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage accounts.
    Accounts {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Manage topics.
    Topics {
        #[command(subcommand)]
        action: TopicAction,
    },
    /// Fetch new posts from enabled accounts.
    Sync {
        /// Only sync the account with this exact handle.
        #[arg(long)]
        account: Option<String>,
    },
    /// Print the aggregated feed for a topic.
    Feed {
        /// Topic name to display.
        #[arg(long)]
        topic: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Write a sample config file.
    Init,
    /// Print the resolved config path.
    Path,
}

#[derive(Subcommand)]
enum AccountAction {
    /// Add or update an account.
    Add {
        #[arg(long, value_parser = parse_network)]
        network: Network,
        #[arg(long)]
        handle: String,
        #[arg(long, default_value = "")]
        display: String,
        #[arg(long, default_value = "")]
        secret: String,
    },
    /// List accounts.
    List,
    /// Remove an account by id.
    Remove {
        #[arg(long)]
        id: i64,
    },
    /// Enable an account by id.
    Enable {
        #[arg(long)]
        id: i64,
    },
    /// Disable an account by id.
    Disable {
        #[arg(long)]
        id: i64,
    },
}

#[derive(Subcommand)]
enum TopicAction {
    /// Add or update a topic. Keywords are comma-separated.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, value_delimiter = ',')]
        keywords: Vec<String>,
    },
    /// List topics.
    List,
    /// Remove a topic by id.
    Remove {
        #[arg(long)]
        id: i64,
    },
    /// Enable a topic by id.
    Enable {
        #[arg(long)]
        id: i64,
    },
    /// Disable a topic by id.
    Disable {
        #[arg(long)]
        id: i64,
    },
}

fn parse_network(s: &str) -> std::result::Result<Network, String> {
    s.parse().map_err(|_e| {
        format!(
            "unknown network `{s}` (expected one of: facebook, reddit, x, instagram, threads, tiktok)"
        )
    })
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path(cli.config.as_deref());
    let config = Config::load_or_default(&path)?;

    match cli.command {
        None => social_media_aggregator::tui::run(&config),
        Some(Command::Config { action }) => match action {
            ConfigAction::Init => {
                if path.exists() {
                    eprintln!("{} already exists; not overwriting", path.display());
                } else {
                    std::fs::write(&path, Config::sample_toml())?;
                    println!("wrote {}", path.display());
                }
                Ok(())
            }
            ConfigAction::Path => {
                println!("{}", path.display());
                Ok(())
            }
        },
        Some(Command::Accounts { action }) => {
            let storage = Storage::open(&config.db_path())?;
            match action {
                AccountAction::Add {
                    network,
                    handle,
                    display,
                    secret,
                } => {
                    let display = if display.is_empty() {
                        handle.clone()
                    } else {
                        display
                    };
                    let id = storage.add_account(network, &handle, &display, &secret)?;
                    println!("account {id}: {network} {handle}");
                    Ok(())
                }
                AccountAction::List => {
                    for a in storage.list_accounts()? {
                        let state = if a.enabled { "on" } else { "off" };
                        println!(
                            "{:>4} {:<10} {:<20} {:<20} {} secret={}",
                            a.id, a.network, a.handle, a.display_name, state, a.secret_ref
                        );
                    }
                    Ok(())
                }
                AccountAction::Remove { id } => {
                    if storage.remove_account(id)? {
                        println!("removed account {id}");
                    } else {
                        return Err(Error::NotFound(format!("account {id}")));
                    }
                    Ok(())
                }
                AccountAction::Enable { id } => {
                    require_updated(storage.set_account_enabled(id, true)?, "account", id)
                }
                AccountAction::Disable { id } => {
                    require_updated(storage.set_account_enabled(id, false)?, "account", id)
                }
            }
        }
        Some(Command::Topics { action }) => {
            let storage = Storage::open(&config.db_path())?;
            match action {
                TopicAction::Add { name, keywords } => {
                    let id = storage.add_topic(&name, &keywords)?;
                    println!("topic {id}: {name} ({})", keywords.join(", "));
                    Ok(())
                }
                TopicAction::List => {
                    for t in storage.list_topics()? {
                        let state = if t.enabled { "on" } else { "off" };
                        println!(
                            "{:>4} {:<20} {} {}",
                            t.id,
                            t.name,
                            state,
                            t.keywords.join(", ")
                        );
                    }
                    Ok(())
                }
                TopicAction::Remove { id } => {
                    if storage.remove_topic(id)? {
                        println!("removed topic {id}");
                    } else {
                        return Err(Error::NotFound(format!("topic {id}")));
                    }
                    Ok(())
                }
                TopicAction::Enable { id } => {
                    require_updated(storage.set_topic_enabled(id, true)?, "topic", id)
                }
                TopicAction::Disable { id } => {
                    require_updated(storage.set_topic_enabled(id, false)?, "topic", id)
                }
            }
        }
        Some(Command::Sync { account }) => {
            let storage = Storage::open(&config.db_path())?;
            let secrets = config.secret_resolver();
            let report =
                social_media_aggregator::sync::sync(&storage, &secrets, account.as_deref())?;
            for r in report {
                println!(
                    "{} {:<10} {:<20} fetched={} new={} {}",
                    r.account_id,
                    r.network,
                    r.handle,
                    r.fetched,
                    r.inserted,
                    match r.status {
                        social_media_aggregator::sync::SyncStatus::Ok => "ok".to_string(),
                        social_media_aggregator::sync::SyncStatus::Skipped => "skipped".to_string(),
                        social_media_aggregator::sync::SyncStatus::CredentialsRequired(s) => {
                            format!("missing credentials: {s}")
                        }
                        social_media_aggregator::sync::SyncStatus::RateLimited(secs) => {
                            format!("rate limited, retry in {secs}s")
                        }
                        social_media_aggregator::sync::SyncStatus::Error(e) =>
                            format!("error: {e}"),
                    }
                );
            }
            Ok(())
        }
        Some(Command::Feed {
            topic,
            limit,
            offset,
        }) => {
            let storage = Storage::open(&config.db_path())?;
            let topic_id = storage
                .list_topics()?
                .into_iter()
                .find(|t| t.name == topic)
                .map(|t| t.id)
                .ok_or_else(|| Error::NotFound(format!("topic `{topic}`")))?;
            let posts = storage.feed(topic_id, limit, offset)?;
            for p in posts {
                print_post(&p);
            }
            Ok(())
        }
    }
}

fn require_updated(updated: bool, kind: &str, id: i64) -> Result<()> {
    if updated {
        Ok(())
    } else {
        Err(Error::NotFound(format!("{kind} {id}")))
    }
}

fn print_post(p: &Post) {
    let when = p.created_at.format("%Y-%m-%d %H:%M:%S");
    let images = if p.images.is_empty() {
        String::new()
    } else {
        format!(" [{} image(s)]", p.images.len())
    };
    println!(
        "[{}] {} @{} · {}{}",
        p.network.icon(),
        p.author,
        p.account_handle,
        when,
        images
    );
    println!("  {}", p.content.replace('\n', "\n  "));
    if !p.url.is_empty() {
        println!("  {}", p.url);
    }
    println!();
}

fn config_path(flag: Option<&Path>) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    if let Ok(p) = std::env::var("SMA_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from("sma.toml")
}
