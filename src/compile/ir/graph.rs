//! Dependency-graph pass: derive job- and stage-level `dependsOn`
//! from the typed [`super::output::OutputRef`]s declared in steps.
//!
//! ## What the graph captures
//!
//! Every [`super::env::EnvValue::StepOutput`],
//! [`super::env::EnvValue::Coalesce`] / [`super::env::EnvValue::Concat`]
//! child, and
//! [`super::condition::Expr::StepOutput`] inside a step's `env` /
//! `condition` is an edge from the **consumer** step (the one that
//! reads the value) to the **producer** step (the one that names the
//! output). The graph pass lifts those step-level edges to:
//!
//! - **Same-stage cross-job edges** — added to
//!   [`super::job::Job::depends_on`].
//! - **Cross-stage edges** — added to
//!   [`super::stage::Stage::depends_on`].
//!
//! Same-job edges (consumer and producer share both stage and job)
//! contribute nothing to `dependsOn`; ADO orders steps within a job
//! by their position in the YAML.
//!
//! ## Validation
//!
//! As a side-effect of walking the graph this module rejects:
//!
//! - References to a step that does not exist anywhere in the
//!   pipeline (`UnknownProducer`).
//! - References to a step whose [`super::step::Step::id`] is `None`
//!   (`AnonymousProducer`).
//! - References to a producer that does not declare the named output
//!   (`UnknownOutput`).
//! - Duplicate step / job / stage ids (`DuplicateStepId`,
//!   `DuplicateJobId`, `DuplicateStageId`).
//! - Cycles in the derived `dependsOn` graph (`Cycle`).
//!
//! ## Entry points
//!
//! - [`resolve`] is the all-in-one pass: build the graph, validate,
//!   populate `depends_on`. Most callers want this.
//! - [`build_graph`] returns the typed graph without mutating the
//!   pipeline (useful for diagnostics / tests).

use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use super::condition::{Condition, Expr};
use super::env::EnvValue;
use super::ids::{JobId, StageId, StepId};
use super::output::OutputRef;
use super::step::{BashStep, Step, TaskStep};
use super::{Pipeline, PipelineBody};

/// Location of a step inside the pipeline.
///
/// `stage` is `None` for steps that live in a flat
/// [`PipelineBody::Jobs`] (no enclosing stage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLocation {
    pub stage: Option<StageId>,
    pub job: JobId,
    /// The set of outputs declared by the producing step. Used by the
    /// validate pass to reject `UnknownOutput` references.
    pub outputs: BTreeSet<String>,
}

/// The derived dependency graph.
///
/// Edges point from **consumer** to **producer** (i.e. consumer
/// `depends_on` producer).
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// `StepId → (stage?, job, declared outputs)`.
    pub step_locations: BTreeMap<StepId, StepLocation>,
    /// `(consumer_job, producer_job)` edges, all in the same stage
    /// or both stage-less.
    pub job_edges: BTreeSet<(JobId, JobId)>,
    /// `(consumer_stage, producer_stage)` edges.
    pub stage_edges: BTreeSet<(StageId, StageId)>,
    /// For each producer step, the set of declared outputs that have
    /// at least one cross-step reader. ADO requires `isOutput=true`
    /// on the matching `##vso[task.setvariable]` directive for the
    /// output to be visible to **any** cross-step consumer; producers
    /// are responsible for emitting that flag — the IR does not
    /// rewrite step bodies. See [`super::output::OutputDecl`] for the
    /// full contract.
    ///
    /// Populated by [`build_graph`] as a side-effect of walking
    /// every consumer's `OutputRef`s. Same-job references DO count
    /// here even though they don't add a `dependsOn` edge — ADO
    /// requires `isOutput=true` on the producer for both
    /// `$(stepName.X)` (same job) and cross-job/cross-stage syntax.
    pub outputs_needing_is_output: BTreeMap<StepId, BTreeSet<String>>,
}

