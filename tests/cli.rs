use assert_cmd::Command;
use chrono::{NaiveDate, Utc};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;
use typub_core::{Content, ContentFormat, ContentMeta};
use typub_storage::{PublishResult, StatusTracker};

const SYNC_STATE_VERSION: u32 = 2;
static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct Fixture {
    _tmp: TempDir,
    home: PathBuf,
    root: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().expect("create tempdir");
        let home = tmp.path().join("home");
        let root = tmp.path().join("nv-kb");
        let project = tmp.path().join("project");
        let source = root.join("projects/cuda-agent/perf");

        fs::create_dir_all(&home).expect("create home");
        fs::create_dir_all(&source).expect("create source");
        fs::create_dir_all(&project).expect("create project");
        fs::write(
            source.join("_index.md"),
            "# Performance\n\nPerformance notes.\n",
        )
        .expect("write directory index");
        fs::write(
            source.join("occupancy.md"),
            "# Occupancy\n\nWarp occupancy matters for latency hiding.\n",
        )
        .expect("write markdown");
        fs::write(root.join("projects/cuda-agent/notes.typ"), "= CUDA Notes\n")
            .expect("write typst");

        Self {
            _tmp: tmp,
            home,
            root,
            project,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::cargo_bin("conpub").expect("find conpub binary");
        cmd.env("CONPUB_HOME", &self.home)
            .env_remove("CONFLUENCE_EMAIL")
            .env_remove("CONFLUENCE_API_KEY")
            .env_remove("CONFLUENCE_API_TOKEN")
            .current_dir(&self.project);
        cmd
    }
}

#[test]
fn root_bind_resolve_emit_json() {
    let fixture = Fixture::new();

    let root = run_json(
        fixture
            .command()
            .arg("root")
            .arg(&fixture.root)
            .arg("--base-url")
            .arg("https://example.atlassian.net/wiki"),
    );
    assert_eq!(root["ok"], true);
    assert_eq!(
        root["data"]["base_url"],
        "https://example.atlassian.net/wiki"
    );

    let bind = run_json(
        fixture
            .command()
            .arg("bind")
            .arg("projects/cuda-agent")
            .arg("--space")
            .arg("GPU")
            .arg("--parent")
            .arg("123456789"),
    );
    assert_eq!(bind["ok"], true);
    assert_eq!(bind["data"]["binding"]["source"], "projects/cuda-agent");

    let resolve = run_json(fixture.command().arg("resolve"));
    assert_eq!(resolve["ok"], true);
    assert_eq!(resolve["data"]["source"], "projects/cuda-agent");
    assert_eq!(resolve["data"]["target"]["space"], "GPU");
    assert_eq!(resolve["data"]["target"]["parent_id"], "123456789");
}

#[cfg(unix)]
#[test]
fn agent_doctor_defaults_to_all_runtimes() {
    let fixture = Fixture::new();
    let fake_bin = fixture._tmp.path().join("fake-bin");
    let log = fixture._tmp.path().join("agent.log");
    install_fake_agent_clis(&fake_bin, false);

    let value = run_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("doctor"),
    );

    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["operation"], "doctor");
    assert_eq!(value["data"]["count"], 2);
    let rows = value["data"]["rows"].as_array().expect("rows array");
    assert_eq!(rows[0]["agent"], "codex");
    assert_eq!(rows[0]["status"], "ready");
    assert_eq!(rows[1]["agent"], "claude");
    assert_eq!(rows[1]["status"], "ready");
}

#[test]
fn agent_install_validates_package_before_native_commands() {
    let fixture = Fixture::new();
    let checkout = fixture._tmp.path().join("empty-plugin");
    fs::create_dir_all(&checkout).expect("create empty plugin checkout");

    let value = run_failure_json(
        fixture
            .command()
            .arg("agent")
            .arg("install")
            .arg("all")
            .arg("--from-checkout")
            .arg(&checkout),
    );

    assert_eq!(value["code"], "AGENT_PACKAGE_INVALID");
    let rows = value["details"]["rows"]
        .as_array()
        .expect("error rows array");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["commands"] == json!([])));
    assert!(
        rows[0]["message"]
            .as_str()
            .expect("message")
            .contains(".agents/plugins/marketplace.json")
    );
}

#[test]
fn agent_install_rejects_invalid_manifest_before_native_commands() {
    let fixture = Fixture::new();
    let checkout = fixture._tmp.path().join("plugin-checkout");
    write_plugin_package(&checkout);
    fs::write(
        checkout.join("plugins/conpub/.codex-plugin/plugin.json"),
        "not-json\n",
    )
    .expect("corrupt Codex manifest");

    let value = run_failure_json(
        fixture
            .command()
            .arg("agent")
            .arg("install")
            .arg("codex")
            .arg("--from-checkout")
            .arg(&checkout),
    );

    assert_eq!(value["code"], "AGENT_PACKAGE_INVALID");
    assert!(
        value["details"]["rows"][0]["message"]
            .as_str()
            .expect("message")
            .contains("invalid JSON")
    );
    assert_eq!(value["details"]["rows"][0]["commands"], json!([]));
}

#[cfg(unix)]
#[test]
fn agent_lifecycle_uses_native_plugin_commands_for_all_runtimes() {
    let fixture = Fixture::new();
    let checkout = fixture._tmp.path().join("plugin-checkout");
    let fake_bin = fixture._tmp.path().join("fake-bin");
    let log = fixture._tmp.path().join("agent.log");
    write_plugin_package(&checkout);
    install_fake_agent_clis(&fake_bin, false);

    let install = run_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("install")
            .arg("all")
            .arg("--from-checkout")
            .arg(&checkout),
    );
    assert_eq!(install["data"]["rows"][0]["status"], "installed");
    assert_eq!(install["data"]["rows"][1]["status"], "installed");

    let update = run_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("update")
            .arg("all"),
    );
    assert_eq!(update["data"]["rows"][0]["status"], "updated");
    assert_eq!(update["data"]["rows"][1]["status"], "updated");

    let uninstall = run_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("uninstall")
            .arg("all"),
    );
    assert_eq!(uninstall["data"]["rows"][0]["status"], "uninstalled");
    assert_eq!(uninstall["data"]["rows"][1]["status"], "uninstalled");

    let commands = fs::read_to_string(log).expect("read agent command log");
    assert!(commands.contains("codex plugin marketplace add"));
    assert!(commands.contains("codex plugin add conpub@conpub"));
    assert!(commands.contains("claude plugin marketplace add"));
    assert!(commands.contains("claude plugin install conpub@conpub"));
    assert!(commands.contains("plugin marketplace upgrade conpub"));
    assert!(commands.contains("plugin uninstall conpub"));
}

