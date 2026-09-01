//! Typed, validated substitution values for [`super::ShellScript`].
//!
//! # Why a binding rather than an interpolation
//!
//! The generators this module replaces built shell with `format!`, which meant
//! a substituted value could land in *any* syntactic position — inside a
//! single-quoted string, inside a `docker run` argument list, inside a `sed`
//! expression. Whether that value could alter the structure of the script
//! depended on the position, and the position was invisible from the call
//! site.
//!
//! A [`Binding`] can only ever be emitted as the right-hand side of a shell
//! assignment in the generated prelude. That is the single position where a
//! value's own quoting fully determines its meaning, so the escaping is
//! decided once, here, rather than per call site.
//!
//! Each constructor validates the shape it accepts. The types are deliberately
//! narrow: [`Binding::words`] refuses a value containing whitespace because a
//! word list is expanded unquoted by its consumer, and [`Binding::ado_macro`]
//! accepts only a well-formed predefined-variable name. A caller that needs
//! something outside these shapes has to add a constructor and justify it,
//! which is the point.

/// A validated value bound to a shell variable in the generated prelude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The rendered right-hand side of the assignment, already quoted for
    /// the shell as the constructor determined appropriate.
    rhs: String,
    kind: BindingKind,
}

/// How a [`Binding`]'s value was validated. Retained for diagnostics and so
/// tests can assert on intent rather than on rendered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// An arbitrary literal string, single-quoted.
    Text,
    /// An integer, emitted bare.
    Number,
    /// `true` or `false`, emitted bare.
    ///
    /// Retained even while no current script needs it: [`Binding`] is a closed
    /// vocabulary, and without this a future author would reach for
    /// `text("true")` — the untyped fallback the whole design exists to avoid.
    #[allow(dead_code)]
    Bool,
    /// A whitespace-separated word list, single-quoted, intended for
    /// unquoted `for` expansion.
    Words,
    /// An Azure DevOps macro such as `$(Agent.TempDirectory)`, single-quoted
    /// so the shell treats the already-substituted text as a literal.
    AdoMacro,
    /// Bulk text carried in a quoted heredoc.
    Document,
}

/// Variable names that carry a credential. A credential must reach a step
/// through `env:` (as `EnvValue::secret`) so Azure DevOps can mask it in logs;
/// routing one through the prelude would print it verbatim into the generated
/// YAML committed to the repository.
const SECRET_NAMES: &[&str] = &[
    "SC_READ_TOKEN",
    "SC_WRITE_TOKEN",
    "System.AccessToken",
    "SYSTEM_ACCESSTOKEN",
    "GITHUB_TOKEN",
    "AZURE_DEVOPS_EXT_PAT",
    "ADO_PROXY_BEARER",
];

impl Binding {
    /// An arbitrary literal string.
    ///
    /// Rejects newlines (which would break the one-assignment-per-line prelude
    /// shape) and `$(` (which would smuggle either a command substitution or
    /// an unreviewed Azure DevOps macro past the typed channel — use
    /// [`Binding::ado_macro`] for the latter).
    #[track_caller]
    pub fn text(value: impl AsRef<str>) -> Self {
        let value = value.as_ref();
        assert!(
            !value.contains('\n') && !value.contains('\r'),
            "shell binding value must be a single line, got {value:?}"
        );
        assert!(
            !value.contains("$("),
            "shell binding value must not contain `$(`; use Binding::ado_macro \
             for an Azure DevOps predefined variable, got {value:?}"
        );
        assert_not_secret(value);
        Self {
            rhs: single_quote(value),
            kind: BindingKind::Text,
        }
    }

    /// An unsigned integer, emitted bare so arithmetic contexts work.
    pub fn number(value: u64) -> Self {
        Self {
            rhs: value.to_string(),
            kind: BindingKind::Number,
        }
    }