/// Walk the pipeline, validate the OutputRef graph, derive
/// `dependsOn`, write the derived edges back to
/// [`super::job::Job::depends_on`] and
/// [`super::stage::Stage::depends_on`], and propagate the
/// auto-`isOutput` flag back to every relevant
/// [`super::output::OutputDecl::auto_is_output`].
///
/// Existing values in either `depends_on` field are treated as
/// manual overrides and **preserved**; the graph pass adds missing
/// edges but never removes user-supplied ones.
pub fn resolve(p: &mut Pipeline) -> Result<()> {
    let graph = build_graph(p)?;
    detect_cycles(&graph)?;
    apply_edges(p, &graph);
    apply_auto_is_output(p, &graph);
    Ok(())
}

/// Build a [`Graph`] without mutating the pipeline.
///
/// Performs all per-step validation (`UnknownProducer`,
/// `AnonymousProducer`, `UnknownOutput`, `Duplicate*Id`) but does not
/// run cycle detection — call [`detect_cycles`] separately if needed.
pub fn build_graph(p: &Pipeline) -> Result<Graph> {
    let mut g = Graph::default();
    let mut seen_stage_ids: HashSet<&str> = HashSet::new();
    let mut seen_job_ids: HashSet<&str> = HashSet::new();

    // Pass 1: index every step's location + outputs. Reject duplicate
    // ids of every kind.
    match &p.body {
        PipelineBody::Jobs(jobs) => {
            for job in jobs {
                if !seen_job_ids.insert(job.id.as_str()) {
                    bail!("ir::graph: duplicate JobId '{}'", job.id);
                }
                index_job_steps(None, job, &mut g)?;
            }
        }
        PipelineBody::Stages(stages) => {
            for stage in stages {
                if !seen_stage_ids.insert(stage.id.as_str()) {
                    bail!("ir::graph: duplicate StageId '{}'", stage.id);
                }
                // Job-id uniqueness is **per-stage** in ADO, so reset
                // the seen-set for each stage.
                let mut local_jobs: HashSet<&str> = HashSet::new();
                for job in &stage.jobs {
                    if !local_jobs.insert(job.id.as_str()) {
                        bail!(
                            "ir::graph: duplicate JobId '{}' inside stage '{}'",
                            job.id, stage.id
                        );
                    }
                    index_job_steps(Some(stage.id.clone()), job, &mut g)?;
                }
            }
        }
    }

    // Pass 2: walk every OutputRef and add the corresponding edges.
    match &p.body {
        PipelineBody::Jobs(jobs) => {
            for job in jobs {
                add_edges_from_job(None, job, &mut g)?;
            }
        }
        PipelineBody::Stages(stages) => {
            for stage in stages {
                for job in &stage.jobs {
                    add_edges_from_job(Some(stage.id.clone()), job, &mut g)?;
                }
            }
        }
    }

    Ok(g)
}

fn index_job_steps(
    stage: Option<StageId>,
    job: &super::job::Job,
    g: &mut Graph,
) -> Result<()> {
    for step in &job.steps {
        if let Some(id) = step.id() {
            // Step ids are pipeline-wide identifiers in ADO when
            // referenced via `dependencies.<job>.outputs['<step>.X']`,
            // so duplicate ids across jobs are technically allowed if
            // both jobs are referenced through the qualifying job
            // name. We still reject true duplicates inside the SAME
            // job, which would silently shadow.
            if let Some(prev) = g.step_locations.get(id)
                && prev.stage == stage
                && prev.job == job.id
            {
                bail!(
                    "ir::graph: duplicate StepId '{}' inside job '{}'",
                    id, job.id
                );
            }
            let outputs: BTreeSet<String> = collect_step_outputs(step);
            g.step_locations.insert(
                id.clone(),
                StepLocation {
                    stage: stage.clone(),
                    job: job.id.clone(),
                    outputs,
                },
            );
        }
    }
    Ok(())
}