#[cfg(unix)]
#[test]
fn agent_failure_reports_completed_native_command_prefix() {
    let fixture = Fixture::new();
    let checkout = fixture._tmp.path().join("plugin-checkout");
    let fake_bin = fixture._tmp.path().join("fake-bin");
    let log = fixture._tmp.path().join("agent.log");
    write_plugin_package(&checkout);
    install_fake_agent_clis(&fake_bin, true);

    let value = run_failure_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("install")
            .arg("codex")
            .arg("--from-checkout")
            .arg(&checkout),
    );

    assert_eq!(value["code"], "AGENT_OPERATION_FAILED");
    let commands = value["details"]["rows"][0]["commands"]
        .as_array()
        .expect("commands array");
    assert!(commands.iter().any(|command| {
        command
            .as_str()
            .is_some_and(|command| command.contains("plugin marketplace add"))
    }));
    assert!(commands.iter().any(|command| {
        command
            .as_str()
            .is_some_and(|command| command.contains("plugin add conpub@conpub"))
    }));
}

#[cfg(unix)]
#[test]
fn agent_preflight_failure_blocks_all_runtime_mutations() {
    let fixture = Fixture::new();
    let fake_bin = fixture._tmp.path().join("fake-bin");
    let log = fixture._tmp.path().join("agent.log");
    install_fake_agent_clis(&fake_bin, false);
    fs::remove_file(fake_bin.join("claude")).expect("remove fake Claude CLI");

    let value = run_failure_json(
        fixture
            .command()
            .env("PATH", &fake_bin)
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("update")
            .arg("all"),
    );

    assert_eq!(value["code"], "AGENT_NOT_READY");
    assert_eq!(value["details"]["rows"][0]["status"], "skipped");
    assert_eq!(value["details"]["rows"][1]["status"], "missing");
    let commands = fs::read_to_string(log).expect("read agent command log");
    assert!(!commands.lines().any(|line| {
        line == "codex plugin marketplace upgrade conpub"
            || line == "codex plugin add conpub@conpub"
    }));
}

#[cfg(unix)]
#[test]
fn agent_mutation_failure_skips_later_runtime() {
    let fixture = Fixture::new();
    let checkout = fixture._tmp.path().join("plugin-checkout");
    let fake_bin = fixture._tmp.path().join("fake-bin");
    let log = fixture._tmp.path().join("agent.log");
    write_plugin_package(&checkout);
    install_fake_agent_clis(&fake_bin, true);

    let value = run_failure_json(
        fixture
            .command()
            .env("PATH", path_with(&fake_bin))
            .env("AGENT_TEST_LOG", &log)
            .arg("agent")
            .arg("install")
            .arg("all")
            .arg("--from-checkout")
            .arg(&checkout),
    );

    assert_eq!(value["code"], "AGENT_OPERATION_FAILED");
    assert_eq!(value["details"]["rows"][0]["status"], "failed");
    assert_eq!(value["details"]["rows"][1]["status"], "skipped");
    let commands = fs::read_to_string(log).expect("read agent command log");
    assert!(!commands.lines().any(|line| {
        line.starts_with("claude plugin marketplace add ") && !line.ends_with("--help")
    }));
}

#[test]
fn root_and_bind_accept_conpub_environment_defaults() {
    let fixture = Fixture::new();

    let root = run_json(
        fixture
            .command()
            .env("CONPUB_KB_ROOT", &fixture.root)
            .env("CONPUB_BASE_URL", "https://env.atlassian.net/wiki")
            .arg("root"),
    );
    assert_eq!(root["ok"], true);
    assert_eq!(root["data"]["root"], fixture.root.display().to_string());
    assert_eq!(root["data"]["base_url"], "https://env.atlassian.net/wiki");

    let bind = run_json(
        fixture
            .command()
            .env("CONPUB_SPACE", "ENV")
            .env("CONPUB_PARENT_ID", "987654321")
            .arg("bind")
            .arg("projects/cuda-agent"),
    );
    assert_eq!(bind["ok"], true);
    assert_eq!(bind["data"]["binding"]["space"], "ENV");
    assert_eq!(bind["data"]["binding"]["parent_id"], "987654321");
}

#[test]
fn bind_and_resolve_can_use_environment_root_without_user_config() {
    let fixture = Fixture::new();

    let bind = run_json(
        fixture
            .command()
            .env("CONPUB_KB_ROOT", &fixture.root)
            .env("CONPUB_BASE_URL", "https://env.atlassian.net/wiki")
            .env("CONPUB_SPACE", "ENV")
            .env("CONPUB_PARENT_ID", "987654321")
            .arg("bind")
            .arg("projects/cuda-agent"),
    );
    assert_eq!(bind["ok"], true);
    assert!(!fixture.home.join("conpub.toml").exists());

    let resolve = run_json(
        fixture
            .command()
            .env("CONPUB_KB_ROOT", &fixture.root)
            .arg("resolve"),
    );
    assert_eq!(resolve["ok"], true);
    assert_eq!(resolve["data"]["root"], fixture.root.display().to_string());
    assert_eq!(
        resolve["data"]["target"]["base_url"],
        "https://env.atlassian.net/wiki"
    );
    assert_eq!(resolve["data"]["target"]["space"], "ENV");
    assert_eq!(resolve["data"]["target"]["parent_id"], "987654321");
}

#[test]
fn missing_binding_hint_mentions_conpub_target_environment_defaults() {
    let fixture = Fixture::new();
    run_json(fixture.command().arg("root").arg(&fixture.root));

    let value = run_failure_json(fixture.command().arg("resolve"));

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "CONFIG_MISSING_BINDING");
    let message = value["message"].as_str().expect("message string");
    assert!(message.contains("CONPUB_SPACE"));
    assert!(message.contains("CONPUB_PARENT_ID"));
    assert!(message.contains("CONPUB_BASE_URL"));
}

#[test]
fn search_read_and_plan_use_bound_source() {
    let fixture = Fixture::new();
    configure(&fixture);

    let search = run_json(fixture.command().arg("search").arg("warp occupancy"));
    assert_eq!(search["ok"], true);
    assert_eq!(search["data"]["matches"][0]["line"], 3);
    assert_eq!(
        search["data"]["matches"][0]["read_ref"],
        "projects/cuda-agent/perf/occupancy.md:3"
    );

    let read_ref = search["data"]["matches"][0]["read_ref"]
        .as_str()
        .expect("read_ref is string");
    let read = run_json(
        fixture
            .command()
            .arg("read")
            .arg(read_ref)
            .arg("--context")
            .arg("0"),
    );
    assert_eq!(read["ok"], true);
    assert_eq!(
        read["data"]["lines"][0]["text"],
        "Warp occupancy matters for latency hiding."
    );

    let plan = run_json(fixture.command().arg("plan"));
    assert_eq!(plan["ok"], true);
    assert_eq!(plan["data"]["count"], 3);
}

