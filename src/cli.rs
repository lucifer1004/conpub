pub(crate) use agent_plugin_installer::AgentSelector;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "conpub",
    version,
    about = "Publish local knowledge files to Confluence"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Pretty-print JSON output for humans.
    #[arg(long, global = true)]
    pub(crate) pretty: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Manage the conpub Agent Skill plugin.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Set the user-level local knowledge root.
    Root {
        /// Local knowledge-base root directory.
        dir: Option<PathBuf>,
        /// Confluence Cloud base URL, for example https://example.atlassian.net/wiki.
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Bind the current project to a source subdirectory and Confluence parent.
    Bind {
        /// Source path relative to the configured root.
        source: String,
        /// Confluence space key.
        #[arg(long)]
        space: Option<String>,
        /// Confluence parent page ID.
        #[arg(long)]
        parent: Option<String>,
        /// Override the user-level Confluence Cloud base URL for this project.
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Resolve effective configuration for the current project.
    Resolve,
    /// Search local Markdown and Typst files.
    Search {
        /// Case-insensitive query string.
        query: String,
        /// Search the whole configured root instead of the bound source.
        #[arg(long)]
        all: bool,
        /// Maximum number of matches to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Build a persistent local search index.
    Index {
        /// Index the whole configured root instead of the bound source.
        #[arg(long)]
        all: bool,
    },
    /// Read a local file excerpt using a root-relative path or path:line reference.
    Read {
        /// Root-relative path, optionally suffixed with :line.
        reference: String,
        /// 1-based line number. Overrides a :line suffix.
        #[arg(long)]
        line: Option<usize>,
        /// Number of lines before and after the target line.
        #[arg(long, default_value_t = 20)]
        context: usize,
    },
    /// Produce a local publish plan without remote writes.
    Plan,
    /// Publish to Confluence.
    Publish {
        /// Confirm remote writes.
        #[arg(long)]
        yes: bool,
        /// Validate the local publish set without remote writes.
        #[arg(long)]
        dry_run: bool,
        /// Delay between Confluence publish calls.
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        /// Publish concurrency. Currently only 1 is supported.
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
    },
    /// Reconcile local documents with the last successful publish state.
    Sync {
        /// Optional root-relative, source-relative, or directory paths to sync.
        paths: Vec<String>,
        /// Confirm remote writes for changed local documents.
        #[arg(long)]
        yes: bool,
        /// Plan the sync without remote writes.
        #[arg(long)]
        dry_run: bool,
        /// Delay between Confluence publish calls.
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        /// Publish concurrency. Currently only 1 is supported.
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        /// Archive deleted pages whose Confluence page IDs are known in local state.
        #[arg(long)]
        archive_deleted: bool,
    },
    /// Reconcile deleted state entries: drop bookkeeping residue, optionally
    /// archiving or permanently deleting the remote pages first.
    Prune {
        /// Confirm state changes (and remote writes with --archive/--delete).
        #[arg(long)]
        yes: bool,
        /// Archive the Confluence pages of deleted entries before dropping them.
        #[arg(long)]
        archive: bool,
        /// Permanently delete the Confluence pages of deleted entries before dropping them.
        #[arg(long)]
        delete: bool,
    },
    /// Show local configuration and publish status summary.
    Status,
}

#[derive(Subcommand)]
pub(crate) enum AgentCommand {
    /// Check whether native agent plugin commands are available.
    Doctor {
        /// Agent runtime to inspect.
        #[arg(value_enum, default_value = "all")]
        agent: AgentSelector,
    },
    /// Install the conpub plugin from a checkout or unpacked plugin archive.
    Install {
        /// Agent runtime to install into.
        #[arg(value_enum)]
        agent: AgentSelector,
        /// Repository checkout or unpacked conpub plugin archive.
        #[arg(long, value_name = "PATH")]
        from_checkout: PathBuf,
    },
    /// Update an installed conpub plugin through its native marketplace.
    Update {
        /// Agent runtime to update.
        #[arg(value_enum)]
        agent: AgentSelector,
    },
    /// Uninstall the conpub plugin through the native agent CLI.
    Uninstall {
        /// Agent runtime to uninstall from.
        #[arg(value_enum)]
        agent: AgentSelector,
    },
}
