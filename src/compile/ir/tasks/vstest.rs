//! Typed builder for `VSTest@2`.
//!
//! `VSTest@2` is a selector-based task: the `testSelector` input controls which
//! set of inputs is active. This module uses the command-enum pattern
//! (see [`super::docker`]) so invalid selector/input combinations are
//! unrepresentable:
//!
//! - [`VsTestSelector::Assemblies`] — run tests matched by file-glob patterns
//!   (`testAssemblyVer2`).
//! - [`VsTestSelector::Plan`] — run tests from an Azure Test Plan.
//! - [`VsTestSelector::Run`] — run tests from a triggered test run.
//!
//! Options common across all selectors (search folder, code coverage, parallel
//! execution, etc.) live on the [`VsTest`] builder itself.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/vstest-v2>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `VSTest@2` `inputs:` mapping (advisory front-matter
/// validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let selector = match map.remove("testSelector") {
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`testSelector` must be a string".to_string())?,
        None => "testAssemblies".to_string(),
    };

    validate_common(&Value::Mapping(map.clone())).map_err(|e| format!("common inputs: {e}"))?;
    remove_common_inputs(&mut map);
    let rest = Value::Mapping(map);

    let result = match selector.as_str() {
        "testAssemblies" => serde_yaml::from_value::<VsTestAssemblies>(rest).map(drop),
        "testPlan" => serde_yaml::from_value::<VsTestPlan>(rest).map(drop),
        "testRun" => serde_yaml::from_value::<VsTestRun>(rest).map(drop),
        other => return Err(format!("VSTest@2: unknown testSelector `{other}`")),
    };
    result.map_err(|e| format!("testSelector `{selector}`: {e}"))
}

/// Type-checks the inputs shared by every `testSelector` variant.
///
/// Note the backing [`VsTestCommonInputs`] deliberately does **not** use
/// `deny_unknown_fields`: this runs on the full input map, which still contains
/// the *selector-specific* inputs (e.g. `testAssemblyVer2`, `testPlan`), so
/// denying unknowns here would reject those valid fields. Typos are still
/// caught — an unknown/misspelled input survives `remove_common_inputs` and is
/// reported by the selector variant's own `deny_unknown_fields` (which names the
/// offending field, attributed to the selector rather than to "common inputs").
fn validate_common(inputs: &Value) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<VsTestCommonInputs>(inputs.clone()).map(drop)
}

fn remove_common_inputs(map: &mut serde_yaml::Mapping) {
    for key in [
        "searchFolder",
        "resultsFolder",
        "runSettingsFile",
        "overrideTestrunParameters",
        "pathtoCustomTestAdapters",
        "runInParallel",
        "runTestsInIsolation",
        "codeCoverageEnabled",
        "testRunTitle",
        "platform",
        "configuration",
        "publishRunAttachments",
        "otherConsoleOptions",
        "vsTestVersion",
    ] {
        map.remove(key);
    }
}

#[derive(Debug, Deserialize)]
struct VsTestCommonInputs {
    #[serde(rename = "searchFolder", default)]
    _search_folder: Option<String>,
    #[serde(rename = "resultsFolder", default)]
    _results_folder: Option<String>,
    #[serde(rename = "runSettingsFile", default)]
    _run_settings_file: Option<String>,
    #[serde(rename = "overrideTestrunParameters", default)]
    _override_testrun_parameters: Option<String>,
    #[serde(rename = "pathtoCustomTestAdapters", default)]
    _path_to_custom_test_adapters: Option<String>,
    #[serde(
        rename = "runInParallel",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _run_in_parallel: Option<bool>,
    #[serde(
        rename = "runTestsInIsolation",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _run_tests_in_isolation: Option<bool>,
    #[serde(
        rename = "codeCoverageEnabled",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _code_coverage_enabled: Option<bool>,
    #[serde(rename = "testRunTitle", default)]
    _test_run_title: Option<String>,
    #[serde(rename = "platform", default)]
    _platform: Option<String>,
    #[serde(rename = "configuration", default)]
    _configuration: Option<String>,
    #[serde(
        rename = "publishRunAttachments",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _publish_run_attachments: Option<bool>,
    #[serde(rename = "otherConsoleOptions", default)]
    _other_console_options: Option<String>,
    #[serde(rename = "vsTestVersion", default)]
    _vs_test_version: Option<VsTestVersion>,
}

