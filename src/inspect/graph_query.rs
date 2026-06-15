//! Graph-query rendering helpers.
//!
//! `cli::dispatch_graph` builds the [`PipelineSummary`] (which
//! contains the resolved [`crate::compile::ir::summary::GraphSummary`])
//! and asks this module to render it in the user-selected format.
//!
//! Text mode is human-scannable; JSON is the public schema (rendered
//! by `cli::dispatch_graph` directly via serde); DOT is a tiny
//! Graphviz adapter so users can pipe to `dot -Tsvg`.

use crate::compile::ir::summary::{
    EdgeEntry, GraphSummary, PipelineBodySummary, PipelineSummary, StepOutputsEntry,
};

/// Render a [`PipelineSummary`] as scannable text suitable for a
/// terminal.
pub fn render_text(s: &PipelineSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!("Pipeline: {} ({})\n", s.name, s.shape));
    out.push('\n');

    out.push_str("Step locations\n");
    if s.graph.step_locations.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for loc in &s.graph.step_locations {
            let stage = loc
                .stage
                .as_deref()
                .map(|s| format!("{}.", s))
                .unwrap_or_default();
            let outs = if loc.outputs.is_empty() {
                String::new()
            } else {
                format!(" outputs=[{}]", loc.outputs.join(", "))
            };
            out.push_str(&format!("  {}{}.{}{}\n", stage, loc.job, loc.step, outs));
        }
    }
    out.push('\n');

    out.push_str("Job edges (consumer -> producer)\n");
    render_edges(&s.graph.job_edges, &mut out);
    out.push('\n');

    out.push_str("Stage edges (consumer -> producer)\n");
    render_edges(&s.graph.stage_edges, &mut out);
    out.push('\n');

    out.push_str("Outputs needing isOutput=true\n");
    render_step_outputs(&s.graph.outputs_needing_is_output, &mut out);

    // Job step-count footer so users see at-a-glance how many steps
    // each job carries; helpful when comparing builds.
    out.push('\n');
    out.push_str("Job step counts\n");
    match &s.body {
        PipelineBodySummary::Jobs { jobs } => {
            for j in jobs {
                out.push_str(&format!("  {}: {}\n", j.id, j.steps.len()));
            }
        }
        PipelineBodySummary::Stages { stages } => {
            for st in stages {
                for j in &st.jobs {
                    out.push_str(&format!("  {}.{}: {}\n", st.id, j.id, j.steps.len()));
                }
            }
        }
    }
    out
}

/// Render a [`PipelineSummary`] in Graphviz DOT format.
///
/// Two clusters are emitted — one for jobs, one for stages — and
/// edges point from consumer to producer (matching the IR
/// `depends_on` semantics). Stage-grouped jobs are placed inside
/// their stage's cluster so `dot` lays them out together.
pub fn render_dot(s: &PipelineSummary) -> String {
    let mut out = String::new();
    out.push_str("digraph ado_aw_pipeline {\n");
    out.push_str("  rankdir=LR;\n");
    out.push_str("  node [shape=box, fontname=\"Helvetica\"];\n");

    match &s.body {
        PipelineBodySummary::Jobs { jobs } => {
            for j in jobs {
                out.push_str(&format!(
                    "  \"{}\" [label=\"{}\\n({} steps)\"];\n",
                    j.id,
                    escape_dot(&j.display_name),
                    j.steps.len()
                ));
            }
        }
        PipelineBodySummary::Stages { stages } => {
            for st in stages {
                out.push_str(&format!(
                    "  subgraph \"cluster_{}\" {{\n    label=\"{}\";\n    style=dashed;\n",
                    st.id,
                    escape_dot(&st.display_name),
                ));
                for j in &st.jobs {
                    out.push_str(&format!(
                        "    \"{}.{}\" [label=\"{}\\n({} steps)\"];\n",
                        st.id,
                        j.id,
                        escape_dot(&j.display_name),
                        j.steps.len()
                    ));
                }
                out.push_str("  }\n");
            }
        }
    }

    for e in &s.graph.job_edges {
        // Stages-bodied pipelines use `stage.job` as the node id so
        // we don't collide on identical job ids across stages.
        let (cons, prod) = match &s.body {
            PipelineBodySummary::Jobs { .. } => (e.consumer.clone(), e.producer.clone()),
            PipelineBodySummary::Stages { stages } => {
                let lookup = |job: &str| -> String {
                    for st in stages {
                        if st.jobs.iter().any(|j| j.id == job) {
                            return format!("{}.{}", st.id, job);
                        }
                    }
                    job.to_string()
                };
                (lookup(&e.consumer), lookup(&e.producer))
            }
        };
        out.push_str(&format!("  \"{}\" -> \"{}\";\n", cons, prod));
    }
    out.push_str("}\n");
    out
}

fn render_edges(edges: &[EdgeEntry], out: &mut String) {
    if edges.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for e in edges {
            out.push_str(&format!("  {} -> {}\n", e.consumer, e.producer));
        }
    }
}

fn render_step_outputs(entries: &[StepOutputsEntry], out: &mut String) {
    if entries.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for e in entries {
            out.push_str(&format!("  {}: {}\n", e.step, e.outputs.join(", ")));
        }
    }
}

fn escape_dot(s: &str) -> String {
    s.replace('"', "\\\"")
}

#[allow(dead_code)] // Re-export shorthand for future call sites.
pub fn graph(s: &PipelineSummary) -> &GraphSummary {
    &s.graph
}
