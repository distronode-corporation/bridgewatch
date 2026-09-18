//! The optional Rhai hook that replaces the built-in icon-state rules.
//!
//! Rules 1 to 7 are the opinion bridgewatch ships with. They will not fit every
//! estate, and the alternative to a hook is a settings pane full of half-rules,
//! so a watch may hand the whole decision to a script instead.
//!
//! The script is given the [`PipelineView`] as a map and returns an icon-state
//! name. It is sandboxed: the module resolver is disabled so `import` cannot
//! reach a file, `eval` is disabled, and both the operation count and the call
//! depth are capped, so a runaway script stalls one verdict rather than the
//! tray.
//!
//! ```
//! use bridgewatch_core::verdict::VerdictScript;
//!
//! let script = VerdictScript::from_source(
//!     r#"
//!     if pipeline.deploy == "live" && pipeline.failures.is_empty() {
//!         "deployed"
//!     } else if pipeline.failures.is_empty() {
//!         "running"
//!     } else {
//!         "failed"
//!     }
//!     "#,
//! )
//! .expect("compiles");
//! # let _ = script;
//! ```

use std::path::Path;

use super::PipelineView;
use super::icon::IconState;

/// The maximum number of Rhai operations one verdict may spend.
///
/// Generous for a decision over a few dozen jobs, small enough that an
/// accidental infinite loop is caught in microseconds.
pub const MAX_OPERATIONS: u64 = 250_000;

/// The maximum call depth a verdict script may reach.
pub const MAX_CALL_LEVELS: usize = 16;

/// Why a verdict script did not produce a state.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The script could not be read from disk.
    #[error("cannot read verdict script {path}: {source}")]
    Io {
        /// The path that could not be read.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The script does not compile.
    #[error("verdict script does not compile: {0}")]
    Compile(String),
    /// The script failed at run time, or exceeded a sandbox limit.
    #[error("verdict script failed: {0}")]
    Runtime(String),
    /// The script returned something that is not an icon-state name.
    #[error(
        "verdict script returned {returned:?}, which is not one of: unknown, failed, deployed_with_failure, deployed, running, canceled, parked_gate, succeeded_no_deploy"
    )]
    BadReturn {
        /// What the script actually returned.
        returned: String,
    },
    /// The pipeline view could not be handed to the script.
    #[error("cannot pass the pipeline to the script: {0}")]
    Marshal(String),
}

/// A compiled verdict script.
#[derive(Debug)]
pub struct VerdictScript {
    engine: rhai::Engine,
    ast: rhai::AST,
}

impl VerdictScript {
    /// Compile a script from source.
    pub fn from_source(source: &str) -> Result<Self, ScriptError> {
        let engine = sandboxed_engine();
        let ast = engine
            .compile(source)
            .map_err(|e| ScriptError::Compile(e.to_string()))?;
        Ok(Self { engine, ast })
    }

    /// Compile a script from a file. A leading `~` is expanded.
    pub fn from_path(path: &Path) -> Result<Self, ScriptError> {
        let path = crate::config::expand_tilde(path);
        let source = std::fs::read_to_string(&path).map_err(|source| ScriptError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_source(&source)
    }

    /// Run the script over one pipeline.
    ///
    /// The view is bound as `pipeline`, and also as `p` for brevity.
    pub fn evaluate(&self, view: &PipelineView) -> Result<IconState, ScriptError> {
        let dynamic =
            rhai::serde::to_dynamic(view).map_err(|e| ScriptError::Marshal(e.to_string()))?;
        let mut scope = rhai::Scope::new();
        scope.push_constant("pipeline", dynamic.clone());
        scope.push_constant("p", dynamic);

        let result: rhai::Dynamic = self
            .engine
            .eval_ast_with_scope(&mut scope, &self.ast)
            .map_err(|e| ScriptError::Runtime(e.to_string()))?;

        let returned = result
            .into_string()
            .map_err(|actual| ScriptError::BadReturn {
                returned: actual.to_string(),
            })?;
        IconState::parse(&returned).ok_or(ScriptError::BadReturn { returned })
    }
}

/// An engine with the sandbox applied.
///
/// ⛔ Rhai's default engine DOES reach the filesystem. It has no `open`/`read`
/// function, which is what made "no filesystem" look true, but its default
/// module resolver is `FileModuleResolver`: `import "/path/to/x" as x;` reads
/// that file off disk and RUNS it, inside a process holding a GitLab token and
/// pointed at a path the user's own config file names. The resolver is replaced
/// with [`rhai::module_resolvers::DummyModuleResolver`], which refuses every
/// import, so there is no path from a script to a file at all.
///
/// The rest is belt to that braces: `eval` off, so a script cannot build code at
/// run time, and hard caps on work and recursion.
fn sandboxed_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);
    engine.disable_symbol("eval");
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine
}