/// Visual Studio Test runner version (`vsTestVersion` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum VsTestVersion {
    /// `"latest"` — always use the newest installed version.
    #[serde(rename = "latest")]
    Latest,
    /// `"17.0"` — Visual Studio 2022.
    #[serde(rename = "17.0")]
    V17,
    /// `"16.0"` — Visual Studio 2019.
    #[serde(rename = "16.0")]
    V16,
    /// `"15.0"` — Visual Studio 2017.
    #[serde(rename = "15.0")]
    V15,
    /// `"14.0"` — Visual Studio 2015.
    #[serde(rename = "14.0")]
    V14,
    /// `"toolsInstaller"` — use the version installed by `VisualStudioTestPlatformInstaller@1`.
    #[serde(rename = "toolsInstaller")]
    ToolsInstaller,
}

impl VsTestVersion {
    /// The exact token the ADO task expects for `vsTestVersion`.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            VsTestVersion::Latest => "latest",
            VsTestVersion::V17 => "17.0",
            VsTestVersion::V16 => "16.0",
            VsTestVersion::V15 => "15.0",
            VsTestVersion::V14 => "14.0",
            VsTestVersion::ToolsInstaller => "toolsInstaller",
        }
    }
}

/// Per-selector data for `testSelector: testAssemblies` — the most common mode.
///
/// Tests are discovered via file-glob patterns (`testAssemblyVer2`). An optional
/// `testFiltercriteria` can narrow which tests within matched assemblies to run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VsTestAssemblies {
    /// `testAssemblyVer2` — newline-separated glob patterns for test DLLs,
    /// e.g. `"**\\bin\\**\\*tests.dll"`.
    #[serde(rename = "testAssemblyVer2")]
    test_assembly: String,
    /// `testFiltercriteria` — VSTest filter expression, e.g. `"TestCategory=Unit"`.
    #[serde(rename = "testFiltercriteria", default)]
    test_filter_criteria: Option<String>,
}

impl VsTestAssemblies {
    /// Required: the glob pattern(s) for test assembly DLLs.
    pub fn new(test_assembly: impl Into<String>) -> Self {
        Self {
            test_assembly: test_assembly.into(),
            test_filter_criteria: None,
        }
    }

    /// `testFiltercriteria` — e.g. `"TestCategory=Unit&FullyQualifiedName~MyNamespace"`.
    pub fn test_filter_criteria(mut self, value: impl Into<String>) -> Self {
        self.test_filter_criteria = Some(value.into());
        self
    }
}

/// Per-selector data for `testSelector: testPlan` — run tests from an Azure Test Plan.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VsTestPlan {
    /// `testPlan` — ID of the Azure Test Plan.
    #[serde(rename = "testPlan")]
    test_plan: String,
    /// `testSuite` — ID(s) of the test suite(s) within the plan.
    #[serde(rename = "testSuite")]
    test_suite: String,
    /// `testConfiguration` — ID of the test configuration.
    #[serde(rename = "testConfiguration")]
    test_configuration: String,
}

impl VsTestPlan {
    /// All three IDs are required when using a Test Plan.
    pub fn new(
        test_plan: impl Into<String>,
        test_suite: impl Into<String>,
        test_configuration: impl Into<String>,
    ) -> Self {
        Self {
            test_plan: test_plan.into(),
            test_suite: test_suite.into(),
            test_configuration: test_configuration.into(),
        }
    }
}

/// Per-selector data for `testSelector: testRun` — run tests from a triggered test run.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VsTestRun {
    /// `tcmTestRun` — test run ID; defaults to `$(test.RunId)` when omitted.
    #[serde(rename = "tcmTestRun", default)]
    tcm_test_run: Option<String>,
}