fn collect_step_outputs(step: &Step) -> BTreeSet<String> {
    match step {
        Step::Bash(BashStep { outputs, .. }) => {
            outputs.iter().map(|o| o.name.clone()).collect()
        }
        // TaskStep doesn't currently model outputs; if we ever add
        // them, extend here. CheckoutStep / DownloadStep / PublishStep
        // don't emit step outputs. RawYaml is opaque to the IR.
        Step::Task(TaskStep { .. })
        | Step::Checkout(_)
        | Step::Download(_)
        | Step::Publish(_)
        | Step::RawYaml(_) => BTreeSet::new(),
    }
}

fn add_edges_from_job(
    stage: Option<StageId>,
    job: &super::job::Job,
    g: &mut Graph,
) -> Result<()> {
    // Walk job-level condition references.
    if let Some(cond) = &job.condition {
        for r in collect_condition_refs(cond) {
            add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
        }
    }
    // Walk every step's env + condition.
    for step in &job.steps {
        match step {
            Step::Bash(b) => {
                for r in collect_env_refs(b.env.values()) {
                    add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                }
                if let Some(cond) = &b.condition {
                    for r in collect_condition_refs(cond) {
                        add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                    }
                }
            }
            Step::Task(t) => {
                for r in collect_env_refs(t.env.values()) {
                    add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                }
                if let Some(cond) = &t.condition {
                    for r in collect_condition_refs(cond) {
                        add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                    }
                }
            }
            Step::Checkout(_) => {}
            Step::Download(d) => {
                if let Some(cond) = &d.condition {
                    for r in collect_condition_refs(cond) {
                        add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                    }
                }
            }
            Step::Publish(p) => {
                if let Some(cond) = &p.condition {
                    for r in collect_condition_refs(cond) {
                        add_edge_for_ref(stage.as_ref(), &job.id, r, g)?;
                    }
                }
            }
            // `RawYaml` carries opaque user-authored YAML; the graph
            // pass cannot introspect it. Producers that need
            // cross-step refs must use a typed Bash/Task variant.
            Step::RawYaml(_) => {}
        }
    }
    Ok(())
}

fn collect_env_refs<'a, I: IntoIterator<Item = &'a EnvValue>>(
    values: I,
) -> Vec<&'a OutputRef> {
    let mut out = Vec::new();
    for v in values {
        collect_env_refs_into(v, &mut out);
    }
    out
}

fn collect_env_refs_into<'a>(v: &'a EnvValue, out: &mut Vec<&'a OutputRef>) {
    match v {
        EnvValue::Literal(_)
        | EnvValue::AdoMacro(_)
        | EnvValue::PipelineVar(_)
        | EnvValue::Secret(_)
        | EnvValue::RawYamlScalar(_) => {}
        EnvValue::StepOutput(r) => out.push(r),
        EnvValue::Coalesce(children) | EnvValue::Concat(children) => {
            for c in children {
                collect_env_refs_into(c, out);
            }
        }
    }
}

fn collect_condition_refs(c: &Condition) -> Vec<&OutputRef> {
    let mut out = Vec::new();
    walk_condition(c, &mut out);
    out
}

fn walk_condition<'a>(c: &'a Condition, out: &mut Vec<&'a OutputRef>) {
    match c {
        Condition::Succeeded
        | Condition::Always
        | Condition::Failed
        | Condition::SucceededOrFailed
        | Condition::Custom(_) => {}
        Condition::And(parts) | Condition::Or(parts) => {
            for p in parts {
                walk_condition(p, out);
            }
        }
        Condition::Not(inner) => walk_condition(inner, out),
        Condition::Eq(a, b) | Condition::Ne(a, b) => {
            walk_expr(a, out);
            walk_expr(b, out);
        }
    }
}

fn walk_expr<'a>(e: &'a Expr, out: &mut Vec<&'a OutputRef>) {
    match e {
        Expr::Literal(_) | Expr::Variable(_) => {}
        Expr::StepOutput(r) => out.push(r),
    }
}