    /// A boolean, emitted bare as `true` / `false` so `[ "$V" = true ]` reads
    /// naturally.
    ///
    /// See [`BindingKind::Bool`] for why this exists ahead of a caller.
    #[allow(dead_code)]
    pub fn boolean(value: bool) -> Self {
        Self {
            rhs: if value { "true" } else { "false" }.to_string(),
            kind: BindingKind::Bool,
        }
    }

    /// A whitespace-separated word list, for bodies that iterate it with an
    /// intentionally unquoted `for W in $LIST`.
    ///
    /// Each word is rejected if it contains whitespace or a glob metacharacter,
    /// because the consumer's word splitting would otherwise silently produce a
    /// different list than the caller wrote.
    #[track_caller]
    pub fn words<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> Self {
        let mut joined = String::new();
        for value in values {
            let word = value.as_ref();
            assert!(!word.is_empty(), "shell word-list entry must not be empty");
            assert!(
                !word.contains(|c: char| c.is_whitespace()),
                "shell word-list entry must not contain whitespace, got {word:?}"
            );
            assert!(
                !word.contains(['*', '?', '[', ']', '\'', '\\', '$', '`']),
                "shell word-list entry must not contain a glob or quoting \
                 metacharacter, got {word:?}"
            );
            assert_not_secret(word);
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(word);
        }
        Self {
            rhs: single_quote(&joined),
            kind: BindingKind::Words,
        }
    }

    /// An Azure DevOps predefined variable, e.g. `Agent.TempDirectory`.
    ///
    /// Rendered as `'$(Agent.TempDirectory)'`. Azure DevOps substitutes the
    /// macro before the shell ever sees the script, and the single quotes then
    /// keep the substituted text literal — so a path containing a space or a
    /// shell metacharacter cannot alter the script.
    #[track_caller]
    pub fn ado_macro(name: &str) -> Self {
        assert!(
            is_ado_macro_name(name),
            "Azure DevOps macro name must be dotted alphanumeric, got {name:?}"
        );
        assert_not_secret(name);
        Self {
            rhs: single_quote(&format!("$({name})")),
            kind: BindingKind::AdoMacro,
        }
    }

    /// A path that embeds one or more Azure DevOps predefined variables, e.g.
    /// `$(Pipeline.Workspace)/agentic-pipeline-compiler`.
    ///
    /// [`Binding::ado_macro`] takes a bare variable name; this takes a path
    /// built around one. Every `$(…)` occurrence is validated as a well-formed
    /// predefined-variable name, so the only thing the value can expand to is
    /// a variable Azure DevOps substitutes before bash runs — not a command
    /// substitution, and not a shell metacharacter. The result is
    /// single-quoted, so the substituted text stays literal.
    #[track_caller]
    pub fn ado_path(value: impl AsRef<str>) -> Self {
        let value = value.as_ref();
        assert!(
            !value.contains('\n') && !value.contains('\r'),
            "shell binding value must be a single line, got {value:?}"
        );
        assert!(
            !value.contains('`') && !value.contains("${"),
            "an ADO path must not contain a backtick or `${{`, got {value:?}"
        );
        let mut rest = value;
        while let Some(open) = rest.find("$(") {
            let after = &rest[open + 2..];
            let close = after.find(')').unwrap_or_else(|| {
                panic!("unterminated `$(` in ADO path {value:?}")
            });
            let name = &after[..close];
            assert!(
                is_ado_macro_name(name),
                "ADO path {value:?} embeds {name:?}, which is not a dotted \
                 alphanumeric predefined-variable name; a command substitution \
                 is not permitted here"
            );
            rest = &after[close + 1..];
        }
        assert_not_secret(value);
        Self {
            rhs: single_quote(value),
            kind: BindingKind::AdoMacro,
        }
    }

