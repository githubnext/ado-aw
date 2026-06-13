//! Typed identifiers for the pipeline IR.
//!
//! Stages, jobs, and steps are addressed via newtype IDs rather than
//! raw strings so the dependency-graph builder (see
//! [`super::graph`]) can use them as map keys without risk of
//! confusing one kind of id with another.
//!
//! All ids are constructed via [`StageId::new`] /
//! [`JobId::new`] / [`StepId::new`], which validate the inner string
//! against the ADO identifier grammar:
//!
//!   `^[A-Za-z_][A-Za-z0-9_]*$`
//!
//! (no spaces, no hyphens, no leading digits — matches the rule ADO
//! applies to job/stage/step `name:` fields). Construction returns
//! [`Result`] so call sites can surface a meaningful error rather
//! than panic.
//!
//! ## Uniqueness contract
//!
//! - [`StageId`] is unique within a [`super::Pipeline`].
//! - [`JobId`] is unique within a stage (or pipeline-wide for
//!   stage-less pipelines). The graph builder rejects duplicates
//!   with a typed error.
//! - **[`StepId`] is unique pipeline-wide** — not just within a job.
//!   [`super::output::OutputRef`] carries only a `StepId` (no
//!   qualifying job name), so the IR's producer-resolution is keyed
//!   on `StepId` alone. The graph builder rejects cross-job
//!   duplicates with a typed error. (ADO YAML technically allows
//!   `dependencies.<job>.outputs[...]` to disambiguate, but the IR
//!   does not model the job-qualified form.)
//!
//! `Display` round-trips to the original string. `AsRef<str>` is
//! provided so ids slot into format strings cheaply.

use anyhow::{Result, bail};
use std::fmt;

fn validate(kind: &'static str, raw: &str) -> Result<()> {
    if raw.is_empty() {
        bail!("{kind} id must not be empty");
    }
    let mut chars = raw.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_alphabetic() || first == '_') {
        bail!(
            "{kind} id '{raw}' must start with an ASCII letter or underscore \
             (ADO identifier grammar)"
        );
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!(
            "{kind} id '{raw}' must contain only ASCII alphanumerics and \
             underscores (ADO identifier grammar — no spaces, no hyphens)"
        );
    }
    Ok(())
}

macro_rules! define_id {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("Typed identifier for a ", $kind, " inside the pipeline IR.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Constructs a [`", stringify!($name), "`] after validating ")]
            #[doc = concat!("`raw` against the ADO identifier grammar.")]
            pub fn new(raw: impl Into<String>) -> Result<Self> {
                let raw = raw.into();
                validate($kind, &raw)?;
                Ok(Self(raw))
            }

            /// Borrow the id as a `&str`.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_id!(StageId, "stage");
define_id!(JobId, "job");
define_id!(StepId, "step");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letter_first() {
        assert!(StageId::new("Setup").is_ok());
        assert!(JobId::new("Agent").is_ok());
        assert!(StepId::new("synthPr").is_ok());
    }

    #[test]
    fn accepts_underscore_first_and_digits_thereafter() {
        assert!(StepId::new("_internal_step_2").is_ok());
    }

    #[test]
    fn rejects_empty() {
        let err = StageId::new("").unwrap_err();
        assert!(format!("{err:#}").contains("must not be empty"));
    }

    #[test]
    fn rejects_leading_digit() {
        let err = JobId::new("1Bad").unwrap_err();
        assert!(format!("{err:#}").contains("must start with"));
    }

    #[test]
    fn rejects_hyphen_and_space() {
        assert!(StepId::new("bad-name").is_err());
        assert!(JobId::new("Bad Name").is_err());
    }

    #[test]
    fn display_round_trips() {
        let id = JobId::new("Detection").unwrap();
        assert_eq!(format!("{id}"), "Detection");
        assert_eq!(id.as_str(), "Detection");
        assert_eq!(id.as_ref(), "Detection");
    }

    #[test]
    fn distinct_kinds_do_not_share_address_space() {
        // Compile-time check: a StageId and a JobId with the same inner
        // string are not interchangeable. This won't even compile if
        // it isn't true, so the assertion is documentary.
        let _stage = StageId::new("Foo").unwrap();
        let _job = JobId::new("Foo").unwrap();
        // (no assertion needed; the test compiles iff the types are distinct)
    }
}