fn add_edge_for_ref(
    consumer_stage: Option<&StageId>,
    consumer_job: &JobId,
    r: &OutputRef,
    g: &mut Graph,
) -> Result<()> {
    let loc = g.step_locations.get(&r.step).ok_or_else(|| {
        anyhow::anyhow!(
            "ir::graph: OutputRef references unknown step '{}': consumer {}.{}",
            r.step,
            consumer_stage.map(|s| s.to_string()).unwrap_or_else(|| "<no stage>".to_string()),
            consumer_job
        )
    })?;
    if !loc.outputs.contains(&r.name) {
        let known: Vec<String> = loc.outputs.iter().cloned().collect();
        bail!(
            "ir::graph: OutputRef '{step}.{name}' is not declared by the producer step's \
             outputs list (declared outputs: [{known}]).",
            step = r.step,
            name = r.name,
            known = known.join(", "),
        );
    }
    let producer_job = loc.job.clone();
    let producer_stage = loc.stage.clone();

    // Any cross-step (or same-job-different-step) reader is a reason
    // for the producer to set isOutput=true on its ##vso[task.setvariable]
    // line; record it so producers can consult the flag at emit time.
    g.outputs_needing_is_output
        .entry(r.step.clone())
        .or_default()
        .insert(r.name.clone());

    // Same-job edges contribute nothing to dependsOn.
    if producer_job == *consumer_job && producer_stage.as_ref() == consumer_stage {
        return Ok(());
    }

    // Cross-stage edge: add stage edge AND surface a cross-job edge
    // even when the producer's job has the same id as the consumer's
    // job, because ADO requires both `stageDependencies` AND a
    // `dependsOn` declaration on the consumer stage.
    if producer_stage.as_ref() != consumer_stage {
        if let (Some(prod_stage), Some(cons_stage)) = (producer_stage, consumer_stage) {
            if &prod_stage != cons_stage {
                g.stage_edges.insert((cons_stage.clone(), prod_stage));
            }
        } else {
            // Mixed staged/un-staged in the same pipeline is malformed.
            bail!(
                "ir::graph: cross-stage OutputRef between staged and un-staged sections \
                 of the same pipeline is not supported (consumer job '{}', producer step '{}')",
                consumer_job, r.step
            );
        }
    } else {
        // Same stage (or both stage-less): a cross-job edge inside it.
        g.job_edges
            .insert((consumer_job.clone(), producer_job));
    }
    Ok(())
}

/// Detect cycles in the derived graph.
///
/// Uses Kahn's algorithm (BFS over in-degree-0 nodes) on both the
/// job and stage edge sets. Returns an error with the offending
/// nodes when a cycle is detected.
pub fn detect_cycles(g: &Graph) -> Result<()> {
    detect_cycles_in("job", &g.job_edges)?;
    detect_cycles_in("stage", &g.stage_edges)?;
    Ok(())
}

fn detect_cycles_in<T: Clone + Eq + Ord + std::hash::Hash + std::fmt::Display>(
    kind: &'static str,
    edges: &BTreeSet<(T, T)>,
) -> Result<()> {
    // Build adjacency + in-degree maps. Each edge (consumer, producer)
    // means consumer DEPENDS on producer, so for topological purposes
    // we orient producer -> consumer.
    let mut adjacency: HashMap<T, Vec<T>> = HashMap::new();
    let mut in_degree: HashMap<T, usize> = HashMap::new();
    for (consumer, producer) in edges {
        adjacency.entry(producer.clone()).or_default().push(consumer.clone());
        *in_degree.entry(consumer.clone()).or_insert(0) += 1;
        in_degree.entry(producer.clone()).or_insert(0);
    }

    let mut queue: VecDeque<T> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut visited = 0usize;
    while let Some(n) = queue.pop_front() {
        visited += 1;
        if let Some(succs) = adjacency.get(&n) {
            for s in succs {
                let entry = in_degree.get_mut(s).expect("node must be in in_degree");
                *entry -= 1;
                if *entry == 0 {
                    queue.push_back(s.clone());
                }
            }
        }
    }

    if visited != in_degree.len() {
        // Find a node still with positive in-degree — it's on the
        // cycle. The error message lists every such node so an
        // operator can locate the offending sub-graph.
        let mut cycle_nodes: Vec<String> = in_degree
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(n, _)| n.to_string())
            .collect();
        cycle_nodes.sort();
        bail!(
            "ir::graph: cycle in {kind} dependency graph involving: {nodes}",
            nodes = cycle_nodes.join(", "),
        );
    }
    Ok(())
}

