mod agent;
mod binding;
mod local_query;
mod publishing;

use crate::cli::{Cli, Command};
use crate::infrastructure::resolve_config;
use crate::support::{AppResult, ok};
use binding::{cmd_bind, cmd_root, cmd_status};
use local_query::{cmd_index, cmd_read, cmd_search};
use publishing::{cmd_plan, cmd_prune, cmd_publish, cmd_sync};
use serde_json::json;

use agent::cmd_agent;

pub(crate) fn run(cli: Cli) -> AppResult<serde_json::Value> {
    match cli.command {
        Command::Agent { command } => cmd_agent(command),
        Command::Root { dir, base_url } => cmd_root(dir, base_url),
        Command::Bind {
            source,
            space,
            parent,
            base_url,
        } => cmd_bind(source, space, parent, base_url),
        Command::Resolve => {
            let resolved = resolve_config()?;
            Ok(ok(json!(resolved)))
        }
        Command::Search { query, all, limit } => cmd_search(&query, all, limit),
        Command::Index { all } => cmd_index(all),
        Command::Read {
            reference,
            line,
            context,
        } => cmd_read(&reference, line, context),
        Command::Plan => cmd_plan(),
        Command::Publish {
            yes,
            dry_run,
            delay_ms,
            concurrency,
        } => cmd_publish(yes, dry_run, delay_ms, concurrency),
        Command::Sync {
            paths,
            yes,
            dry_run,
            delay_ms,
            concurrency,
            archive_deleted,
        } => cmd_sync(paths, yes, dry_run, delay_ms, concurrency, archive_deleted),
        Command::Prune {
            yes,
            archive,
            delete,
        } => cmd_prune(yes, archive, delete),
        Command::Status => cmd_status(),
    }
}