#[test]
fn document_titles_use_typst_introspection_with_filename_fallback() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/rich-markdown.md"),
        "# **Markdown** _Title_\n\n![missing](missing.png)\n",
    )
    .expect("write rich markdown");
    fs::write(
        fixture.root.join("projects/cuda-agent/rich-typst.typ"),
        "= #strong[Typst] Title\n",
    )
    .expect("write rich typst");
    fs::write(
        fixture.root.join("projects/cuda-agent/no-heading.md"),
        "Body without heading.\n",
    )
    .expect("write headingless markdown");

    let plan = run_json(fixture.command().arg("plan"));

    assert_eq!(plan["ok"], true);
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/rich-markdown.md")["title"],
        "Markdown Title"
    );
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/rich-typst.typ")["title"],
        "Typst Title"
    );
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/no-heading.md")["title"],
        "no-heading"
    );
}

#[test]
fn source_tags_flow_through_local_and_publish_outputs() {
    let fixture = Fixture::new();
    configure(&fixture);
    let markdown =
        "---\ntags: [platform, inferlab, platform]\n---\n# Occupancy\n\nWarp occupancy matters.\n";
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        markdown,
    )
    .expect("write tagged markdown");
    fs::write(
        fixture.root.join("projects/cuda-agent/notes.typ"),
        "#metadata((tags: (\"typst\", \"inferlab\"))) <typub-meta>\n= CUDA Notes\n",
    )
    .expect("write tagged typst");

    let plan = run_json(fixture.command().arg("plan"));
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/perf/occupancy.md")["tags"],
        json!(["inferlab", "platform"])
    );
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/notes.typ")["tags"],
        json!(["inferlab", "typst"])
    );

    let read = run_json(
        fixture
            .command()
            .arg("read")
            .arg("projects/cuda-agent/perf/occupancy.md"),
    );
    assert_eq!(read["data"]["tags"], json!(["inferlab", "platform"]));

    let dry_run = run_json(fixture.command().arg("publish").arg("--dry-run"));
    assert_eq!(
        plan_item(&dry_run, "projects/cuda-agent/perf/occupancy.md")["tags"],
        json!(["inferlab", "platform"])
    );
    let staged = stage_root(&fixture).join("posts/projects-cuda-agent-perf-occupancy/content.md");
    assert_eq!(
        fs::read_to_string(staged).expect("read staged markdown"),
        markdown
    );
}

#[test]
fn search_supports_tag_only_and_text_with_tag_intersection() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "---\ntags: [inferlab, platform]\n---\n# Occupancy\n\nWarp occupancy matters.\n",
    )
    .expect("write tagged markdown");
    fs::write(
        fixture.root.join("projects/cuda-agent/notes.typ"),
        "#metadata((tags: (\"inferlab\", \"typst\"))) <typub-meta>\n= CUDA Notes\n",
    )
    .expect("write tagged typst");

    let tag_only = run_json(fixture.command().arg("search").arg("--tag").arg("inferlab"));
    assert_eq!(tag_only["data"]["query"], Value::Null);
    assert_eq!(tag_only["data"]["tags"], json!(["inferlab"]));
    assert_eq!(
        tag_only["data"]["matches"]
            .as_array()
            .expect("matches")
            .len(),
        2
    );
    assert!(tag_only["data"]["matches"][0]["line"].is_null());
    assert!(tag_only["data"]["matches"][0]["snippet"].is_null());

    let intersection = run_json(
        fixture
            .command()
            .arg("search")
            .arg("occupancy")
            .arg("--tag")
            .arg("platform")
            .arg("--tag")
            .arg("inferlab"),
    );
    assert_eq!(
        intersection["data"]["matches"]
            .as_array()
            .expect("matches")
            .len(),
        2
    );
    assert!(
        intersection["data"]["matches"]
            .as_array()
            .expect("matches")
            .iter()
            .all(|item| item["tags"] == json!(["inferlab", "platform"]))
    );

    run_json(fixture.command().arg("index"));
    let indexed = run_json(
        fixture
            .command()
            .arg("search")
            .arg("--tag")
            .arg("typst")
            .arg("--tag")
            .arg("inferlab"),
    );
    assert_eq!(indexed["data"]["index"]["used"], true);
    assert_eq!(
        indexed["data"]["matches"]
            .as_array()
            .expect("matches")
            .len(),
        1
    );
    assert_eq!(
        indexed["data"]["matches"][0]["path"],
        "projects/cuda-agent/notes.typ"
    );
}

#[test]
fn search_requires_a_filter_and_rejects_non_canonical_tags() {
    let fixture = Fixture::new();
    configure(&fixture);

    let missing = run_failure_json(fixture.command().arg("search"));
    assert_eq!(missing["code"], "SEARCH_FILTER_REQUIRED");

    let invalid = run_failure_json(
        fixture
            .command()
            .arg("search")
            .arg("--tag")
            .arg("Not_Canonical"),
    );
    assert_eq!(invalid["code"], "INVALID_TAG");
}

#[test]
fn source_metadata_rejects_non_canonical_tags() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "---\ntags: [Not_Canonical]\n---\n# Occupancy\n",
    )
    .expect("write invalid metadata");

    let value = run_failure_json(fixture.command().arg("plan"));

    assert_eq!(value["code"], "INVALID_TAG");
}

#[test]
fn source_metadata_rejects_non_array_tags() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "---\ntags: inferlab\n---\n# Occupancy\n",
    )
    .expect("write invalid metadata type");

    let value = run_failure_json(fixture.command().arg("plan"));

    assert_eq!(value["code"], "SOURCE_METADATA_ERROR");
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("array of strings")
    );
}

#[test]
fn index_builds_and_search_uses_fresh_index() {
    let fixture = Fixture::new();
    configure(&fixture);

    let index = run_json(fixture.command().arg("index"));
    assert_eq!(index["ok"], true);
    assert_eq!(index["data"]["documents"], 3);

    let search = run_json(fixture.command().arg("search").arg("warp occupancy"));
    assert_eq!(search["ok"], true);
    assert_eq!(search["data"]["index"]["used"], true);
    assert_eq!(
        search["data"]["matches"][0]["read_ref"],
        "projects/cuda-agent/perf/occupancy.md:3"
    );
}

#[test]
fn publish_dry_run_returns_staged_items_without_credentials() {
    let fixture = Fixture::new();
    configure(&fixture);

    let dry_run = run_json(fixture.command().arg("publish").arg("--dry-run"));
    assert_eq!(dry_run["ok"], true);
    assert_eq!(dry_run["data"]["dry_run"], true);
    assert_eq!(dry_run["data"]["count"], 3);
    assert_eq!(dry_run["data"]["items"][0]["status"], "dry_run");
    let root_item = dry_run["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("root item");
    assert!(root_item["parent_path"].is_null());
    let child_item = dry_run["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/perf/occupancy.md")
        .expect("child item");
    assert_eq!(
        child_item["parent_path"],
        "projects/cuda-agent/perf/_index.md"
    );
    assert!(child_item["parent_id"].is_null());
}

#[test]
fn publish_requires_confirmation_for_remote_writes() {
    let fixture = Fixture::new();
    configure(&fixture);

    let assert = fixture.command().arg("publish").assert().failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("parse JSON");

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "CONFIRMATION_REQUIRED");
}