fn apply_edges(p: &mut Pipeline, g: &Graph) {
    // Build per-consumer lookup maps once.
    let mut job_to_producers: HashMap<JobId, BTreeSet<JobId>> = HashMap::new();
    for (consumer, producer) in &g.job_edges {
        job_to_producers
            .entry(consumer.clone())
            .or_default()
            .insert(producer.clone());
    }
    let mut stage_to_producers: HashMap<StageId, BTreeSet<StageId>> = HashMap::new();
    for (consumer, producer) in &g.stage_edges {
        stage_to_producers
            .entry(consumer.clone())
            .or_default()
            .insert(producer.clone());
    }

    match &mut p.body {
        PipelineBody::Jobs(jobs) => {
            for job in jobs {
                merge_job_deps(job, &job_to_producers);
            }
        }
        PipelineBody::Stages(stages) => {
            for stage in stages {
                if let Some(prods) = stage_to_producers.get(&stage.id) {
                    let mut existing: BTreeSet<StageId> =
                        stage.depends_on.iter().cloned().collect();
                    existing.extend(prods.iter().cloned());
                    stage.depends_on = existing.into_iter().collect();
                }
                for job in &mut stage.jobs {
                    merge_job_deps(job, &job_to_producers);
                }
            }
        }
    }
}

fn merge_job_deps(
    job: &mut super::job::Job,
    job_to_producers: &HashMap<JobId, BTreeSet<JobId>>,
) {
    if let Some(prods) = job_to_producers.get(&job.id) {
        let mut existing: BTreeSet<JobId> = job.depends_on.iter().cloned().collect();
        existing.extend(prods.iter().cloned());
        job.depends_on = existing.into_iter().collect();
    }
}