impl VsTestRun {
    pub fn new() -> Self {
        Self::default()
    }

    /// `tcmTestRun` — override the default `$(test.RunId)` with an explicit run ID.
    pub fn tcm_test_run(mut self, value: impl Into<String>) -> Self {
        self.tcm_test_run = Some(value.into());
        self
    }
}

/// `VSTest@2` selector, carrying the per-selector required/optional inputs.
#[derive(Debug, Clone)]
pub enum VsTestSelector {
    /// `testSelector: testAssemblies` — discover tests by DLL glob patterns.
    Assemblies(VsTestAssemblies),
    /// `testSelector: testPlan` — run from an Azure Test Plan.
    Plan(VsTestPlan),
    /// `testSelector: testRun` — run from a triggered test run.
    Run(VsTestRun),
}

/// Builder for a [`TaskStep`] invoking `VSTest@2`.
///
/// Construct via the selector-specific factory methods ([`VsTest::assemblies`],
/// [`VsTest::plan`], [`VsTest::run`]) and chain any common options before
/// calling [`into_step`](VsTest::into_step).
///
/// ```rust,ignore
/// use crate::compile::ir::tasks::vstest::{VsTest, VsTestAssemblies, VsTestVersion};
/// use crate::compile::ir::step::Step;
///
/// let step = Step::Task(
///     VsTest::assemblies(VsTestAssemblies::new("**\\bin\\**\\*tests.dll"))
///         .code_coverage_enabled(true)
///         .test_run_title("Unit Tests")
///         .vs_test_version(VsTestVersion::Latest)
///         .into_step(),
/// );
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/vstest-v2>
#[derive(Debug, Clone)]
pub struct VsTest {
    selector: VsTestSelector,
    /// `searchFolder` — root directory for glob expansion.
    search_folder: Option<String>,
    /// `resultsFolder` — folder for test result files.
    results_folder: Option<String>,
    /// `runSettingsFile` — path to a `.runsettings` file.
    run_settings_file: Option<String>,
    /// `overrideTestrunParameters` — space-separated `-key value` overrides.
    override_testrun_parameters: Option<String>,
    /// `pathtoCustomTestAdapters` — directory containing custom test adapters.
    path_to_custom_test_adapters: Option<String>,
    /// `runInParallel` — distribute tests across available CPU cores.
    run_in_parallel: Option<bool>,
    /// `runTestsInIsolation` — run each test in an isolated process.
    run_tests_in_isolation: Option<bool>,
    /// `codeCoverageEnabled` — collect code coverage data.
    code_coverage_enabled: Option<bool>,
    /// `testRunTitle` — label shown in the build summary and test results.
    test_run_title: Option<String>,
    /// `platform` — build platform (e.g. `x86`, `x64`, `Any CPU`).
    platform: Option<String>,
    /// `configuration` — build configuration (e.g. `Debug`, `Release`).
    configuration: Option<String>,
    /// `publishRunAttachments` — upload result files as build artifacts.
    publish_run_attachments: Option<bool>,
    /// `otherConsoleOptions` — extra options passed to `vstest.console.exe`.
    other_console_options: Option<String>,
    /// `vsTestVersion` — pin the VS Test runner version.
    vs_test_version: Option<VsTestVersion>,
    display_name: Option<String>,
}

impl VsTest {
    /// Construct from an explicit [`VsTestSelector`].
    pub fn new(selector: VsTestSelector) -> Self {
        Self {
            selector,
            search_folder: None,
            results_folder: None,
            run_settings_file: None,
            override_testrun_parameters: None,
            path_to_custom_test_adapters: None,
            run_in_parallel: None,
            run_tests_in_isolation: None,
            code_coverage_enabled: None,
            test_run_title: None,
            platform: None,
            configuration: None,
            publish_run_attachments: None,
            other_console_options: None,
            vs_test_version: None,
            display_name: None,
        }
    }

    /// `testSelector: testAssemblies` — the most common mode.
    pub fn assemblies(spec: VsTestAssemblies) -> Self {
        Self::new(VsTestSelector::Assemblies(spec))
    }

