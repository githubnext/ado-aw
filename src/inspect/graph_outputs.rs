//! Output declaration/reference table for `ado-aw graph outputs`.
//!
//! This module intentionally works from the public [`PipelineSummary`]
//! instead of the compiler's internal graph. That keeps the command's
//! JSON shape aligned with the stable inspect schema while still
//! answering producer/consumer questions precisely.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::compile::ir::summary::{JobSummary, PipelineBodySummary, PipelineSummary, StepSummary};

/// Source location of an output reference on a consumer step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputConsumerSource {
    /// Reference came from the step's `env:` map.
    Env,
    /// Reference came from the step's `condition:` expression.
    Condition,
}

/// A step that reads a producer output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputConsumer {
    /// Consumer step id, or a stable anonymous label for steps without `id`.
    pub step: String,
    /// Whether the reference came from `env` or `condition`.
    pub source: OutputConsumerSource,
}

/// Public output edge emitted by `ado-aw graph outputs --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputEdge {
    /// Step that declares the output.
    pub producer_step: String,
    /// Declared output variable name.
    pub output_name: String,
    /// Whether the output is marked secret.
    pub is_secret: bool,
    /// Whether the graph pass determined the output needs `isOutput=true`.
    pub auto_is_output: bool,
    /// Steps that read this output.
    pub consumers: Vec<OutputConsumer>,
}

/// Build the declared-output table, optionally filtering by producer and/or consumer.
pub fn output_edges(
    summary: &PipelineSummary,
    producer_filter: Option<&str>,
    consumer_filter: Option<&str>,
) -> Vec<OutputEdge> {
    let steps = step_records(summary);
    let mut edges = Vec::new();

    for producer in &steps {
        let Some(producer_step) = producer.id.as_deref() else {
            continue;
        };
        if producer_filter.is_some_and(|filter| filter != producer_step) {
            continue;
        }

        for output in &producer.step.outputs {
            let mut consumers = Vec::new();
            for consumer in &steps {
                if consumer_filter.is_some_and(|filter| consumer.id.as_deref() != Some(filter)) {
                    continue;
                }
                for r in &consumer.step.env_refs {
                    if r.step == producer_step && r.name == output.name {
                        consumers.push(OutputConsumer {
                            step: consumer.label.clone(),
                            source: OutputConsumerSource::Env,
                        });
                    }
                }
                for r in &consumer.step.condition_refs {
                    if r.step == producer_step && r.name == output.name {
                        consumers.push(OutputConsumer {
                            step: consumer.label.clone(),
                            source: OutputConsumerSource::Condition,
                        });
                    }
                }
            }

            if consumer_filter.is_some() && consumers.is_empty() {
                continue;
            }

            edges.push(OutputEdge {
                producer_step: producer_step.to_string(),
                output_name: output.name.clone(),
                is_secret: output.is_secret,
                auto_is_output: output.auto_is_output,
                consumers,
            });
        }
    }

    edges
}

/// Render output edges as a concise terminal table.
pub fn render_text(edges: &[OutputEdge]) -> String {
    let mut out = String::new();
    if edges.is_empty() {
        out.push_str("(no declared outputs)\n");
        return out;
    }

    for edge in edges {
        let consumers = unique_consumer_steps(edge);
        let consumer_text = if consumers.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", consumers.into_iter().collect::<Vec<_>>().join(", "))
        };
        out.push_str(&format!(
            "{}.{} → consumers: {}\n",
            edge.producer_step, edge.output_name, consumer_text
        ));
    }
    out
}

fn unique_consumer_steps(edge: &OutputEdge) -> BTreeSet<String> {
    edge.consumers
        .iter()
        .map(|consumer| consumer.step.clone())
        .collect()
}

#[derive(Clone)]
struct StepRecord<'a> {
    id: Option<String>,
    label: String,
    step: &'a StepSummary,
}

fn step_records(summary: &PipelineSummary) -> Vec<StepRecord<'_>> {
    let mut records = Vec::new();
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => {
            for job in jobs {
                push_job_steps(&mut records, job);
            }
        }
        PipelineBodySummary::Stages { stages } => {
            for stage in stages {
                for job in &stage.jobs {
                    push_job_steps(&mut records, job);
                }
            }
        }
    }
    records
}