/// Set [`super::output::OutputDecl::auto_is_output`] on every output
/// declaration that has at least one cross-step reader.
fn apply_auto_is_output(p: &mut Pipeline, g: &Graph) {
    if g.outputs_needing_is_output.is_empty() {
        return;
    }
    fn visit_job(job: &mut super::job::Job, g: &Graph) {
        for step in &mut job.steps {
            if let Step::Bash(b) = step
                && let Some(id) = &b.id
                && let Some(promoted) = g.outputs_needing_is_output.get(id)
            {
                for decl in &mut b.outputs {
                    if promoted.contains(&decl.name) {
                        decl.auto_is_output = true;
                    }
                }
            }
        }
    }
    match &mut p.body {
        PipelineBody::Jobs(jobs) => {
            for job in jobs {
                visit_job(job, g);
            }
        }
        PipelineBody::Stages(stages) => {
            for stage in stages {
                for job in &mut stage.jobs {
                    visit_job(job, g);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::condition::{Condition, Expr};
    use crate::compile::ir::env::EnvValue;
    use crate::compile::ir::job::{Job, Pool};
    use crate::compile::ir::output::{OutputDecl, OutputRef};
    use crate::compile::ir::stage::Stage;
    use crate::compile::ir::step::{BashStep, Step};
    use crate::compile::ir::{PipelineBody, PipelineShape, Resources, Triggers};

    fn pool() -> Pool {
        Pool::VmImage("ubuntu-22.04".into())
    }

    fn pipe(body: PipelineBody) -> Pipeline {
        Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body,
            shape: PipelineShape::Standalone,
        }
    }

    #[test]
    fn cross_job_outputref_adds_dependson_edge() {
        // Setup.synthPr -> Agent.runner (cross-job, same body)
        let synth = StepId::new("synthPr").unwrap();
        let setup_step = Step::Bash(
            BashStep::new("Setup work", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR")),
        );
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", pool());
        setup.push_step(setup_step);

        let agent_step = Step::Bash(
            BashStep::new("Agent work", "echo a")
                .with_env(
                    "AW_SYNTHETIC_PR",
                    EnvValue::step_output(OutputRef::new(synth, "AW_SYNTHETIC_PR")),
                ),
        );
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(agent_step);

        let mut p = pipe(PipelineBody::Jobs(vec![setup, agent]));
        resolve(&mut p).unwrap();

        if let PipelineBody::Jobs(jobs) = &p.body {
            let agent = jobs.iter().find(|j| j.id.as_str() == "Agent").unwrap();
            assert_eq!(agent.depends_on.len(), 1);
            assert_eq!(agent.depends_on[0].as_str(), "Setup");
            // Producer's OutputDecl now has auto_is_output = true.
            let setup = jobs.iter().find(|j| j.id.as_str() == "Setup").unwrap();
            if let Step::Bash(b) = &setup.steps[0] {
                assert_eq!(b.outputs.len(), 1);
                assert!(
                    b.outputs[0].auto_is_output,
                    "auto_is_output must be set on producers with cross-step readers"
                );
            } else {
                panic!();
            }
        } else {
            panic!();
        }
    }

    #[test]
    fn auto_is_output_flag_only_promotes_referenced_outputs() {
        // Producer declares TWO outputs but only one is read.
        let synth = StepId::new("synthPr").unwrap();
        let producer = Step::Bash(
            BashStep::new("s", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("READ_ME"))
                .with_output(OutputDecl::new("IGNORED")),
        );
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", pool());
        setup.push_step(producer);
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(Step::Bash(BashStep::new("a", "echo a").with_env(
            "X",
            EnvValue::step_output(OutputRef::new(synth, "READ_ME")),
        )));
        let mut p = pipe(PipelineBody::Jobs(vec![setup, agent]));
        resolve(&mut p).unwrap();

        if let PipelineBody::Jobs(jobs) = &p.body {
            let setup = jobs.iter().find(|j| j.id.as_str() == "Setup").unwrap();
            if let Step::Bash(b) = &setup.steps[0] {
                let read = b.outputs.iter().find(|o| o.name == "READ_ME").unwrap();
                let ignored = b.outputs.iter().find(|o| o.name == "IGNORED").unwrap();
                assert!(read.auto_is_output, "READ_ME must be promoted");
                assert!(
                    !ignored.auto_is_output,
                    "IGNORED has no cross-step reader; must not be promoted"
                );
            } else {
                panic!();
            }
        }
    }

    #[test]
    fn cross_stage_outputref_adds_stage_and_job_dependson() {
        // (StageA, Setup).synthPr  ->  (StageB, Agent) condition uses it.
        let synth = StepId::new("synthPr").unwrap();
        let setup_step = Step::Bash(
            BashStep::new("Setup", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR_SKIP")),
        );
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", pool());
        setup.push_step(setup_step);
        let mut stage_a = Stage::new(StageId::new("StageA").unwrap(), "Setup-stage");
        stage_a.push_job(setup);

        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.condition = Some(Condition::Ne(
            Expr::StepOutput(OutputRef::new(synth, "AW_SYNTHETIC_PR_SKIP")),
            Expr::Literal("true".into()),
        ));
        agent.push_step(Step::Bash(BashStep::new("a", "echo a")));
        let mut stage_b = Stage::new(StageId::new("StageB").unwrap(), "Agent-stage");
        stage_b.push_job(agent);

        let mut p = pipe(PipelineBody::Stages(vec![stage_a, stage_b]));
        resolve(&mut p).unwrap();

        if let PipelineBody::Stages(stages) = &p.body {
            let stage_b = stages.iter().find(|s| s.id.as_str() == "StageB").unwrap();
            assert_eq!(stage_b.depends_on.len(), 1);
            assert_eq!(stage_b.depends_on[0].as_str(), "StageA");
            // Note: cross-stage refs *don't* add a per-job dependsOn —
            // ADO models cross-stage deps at the stage level.
            assert!(stage_b.jobs[0].depends_on.is_empty());
        } else {
            panic!();
        }
    }

    #[test]
    fn same_job_outputref_does_not_add_self_dependency() {
        let synth = StepId::new("synthPr").unwrap();
        let producer = Step::Bash(
            BashStep::new("p", "echo p")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("X")),
        );
        let consumer = Step::Bash(BashStep::new("c", "echo c").with_env(
            "X",
            EnvValue::step_output(OutputRef::new(synth, "X")),
        ));
        let mut job = Job::new(JobId::new("Same").unwrap(), "Same", pool());
        job.push_step(producer);
        job.push_step(consumer);

        let mut p = pipe(PipelineBody::Jobs(vec![job]));
        resolve(&mut p).unwrap();
        if let PipelineBody::Jobs(jobs) = &p.body {
            assert!(jobs[0].depends_on.is_empty());
        } else {
            panic!();
        }
    }

    #[test]
    fn unknown_producer_is_rejected() {
        let consumer = Step::Bash(BashStep::new("c", "echo c").with_env(
            "X",
            EnvValue::step_output(OutputRef::new(StepId::new("ghost").unwrap(), "X")),
        ));
        let mut job = Job::new(JobId::new("J").unwrap(), "J", pool());
        job.push_step(consumer);
        let mut p = pipe(PipelineBody::Jobs(vec![job]));
        let err = resolve(&mut p).unwrap_err();
        assert!(format!("{err:#}").contains("unknown step 'ghost'"));
    }

    #[test]
    fn unknown_output_is_rejected() {
        let id = StepId::new("p").unwrap();
        let producer = Step::Bash(
            BashStep::new("p", "echo p")
                .with_id(id.clone())
                .with_output(OutputDecl::new("KNOWN")),
        );
        let consumer = Step::Bash(BashStep::new("c", "echo c").with_env(
            "X",
            EnvValue::step_output(OutputRef::new(id, "MISSING")),
        ));
        let mut job_a = Job::new(JobId::new("A").unwrap(), "A", pool());
        job_a.push_step(producer);
        let mut job_b = Job::new(JobId::new("B").unwrap(), "B", pool());
        job_b.push_step(consumer);
        let mut p = pipe(PipelineBody::Jobs(vec![job_a, job_b]));
        let err = resolve(&mut p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("OutputRef 'p.MISSING' is not declared"));
        assert!(msg.contains("KNOWN"));
    }

    #[test]
    fn duplicate_job_id_in_same_stage_is_rejected() {
        let make = |id: &str| Job::new(JobId::new(id).unwrap(), id, pool());
        let mut p = pipe(PipelineBody::Jobs(vec![make("Dup"), make("Dup")]));
        let err = build_graph(&p).unwrap_err();
        assert!(format!("{err:#}").contains("duplicate JobId 'Dup'"));
        // also via resolve (same code path)
        let err = resolve(&mut p).unwrap_err();
        assert!(format!("{err:#}").contains("duplicate JobId 'Dup'"));
    }

    #[test]
    fn cycle_in_job_graph_is_rejected_with_listed_nodes() {
        // A.X consumed by B; B.Y consumed by A => cycle.
        let a_step_id = StepId::new("aStep").unwrap();
        let b_step_id = StepId::new("bStep").unwrap();
        let a = {
            let mut j = Job::new(JobId::new("A").unwrap(), "A", pool());
            j.push_step(Step::Bash(
                BashStep::new("a", "echo a")
                    .with_id(a_step_id.clone())
                    .with_output(OutputDecl::new("X"))
                    .with_env(
                        "FROM_B",
                        EnvValue::step_output(OutputRef::new(b_step_id.clone(), "Y")),
                    ),
            ));
            j
        };
        let b = {
            let mut j = Job::new(JobId::new("B").unwrap(), "B", pool());
            j.push_step(Step::Bash(
                BashStep::new("b", "echo b")
                    .with_id(b_step_id)
                    .with_output(OutputDecl::new("Y"))
                    .with_env(
                        "FROM_A",
                        EnvValue::step_output(OutputRef::new(a_step_id, "X")),
                    ),
            ));
            j
        };
        let mut p = pipe(PipelineBody::Jobs(vec![a, b]));
        let err = resolve(&mut p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("cycle in job dependency graph"));
        assert!(msg.contains("A"));
        assert!(msg.contains("B"));
    }

    #[test]
    fn coalesce_children_contribute_edges() {
        let synth = StepId::new("synthPr").unwrap();
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", pool());
        setup.push_step(Step::Bash(
            BashStep::new("s", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID")),
        ));

        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(Step::Bash(BashStep::new("a", "echo a").with_env(
            "PR_ID",
            EnvValue::coalesce(vec![
                EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap(),
                EnvValue::step_output(OutputRef::new(synth, "AW_SYNTHETIC_PR_ID")),
            ]),
        )));

        let mut p = pipe(PipelineBody::Jobs(vec![setup, agent]));
        resolve(&mut p).unwrap();
        if let PipelineBody::Jobs(jobs) = &p.body {
            let agent = jobs.iter().find(|j| j.id.as_str() == "Agent").unwrap();
            assert_eq!(agent.depends_on.len(), 1);
            assert_eq!(agent.depends_on[0].as_str(), "Setup");
        }
    }

    #[test]
    fn five_stage_chain_derives_full_dependson_path() {
        // S1 -> S2 -> S3 -> S4 -> S5 (each stage's only job reads the
        // previous stage's output).
        let make_step = |name: &str, output: &str| -> Step {
            Step::Bash(
                BashStep::new(name, format!("echo {name}"))
                    .with_id(StepId::new(name).unwrap())
                    .with_output(OutputDecl::new(output)),
            )
        };
        let make_consumer_step = |name: &str, producer: &str, output: &str| -> Step {
            Step::Bash(BashStep::new(name, format!("echo {name}")).with_env(
                output,
                EnvValue::step_output(OutputRef::new(
                    StepId::new(producer).unwrap(),
                    output,
                )),
            ))
        };

        // Build chain.
        let mut stages = Vec::new();
        for i in 1..=5 {
            let stage_id = format!("S{i}");
            let mut job = Job::new(
                JobId::new(format!("J{i}")).unwrap(),
                format!("J{i}"),
                pool(),
            );
            // Producer step in this stage.
            job.push_step(make_step(&format!("p{i}"), &format!("V{i}")));
            // If not the first stage, this job's job-level condition
            // also reads the previous stage's output (forces a
            // stage->stage edge).
            if i > 1 {
                let prev_step = format!("p{}", i - 1);
                let prev_var = format!("V{}", i - 1);
                job.condition = Some(Condition::Ne(
                    Expr::StepOutput(OutputRef::new(
                        StepId::new(prev_step).unwrap(),
                        prev_var,
                    )),
                    Expr::Literal("skip".into()),
                ));
                // Belt and suspenders: also reference it from a step's env so the
                // graph code touches both the condition walk and the env walk.
                job.push_step(make_consumer_step(
                    &format!("c{i}"),
                    &format!("p{}", i - 1),
                    &format!("V{}", i - 1),
                ));
            }
            let mut st = Stage::new(StageId::new(stage_id).unwrap(), format!("S{i}"));
            st.push_job(job);
            stages.push(st);
        }

        let mut p = pipe(PipelineBody::Stages(stages));
        resolve(&mut p).unwrap();

        if let PipelineBody::Stages(stages) = &p.body {
            // S1 has no producer => empty depends_on. S2..S5 each
            // depend on the immediately preceding stage exactly once.
            assert!(stages[0].depends_on.is_empty(), "S1 must be a leaf");
            for (i, stage) in stages.iter().enumerate().skip(1) {
                let expected = format!("S{}", i);
                let dependences: Vec<&str> = stage.depends_on.iter().map(|s| s.as_str()).collect();
                assert_eq!(
                    dependences,
                    vec![expected.as_str()],
                    "S{} depends_on must be exactly [S{}]", i + 1, i
                );
            }
        } else {
            panic!();
        }
    }
}
