//! Structured generation of the shell scripts this compiler emits.
//!
//! # The problem this solves
//!
//! Generated shell used to be built with `format!`, which forced three
//! layers of escaping onto every script: `\n\` continuations to fake
//! multi-line source, `{{` / `}}` to survive `format!`'s own syntax (so a
//! Docker Go-template read `{{{{.State.Status}}}}`), and `\"` for every
//! quoted word. A 200-line body written that way is not reviewable as shell,
//! and reviewing it as shell is the only way to know it is correct.
//!
//! # The shape
//!
//! A script is a plain raw-string constant — the shell exactly as it will run,
//! with no escaping — registered with metadata by the [`shell_script!`] macro
//! and rendered through [`ShellScript`]:
//!
//! ```ignore
//! shell_script! {
//!     /// Greppable: search `mkfifo` and you land on the producer.
//!     START_ADO_PROXY {
//!         interpreter: Bash,
//!         bindings: [PROXY_IMAGE, PROXY_CONTAINER, AGENT_TEMP],
//!         externals: [ADO_PROXY_BEARER],
//!         fragments: [resolve_org],
//!         body: r#"
//! set -euo pipefail
//! # ado-aw:fragment resolve_org
//! PROXY_DIR=$(mktemp -d "$AGENT_TEMP/ado-proxy.XXXXXX")
//! docker run -d --name "$PROXY_CONTAINER" "$PROXY_IMAGE" >/dev/null
//! docker inspect -f 'state={{.State.Status}}' "$PROXY_CONTAINER"
//! "#,
//!     }
//! }
//!
//! ShellScript::new(&START_ADO_PROXY)
//!     .bind("PROXY_IMAGE", Binding::text(ADO_PROXY_IMAGE))
//!     .bind("PROXY_CONTAINER", Binding::text(ADO_PROXY_CONTAINER_NAME))
//!     .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
//!     .fragment("resolve_org", common::resolve_ado_organization_bash())
//!     .into_step("Start ado-proxy policy engine")
//! ```
//!
//! [`ShellScript::render`] emits a generated prelude of quoted assignments,
//! then the body with each fragment spliced at its marker.
//!
//! # The properties that matter
//!
//! * **One injection position.** A caller-supplied value can only be the
//!   right-hand side of a prelude assignment. It can never land mid-command,
//!   so it can never alter the structure of the script. See [`bindings`].
//! * **Declared surface.** Every variable the body reads is declared as either
//!   a `binding` (the compiler supplies it) or an `external` (the runtime
//!   does: `env:`, a fragment, or an ADO `setvariable`). [`ShellScript::render`]
//!   refuses to render if the bound set does not match the declared set, so a
//!   forgotten binding fails loudly rather than emitting `$UNSET`.
//! * **Composable without becoming opaque.** A long script is assembled from
//!   independently registered, independently shellchecked phases spliced at
//!   `# ado-aw:fragment` markers. The markers are ordinary comments, so the
//!   outline body remains valid shell, and the inter-phase variable contract
//!   is forced into the `externals:` declaration where a reviewer can see it.
//! * **Static enumerability.** Registration is automatic via `inventory`, so
//!   `ado-aw export-bash-scripts` and the shellcheck harness can reach *every*
//!   script without a fixture having to exercise the generator first. Before
//!   this, a generator no fixture reached was linted by nothing.
//! * **Single-hop editing.** The shell stays in the file that produces it:
//!   grep the shell text, land on the producer, edit in place.
//!
//! # Secrets
//!
//! A credential must never appear in the prelude — the prelude is written into
//! the `*.lock.yml` committed to the repository. [`Binding`] rejects values
//! that name a known credential; credentials continue to arrive through
//! `env:` as `EnvValue::secret`, which Azure DevOps masks.

pub mod bindings;
pub mod export;
pub mod registry;

#[cfg(test)]
mod lint;

use indexmap::IndexMap;

use super::ir::step::BashStep;

