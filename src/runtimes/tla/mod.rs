//! TLA+ / TLC runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: tla:`, the compiler auto-installs a JRE from
//! Eclipse Temurin (Adoptium) and the `tla2tools.jar` from the TLA+ GitHub
//! releases page, creates convenience shims (`tlc`, `pluscal`, `sany`),
//! adds Java ecosystem domains to the AWF network allowlist, extends the bash
//! command allow-list, and appends a prompt supplement informing the agent
//! that TLA+ is available.
//!
//! The toolchain is installed into `$HOME/.tla/`, which is mounted read-only
//! into the AWF chroot via the `required_awf_mounts()` mechanism (same pattern
//! as `runtimes: lean` using `$HOME/.elan/`).
//!
//! ## Network requirements
//!
//! - **JRE download**: `api.adoptium.net` — covered by the `java` ecosystem
//!   identifier, which is automatically added to the AWF allowlist.
//! - **`tla2tools.jar` download**: GitHub releases — covered by the built-in
//!   GitHub allowlist that every pipeline includes by default.
//!
//! ## Shims
//!
//! Three shims are created in `$HOME/.tla/bin/`:
//! - `tlc` — runs `tlc2.TLC` (the model checker)
//! - `pluscal` — runs `pcal.trans` (PlusCal → TLA+ translator)
//! - `sany` — runs `tla2sany.SANY` (the SANY parser)

pub mod extension;

pub use extension::TlaExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// TLA+ runtime configuration — accepts both `true` and object formats.
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs latest tla2tools.jar with JDK 21)
/// runtimes:
///   tla: true
///
/// # With options (pin versions)
/// runtimes:
///   tla:
///     version: "1.8.0"   # tla2tools.jar version; omit for latest
///     jdk: "21"          # JRE major version (default 21)
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum TlaRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(TlaOptions),
}

impl TlaRuntimeConfig {
    /// Whether TLA+ is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            TlaRuntimeConfig::Enabled(enabled) => *enabled,
            TlaRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Get the `tla2tools.jar` version override (None = use latest from GitHub).
    pub fn version(&self) -> Option<&str> {
        match self {
            TlaRuntimeConfig::Enabled(_) => None,
            TlaRuntimeConfig::WithOptions(opts) => opts.version.as_deref(),
        }
    }