#[test]
fn publish_yes_reaches_typub_backend_and_validates_credentials() {
    let fixture = Fixture::new();
    configure(&fixture);

    let assert = fixture
        .command()
        .arg("publish")
        .arg("--yes")
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("parse JSON");

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "PUBLISH_CONFIG_ERROR");
}

#[test]
fn publish_item_error_preserves_the_full_context_chain() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("_assets")).expect("create _assets");
    fs::write(fixture.root.join("_assets/figure.png"), b"png").expect("write asset");
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "# Occupancy\n\n![figure](assets/figure.png)\n",
    )
    .expect("rewrite doc with asset reference");

    let base_url = start_confluence_attachment_failure_server();
    configure_with_base_url(&fixture, &base_url);

    let value = run_json(
        fixture
            .command()
            .env("CONFLUENCE_API_KEY", "test-token")
            .env("CONFLUENCE_EMAIL", "test@example.com")
            .arg("publish")
            .arg("--yes"),
    );
    assert_eq!(value["ok"], true);
    let items = value["data"]["items"].as_array().expect("items array");
    let item = items
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/perf/occupancy.md")
        .expect("attachment document item");
    assert_eq!(item["status"], "failed");

    // The outermost context alone used to be the entire message; the item
    // error must now carry the remote cause (status and body) as well.
    let error = item["error"].as_str().expect("error string");
    assert!(
        error.contains("attachment"),
        "outer context names the attachment step: {error}"
    );
    assert!(error.contains("403"), "chain carries the status: {error}");
    assert!(
        error.contains("attachment quota exceeded"),
        "chain carries the remote body: {error}"
    );
}

#[test]
fn publish_missing_base_url_hint_mentions_conpub_environment_default() {
    let fixture = Fixture::new();
    run_json(fixture.command().arg("root").arg(&fixture.root));
    run_json(
        fixture
            .command()
            .arg("bind")
            .arg("projects/cuda-agent")
            .arg("--space")
            .arg("GPU")
            .arg("--parent")
            .arg("123456789"),
    );

    let value = run_failure_json(fixture.command().arg("publish").arg("--yes"));

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "CONFIG_MISSING_BASE_URL");
    assert!(
        value["message"]
            .as_str()
            .expect("message string")
            .contains("CONPUB_BASE_URL")
    );
}

#[test]
fn sync_dry_run_classifies_new_documents_without_credentials() {
    let fixture = Fixture::new();
    configure(&fixture);

    let sync = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(sync["ok"], true);
    assert_eq!(sync["data"]["dry_run"], true);
    assert_eq!(sync["data"]["count"], 3);
    assert_eq!(sync["data"]["publishable"], 3);
    assert_eq!(sync["data"]["summary"]["create"], 3);
    assert_eq!(sync["data"]["summary"]["update"], 0);
    assert_eq!(sync["data"]["items"][0]["action"], "create");
    assert_eq!(sync["data"]["items"][0]["status"], "pending");
    assert!(sync["data"]["items"][0]["fingerprint"].is_string());
}

#[test]
fn sync_requires_confirmation_for_remote_writes() {
    let fixture = Fixture::new();
    configure(&fixture);

    let assert = fixture.command().arg("sync").assert().failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("parse JSON");

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "CONFIRMATION_REQUIRED");
}

#[test]
fn sync_yes_reaches_typub_backend_and_validates_credentials() {
    let fixture = Fixture::new();
    configure(&fixture);

    let assert = fixture
        .command()
        .arg("sync")
        .arg("--yes")
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("parse JSON");

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "PUBLISH_CONFIG_ERROR");
}

#[test]
fn sync_uses_state_to_skip_unchanged_and_detect_update_and_deleted() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);

    let unchanged = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(unchanged["data"]["publishable"], 0);
    assert_eq!(unchanged["data"]["summary"]["unchanged"], 3);

    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "# Occupancy\n\nWarp occupancy changed.\n",
    )
    .expect("update markdown");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let changed = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(changed["data"]["summary"]["update"], 1);
    assert_eq!(changed["data"]["summary"]["deleted"], 1);
    assert_eq!(changed["data"]["publishable"], 1);
}

#[test]
fn sync_state_is_shared_across_source_bindings_for_same_root_and_target() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);

    run_json(
        fixture
            .command()
            .arg("bind")
            .arg("projects/cuda-agent/perf")
            .arg("--space")
            .arg("GPU")
            .arg("--parent")
            .arg("123456789"),
    );

    let narrowed = run_json(fixture.command().arg("sync").arg("--dry-run"));

    assert_eq!(narrowed["ok"], true);
    assert_eq!(narrowed["data"]["stage_root"], first["data"]["stage_root"]);
    assert_eq!(narrowed["data"]["count"], 2);
    assert_eq!(narrowed["data"]["publishable"], 1);
    assert_eq!(narrowed["data"]["summary"]["create"], 0);
    assert_eq!(narrowed["data"]["summary"]["deleted"], 0);
    assert_eq!(narrowed["data"]["summary"]["update"], 1);
    assert_eq!(narrowed["data"]["summary"]["unchanged"], 1);
}

#[test]
fn sync_path_subset_limits_plan_and_omits_global_deleted_entries() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let subset = run_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("perf/occupancy.md"),
    );

    assert_eq!(subset["ok"], true);
    assert_eq!(subset["data"]["subset"], true);
    assert_eq!(subset["data"]["count"], 2);
    assert_eq!(subset["data"]["summary"]["deleted"], 0);
    assert!(
        subset["data"]["items"]
            .as_array()
            .expect("items array")
            .iter()
            .any(|item| item["path"] == "projects/cuda-agent/perf/occupancy.md")
    );
}

#[test]
fn sync_rejects_missing_hierarchy_index_for_non_root_doc() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::remove_file(fixture.root.join("projects/cuda-agent/perf/_index.md"))
        .expect("remove directory index");

    let value = run_failure_json(fixture.command().arg("sync").arg("--dry-run"));

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "HIERARCHY_INDEX_MISSING");
    assert!(
        value["message"]
            .as_str()
            .expect("error message")
            .contains("_index.typ")
    );
}

#[test]
fn sync_path_subset_includes_parent_index_page() {
    let fixture = Fixture::new();
    configure(&fixture);

    let subset = run_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("perf/occupancy.md"),
    );

    assert_eq!(subset["ok"], true);
    assert_eq!(subset["data"]["count"], 2);
    assert_eq!(subset["data"]["publishable"], 2);
    let items = subset["data"]["items"].as_array().expect("items array");
    assert_eq!(items[0]["path"], "projects/cuda-agent/perf/_index.md");
    assert_eq!(items[1]["path"], "projects/cuda-agent/perf/occupancy.md");
    assert_eq!(
        items[1]["parent_path"],
        "projects/cuda-agent/perf/_index.md"
    );
}

