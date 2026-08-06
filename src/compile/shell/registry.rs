//! Static registration of every shell script the compiler can emit.
//!
//! # Why a registry
//!
//! The bash lint used to work by compiling a set of fixtures and walking the
//! emitted YAML for `bash:` bodies. That makes lint coverage a function of
//! *reachability*: a generator no fixture happens to exercise is linted by
//! nothing. The `ado-proxy` lifecycle steps and the `az` wrapper were in
//! exactly that position — several hundred lines of unlinted shell.
//!
//! Registration inverts it. Every script announces itself at link time via
//! `inventory`, so [`all_scripts`] enumerates the complete set without
//! compiling anything. `ado-aw export-bash-scripts` and the shellcheck
//! harness both read from here, which makes coverage total by construction
//! rather than by a hand-maintained list.
//!
//! There is deliberately no manual catalogue to keep in sync: the
//! [`shell_script!`](crate::shell_script) macro registers as a side effect of
//! declaring, so the two cannot drift.

/// Which shell a script is written for.
///
/// This is not cosmetic: it selects `shellcheck --shell`, and the two dialects
/// genuinely differ (`set -o pipefail`, `[[`, arrays and `local` are bash-only).
/// The `az` wrapper is `sh` because it runs on whatever image the agent pool
/// provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpreter {
    Bash,
    Sh,
}

impl Interpreter {
    /// The `--shell=` value shellcheck expects.
    pub fn shellcheck_dialect(self) -> &'static str {
        match self {
            Interpreter::Bash => "bash",
            Interpreter::Sh => "sh",
        }
    }
}

/// A registered script: its verbatim body plus the surface it declares.
///
/// Construct through [`shell_script!`](crate::shell_script) rather than
/// directly, so registration cannot be forgotten.
#[derive(Debug, Clone, Copy)]
pub struct ShellScriptDef {
    /// Fully qualified name, `module::path::IDENT`. Unique by construction,
    /// and used as the export filename.
    pub name: &'static str,
    pub interpreter: Interpreter,
    /// Variables the **compiler** supplies through the generated prelude.
    /// Every one must be bound before [`super::ShellScript::render`] will
    /// produce a script.
    pub bindings: &'static [&'static str],
    /// Variables the **runtime** supplies: step `env:`, an ADO
    /// `##vso[task.setvariable]` from an earlier step, or a spliced fragment.
    /// Declaring them is what lets the shellcheck harness distinguish "arrives
    /// from outside" from "genuinely never assigned" (SC2154).
    pub externals: &'static [&'static str],
    /// Named blocks of shell composed in from elsewhere. The composition
    /// escape hatch — see [`super::ShellScript::fragment`].
    pub fragments: &'static [&'static str],
    /// The script itself, verbatim, exactly as it will run.
    pub body: &'static str,
    /// Source file, for the export provenance header.
    pub file: &'static str,
    /// Source line, for the export provenance header.
    pub line: u32,
}

impl ShellScriptDef {
    /// The script as shellcheck should see it in isolation.
    ///
    /// Declared bindings and externals are stub-assigned so SC2154
    /// ("referenced but not assigned") still fires for a variable the body
    /// reads without declaring — which is the bug worth catching — while not
    /// firing for every legitimately-injected value.
    ///
    /// Fragment markers are left in place: they are ordinary comments, and the
    /// fragment's own shell is linted where it is produced. Splicing an
    /// unknown block in here would only produce noise.
    pub fn lint_source(&self) -> String {
        let (shebang, body) = super::split_shebang(self.body);
        let mut out = String::with_capacity(self.body.len() + 256);
        if let Some(shebang) = shebang {
            out.push_str(shebang);
            out.push('\n');
        }
        out.push_str("# --- ado-aw lint stubs (not emitted) ---\n");
        for name in self.bindings.iter().chain(self.externals.iter()) {
            // A non-empty stub: an empty one makes `[ -z "$V" ]` branches
            // unreachable to shellcheck's flow analysis.
            out.push_str(name);
            out.push_str("='ado-aw-lint-stub'\n");
        }
        out.push_str("# --- end lint stubs ---\n");
        out.push_str(&super::dedent(body.trim_start_matches('\n')));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Filename this script exports to, e.g. `compile__shell__START_PROXY.sh`.
    pub fn export_file_name(&self) -> String {
        format!("{}.sh", self.name.replace("::", "__"))
    }
}

inventory::collect!(ShellScriptDef);

/// Every registered script, in a stable order (sorted by [`ShellScriptDef::name`]).
///
/// `inventory` iteration order is link-order and therefore not stable across
/// builds; sorting keeps the export output and any lint report diffable.
pub fn all_scripts() -> Vec<&'static ShellScriptDef> {
    let mut scripts: Vec<&'static ShellScriptDef> =
        inventory::iter::<ShellScriptDef>.into_iter().collect();
    scripts.sort_by_key(|def| def.name);
    scripts
}