    /// Bulk text — a JSON document, a prompt, a certificate — assigned through
    /// a quoted heredoc so no expansion occurs and no escaping is needed.
    ///
    /// Rendered across multiple lines, unlike every other binding.
    #[track_caller]
    pub fn document(value: impl AsRef<str>) -> Self {
        let value = value.as_ref();
        assert!(
            !value.lines().any(|line| line.trim() == DOCUMENT_DELIMITER),
            "document binding must not contain the heredoc delimiter {DOCUMENT_DELIMITER}"
        );
        assert_not_secret(value);
        let trimmed = value.trim_end_matches('\n');
        Self {
            rhs: format!("$(cat <<'{DOCUMENT_DELIMITER}'\n{trimmed}\n{DOCUMENT_DELIMITER}\n)"),
            kind: BindingKind::Document,
        }
    }

    /// The rendered right-hand side of the assignment.
    pub fn rhs(&self) -> &str {
        &self.rhs
    }

    /// How this binding was validated.
    ///
    /// Used by tests asserting on producer intent rather than rendered text.
    #[allow(dead_code)]
    pub fn kind(&self) -> BindingKind {
        self.kind
    }
}

/// Heredoc delimiter for [`Binding::document`]. Long and namespaced so it
/// cannot collide with real content by accident.
const DOCUMENT_DELIMITER: &str = "ADO_AW_SHELL_DOC_EOF";

/// Refuse a value that names a credential. See [`SECRET_NAMES`].
#[track_caller]
fn assert_not_secret(value: &str) {
    for secret in SECRET_NAMES {
        assert!(
            !value.contains(secret),
            "a credential must not reach the generated prelude: {value:?} \
             mentions {secret}. Pass it through `with_env` / `EnvValue::secret` \
             so Azure DevOps masks it."
        );
    }
}