fn push_job_steps<'a>(records: &mut Vec<StepRecord<'a>>, job: &'a JobSummary) {
    for (idx, step) in job.steps.iter().enumerate() {
        records.push(StepRecord {
            id: step.id.clone(),
            label: step
                .id
                .clone()
                .unwrap_or_else(|| format!("{}#{}", job.id, idx + 1)),
            step,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::summary::{
        EdgeEntry, GraphSummary, OutputDeclSummary, OutputRefSummary, PoolSummary, StepKind,
    };

    fn summary(steps: Vec<StepSummary>) -> PipelineSummary {
        let jobs = vec![JobSummary {
            id: "Job".to_string(),
            stage: None,
            display_name: "Job".to_string(),
            depends_on: Vec::new(),
            condition: None,
            pool: PoolSummary::VmImage {
                image: "ubuntu-latest".to_string(),
            },
            steps,
        }];
        PipelineSummary {
            schema_version: 1,
            name: "test".to_string(),
            shape: "standalone".to_string(),
            body: PipelineBodySummary::Jobs { jobs },
            graph: GraphSummary {
                step_locations: Vec::new(),
                job_edges: Vec::<EdgeEntry>::new(),
                stage_edges: Vec::new(),
                outputs_needing_is_output: Vec::new(),
            },
        }
    }

    fn producer(id: &str, outputs: &[&str]) -> StepSummary {
        StepSummary {
            id: Some(id.to_string()),
            kind: StepKind::Bash,
            display_name: Some(id.to_string()),
            task: None,
            condition: None,
            outputs: outputs
                .iter()
                .map(|name| OutputDeclSummary {
                    name: (*name).to_string(),
                    is_secret: false,
                    auto_is_output: false,
                })
                .collect(),
            env_refs: Vec::new(),
            condition_refs: Vec::new(),
        }
    }

    fn consumer(
        id: &str,
        env_refs: &[(&str, &str)],
        condition_refs: &[(&str, &str)],
    ) -> StepSummary {
        StepSummary {
            id: Some(id.to_string()),
            kind: StepKind::Bash,
            display_name: Some(id.to_string()),
            task: None,
            condition: None,
            outputs: Vec::new(),
            env_refs: env_refs
                .iter()
                .map(|(step, name)| OutputRefSummary {
                    step: (*step).to_string(),
                    name: (*name).to_string(),
                })
                .collect(),
            condition_refs: condition_refs
                .iter()
                .map(|(step, name)| OutputRefSummary {
                    step: (*step).to_string(),
                    name: (*name).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn output_with_no_consumers_is_preserved() {
        let s = summary(vec![producer("P", &["value"])]);

        let edges = output_edges(&s, None, None);

        assert_eq!(edges.len(), 1);
        assert!(edges[0].consumers.is_empty());
        assert!(
            serde_json::to_string(&edges)
                .unwrap()
                .contains("\"consumers\":[]")
        );
    }

    #[test]
    fn producer_filter_selects_matching_outputs() {
        let s = summary(vec![producer("A", &["one"]), producer("B", &["two"])]);

        let edges = output_edges(&s, Some("B"), None);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].producer_step, "B");
    }

    #[test]
    fn consumer_filter_selects_outputs_read_by_consumer() {
        let s = summary(vec![
            producer("A", &["one"]),
            producer("B", &["two"]),
            consumer("C", &[("B", "two")], &[]),
        ]);

        let edges = output_edges(&s, None, Some("C"));

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].producer_step, "B");
        assert_eq!(edges[0].consumers[0].step, "C");
    }

    #[test]
    fn consumers_include_env_and_condition_refs() {
        let s = summary(vec![
            producer("A", &["one"]),
            consumer("Env", &[("A", "one")], &[]),
            consumer("Cond", &[], &[("A", "one")]),
        ]);

        let edges = output_edges(&s, None, None);
        let sources = edges[0]
            .consumers
            .iter()
            .map(|consumer| match consumer.source {
                OutputConsumerSource::Env => "env",
                OutputConsumerSource::Condition => "condition",
            })
            .collect::<Vec<_>>();

        assert_eq!(sources, vec!["env", "condition"]);
    }
}