/// Declare and register a shell script.
///
/// The body is a raw string containing the shell **exactly as it will run** —
/// no `format!`, no `\n\` continuations, no escaping of quotes or `{{`.
/// Substitution happens only through the declared `bindings`, which
/// [`ShellScript`](super::ShellScript) renders into a quoted prelude.
///
/// ```ignore
/// shell_script! {
///     /// One line on why this script exists.
///     STOP_ADO_PROXY {
///         interpreter: Bash,
///         bindings: [PROXY_CONTAINER],
///         externals: [],
///         fragments: [],
///         body: r#"
/// docker rm -f "$PROXY_CONTAINER" 2>/dev/null || true
/// "#,
///     }
/// }
/// ```
#[macro_export]
macro_rules! shell_script {
    (
        $(#[$meta:meta])*
        $ident:ident {
            interpreter: $interpreter:ident,
            bindings: [$($binding:ident),* $(,)?],
            externals: [$($external:ident),* $(,)?],
            fragments: [$($fragment:ident),* $(,)?],
            body: $body:expr $(,)?
        }
    ) => {
        $(#[$meta])*
        #[allow(dead_code)]
        pub const $ident: $crate::compile::shell::ShellScriptDef =
            $crate::compile::shell::ShellScriptDef {
                name: concat!(module_path!(), "::", stringify!($ident)),
                interpreter: $crate::compile::shell::Interpreter::$interpreter,
                bindings: &[$(stringify!($binding)),*],
                externals: &[$(stringify!($external)),*],
                fragments: &[$(stringify!($fragment)),*],
                body: $body,
                file: file!(),
                line: line!(),
            };

        $crate::inventory::submit! { $ident }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_script;

    shell_script! {
        /// Fixture for registry behaviour.
        REGISTRY_FIXTURE {
            interpreter: Sh,
            bindings: [TARGET],
            externals: [FROM_ENV, ORG],
            fragments: [resolve_org],
            body: r#"
#!/bin/sh
# ado-aw:fragment resolve_org
echo "$TARGET $FROM_ENV $ORG"
"#,
        }
    }

    #[test]
    fn a_declared_script_registers_itself() {
        // No manual catalogue: declaring is registering, so the two cannot
        // drift apart.
        let found = all_scripts()
            .into_iter()
            .find(|def| def.name.ends_with("::REGISTRY_FIXTURE"))
            .expect("the fixture must appear in the registry");
        assert_eq!(found.interpreter, Interpreter::Sh);
        assert_eq!(found.bindings, &["TARGET"]);
        assert_eq!(found.externals, &["FROM_ENV", "ORG"]);
        assert_eq!(found.fragments, &["resolve_org"]);
    }

    #[test]
    fn names_are_module_qualified_and_unique() {
        let mut names: Vec<&str> = all_scripts().iter().map(|def| def.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            total,
            "two scripts registered under the same name; \
             `ado-aw export-bash-scripts` would overwrite one with the other"
        );
    }

    #[test]
    fn the_registry_is_sorted_so_exports_stay_diffable() {
        let names: Vec<&str> = all_scripts().iter().map(|def| def.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn lint_source_stubs_every_declared_variable() {
        let source = REGISTRY_FIXTURE.lint_source();
        assert!(source.starts_with("#!/bin/sh\n"), "shebang stays first: {source}");
        assert!(source.contains("TARGET='ado-aw-lint-stub'"));
        // An external is stubbed too: it genuinely arrives from outside, so
        // SC2154 on it would be noise.
        assert!(source.contains("FROM_ENV='ado-aw-lint-stub'"));
        // `ORG` reaches the body from the fragment, and is declared external
        // for exactly that reason.
        assert!(source.contains("ORG='ado-aw-lint-stub'"));
        // A fragment marker stays in the body as an ordinary comment: the
        // fragment's own shell is linted where it is produced.
        assert!(source.contains("# ado-aw:fragment resolve_org"));
        // Nothing undeclared is invented.
        assert!(!source.contains("UNDECLARED='ado-aw-lint-stub'"));
        assert!(source.trim_end().ends_with("echo \"$TARGET $FROM_ENV $ORG\""));
    }

    #[test]
    fn export_file_names_are_path_safe() {
        assert!(REGISTRY_FIXTURE.export_file_name().ends_with("__REGISTRY_FIXTURE.sh"));
        assert!(!REGISTRY_FIXTURE.export_file_name().contains(':'));
    }

    #[test]
    fn interpreters_map_to_shellcheck_dialects() {
        assert_eq!(Interpreter::Bash.shellcheck_dialect(), "bash");
        assert_eq!(Interpreter::Sh.shellcheck_dialect(), "sh");
    }
}