#[test]
fn sync_path_subset_accepts_typst_parent_index() {
    let fixture = Fixture::new();
    configure(&fixture);
    let directory = fixture.root.join("projects/cuda-agent/perf");
    fs::remove_file(directory.join("_index.md")).expect("remove markdown directory index");
    fs::write(
        directory.join("_index.typ"),
        "= Performance\n\nPerformance notes.\n",
    )
    .expect("write typst directory index");

    let subset = run_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("perf/occupancy.md"),
    );

    assert_eq!(subset["ok"], true);
    let items = subset["data"]["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["path"], "projects/cuda-agent/perf/_index.typ");
    assert_eq!(items[1]["path"], "projects/cuda-agent/perf/occupancy.md");
    assert_eq!(
        items[1]["parent_path"],
        "projects/cuda-agent/perf/_index.typ"
    );
}

#[test]
fn sync_rejects_conflicting_hierarchy_index_documents() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/index.md"),
        "# Conflicting Index\n",
    )
    .expect("write conflicting directory index");

    let value = run_failure_json(fixture.command().arg("sync").arg("--dry-run"));

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "HIERARCHY_INDEX_CONFLICT");
}

#[test]
fn sync_rejects_unsupported_concurrency() {
    let fixture = Fixture::new();
    configure(&fixture);

    let value = run_failure_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("--concurrency")
            .arg("2"),
    );

    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "CONCURRENCY_UNSUPPORTED");
}

#[test]
fn sync_archive_deleted_dry_run_marks_deleted_pages_with_ids() {
    let fixture = Fixture::new();
    configure(&fixture);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    seed_typub_platform_status(&fixture, &first, "projects/cuda-agent/notes.typ", "42");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let sync = run_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("--archive-deleted"),
    );

    let deleted = sync["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["action"] == "deleted")
        .expect("deleted item");
    assert_eq!(deleted["status"], "pending_archive");
    assert_eq!(deleted["platform_id"], "42");
}

#[test]
fn sync_archive_deleted_yes_removes_archived_state_entries() {
    let fixture = Fixture::new();
    let (base_url, request_rx) = start_archive_server();
    configure_with_base_url(&fixture, &base_url);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run_with_base_url(&fixture, &first, &base_url);
    seed_typub_platform_status(&fixture, &first, "projects/cuda-agent/notes.typ", "42");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let sync = run_json(
        fixture
            .command()
            .env("CONFLUENCE_EMAIL", "agent@example.com")
            .env("CONFLUENCE_API_KEY", "token")
            .arg("sync")
            .arg("--yes")
            .arg("--archive-deleted"),
    );

    assert_eq!(sync["ok"], true);
    let request = request_rx.recv().expect("archive request");
    assert!(request.contains("POST /wiki/rest/api/content/archive"));
    assert!(request.contains("\"id\":42"));

    let state: Value =
        serde_json::from_str(&fs::read_to_string(sync_state_path(&fixture)).expect("read state"))
            .expect("parse state");
    assert!(state["documents"]["projects/cuda-agent/notes.typ"].is_null());
}

#[test]
fn sync_archive_deleted_yes_hides_failed_archive_response_body() {
    let fixture = Fixture::new();
    let hidden_body = "body-should-not-appear";
    let (base_url, request_rx) =
        start_archive_server_with_response("500 Internal Server Error", hidden_body);
    configure_with_base_url(&fixture, &base_url);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run_with_base_url(&fixture, &first, &base_url);
    seed_typub_platform_status(&fixture, &first, "projects/cuda-agent/notes.typ", "42");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let sync = run_failure_json(
        fixture
            .command()
            .env("CONFLUENCE_EMAIL", "agent@example.com")
            .env("CONFLUENCE_API_KEY", "token")
            .arg("sync")
            .arg("--yes")
            .arg("--archive-deleted"),
    );

    assert_eq!(sync["ok"], false);
    assert_eq!(sync["code"], "ARCHIVE_REQUEST_FAILED");
    let message = sync["message"].as_str().expect("message");
    assert!(message.contains("500 Internal Server Error"));
    assert!(!message.contains(hidden_body));
    let request = request_rx.recv().expect("archive request");
    assert!(request.contains("POST /wiki/rest/api/content/archive"));
}

/// Simulate a local move whose remote page was adopted by title under the
/// new path: the old path's deleted entry carries the SAME page id as the
/// live document and must classify `superseded`, never `pending_archive`.
fn stage_moved_document(fixture: &Fixture) -> Value {
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(fixture, &first);
    seed_typub_platform_status(fixture, &first, "projects/cuda-agent/notes.typ", "42");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");
    fs::write(
        fixture.root.join("projects/cuda-agent/notes-moved.typ"),
        "= CUDA Notes\n",
    )
    .expect("write moved typst");
    let second = run_json(fixture.command().arg("sync").arg("--dry-run"));
    seed_typub_platform_status(
        fixture,
        &second,
        "projects/cuda-agent/notes-moved.typ",
        "42",
    );
    second
}

#[test]
fn sync_dry_run_marks_moved_document_deleted_entry_superseded() {
    let fixture = Fixture::new();
    configure(&fixture);
    stage_moved_document(&fixture);

    let sync = run_json(
        fixture
            .command()
            .arg("sync")
            .arg("--dry-run")
            .arg("--archive-deleted"),
    );

    let items = sync["data"]["items"].as_array().expect("items array");
    let deleted = items
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("deleted item");
    assert_eq!(deleted["action"], "deleted");
    assert_eq!(deleted["status"], "superseded");
    assert_eq!(deleted["platform_id"], "42");
    let moved = items
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes-moved.typ")
        .expect("moved item");
    assert_eq!(moved["platform_id"], "42");
    assert_eq!(sync["data"]["summary"]["superseded"], 1);
}

#[test]
fn prune_without_yes_reports_pending_actions_and_keeps_state() {
    let fixture = Fixture::new();
    configure(&fixture);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let prune = run_json(fixture.command().arg("prune"));

    assert_eq!(prune["data"]["dry_run"], true);
    let item = prune["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("deleted item");
    assert_eq!(item["status"], "pending_prune");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(sync_state_path(&fixture)).expect("read state"))
            .expect("parse state");
    assert!(!state["documents"]["projects/cuda-agent/notes.typ"].is_null());
}

#[test]
fn prune_yes_drops_stale_entries_without_remote_calls() {
    let fixture = Fixture::new();
    configure(&fixture);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let prune = run_json(fixture.command().arg("prune").arg("--yes"));

    assert_eq!(prune["data"]["pruned"], 1);
    let item = prune["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("pruned item");
    assert_eq!(item["status"], "pruned");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(sync_state_path(&fixture)).expect("read state"))
            .expect("parse state");
    assert!(state["documents"]["projects/cuda-agent/notes.typ"].is_null());
}