    /// Get the JDK major version (None = use default of `"21"`).
    pub fn jdk(&self) -> Option<&str> {
        match self {
            TlaRuntimeConfig::Enabled(_) => None,
            TlaRuntimeConfig::WithOptions(opts) => opts.jdk.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for TlaRuntimeConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            TlaRuntimeConfig::Enabled(_) => {}
            TlaRuntimeConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// TLA+ runtime options.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct TlaOptions {
    /// `tla2tools.jar` version to download from the TLA+ GitHub releases.
    ///
    /// Examples: `"1.8.0"`, `"1.7.3"`.
    ///
    /// When omitted the compiler downloads the latest GitHub release using
    /// the `releases/latest/download/` redirect — no explicit version pinning
    /// is required for most workflows.
    #[serde(default)]
    pub version: Option<String>,

    /// JRE/JDK major version to download from Eclipse Temurin (Adoptium).
    ///
    /// Examples: `"21"`, `"17"`. Defaults to `"21"` (current LTS).
    ///
    /// The JRE (not a full JDK) is downloaded since TLC only needs a runtime.
    #[serde(default)]
    pub jdk: Option<String>,
}

/// Default JRE major version used when `jdk:` is not specified.
pub const DEFAULT_JDK_VERSION: &str = "21";

/// Bash commands that the TLA+ runtime adds to the allow-list.
///
/// - `java` — direct JVM invocation (e.g. `java -cp $TLA_JAR tlc2.TLC`)
/// - `tlc` — convenience shim for running the TLC model checker
/// - `pluscal` — shim for the PlusCal → TLA+ translator
/// - `sany` — shim for the SANY parser / syntax checker
pub const TLA_BASH_COMMANDS: &[&str] = &["java", "tlc", "pluscal", "sany"];

/// Generate the TLA+ installation bash script body.
///
/// Downloads and stages:
/// 1. A JRE from Eclipse Temurin (Adoptium) into `$HOME/.tla/jre/`
/// 2. `tla2tools.jar` from TLA+ GitHub releases into `$HOME/.tla/`
/// 3. Shim scripts (`tlc`, `pluscal`, `sany`) in `$HOME/.tla/bin/`
///
/// Sets `TLA_JAR` and `TLA_JAVA_HOME` via `##vso[task.setvariable]` so
/// downstream steps can reference the jar and JRE paths. Also prepends
/// `$HOME/.tla/bin` to PATH via `##vso[task.prependpath]`.
pub fn generate_tla_install(config: &TlaRuntimeConfig) -> String {
    let jdk_version = config.jdk().unwrap_or(DEFAULT_JDK_VERSION);
    let jar_url = match config.version() {
        Some(v) => format!(
            "https://github.com/tlaplus/tlaplus/releases/download/v{v}/tla2tools.jar"
        ),
        None => {
            "https://github.com/tlaplus/tlaplus/releases/latest/download/tla2tools.jar"
                .to_string()
        }
    };

    format!(
        "\
set -eo pipefail
TLA_HOME=\"$HOME/.tla\"
JRE_DIR=\"$TLA_HOME/jre\"
BIN_DIR=\"$TLA_HOME/bin\"
mkdir -p \"$JRE_DIR\" \"$BIN_DIR\"

# Download JRE from Eclipse Temurin (Adoptium)
JRE_URL=\"https://api.adoptium.net/v3/binary/latest/{jdk_version}/ga/linux/x64/jre/hotspot/normal/eclipse?project=jdk\"
echo \"Downloading Temurin JRE {jdk_version} from Adoptium...\"
curl -sSfL \"$JRE_URL\" -o \"$TLA_HOME/jre.tgz\"
tar -xzf \"$TLA_HOME/jre.tgz\" -C \"$JRE_DIR\" --strip-components=1
rm -f \"$TLA_HOME/jre.tgz\"

# Download tla2tools.jar
echo \"Downloading tla2tools.jar...\"
curl -sSfL \"{jar_url}\" -o \"$TLA_HOME/tla2tools.jar\"

# Create shim: tlc (TLC model checker)
cat > \"$BIN_DIR/tlc\" << 'TLA_SHIM_EOF'
#!/usr/bin/env bash
exec \"$HOME/.tla/jre/bin/java\" -XX:+UseParallelGC -cp \"$HOME/.tla/tla2tools.jar\" tlc2.TLC \"$@\"
TLA_SHIM_EOF

# Create shim: pluscal (PlusCal -> TLA+ translator)
cat > \"$BIN_DIR/pluscal\" << 'TLA_SHIM_EOF'
#!/usr/bin/env bash
exec \"$HOME/.tla/jre/bin/java\" -cp \"$HOME/.tla/tla2tools.jar\" pcal.trans \"$@\"
TLA_SHIM_EOF

# Create shim: sany (SANY parser / syntax checker)
cat > \"$BIN_DIR/sany\" << 'TLA_SHIM_EOF'
#!/usr/bin/env bash
exec \"$HOME/.tla/jre/bin/java\" -cp \"$HOME/.tla/tla2tools.jar\" tla2sany.SANY \"$@\"
TLA_SHIM_EOF

chmod +x \"$BIN_DIR/tlc\" \"$BIN_DIR/pluscal\" \"$BIN_DIR/sany\"

# Expose environment
echo \"##vso[task.prependpath]$HOME/.tla/bin\"
echo \"##vso[task.setvariable variable=TLA_JAR]$HOME/.tla/tla2tools.jar\"
echo \"##vso[task.setvariable variable=TLA_JAVA_HOME]$HOME/.tla/jre\"
export PATH=\"$HOME/.tla/bin:$PATH\"
\"$HOME/.tla/jre/bin/java\" -version
echo \"TLA+ toolchain ready: $TLA_HOME\"\
"
    )
}