    /// `testSelector: testPlan` — run from an Azure Test Plan.
    pub fn plan(spec: VsTestPlan) -> Self {
        Self::new(VsTestSelector::Plan(spec))
    }

    /// `testSelector: testRun` — run from a triggered test run.
    pub fn run(spec: VsTestRun) -> Self {
        Self::new(VsTestSelector::Run(spec))
    }

    /// `searchFolder` — root directory for glob expansion (default: `$(System.DefaultWorkingDirectory)`).
    pub fn search_folder(mut self, value: impl Into<String>) -> Self {
        self.search_folder = Some(value.into());
        self
    }

    /// `resultsFolder` — where to write test result files.
    pub fn results_folder(mut self, value: impl Into<String>) -> Self {
        self.results_folder = Some(value.into());
        self
    }

    /// `runSettingsFile` — path to a `.runsettings` configuration file.
    pub fn run_settings_file(mut self, value: impl Into<String>) -> Self {
        self.run_settings_file = Some(value.into());
        self
    }

    /// `overrideTestrunParameters` — space-separated `-key value` parameter overrides.
    pub fn override_testrun_parameters(mut self, value: impl Into<String>) -> Self {
        self.override_testrun_parameters = Some(value.into());
        self
    }

    /// `pathtoCustomTestAdapters` — directory with custom test adapter assemblies.
    pub fn path_to_custom_test_adapters(mut self, value: impl Into<String>) -> Self {
        self.path_to_custom_test_adapters = Some(value.into());
        self
    }

    /// `runInParallel` — distribute tests across CPU cores.
    pub fn run_in_parallel(mut self, value: bool) -> Self {
        self.run_in_parallel = Some(value);
        self
    }

    /// `runTestsInIsolation` — run each test in an isolated process.
    pub fn run_tests_in_isolation(mut self, value: bool) -> Self {
        self.run_tests_in_isolation = Some(value);
        self
    }

    /// `codeCoverageEnabled` — collect code coverage data.
    pub fn code_coverage_enabled(mut self, value: bool) -> Self {
        self.code_coverage_enabled = Some(value);
        self
    }

    /// `testRunTitle` — label shown in the build summary.
    pub fn test_run_title(mut self, value: impl Into<String>) -> Self {
        self.test_run_title = Some(value.into());
        self
    }

    /// `platform` — target platform (e.g. `"x64"`, `"Any CPU"`).
    pub fn platform(mut self, value: impl Into<String>) -> Self {
        self.platform = Some(value.into());
        self
    }

    /// `configuration` — build configuration (e.g. `"Release"`, `"Debug"`).
    pub fn configuration(mut self, value: impl Into<String>) -> Self {
        self.configuration = Some(value.into());
        self
    }

    /// `publishRunAttachments` — upload test result files as build artifacts.
    pub fn publish_run_attachments(mut self, value: bool) -> Self {
        self.publish_run_attachments = Some(value);
        self
    }

    /// `otherConsoleOptions` — extra options forwarded to `vstest.console.exe`.
    pub fn other_console_options(mut self, value: impl Into<String>) -> Self {
        self.other_console_options = Some(value.into());
        self
    }