#[allow(unused_imports)]
pub use bindings::{Binding, BindingKind};
#[allow(unused_imports)]
pub use registry::{Interpreter, ShellScriptDef, all_scripts};

/// Opening marker of the generated prelude.
const PRELUDE_OPEN: &str = "# --- ado-aw generated bindings (do not edit) ---";
/// Closing marker of the generated prelude.
const PRELUDE_CLOSE: &str = "# --- end generated bindings ---";

/// A registered script plus the values bound to its declared variables.
#[derive(Debug, Clone)]
pub struct ShellScript {
    def: &'static ShellScriptDef,
    bindings: IndexMap<&'static str, Binding>,
    fragments: IndexMap<&'static str, String>,
}

impl ShellScript {
    /// Begin binding values to a registered script.
    pub fn new(def: &'static ShellScriptDef) -> Self {
        Self {
            def,
            bindings: IndexMap::new(),
            fragments: IndexMap::new(),
        }
    }

    /// Bind a declared variable.
    ///
    /// # Panics
    ///
    /// If `name` is not declared in the script's `bindings:` list. A typo
    /// would otherwise emit an assignment the body never reads while leaving
    /// the variable the body *does* read unset.
    #[track_caller]
    pub fn bind(mut self, name: &str, value: Binding) -> Self {
        let declared = self
            .def
            .bindings
            .iter()
            .find(|d| **d == name)
            .unwrap_or_else(|| {
                panic!(
                    "{}: `{name}` is not a declared binding; declared: {:?}",
                    self.def.name, self.def.bindings
                )
            });
        self.bindings.insert(*declared, value);
        self
    }

    /// Bind a declared variable to a literal string. Shorthand for
    /// `bind(name, Binding::text(value))`, which is the common case.
    #[track_caller]
    pub fn text(self, name: &str, value: impl AsRef<str>) -> Self {
        self.bind(name, Binding::text(value))
    }

    /// Splice a declared fragment — a block of shell produced elsewhere —
    /// into the body at its marker.
    ///
    /// The body marks the splice point with a comment line:
    ///
    /// ```sh
    /// # ado-aw:fragment resolve_org
    /// ```
    ///
    /// A marker is an ordinary shell comment, so the body stays valid,
    /// shellcheck-able shell whether or not the fragment is spliced — and the
    /// splice point is visible in the source rather than implied by call
    /// order.
    ///
    /// This is the composition escape hatch, and the only way arbitrary shell
    /// text (rather than a quoted value) enters a script. It is deliberately
    /// awkward: a fragment must be declared in the script's `fragments:` list
    /// *and* marked in the body, and any variable it defines must be declared
    /// in `externals:` so the shellcheck harness still sees a complete
    /// variable surface.
    #[track_caller]
    pub fn fragment(mut self, name: &str, shell: impl Into<String>) -> Self {
        let declared = self
            .def
            .fragments
            .iter()
            .find(|d| **d == name)
            .unwrap_or_else(|| {
                panic!(
                    "{}: `{name}` is not a declared fragment; declared: {:?}",
                    self.def.name, self.def.fragments
                )
            });
        self.fragments.insert(*declared, shell.into());
        self
    }