#[test]
fn prune_yes_delete_removes_remote_page_and_drops_state_entry() {
    let fixture = Fixture::new();
    let (base_url, request_rx) = start_archive_server_with_response("204 No Content", "");
    configure_with_base_url(&fixture, &base_url);
    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run_with_base_url(&fixture, &first, &base_url);
    seed_typub_platform_status(&fixture, &first, "projects/cuda-agent/notes.typ", "42");
    fs::remove_file(fixture.root.join("projects/cuda-agent/notes.typ")).expect("remove typst");

    let prune = run_json(
        fixture
            .command()
            .env("CONFLUENCE_EMAIL", "agent@example.com")
            .env("CONFLUENCE_API_KEY", "token")
            .arg("prune")
            .arg("--yes")
            .arg("--delete"),
    );

    assert_eq!(prune["ok"], true);
    let request = request_rx.recv().expect("delete request");
    assert!(request.contains("DELETE /wiki/rest/api/content/42"));
    let item = prune["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("deleted item");
    assert_eq!(item["status"], "deleted_remote");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(sync_state_path(&fixture)).expect("read state"))
            .expect("parse state");
    assert!(state["documents"]["projects/cuda-agent/notes.typ"].is_null());
}

#[test]
fn prune_yes_superseded_entry_dropped_without_remote_action() {
    let fixture = Fixture::new();
    configure(&fixture);
    stage_moved_document(&fixture);

    // No mock server and no credentials: a superseded entry must be resolved
    // purely in local state, whatever remote flags a later run might add.
    let prune = run_json(fixture.command().arg("prune").arg("--yes"));

    let item = prune["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == "projects/cuda-agent/notes.typ")
        .expect("superseded item");
    assert_eq!(item["status"], "superseded");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(sync_state_path(&fixture)).expect("read state"))
            .expect("parse state");
    assert!(state["documents"]["projects/cuda-agent/notes.typ"].is_null());
}

#[test]
fn prune_rejects_conflicting_archive_and_delete_flags() {
    let fixture = Fixture::new();
    configure(&fixture);

    let prune = run_failure_json(
        fixture
            .command()
            .arg("prune")
            .arg("--yes")
            .arg("--archive")
            .arg("--delete"),
    );

    assert_eq!(prune["ok"], false);
    assert_eq!(prune["code"], "PRUNE_CONFLICTING_FLAGS");
}

#[test]
fn sync_fingerprint_includes_shared_assets() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::create_dir_all(fixture.root.join("_assets")).expect("create shared assets");
    fs::write(fixture.root.join("_assets/diagram.png"), "asset v1\n").expect("write asset");

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    fs::write(fixture.root.join("_assets/diagram.png"), "asset v2\n").expect("update asset");

    let changed = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(changed["data"]["summary"]["update"], 3);
    assert_eq!(changed["data"]["summary"]["unchanged"], 0);
    assert_eq!(changed["data"]["publishable"], 3);
}

#[test]
fn sync_invalidates_v2_fingerprints_after_publish_semantics_change() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);

    let mut shared_assets = blake3::Hasher::new();
    shared_assets.update(b"conpub-shared-assets-v1\0");
    let shared_assets = shared_assets.finalize().to_hex().to_string();

    let state_path = sync_state_path(&fixture);
    let mut state: Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read current sync state"))
            .expect("parse current sync state");
    for item in first["data"]["items"].as_array().expect("sync items") {
        let path = item["path"].as_str().expect("document path");
        let bytes = fs::read(fixture.root.join(path)).expect("read document");
        let mut legacy = blake3::Hasher::new();
        legacy.update(b"conpub-document-v2\0");
        legacy.update(path.as_bytes());
        legacy.update(b"\0");
        legacy.update(&bytes);
        legacy.update(b"\0shared-assets\0");
        legacy.update(shared_assets.as_bytes());
        state["documents"][path]["fingerprint"] = json!(legacy.finalize().to_hex().to_string());
    }
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&state).expect("encode v2 sync state"),
    )
    .expect("write v2 sync state");

    let changed = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(changed["data"]["summary"]["update"], 3);
    assert_eq!(changed["data"]["summary"]["unchanged"], 0);
    assert_eq!(changed["data"]["publishable"], 3);
}

#[test]
fn publish_staging_maps_shared_assets_and_ignores_siblings() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::create_dir_all(fixture.root.join("_assets")).expect("create shared assets");
    fs::write(fixture.root.join("_assets/diagram.png"), "asset\n").expect("write asset");
    fs::write(fixture.root.join("_assets/.env"), "TOKEN=secret\n").expect("write hidden asset");
    fs::write(fixture.root.join("_assets/private.pem"), "private\n").expect("write key asset");
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/side-note.md"),
        "# Side Note\n",
    )
    .expect("write sibling doc");

    let dry_run = run_json(fixture.command().arg("publish").arg("--dry-run"));
    assert_eq!(dry_run["ok"], true);

    let occupancy_stage = stage_root(&fixture)
        .join("posts")
        .join("projects-cuda-agent-perf-occupancy");
    assert!(occupancy_stage.join("assets/diagram.png").exists());
    assert!(!occupancy_stage.join("side-note.md").exists());
    assert!(!occupancy_stage.join("assets/.env").exists());
    assert!(!occupancy_stage.join("assets/private.pem").exists());
}

#[test]
fn sync_fingerprint_ignores_unpublished_sibling_files() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture
            .root
            .join("projects/cuda-agent/perf/local-secret.env"),
        "TOKEN=one\n",
    )
    .expect("write sibling secret");

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    fs::write(
        fixture
            .root
            .join("projects/cuda-agent/perf/local-secret.env"),
        "TOKEN=two\n",
    )
    .expect("update sibling secret");

    let changed = run_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(changed["data"]["summary"]["update"], 0);
    assert_eq!(changed["data"]["summary"]["unchanged"], 3);
    assert_eq!(changed["data"]["publishable"], 0);
}

#[test]
fn plan_rejects_slug_collisions() {
    let fixture = Fixture::new();
    configure(&fixture);
    fs::write(
        fixture.root.join("projects/cuda-agent/a-b.md"),
        "# A Dash B\n",
    )
    .expect("write first collision doc");
    fs::create_dir_all(fixture.root.join("projects/cuda-agent/a")).expect("create collision dir");
    fs::write(
        fixture.root.join("projects/cuda-agent/a/b.md"),
        "# Nested B\n",
    )
    .expect("write second collision doc");

    let value = run_failure_json(fixture.command().arg("plan"));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "SLUG_COLLISION");
}