    /// `vsTestVersion` — pin the Visual Studio Test runner version.
    pub fn vs_test_version(mut self, value: VsTestVersion) -> Self {
        self.vs_test_version = Some(value);
        self
    }

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (selector_str, default_display): (&str, &str) = match &self.selector {
            VsTestSelector::Assemblies(_) => ("testAssemblies", "Run Visual Studio Tests"),
            VsTestSelector::Plan(_) => ("testPlan", "Run Visual Studio Tests (Test Plan)"),
            VsTestSelector::Run(_) => ("testRun", "Run Visual Studio Tests (Test Run)"),
        };
        let mut t = TaskStep::new(
            "VSTest@2",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("testSelector", selector_str);

        // Selector-specific inputs
        match self.selector {
            VsTestSelector::Assemblies(s) => {
                t = t.with_input("testAssemblyVer2", s.test_assembly);
                push_opt(&mut t, "testFiltercriteria", s.test_filter_criteria);
            }
            VsTestSelector::Plan(s) => {
                t = t.with_input("testPlan", s.test_plan);
                t = t.with_input("testSuite", s.test_suite);
                t = t.with_input("testConfiguration", s.test_configuration);
            }
            VsTestSelector::Run(s) => {
                push_opt(&mut t, "tcmTestRun", s.tcm_test_run);
            }
        }

        // Common options
        push_opt(&mut t, "searchFolder", self.search_folder);
        push_opt(&mut t, "resultsFolder", self.results_folder);
        push_opt(&mut t, "runSettingsFile", self.run_settings_file);
        push_opt(
            &mut t,
            "overrideTestrunParameters",
            self.override_testrun_parameters,
        );
        push_opt(
            &mut t,
            "pathtoCustomTestAdapters",
            self.path_to_custom_test_adapters,
        );
        push_bool(&mut t, "runInParallel", self.run_in_parallel);
        push_bool(&mut t, "runTestsInIsolation", self.run_tests_in_isolation);
        push_bool(&mut t, "codeCoverageEnabled", self.code_coverage_enabled);
        push_opt(&mut t, "testRunTitle", self.test_run_title);
        push_opt(&mut t, "platform", self.platform);
        push_opt(&mut t, "configuration", self.configuration);
        push_bool(
            &mut t,
            "publishRunAttachments",
            self.publish_run_attachments,
        );
        push_opt(&mut t, "otherConsoleOptions", self.other_console_options);
        if let Some(v) = self.vs_test_version {
            t = t.with_input("vsTestVersion", v.as_ado_str());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard for the two-pass validator: `remove_common_inputs` must stay
    /// in sync with `VsTestCommonInputs`. A step that sets **every** common input
    /// must validate for all three selectors — if a common field were added to
    /// the struct but forgotten in the removal list, the leftover key would trip
    /// the variant's `deny_unknown_fields` and this test would fail.
    #[test]
    fn all_common_inputs_validate_across_every_selector() {
        let common = concat!(
            "\nsearchFolder: $(System.DefaultWorkingDirectory)",
            "\nresultsFolder: $(Agent.TempDirectory)/results",
            "\nrunSettingsFile: tests.runsettings",
            "\noverrideTestrunParameters: -key value",
            "\npathtoCustomTestAdapters: adapters/",
            "\nrunInParallel: true",
            "\nrunTestsInIsolation: true",
            "\ncodeCoverageEnabled: true",
            "\ntestRunTitle: My Tests",
            "\nplatform: x64",
            "\nconfiguration: Release",
            "\npublishRunAttachments: true",
            "\notherConsoleOptions: /Blame",
            "\nvsTestVersion: latest",
        );

        let assemblies = serde_yaml::from_str(&format!(
            "testSelector: testAssemblies\ntestAssemblyVer2: '**/*tests.dll'{common}"
        ))
        .unwrap();
        assert!(
            validate_inputs(assemblies).is_ok(),
            "common inputs must validate for testAssemblies"
        );

        let plan = serde_yaml::from_str(&format!(
            "testSelector: testPlan\ntestPlan: '1'\ntestSuite: '2'\ntestConfiguration: '3'{common}"
        ))
        .unwrap();
        assert!(
            validate_inputs(plan).is_ok(),
            "common inputs must validate for testPlan"
        );

        let run = serde_yaml::from_str(&format!("testSelector: testRun{common}")).unwrap();
        assert!(
            validate_inputs(run).is_ok(),
            "common inputs must validate for testRun"
        );
    }

    #[test]
    fn assemblies_sets_selector_and_pattern() {
        let t = VsTest::assemblies(VsTestAssemblies::new("**\\bin\\**\\*tests.dll")).into_step();
        assert_eq!(t.task, "VSTest@2");
        assert_eq!(t.display_name, "Run Visual Studio Tests");
        assert_eq!(
            t.inputs.get("testSelector").map(String::as_str),
            Some("testAssemblies")
        );
        assert_eq!(
            t.inputs.get("testAssemblyVer2").map(String::as_str),
            Some("**\\bin\\**\\*tests.dll")
        );
        // Selector-specific plan inputs must not leak into assemblies step
        assert!(t.inputs.get("testPlan").is_none());
        assert!(t.inputs.get("testSuite").is_none());
    }

    #[test]
    fn assemblies_with_filter_criteria() {
        let t = VsTest::assemblies(
            VsTestAssemblies::new("**\\*tests.dll").test_filter_criteria("TestCategory=Unit"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("testFiltercriteria").map(String::as_str),
            Some("TestCategory=Unit")
        );
    }

    #[test]
    fn assemblies_common_options() {
        let t = VsTest::assemblies(VsTestAssemblies::new("**\\*tests.dll"))
            .code_coverage_enabled(true)
            .run_in_parallel(true)
            .test_run_title("Unit Tests")
            .platform("x64")
            .configuration("Release")
            .vs_test_version(VsTestVersion::V17)
            .run_settings_file("test.runsettings")
            .into_step();
        assert_eq!(
            t.inputs.get("codeCoverageEnabled").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("runInParallel").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("testRunTitle").map(String::as_str),
            Some("Unit Tests")
        );
        assert_eq!(t.inputs.get("platform").map(String::as_str), Some("x64"));
        assert_eq!(
            t.inputs.get("configuration").map(String::as_str),
            Some("Release")
        );
        assert_eq!(
            t.inputs.get("vsTestVersion").map(String::as_str),
            Some("17.0")
        );
        assert_eq!(
            t.inputs.get("runSettingsFile").map(String::as_str),
            Some("test.runsettings")
        );
    }

    #[test]
    fn plan_selector_sets_required_inputs() {
        let t = VsTest::plan(VsTestPlan::new("42", "101", "5")).into_step();
        assert_eq!(t.task, "VSTest@2");
        assert_eq!(
            t.inputs.get("testSelector").map(String::as_str),
            Some("testPlan")
        );
        assert_eq!(t.inputs.get("testPlan").map(String::as_str), Some("42"));
        assert_eq!(t.inputs.get("testSuite").map(String::as_str), Some("101"));
        assert_eq!(
            t.inputs.get("testConfiguration").map(String::as_str),
            Some("5")
        );
        // Assemblies-specific input must not appear
        assert!(t.inputs.get("testAssemblyVer2").is_none());
    }

    #[test]
    fn run_selector_defaults() {
        let t = VsTest::run(VsTestRun::new()).into_step();
        assert_eq!(
            t.inputs.get("testSelector").map(String::as_str),
            Some("testRun")
        );
        // tcmTestRun omitted — ADO uses $(test.RunId) default
        assert!(t.inputs.get("tcmTestRun").is_none());
    }

    #[test]
    fn run_selector_explicit_run_id() {
        let t = VsTest::run(VsTestRun::new().tcm_test_run("$(test.RunId)")).into_step();
        assert_eq!(
            t.inputs.get("tcmTestRun").map(String::as_str),
            Some("$(test.RunId)")
        );
    }

    #[test]
    fn display_name_override() {
        let t = VsTest::assemblies(VsTestAssemblies::new("**\\*tests.dll"))
            .with_display_name("Run MyApp Tests")
            .into_step();
        assert_eq!(t.display_name, "Run MyApp Tests");
    }

    #[test]
    fn vs_test_version_ado_strings() {
        assert_eq!(VsTestVersion::Latest.as_ado_str(), "latest");
        assert_eq!(VsTestVersion::V17.as_ado_str(), "17.0");
        assert_eq!(VsTestVersion::V16.as_ado_str(), "16.0");
        assert_eq!(VsTestVersion::V15.as_ado_str(), "15.0");
        assert_eq!(VsTestVersion::V14.as_ado_str(), "14.0");
        assert_eq!(VsTestVersion::ToolsInstaller.as_ado_str(), "toolsInstaller");
    }
}
