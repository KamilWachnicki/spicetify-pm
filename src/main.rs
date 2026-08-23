mod cache;
mod cli;
mod commands;
mod errors;
mod http;
mod ledger;
mod lockfile;
mod market;
mod platform;
mod spicetify;
mod ui;

use clap::Parser;

#[tokio::main]
async fn main() {
    let args = cli::Cli::parse();

    init_tracing(args.verbose);

    if let Err(err) = platform::enforce(platform::is_privileged(), args.bypass_admin) {
        ui::error(err);
        std::process::exit(1);
    }

    let exit_code = match run(args).await {
        Ok(()) => 0,
        Err(err) => {
            ui::error(err);
            1
        }
    };
    std::process::exit(exit_code);
}

fn init_tracing(verbosity: u8) {
    let filter = match verbosity {
        0 => "off",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}

async fn run(args: cli::Cli) -> errors::Result<()> {
    use cli::{CacheCommands, Commands, SnippetCommands, ThemeCommands};

    let http = http::HttpClient::new(args.no_cache)?;
    if args.apply {
        ui::info("--apply set: `spicetify apply` will run after changes");
        commands::apply_hook::set_flag();
    }

    match args.command {
        Commands::Search {
            query,
            r#type,
            sort,
            page,
            archived,
            json,
        } => commands::search::run(&http, query, r#type, sort, page, archived, json).await,

        Commands::Info { target, json } => commands::info::run(&http, &target, json).await,

        Commands::Install {
            target,
            yes,
            lockfile,
        } => commands::install::run(&http, target, yes, lockfile).await,

        Commands::Lock { out } => commands::lock_cmd::run(out),

        Commands::Remove { target, yes } => commands::remove::run(&http, &target, yes).await,

        Commands::Update { target } => commands::update::run(&http, target).await,

        Commands::List { filter } => commands::list::run(filter.as_ref()),

        Commands::Snippets(cmd) => match cmd {
            SnippetCommands::Search { query, json } => {
                commands::snippets::run_search(&http, query.as_deref(), json).await
            }
            SnippetCommands::Show { name } => commands::snippets::run_show(&http, &name).await,
            SnippetCommands::Add { name } => commands::snippets::run_add(&http, &name).await,
            SnippetCommands::Remove { name } => commands::snippets::run_remove(&http, &name).await,
        },

        Commands::Theme(cmd) => match cmd {
            ThemeCommands::Set { name, scheme } => {
                commands::theme_cmd::run_set(name, scheme.as_deref())
            }
            ThemeCommands::Scheme { scheme } => commands::theme_cmd::run_scheme(&scheme),
            ThemeCommands::Current { json } => commands::theme_cmd::run_current(json),
        },

        Commands::SelfUpdate { check, yes } => {
            commands::self_update::run(&http, check, yes).await
        }

        Commands::Cache(cmd) => match cmd {
            CacheCommands::Path => commands::cache_cmd::run_path(),
            CacheCommands::Size => commands::cache_cmd::run_size(),
            CacheCommands::Clear => commands::cache_cmd::run_clear(),
        },
    }
}
