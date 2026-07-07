use agent_plugin_installer::{
    AgentRuntime, AgentSelector, BatchResult, DoctorOutcome, FailurePolicy, InstallRequest,
    PluginRef, UninstallRequest, UpdateRequest, doctor_many, install_many, uninstall_many,
    update_many,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const PLUGIN: PluginRef<'static> = PluginRef {
    selector: "conpub@conpub",
    name: "conpub",
};
const MARKETPLACE: &str = "conpub";

pub(crate) fn doctor_agents(selector: AgentSelector) -> Vec<DoctorOutcome> {
    doctor_many(selector)
}

pub(crate) fn install_agents(selector: AgentSelector, checkout: &Path) -> BatchResult {
    install_many(
        selector,
        |_| InstallRequest::local(checkout, PLUGIN),
        FailurePolicy::StopOnFailure,
    )
}

pub(crate) fn update_agents(selector: AgentSelector) -> BatchResult {
    update_many(
        selector,
        |_| UpdateRequest::new(PLUGIN).with_marketplace_name(MARKETPLACE),
        FailurePolicy::StopOnFailure,
    )
}

pub(crate) fn uninstall_agents(selector: AgentSelector) -> BatchResult {
    uninstall_many(
        selector,
        |_| UninstallRequest::new(PLUGIN),
        FailurePolicy::StopOnFailure,
    )
}

pub(crate) fn validate_agent_package(runtime: AgentRuntime, checkout: &Path) -> Result<(), String> {
    if !checkout.is_dir() {
        return Err(format!(
            "plugin package for {} is not a directory: {}",
            runtime.id(),
            checkout.display()
        ));
    }

    let [marketplace_path, manifest_path, skill_path] = package_requirements(runtime, checkout);
    let marketplace = read_json(&marketplace_path)?;
    let manifest = read_json(&manifest_path)?;
    validate_marketplace(runtime, &marketplace, &marketplace_path)?;
    validate_manifest(&manifest, &manifest_path)?;
    validate_skill(&skill_path)
}

fn package_requirements(runtime: AgentRuntime, checkout: &Path) -> [PathBuf; 3] {
    let marketplace = match runtime {
        AgentRuntime::Codex => ".agents/plugins/marketplace.json",
        AgentRuntime::Claude => ".claude-plugin/marketplace.json",
    };
    let manifest = match runtime {
        AgentRuntime::Codex => "plugins/conpub/.codex-plugin/plugin.json",
        AgentRuntime::Claude => "plugins/conpub/.claude-plugin/plugin.json",
    };
    [
        checkout.join(marketplace),
        checkout.join(manifest),
        checkout.join("plugins/conpub/skills/conpub/SKILL.md"),
    ]
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read plugin file {}: {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("invalid JSON in plugin file {}: {err}", path.display()))
}

fn validate_marketplace(
    runtime: AgentRuntime,
    marketplace: &Value,
    path: &Path,
) -> Result<(), String> {
    if marketplace["name"] != "conpub" {
        return Err(format!(
            "plugin marketplace {} must be named conpub",
            path.display()
        ));
    }
    let entry = marketplace["plugins"]
        .as_array()
        .and_then(|plugins| plugins.iter().find(|plugin| plugin["name"] == "conpub"))
        .ok_or_else(|| format!("plugin marketplace {} does not list conpub", path.display()))?;

    match runtime {
        AgentRuntime::Codex
            if entry["source"]
                != serde_json::json!({"source": "local", "path": "./plugins/conpub"}) =>
        {
            Err(format!(
                "Codex marketplace {} must source ./plugins/conpub",
                path.display()
            ))
        }
        AgentRuntime::Claude if entry["source"] != "./plugins/conpub" => Err(format!(
            "Claude marketplace {} must source ./plugins/conpub",
            path.display()
        )),
        AgentRuntime::Claude if entry["version"] != env!("CARGO_PKG_VERSION") => Err(format!(
            "Claude marketplace {} version must match conpub {}",
            path.display(),
            env!("CARGO_PKG_VERSION")
        )),
        _ => Ok(()),
    }
}

fn validate_manifest(manifest: &Value, path: &Path) -> Result<(), String> {
    if manifest["name"] != "conpub" {
        return Err(format!(
            "plugin manifest {} must be named conpub",
            path.display()
        ));
    }
    if manifest["version"] != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "plugin manifest {} version must match conpub {}",
            path.display(),
            env!("CARGO_PKG_VERSION")
        ));
    }
    Ok(())
}

fn validate_skill(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read conpub skill {}: {err}", path.display()))?;
    if text.lines().take(4).any(|line| line == "name: conpub") {
        Ok(())
    } else {
        Err(format!(
            "conpub skill {} has invalid frontmatter",
            path.display()
        ))
    }
}