#[test]
fn sync_rejects_state_for_different_target_identity() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    let state_path = sync_state_path(&fixture);
    let text = fs::read_to_string(&state_path).expect("read state");
    let mut state: Value = serde_json::from_str(&text).expect("parse state");
    state["identity"]["space"] = json!("OTHER");
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&state).expect("encode state"),
    )
    .expect("write mismatched state");

    let value = run_failure_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "STATE_TARGET_MISMATCH");
}

#[test]
fn sync_rejects_v1_sync_state_without_legacy_migration() {
    let fixture = Fixture::new();
    configure(&fixture);

    let first = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &first);
    let state_path = sync_state_path(&fixture);
    let text = fs::read_to_string(&state_path).expect("read state");
    let mut state: Value = serde_json::from_str(&text).expect("parse state");
    state["version"] = json!(1);
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&state).expect("encode state"),
    )
    .expect("write v1 state");

    let value = run_failure_json(fixture.command().arg("sync").arg("--dry-run"));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "STATE_VERSION_UNSUPPORTED");
}

#[test]
fn plan_reports_create_for_untracked_documents() {
    let fixture = Fixture::new();
    configure(&fixture);

    let plan = run_json(fixture.command().arg("plan"));

    assert_eq!(plan["ok"], true);
    assert_eq!(plan["data"]["count"], 3);
    assert_eq!(plan["data"]["publishable"], 3);
    assert_eq!(plan["data"]["summary"]["create"], 3);
    assert_eq!(plan["data"]["summary"]["unchanged"], 0);
    let item = plan_item(&plan, "projects/cuda-agent/notes.typ");
    assert_eq!(item["action"], "create");
    assert_eq!(item["reason"], "not present in local publish state");
}

#[test]
fn plan_reports_unchanged_from_publish_state_and_flags_edits() {
    let fixture = Fixture::new();
    configure(&fixture);

    let sync = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &sync);

    let plan = run_json(fixture.command().arg("plan"));
    assert_eq!(plan["data"]["publishable"], 0);
    assert_eq!(plan["data"]["summary"]["unchanged"], 3);
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/perf/occupancy.md")["action"],
        "unchanged"
    );

    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "# Occupancy\n\nRevised occupancy notes.\n",
    )
    .expect("update markdown");

    let plan = run_json(fixture.command().arg("plan"));
    assert_eq!(plan["data"]["publishable"], 1);
    assert_eq!(plan["data"]["summary"]["update"], 1);
    assert_eq!(plan["data"]["summary"]["unchanged"], 2);
    assert_eq!(
        plan_item(&plan, "projects/cuda-agent/perf/occupancy.md")["action"],
        "update"
    );
}

#[test]
fn plan_blocks_missing_assets_over_the_state_verdict() {
    let fixture = Fixture::new();
    configure(&fixture);

    let sync = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &sync);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "# Occupancy\n\n![figure](assets/missing-figure.png)\n",
    )
    .expect("update markdown");

    let plan = run_json(fixture.command().arg("plan"));
    let item = plan_item(&plan, "projects/cuda-agent/perf/occupancy.md");
    assert_eq!(item["action"], "blocked");
    let reason = item["reason"].as_str().expect("reason string");
    assert!(reason.contains("assets/missing-figure.png"), "{reason}");
    assert_eq!(plan["data"]["publishable"], 0);
    assert_eq!(plan["data"]["summary"]["blocked"], 1);
    assert_eq!(plan["data"]["summary"]["update"], 0);
}

#[test]
fn plan_and_status_agree_on_the_publish_state_summary() {
    let fixture = Fixture::new();
    configure(&fixture);

    let sync = run_json(fixture.command().arg("sync").arg("--dry-run"));
    write_sync_state_from_dry_run(&fixture, &sync);
    fs::write(
        fixture.root.join("projects/cuda-agent/perf/occupancy.md"),
        "# Occupancy\n\nRevised occupancy notes.\n",
    )
    .expect("update markdown");

    let plan = run_json(fixture.command().arg("plan"));
    let status = run_json(fixture.command().arg("status"));

    for key in ["create", "update", "unchanged", "deleted"] {
        assert_eq!(
            plan["data"]["summary"][key], status["data"]["sync"]["summary"][key],
            "summary key {key}"
        );
    }
    assert_eq!(
        plan["data"]["publishable"],
        status["data"]["sync"]["publishable"]
    );
}

#[test]
fn plan_adopts_typub_status_for_untracked_documents() {
    let fixture = Fixture::new();
    configure(&fixture);

    let sync = run_json(fixture.command().arg("sync").arg("--dry-run"));
    seed_typub_platform_status(&fixture, &sync, "projects/cuda-agent/notes.typ", "4242");

    let plan = run_json(fixture.command().arg("plan"));
    let item = plan_item(&plan, "projects/cuda-agent/notes.typ");
    assert_eq!(item["action"], "update");
    assert_eq!(
        item["confluence_url"],
        "https://example.atlassian.net/wiki/spaces/GPU/pages/4242/CUDA Notes"
    );
}

fn configure(fixture: &Fixture) {
    configure_with_base_url(fixture, "https://example.atlassian.net/wiki");
}

fn configure_with_base_url(fixture: &Fixture, base_url: &str) {
    run_json(
        fixture
            .command()
            .arg("root")
            .arg(&fixture.root)
            .arg("--base-url")
            .arg(base_url),
    );
    run_json(
        fixture
            .command()
            .arg("bind")
            .arg("projects/cuda-agent")
            .arg("--space")
            .arg("GPU")
            .arg("--parent")
            .arg("123456789"),
    );
}

fn write_sync_state_from_dry_run(fixture: &Fixture, sync: &Value) {
    write_sync_state_from_dry_run_with_base_url(fixture, sync, "https://example.atlassian.net");
}

fn write_sync_state_from_dry_run_with_base_url(fixture: &Fixture, sync: &Value, base_url: &str) {
    let mut documents = serde_json::Map::new();
    for item in sync["data"]["items"].as_array().expect("items array") {
        let path = item["path"].as_str().expect("path string");
        documents.insert(
            path.to_string(),
            json!({
                "fingerprint": item["fingerprint"],
                "title": item["title"],
                "slug": item["slug"],
                "parent_path": item["parent_path"],
                "synced_at": 1,
            }),
        );
    }

    let state_path = sync_state_path(fixture);
    fs::create_dir_all(state_path.parent().expect("state parent")).expect("create state parent");
    fs::write(
        state_path,
        serde_json::to_string_pretty(&json!({
            "version": SYNC_STATE_VERSION,
            "identity": {
                "root": fixture.root.display().to_string(),
                "source": ".",
                "base_url": base_url.trim_end_matches('/').trim_end_matches("/wiki"),
                "space": "GPU",
                "parent_id": "123456789",
            },
            "documents": documents,
        }))
        .expect("encode state"),
    )
    .expect("write state");
}

