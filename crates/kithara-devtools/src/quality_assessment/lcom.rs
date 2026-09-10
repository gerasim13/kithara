use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::artifact;
use crate::cohesion::Report;

#[derive(Deserialize)]
struct AssessmentCohesion {
    lcom4: Report,
}

pub(crate) fn render_file(path: &Path, rows: usize) -> Result<String> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read LCOM4 assessment {}", path.display()))?;
    let assessment: AssessmentCohesion = serde_json::from_str(&text)
        .with_context(|| format!("parse LCOM4 assessment {}", path.display()))?;
    let mut output = String::new();
    append(&mut output, &assessment.lcom4, rows);
    Ok(output)
}

pub(super) fn append(output: &mut String, report: &Report, rows: usize) {
    let disconnected = report.types.iter().filter(|entry| entry.lcom4 > 1).count();
    output.push_str(&format!(
        "\n## Type cohesion (LCOM4)\n\nMeasured types: {}; types with LCOM4 > 1: {}.\n\n",
        report.types.len(),
        disconnected,
    ));
    for note in report.notes.iter().take(rows) {
        output.push_str(&format!("- {}\n", artifact::escape(note)));
    }
    if report.notes.len() > rows {
        output.push_str(&format!(
            "- {} additional analysis notes in assessment.json.\n",
            report.notes.len() - rows
        ));
    }
    if report.types.is_empty() {
        output.push_str("\n_No types with receiver methods were measured._\n");
        return;
    }
    output.push_str("\n| Type | Target | Location | LCOM4 | Method groups (fields) |\n| --- | --- | --- | ---: | --- |\n");
    for entry in report.types.iter().take(rows) {
        let groups = entry
            .groups
            .iter()
            .map(|group| {
                format!(
                    "{} ({})",
                    group
                        .methods
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    group.fields.iter().cloned().collect::<Vec<_>>().join(", "),
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            artifact::escape(&entry.name),
            artifact::escape(&entry.target),
            artifact::escape(&entry.location),
            entry.lcom4,
            artifact::escape(&groups),
        ));
    }
    if report.types.len() > rows {
        output.push_str(&format!(
            "\n_{} additional LCOM4 rows in assessment.json._\n",
            report.types.len() - rows
        ));
    }
}