/// POSIX single-quoting: the only escape available inside `'…'` is to close
/// the quote, emit an escaped `'`, and reopen. Every other byte — including
/// `$`, backtick, backslash and newline — is literal.
pub(crate) fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A shell variable name the prelude may assign.
///
/// Only the registry-wide lint calls this today, so it is dead in a non-test
/// build.
#[allow(dead_code)]
pub(crate) fn is_shell_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `Build.Repository.Name`-shaped predefined-variable names.
fn is_ado_macro_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_single_quoted_and_closes_embedded_quotes() {
        assert_eq!(Binding::text("plain").rhs(), "'plain'");
        // The classic escape: close, escaped quote, reopen.
        assert_eq!(Binding::text("it's").rhs(), r"'it'\''s'");
    }

    #[test]
    fn text_keeps_shell_metacharacters_literal() {
        // Nothing inside '…' expands, so a value that looks like code stays
        // data. This is the property that makes the prelude the only safe
        // injection position.
        let binding = Binding::text("rm -rf /; `id`; ${HOME}");
        assert_eq!(binding.rhs(), "'rm -rf /; `id`; ${HOME}'");
    }

    #[test]
    #[should_panic(expected = "must not contain `$(`")]
    fn text_refuses_an_untyped_command_substitution() {
        Binding::text("$(Agent.TempDirectory)/work");
    }

    #[test]
    #[should_panic(expected = "single line")]
    fn text_refuses_a_multi_line_value() {
        Binding::text("first\nsecond");
    }

    #[test]
    #[should_panic(expected = "credential must not reach the generated prelude")]
    fn text_refuses_a_credential() {
        Binding::text("token=SC_READ_TOKEN");
    }

    #[test]
    fn numbers_and_booleans_are_emitted_bare() {
        assert_eq!(Binding::number(11080).rhs(), "11080");
        assert_eq!(Binding::boolean(true).rhs(), "true");
        assert_eq!(Binding::boolean(false).rhs(), "false");
    }

    #[test]
    fn words_join_with_a_single_space() {
        let binding = Binding::words(["dev.azure.com", "vssps.dev.azure.com"]);
        assert_eq!(binding.rhs(), "'dev.azure.com vssps.dev.azure.com'");
        assert_eq!(binding.kind(), BindingKind::Words);
    }

    #[test]
    fn words_are_empty_when_the_list_is() {
        assert_eq!(Binding::words(Vec::<String>::new()).rhs(), "''");
    }

    #[test]
    #[should_panic(expected = "must not contain whitespace")]
    fn words_refuse_an_entry_that_would_split() {
        // The consumer expands this unquoted; a space would silently produce
        // two list entries where the caller wrote one.
        Binding::words(["dev.azure.com", "two words"]);
    }

    #[test]
    #[should_panic(expected = "glob or quoting metacharacter")]
    fn words_refuse_a_glob() {
        Binding::words(["*.azure.com"]);
    }

    #[test]
    fn ado_macro_is_quoted_so_the_substituted_text_stays_literal() {
        let binding = Binding::ado_macro("Agent.TempDirectory");
        assert_eq!(binding.rhs(), "'$(Agent.TempDirectory)'");
        assert_eq!(binding.kind(), BindingKind::AdoMacro);
    }

    #[test]
    #[should_panic(expected = "dotted alphanumeric")]
    fn ado_macro_refuses_an_arbitrary_expression() {
        Binding::ado_macro("Agent.TempDirectory)/x; rm -rf /; echo $(");
    }

    #[test]
    #[should_panic(expected = "credential must not reach the generated prelude")]
    fn ado_macro_refuses_the_access_token() {
        Binding::ado_macro("System.AccessToken");
    }

    #[test]
    fn ado_path_accepts_a_macro_with_a_compiler_owned_suffix() {
        let binding = Binding::ado_path("$(Pipeline.Workspace)/compiler/_pkg");
        assert_eq!(binding.rhs(), "'$(Pipeline.Workspace)/compiler/_pkg'");
        assert_eq!(binding.kind(), BindingKind::AdoMacro);
        // A plain path with no macro is fine too.
        assert_eq!(Binding::ado_path("/tmp/scripts").rhs(), "'/tmp/scripts'");
    }

    #[test]
    #[should_panic(expected = "is not a dotted alphanumeric predefined-variable name")]
    fn ado_path_refuses_a_command_substitution() {
        // This is the whole point: `$(…)` in a path must be an ADO variable
        // Azure DevOps substitutes, never a shell command the runner executes.
        Binding::ado_path("/tmp/$(rm -rf /)/x");
    }

    #[test]
    #[should_panic(expected = "unterminated")]
    fn ado_path_refuses_an_unterminated_macro() {
        Binding::ado_path("/tmp/$(Pipeline.Workspace/x");
    }

    #[test]
    #[should_panic(expected = "backtick")]
    fn ado_path_refuses_a_backtick() {
        Binding::ado_path("/tmp/`id`");
    }

    #[test]
    fn document_uses_a_quoted_heredoc_so_nothing_expands() {
        let binding = Binding::document("{\"a\": \"$NOT_EXPANDED\"}\n");
        assert_eq!(
            binding.rhs(),
            "$(cat <<'ADO_AW_SHELL_DOC_EOF'\n{\"a\": \"$NOT_EXPANDED\"}\nADO_AW_SHELL_DOC_EOF\n)"
        );
    }

    #[test]
    #[should_panic(expected = "heredoc delimiter")]
    fn document_refuses_content_that_would_close_the_heredoc() {
        Binding::document("ok\nADO_AW_SHELL_DOC_EOF\nsmuggled");
    }

    #[test]
    fn shell_var_names_are_screaming_snake_case() {
        assert!(is_shell_var_name("PROXY_DIR"));
        assert!(is_shell_var_name("_PRIVATE"));
        assert!(is_shell_var_name("PORT2"));
        assert!(!is_shell_var_name("proxy_dir"));
        assert!(!is_shell_var_name("2PORT"));
        assert!(!is_shell_var_name("PROXY-DIR"));
        assert!(!is_shell_var_name(""));
    }
}