    /// Render the complete script: shebang (if the body carries one), the
    /// generated binding prelude, then the body with fragments spliced at
    /// their markers.
    ///
    /// # Panics
    ///
    /// If any declared binding or fragment was not supplied. This is a
    /// compiler bug, not a user error, and emitting a script with an unset
    /// variable would fail far away from its cause.
    #[track_caller]
    pub fn render(&self) -> String {
        let missing: Vec<&str> = self
            .def
            .bindings
            .iter()
            .copied()
            .filter(|name| !self.bindings.contains_key(name))
            .collect();
        assert!(
            missing.is_empty(),
            "{}: declared bindings were never bound: {missing:?}",
            self.def.name
        );
        let missing: Vec<&str> = self
            .def
            .fragments
            .iter()
            .copied()
            .filter(|name| !self.fragments.contains_key(name))
            .collect();
        assert!(
            missing.is_empty(),
            "{}: declared fragments were never supplied: {missing:?}",
            self.def.name
        );

        let (shebang, body) = split_shebang(self.def.body);
        let mut out = String::with_capacity(self.def.body.len() + 256);
        if let Some(shebang) = shebang {
            out.push_str(shebang);
            out.push('\n');
        }

        if !self.def.bindings.is_empty() {
            out.push_str(PRELUDE_OPEN);
            out.push('\n');
            // Declared order, not insertion order: the prelude reads the same
            // whatever order the producer happened to call `bind` in, so a
            // reordered call site produces no diff.
            for name in self.def.bindings {
                let rhs = self.bindings[name].rhs();
                if rhs.contains('$') {
                    // Single-quoting a `$` is the point, not a mistake. An
                    // `$(Agent.TempDirectory)` is substituted by Azure DevOps
                    // *before* bash sees the script, and the quotes then keep
                    // the substituted text literal so a path containing a
                    // space or a metacharacter cannot alter the script.
                    // Expanding it in the shell instead is exactly the bug
                    // this design prevents.
                    out.push_str("# shellcheck disable=SC2016\n");
                }
                out.push_str(name);
                out.push('=');
                out.push_str(rhs);
                out.push('\n');
            }
            out.push_str(PRELUDE_CLOSE);
            out.push('\n');
        }

        let body = dedent(body.trim_start_matches('\n'));
        out.push_str(&splice_fragments(self.def, &body, |name| {
            Some(self.fragments[name].as_str())
        }));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Render into an ADO bash step.
    ///
    /// # Panics
    ///
    /// If the script's interpreter is not [`Interpreter::Bash`] — a `sh`
    /// script is a standalone artefact (the `az` wrapper) written to disk by
    /// some *other* step, not a step in its own right.
    #[track_caller]
    pub fn into_step(self, display_name: impl Into<String>) -> BashStep {
        assert_eq!(
            self.def.interpreter,
            Interpreter::Bash,
            "{}: only a bash script can become an ADO bash step",
            self.def.name
        );
        BashStep::new(display_name, self.render())
    }

    /// The registration this script was built from.
    #[allow(dead_code)]
    pub fn def(&self) -> &'static ShellScriptDef {
        self.def
    }

    /// The binding a producer supplied for `name`.
    ///
    /// Lets a test assert on producer intent — `binding("PROXY_CONTAINER")` is
    /// the value the compiler supplied — rather than on a substring of the
    /// rendered script, which would also match a comment.
    #[allow(dead_code)]
    pub fn binding(&self, name: &str) -> Option<&Binding> {
        self.bindings.get(name)
    }
}

/// Split a leading `#!` line off a script body.
fn split_shebang(body: &str) -> (Option<&str>, &str) {
    let trimmed = body.trim_start_matches('\n');
    if !trimmed.starts_with("#!") {
        return (None, body);
    }
    match trimmed.split_once('\n') {
        Some((shebang, rest)) => (Some(shebang.trim_end()), rest),
        None => (Some(trimmed.trim_end()), ""),
    }
}

/// The comment that marks a fragment splice point inside a body.
pub(crate) const FRAGMENT_MARKER: &str = "# ado-aw:fragment ";

