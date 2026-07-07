use crate::cli::{AgentCommand, AgentSelector};
use crate::infrastructure::{
    doctor_agents, install_agents, uninstall_agents, update_agents, validate_agent_package,
};
use crate::support::{AppError, AppResult, ok};
use agent_plugin_installer::{
    AgentRuntime, BatchFailure, BatchOperationReport, BatchResult, BatchRuntimeOutcome,
    BatchStatus, DoctorStatus,
};
use serde::Serialize;
use serde_json::json;
use std::path::Path;

#[derive(Debug, Serialize)]
struct AgentRow {
    agent: &'static str,
    operation: &'static str,
    status: &'static str,
    cli: &'static str,
    commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

pub(crate) fn cmd_agent(command: AgentCommand) -> AppResult<serde_json::Value> {
    match command {
        AgentCommand::Doctor { agent } => cmd_doctor(agent),
        AgentCommand::Install {
            agent,
            from_checkout,
        } => cmd_install(agent, &from_checkout),
        AgentCommand::Update { agent } => cmd_update(agent),
        AgentCommand::Uninstall { agent } => cmd_uninstall(agent),
    }
}

fn cmd_doctor(selector: AgentSelector) -> AppResult<serde_json::Value> {
    let rows = doctor_agents(selector)
        .into_iter()
        .map(|outcome| AgentRow {
            agent: outcome.runtime.id(),
            operation: "doctor",
            status: match outcome.status {
                DoctorStatus::Ready => "ready",
                DoctorStatus::Missing => "missing",
                DoctorStatus::Failed => "failed",
            },
            cli: outcome.runtime.cli(),
            commands: outcome.commands,
            message: outcome.message,
        })
        .collect();
    Ok(report("doctor", rows))
}

fn cmd_install(selector: AgentSelector, checkout: &Path) -> AppResult<serde_json::Value> {
    let selected = selector.runtimes();
    validate_packages(selected, checkout)?;
    let checkout = checkout.canonicalize().map_err(|err| {
        package_error(
            selected,
            format!(
                "cannot canonicalize plugin package {}: {err}",
                checkout.display()
            ),
        )
    })?;

    finish_mutation("install", install_agents(selector, &checkout))
}

fn cmd_update(selector: AgentSelector) -> AppResult<serde_json::Value> {
    finish_mutation("update", update_agents(selector))
}

fn cmd_uninstall(selector: AgentSelector) -> AppResult<serde_json::Value> {
    finish_mutation("uninstall", uninstall_agents(selector))
}

fn finish_mutation(
    operation_name: &'static str,
    result: BatchResult,
) -> AppResult<serde_json::Value> {
    match result {
        Ok(report) => Ok(report_from_batch(operation_name, report)),
        Err(error) => {
            let report = error.into_report();
            let preflight_failed = report.outcomes.iter().any(|outcome| {
                matches!(
                    outcome.failure.as_ref(),
                    Some(BatchFailure::Preflight { .. })
                )
            });
            let rows = rows_from_batch(operation_name, report);
            let details = report_data(operation_name, rows);
            if preflight_failed {
                Err(AppError::new(
                    "AGENT_NOT_READY",
                    format!("native agent CLI is not ready for {operation_name}"),
                )
                .with_details(details))
            } else {
                Err(AppError::new(
                    "AGENT_OPERATION_FAILED",
                    format!("conpub agent {operation_name} failed"),
                )
                .with_details(details))
            }
        }
    }
}

fn report_from_batch(
    operation_name: &'static str,
    batch_report: BatchOperationReport,
) -> serde_json::Value {
    report(
        operation_name,
        rows_from_batch(operation_name, batch_report),
    )
}

fn rows_from_batch(
    operation_name: &'static str,
    batch_report: BatchOperationReport,
) -> Vec<AgentRow> {
    batch_report
        .outcomes
        .into_iter()
        .map(|outcome| row_from_batch(operation_name, outcome))
        .collect()
}

fn row_from_batch(operation_name: &'static str, outcome: BatchRuntimeOutcome) -> AgentRow {
    let status = match outcome.status {
        BatchStatus::Succeeded => match operation_name {
            "install" => "installed",
            "update" => "updated",
            "uninstall" => "uninstalled",
            _ => "completed",
        },
        BatchStatus::Missing => "missing",
        BatchStatus::Failed => "failed",
        BatchStatus::Skipped => "skipped",
        _ => "failed",
    };
    let message = outcome
        .failure
        .as_ref()
        .map(batch_failure_message)
        .or_else(|| {
            outcome
                .skip_reason
                .map(|reason| reason.message().to_owned())
        });
    AgentRow {
        agent: outcome.runtime.id(),
        operation: operation_name,
        status,
        cli: outcome.runtime.cli(),
        commands: outcome.commands,
        message,
    }
}

fn batch_failure_message(failure: &BatchFailure) -> String {
    match failure {
        BatchFailure::Validation(error) => error.to_string(),
        BatchFailure::Preflight { message } => message.clone(),
        BatchFailure::Operation(error) => error.to_string(),
        _ => failure.to_string(),
    }
}

fn validate_packages(selected: &[AgentRuntime], checkout: &Path) -> AppResult<()> {
    let failures = selected
        .iter()
        .copied()
        .filter_map(|runtime| {
            validate_agent_package(runtime, checkout)
                .err()
                .map(|message| (runtime, message))
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(());
    }

    let rows = selected
        .iter()
        .copied()
        .map(|runtime| {
            failures
                .iter()
                .find(|(failed, _)| *failed == runtime)
                .map(|(_, message)| AgentRow {
                    agent: runtime.id(),
                    operation: "install",
                    status: "failed",
                    cli: runtime.cli(),
                    commands: Vec::new(),
                    message: Some(message.clone()),
                })
                .unwrap_or_else(|| {
                    skipped_row(
                        runtime,
                        "install",
                        "mutation not attempted because another package validation failed",
                    )
                })
        })
        .collect::<Vec<_>>();

    Err(AppError::new(
        "AGENT_PACKAGE_INVALID",
        "conpub agent plugin package validation failed",
    )
    .with_details(report_data("install", rows)))
}

fn package_error(selected: &[AgentRuntime], message: String) -> AppError {
    let rows = selected
        .iter()
        .copied()
        .map(|runtime| AgentRow {
            agent: runtime.id(),
            operation: "install",
            status: "failed",
            cli: runtime.cli(),
            commands: Vec::new(),
            message: Some(message.clone()),
        })
        .collect();
    AppError::new("AGENT_PACKAGE_INVALID", message).with_details(report_data("install", rows))
}

fn skipped_row(runtime: AgentRuntime, operation: &'static str, message: &str) -> AgentRow {
    AgentRow {
        agent: runtime.id(),
        operation,
        status: "skipped",
        cli: runtime.cli(),
        commands: Vec::new(),
        message: Some(message.to_string()),
    }
}

fn report(operation: &'static str, rows: Vec<AgentRow>) -> serde_json::Value {
    ok(report_data(operation, rows))
}

fn report_data(operation: &'static str, rows: Vec<AgentRow>) -> serde_json::Value {
    json!({
        "operation": operation,
        "count": rows.len(),
        "rows": rows,
    })
}