fn seed_typub_platform_status(fixture: &Fixture, sync: &Value, path: &str, platform_id: &str) {
    let item = sync["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == path)
        .expect("sync item");
    let title = item["title"].as_str().expect("title string");
    let slug = item["slug"].as_str().expect("slug string");
    let stage_root = stage_root(fixture);
    let content = staged_status_content(&stage_root, slug, title);
    let result = PublishResult {
        url: Some(format!(
            "https://example.atlassian.net/wiki/spaces/GPU/pages/{platform_id}/{title}"
        )),
        platform_id: Some(platform_id.to_string()),
        published_at: Utc::now(),
    };

    with_stage_workdir(&stage_root, || {
        let mut status = StatusTracker::load(&stage_root).expect("load typub status");
        status
            .mark_published(&content, "confluence", &result, Some("published"))
            .expect("mark published");
    });
}

fn staged_status_content(stage_root: &Path, slug: &str, title: &str) -> Content {
    let post_dir = stage_root.join("posts").join(slug);
    fs::create_dir_all(&post_dir).expect("create status post dir");
    let content_file = post_dir.join("content.md");
    fs::write(&content_file, format!("# {title}\n")).expect("write status content");

    Content {
        path: post_dir,
        meta: ContentMeta {
            title: title.to_string(),
            created: NaiveDate::from_ymd_opt(2026, 6, 18).expect("date"),
            updated: None,
            tags: Vec::new(),
            categories: Vec::new(),
            published: Some(true),
            theme: None,
            internal_link_target: None,
            preamble: None,
            platforms: HashMap::new(),
        },
        content_file,
        source_format: ContentFormat::Markdown,
        slides_file: None,
        assets: Vec::new(),
    }
}

fn with_stage_workdir(stage_root: &Path, op: impl FnOnce()) {
    let lock = CWD_LOCK.get_or_init(|| Mutex::new(()));
    let _lock = lock.lock().expect("cwd lock");
    fs::create_dir_all(stage_root).expect("create stage root");
    let previous = env::current_dir().expect("current dir");
    env::set_current_dir(stage_root).expect("enter stage root");
    op();
    env::set_current_dir(previous).expect("restore current dir");
}

fn start_archive_server() -> (String, mpsc::Receiver<String>) {
    start_archive_server_with_response("202 Accepted", r#"{"id":"task-1"}"#)
}

/// Mock Confluence for the publish pipeline: every title lookup finds an
/// existing page (so provision creates nothing), attachment uploads are
/// rejected with a distinctive status and body, and everything else
/// succeeds. Serves connections until the listener is dropped.
fn start_confluence_attachment_failure_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set timeout");
            let mut buffer = [0_u8; 8192];
            let mut bytes = Vec::new();
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        bytes.extend_from_slice(&buffer[..read]);
                        if read < buffer.len() {
                            break;
                        }
                    }
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            let request = String::from_utf8_lossy(&bytes);
            let request_line = request.lines().next().unwrap_or_default();
            let (status, body) = if request_line.starts_with("GET") {
                (
                    "200 OK",
                    r#"{"results":[{"id":"42","type":"page","status":"current","title":"found","version":{"number":1},"_links":{"webui":"/spaces/GPU/pages/42"}}],"size":1}"#,
                )
            } else if request_line.contains("/child/attachment") {
                ("403 Forbidden", "attachment quota exceeded")
            } else {
                (
                    "200 OK",
                    r#"{"id":"42","type":"page","status":"current","title":"found","version":{"number":2},"_links":{"webui":"/spaces/GPU/pages/42"}}"#,
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn start_archive_server_with_response(
    status: &str,
    body: &str,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    let (tx, rx) = mpsc::channel();
    let status = status.to_string();
    let body = body.to_string();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        let mut buffer = [0_u8; 4096];
        let mut bytes = Vec::new();
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    bytes.extend_from_slice(&buffer[..read]);
                    if read < buffer.len() {
                        break;
                    }
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(err) => panic!("read request: {err}"),
            }
        }
        let request = String::from_utf8_lossy(&bytes).to_string();
        tx.send(request).expect("send request");
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (format!("http://{addr}"), rx)
}

fn sync_state_path(fixture: &Fixture) -> PathBuf {
    stage_root(fixture).join("sync-state.json")
}

fn stage_root(fixture: &Fixture) -> PathBuf {
    fixture.home.join("typub-stage").join("gpu-123456789")
}

fn plan_item<'a>(plan: &'a Value, path: &str) -> &'a Value {
    plan["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["path"] == path)
        .expect("plan item")
}

fn write_plugin_package(checkout: &Path) {
    let version = env!("CARGO_PKG_VERSION");
    let files = [
        (
            ".agents/plugins/marketplace.json",
            json!({
                "name": "conpub",
                "plugins": [{
                    "name": "conpub",
                    "source": {"source": "local", "path": "./plugins/conpub"}
                }]
            })
            .to_string(),
        ),
        (
            ".claude-plugin/marketplace.json",
            json!({
                "name": "conpub",
                "plugins": [{
                    "name": "conpub",
                    "version": version,
                    "source": "./plugins/conpub"
                }]
            })
            .to_string(),
        ),
        (
            "plugins/conpub/.codex-plugin/plugin.json",
            json!({"name": "conpub", "version": version}).to_string(),
        ),
        (
            "plugins/conpub/.claude-plugin/plugin.json",
            json!({"name": "conpub", "version": version}).to_string(),
        ),
        (
            "plugins/conpub/skills/conpub/SKILL.md",
            "---\nname: conpub\ndescription: test\n---\n".to_string(),
        ),
    ];
    for (path, contents) in files {
        let path = checkout.join(path);
        fs::create_dir_all(path.parent().expect("plugin parent")).expect("create plugin parent");
        fs::write(path, contents).expect("write plugin file");
    }
}

#[cfg(unix)]
fn install_fake_agent_clis(bin: &Path, fail_codex_install: bool) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(bin).expect("create fake bin");
    for cli in ["codex", "claude"] {
        let path = bin.join(cli);
        let failure = if cli == "codex" && fail_codex_install {
            "case \"$*\" in\n  'plugin add conpub@conpub') exit 17 ;;\nesac\n"
        } else {
            ""
        };
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s %s\\n' '{cli}' \"$*\" >> \"$AGENT_TEST_LOG\"\n{failure}exit 0\n"
            ),
        )
        .expect("write fake agent CLI");
        let mut permissions = fs::metadata(&path)
            .expect("fake CLI metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("make fake CLI executable");
    }
}

#[cfg(unix)]
fn path_with(bin: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    env::join_paths(paths).expect("join PATH")
}

fn run_json(cmd: &mut Command) -> Value {
    let assert = cmd.assert().success();
    parse_stdout(assert.get_output().stdout.as_slice())
}

fn run_failure_json(cmd: &mut Command) -> Value {
    let assert = cmd.assert().failure();
    parse_stdout(assert.get_output().stdout.as_slice())
}

fn parse_stdout(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("stdout utf8");
    serde_json::from_str(text).expect("parse JSON")
}
