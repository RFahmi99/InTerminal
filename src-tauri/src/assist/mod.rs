//! Assistive features layered over the shell: error fix suggestions, risk
//! assessment and command templates.

pub mod errors;
pub mod risk;
pub mod template;

use std::collections::HashMap;

use tauri::State;

use crate::error::CmdResult;
use crate::index::IndexManager;
use errors::{FailureReport, Suggestion};
use risk::RiskReport;
use template::FilledCommand;

#[tauri::command]
pub async fn analyze_failure(index: State<'_, IndexManager>, report: FailureReport) -> CmdResult<Vec<Suggestion>> {
    let index = index.get();
    Ok(errors::analyze(&report, index.as_deref()))
}

#[tauri::command]
pub fn assess_risk(command: String) -> RiskReport {
    risk::assess(&command)
}

#[tauri::command]
pub fn fill_template(template: String, values: HashMap<String, String>) -> FilledCommand {
    template::fill(&template, &values)
}