/// The fragment named by `line`, if it is a marker line.
///
/// `splice_fragments` inlines its own scan, so only the registry-wide lint
/// calls this today — dead in a non-test build.
#[allow(dead_code)]
pub(crate) fn fragment_marker(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix(FRAGMENT_MARKER)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// Replace each fragment marker in `body` with the text `resolve` returns,
/// re-indented to the marker's own indentation. A marker whose fragment
/// `resolve` does not supply is left in place as the comment it already is.
///
/// # Panics
///
/// If the body marks a fragment the definition does not declare, or declares
/// one the body never marks. Either is a silent no-op otherwise: shell that
/// was meant to run simply would not.
#[track_caller]
fn splice_fragments<'a>(
    def: &ShellScriptDef,
    body: &str,
    resolve: impl Fn(&str) -> Option<&'a str>,
) -> String {
    let mut seen: Vec<&'static str> = Vec::new();
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        match fragment_marker(line) {
            None => {
                out.push_str(line);
                out.push('\n');
            }
            Some(name) => {
                let declared = def.fragments.iter().find(|d| **d == name).unwrap_or_else(|| {
                    panic!(
                        "{}: body marks fragment `{name}`, which is not declared; declared: {:?}",
                        def.name, def.fragments
                    )
                });
                assert!(
                    !seen.contains(declared),
                    "{}: fragment `{name}` is marked more than once",
                    def.name
                );
                seen.push(declared);
                let Some(shell) = resolve(name) else {
                    // Not resolvable in this pass (a runtime-supplied fragment
                    // during linting). Keep the marker: it is a comment, so it
                    // is inert, and it shows a reader where the splice happens.
                    out.push_str(line);
                    out.push('\n');
                    continue;
                };
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                for fragment_line in dedent(shell.trim_matches('\n')).lines() {
                    if fragment_line.is_empty() {
                        out.push('\n');
                    } else {
                        out.push_str(&indent);
                        out.push_str(fragment_line);
                        out.push('\n');
                    }
                }
            }
        }
    }
    let unmarked: Vec<&&str> = def
        .fragments
        .iter()
        .filter(|name| !seen.contains(name))
        .collect();
    assert!(
        unmarked.is_empty(),
        "{}: declared fragments have no `{FRAGMENT_MARKER}<name>` marker in the body: {unmarked:?}",
        def.name
    );
    out
}

/// Strip the common leading indentation from every non-empty line, and
/// trailing whitespace from every line.
///
/// Raw-string bodies are normally written at column 0, in which case this is a
/// no-op. It exists for the ones that read better indented inside the
/// producing function, and because trailing whitespace makes `serde_yaml` fall
/// back to the double-quoted scalar form, which would make the emitted YAML
/// unreadable.
pub(crate) fn dedent(s: &str) -> String {
    let min = s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for line in s.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        let strip = leading_spaces.min(min);
        out.push_str(line[strip..].trim_end_matches([' ', '\t']));
    }
    if s.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_script;

    shell_script! {
        /// Fixture: exercises bindings, externals and a fragment.
        TEST_SCRIPT {
            interpreter: Bash,
            bindings: [CONTAINER, PORT],
            externals: [FROM_ENV, ORG],
            fragments: [resolve_org],
            body: r#"
set -euo pipefail
# ado-aw:fragment resolve_org
docker run --name "$CONTAINER" -p "$PORT" >/dev/null
echo "$ORG $FROM_ENV"
"#,
        }
    }

    shell_script! {
        /// Fixture: a bare script with no substitution surface at all.
        TEST_BARE {
            interpreter: Bash,
            bindings: [],
            externals: [],
            fragments: [],
            body: r#"
echo hello
"#,
        }
    }

    /// A definition that is deliberately **not** registered.
    ///
    /// Negative-case fixtures below are malformed on purpose. Registering them
    /// would make the registry-wide guards (shellcheck, fragment-marker drift)
    /// fail on fixtures rather than on real scripts.
    const fn unregistered(
        name: &'static str,
        interpreter: Interpreter,
        externals: &'static [&'static str],
        fragments: &'static [&'static str],
        body: &'static str,
    ) -> ShellScriptDef {
        ShellScriptDef {
            name,
            interpreter,
            bindings: &[],
            externals,
            fragments,
            phases: &[],
            body,
            file: file!(),
            line: line!(),
        }
    }

    fn built() -> ShellScript {
        ShellScript::new(&TEST_SCRIPT)
            .text("CONTAINER", "awmg-ado-proxy")
            .bind("PORT", Binding::number(11080))
            .fragment("resolve_org", "ORG=example")
    }

    #[test]
    fn renders_a_prelude_then_the_body_with_fragments_in_place() {
        let rendered = built().render();
        let expected = concat!(
            "# --- ado-aw generated bindings (do not edit) ---\n",
            "CONTAINER='awmg-ado-proxy'\n",
            "PORT=11080\n",
            "# --- end generated bindings ---\n",
            "set -euo pipefail\n",
            "ORG=example\n",
            "docker run --name \"$CONTAINER\" -p \"$PORT\" >/dev/null\n",
            "echo \"$ORG $FROM_ENV\"\n",
        );
        assert_eq!(rendered, expected);
    }

    #[test]
    fn a_fragment_is_reindented_to_its_marker() {
        // Not registered: the outline is only valid shell *after* splicing, so
        // linting it standalone would report a spurious empty `then` clause.
        // This is exactly why a fragment must not carry a control-flow body in
        // a real script.
        static NESTED: ShellScriptDef = unregistered(
            "test::NESTED",
            Interpreter::Bash,
            &["ORG"],
            &["resolve_org"],
            r#"
if true; then
  # ado-aw:fragment resolve_org
fi
"#,
        );
        let rendered = ShellScript::new(&NESTED)
            .fragment("resolve_org", "ORG=example\necho \"$ORG\"")
            .render();
        assert_eq!(
            rendered,
            "if true; then\n  ORG=example\n  echo \"$ORG\"\nfi\n"
        );
    }

    #[test]
    #[should_panic(expected = "have no `# ado-aw:fragment <name>` marker in the body")]
    fn refuses_a_declared_fragment_the_body_never_marks() {
        static UNMARKED: ShellScriptDef = unregistered(
            "test::UNMARKED",
            Interpreter::Bash,
            &[],
            &["orphan"],
            "\necho hi\n",
        );
        ShellScript::new(&UNMARKED)
            .fragment("orphan", "echo spliced")
            .render();
    }

    #[test]
    #[should_panic(expected = "which is not declared")]
    fn refuses_a_marker_the_definition_never_declares() {
        static STRAY_MARKER: ShellScriptDef = unregistered(
            "test::STRAY_MARKER",
            Interpreter::Bash,
            &[],
            &[],
            "\n# ado-aw:fragment ghost\n",
        );
        ShellScript::new(&STRAY_MARKER).render();
    }

    #[test]
    fn the_body_needs_no_escaping() {
        // The whole point: a Go template survives verbatim. Under `format!`
        // this had to be written `{{{{.State.Status}}}}`.
        shell_script! {
            GO_TEMPLATE {
                interpreter: Bash,
                bindings: [],
                externals: [],
                fragments: [],
                body: r#"
docker inspect -f 'state={{.State.Status}} exit={{.State.ExitCode}}' proxy
"#,
            }
        }
        assert!(
            ShellScript::new(&GO_TEMPLATE)
                .render()
                .contains("'state={{.State.Status}} exit={{.State.ExitCode}}'")
        );
    }

    #[test]
    fn prelude_order_follows_the_declaration_not_the_call_site() {
        // Reordering `bind` calls must not produce a diff in generated YAML.
        let reordered = ShellScript::new(&TEST_SCRIPT)
            .bind("PORT", Binding::number(11080))
            .text("CONTAINER", "awmg-ado-proxy")
            .fragment("resolve_org", "ORG=example");
        assert_eq!(reordered.render(), built().render());
    }

    #[test]
    fn a_script_with_no_substitutions_gets_no_prelude() {
        let rendered = ShellScript::new(&TEST_BARE).render();
        assert_eq!(rendered, "echo hello\n");
        assert!(!rendered.contains(PRELUDE_OPEN));
    }

    #[test]
    #[should_panic(expected = "declared bindings were never bound: [\"PORT\"]")]
    fn refuses_to_render_with_a_binding_missing() {
        ShellScript::new(&TEST_SCRIPT)
            .text("CONTAINER", "c")
            .fragment("resolve_org", "ORG=example")
            .render();
    }

    #[test]
    #[should_panic(expected = "declared fragments were never supplied")]
    fn refuses_to_render_with_a_fragment_missing() {
        ShellScript::new(&TEST_SCRIPT)
            .text("CONTAINER", "c")
            .bind("PORT", Binding::number(1))
            .render();
    }

    #[test]
    #[should_panic(expected = "is not a declared binding")]
    fn refuses_an_undeclared_binding() {
        ShellScript::new(&TEST_SCRIPT).text("CONTAINR", "typo");
    }

    #[test]
    fn an_ado_macro_binding_carries_a_shellcheck_directive() {
        // SC2016 ("expressions don't expand in single quotes") is exactly the
        // behaviour an ADO macro binding wants: the macro is substituted
        // before bash runs, and the quotes keep the substituted text literal.
        // Without the directive every prelude would fail the bash lint.
        shell_script! {
            MACRO_BINDING {
                interpreter: Bash,
                bindings: [AGENT_TEMP, PLAIN],
                externals: [],
                fragments: [],
                body: r#"
echo "$AGENT_TEMP $PLAIN"
"#,
            }
        }
        let rendered = ShellScript::new(&MACRO_BINDING)
            .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
            .text("PLAIN", "no-dollar-here")
            .render();
        assert!(
            rendered.contains(
                "# shellcheck disable=SC2016\nAGENT_TEMP='$(Agent.TempDirectory)'"
            ),
            "an ADO macro binding needs the directive: {rendered}"
        );
        // A value with no `$` gets no directive — the suppression is targeted,
        // not blanket.
        assert!(
            rendered.contains("\nPLAIN='no-dollar-here'"),
            "{rendered}"
        );
        assert_eq!(
            rendered.matches("shellcheck disable=SC2016").count(),
            1,
            "only the binding that needs it should carry the directive: {rendered}"
        );
    }

    #[test]
    fn a_shebang_stays_first_and_the_prelude_follows_it() {
        shell_script! {
            WRAPPER {
                interpreter: Sh,
                bindings: [TARGET],
                externals: [],
                fragments: [],
                body: r#"
#!/bin/sh
exec "$TARGET" "$@"
"#,
            }
        }
        let rendered = ShellScript::new(&WRAPPER).text("TARGET", "/usr/bin/az").render();
        assert_eq!(
            rendered,
            concat!(
                "#!/bin/sh\n",
                "# --- ado-aw generated bindings (do not edit) ---\n",
                "TARGET='/usr/bin/az'\n",
                "# --- end generated bindings ---\n",
                "exec \"$TARGET\" \"$@\"\n",
            )
        );
    }

    #[test]
    #[should_panic(expected = "only a bash script can become an ADO bash step")]
    fn a_sh_script_is_not_a_step() {
        shell_script! {
            SH_ONLY {
                interpreter: Sh,
                bindings: [],
                externals: [],
                fragments: [],
                body: r#"
echo hi
"#,
            }
        }
        let _ = ShellScript::new(&SH_ONLY).into_step("nope");
    }

    #[test]
    fn into_step_carries_the_rendered_script() {
        let step = built().into_step("Start ado-proxy policy engine");
        assert_eq!(step.display_name, "Start ado-proxy policy engine");
        assert_eq!(step.script, built().render());
    }

    #[test]
    fn tests_can_assert_on_a_binding_rather_than_a_substring() {
        // Stronger than `script.contains("awmg-ado-proxy")`, which would also
        // pass if the value appeared in a comment.
        let script = built();
        assert_eq!(script.binding("CONTAINER").unwrap().rhs(), "'awmg-ado-proxy'");
        assert_eq!(script.binding("PORT").unwrap().kind(), BindingKind::Number);
        assert!(script.binding("NOPE").is_none());
    }

    #[test]
    fn dedent_strips_source_indentation_and_trailing_space() {
        assert_eq!(dedent("    a\n      b\n"), "a\n  b\n");
        assert_eq!(dedent("a   \nb\t\n"), "a\nb\n");
    }
}
