//! Shared execution contracts for registration, resolved dispatch, and host preparation.
//! Runtime selects the meaning of command; deserialization applies its default once.
//! Host preparation resolves file commands before later argument construction and launch.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};

/// Typed command reused before and after host path resolution.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "runtime", content = "command", rename_all = "camelCase")]
pub enum ExecutionCommand {
    /// Native executable path; Unix hosts require executable permissions.
    Executable(PathBuf),
    /// Python script path passed to Diamant's embedded Python launcher.
    Python(PathBuf),
    /// Importable Python module name passed with Python's -m option.
    PythonModule(String),
}

/// Standard execution configuration shared by declared and resolved adapters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(try_from = "ExecutionFields", rename_all = "camelCase")]
pub struct ExecutionSpec {
    /// Runtime-selected executable path, Python script path, or Python module name.
    #[serde(flatten)]
    pub command: ExecutionCommand,
    /// Optional working directory; omission selects the installed extension root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
}

/// Wire discriminator selecting how the single command field is interpreted.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Runtime {
    /// Run command as a native executable.
    #[default]
    Executable,
    /// Pass command as a Python script path.
    Python,
    /// Pass command as a Python module name with -m.
    PythonModule,
}
impl JsonSchema for Runtime {
    /// Schema reused for every execution runtime discriminator.
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Runtime".into()
    }
    /// Serialized variants preserve the exact wire names without duplicated strings.
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type":"string","enum":[Self::Executable,Self::Python,Self::PythonModule]}).try_into().unwrap()
    }
}

/// Flat JSON/YAML contract. Standard fields are typed and unknown fields are rejected.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionFields {
    /// Defaults once during deserialization and Kubernetes schema defaulting.
    #[serde(default)]
    #[schemars(extend("default" = "executable"))]
    runtime: Runtime,
    /// One executable/script path or module name, interpreted by runtime.
    #[schemars(length(min = 1))]
    command: String,
    /// Optional process directory, relative to the installed root or explicitly absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    working_directory: Option<PathBuf>,
}
impl TryFrom<ExecutionFields> for ExecutionSpec {
    type Error = io::Error;

    /// Convert the required nonempty wire command to its runtime-specific representation.
    fn try_from(fields: ExecutionFields) -> Result<Self, Self::Error> {
        if fields.command.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "execution.command MUST NOT be empty",
            ));
        }
        if fields
            .working_directory
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workingDirectory MUST NOT be empty",
            ));
        }
        let command = match fields.runtime {
            Runtime::Executable => ExecutionCommand::Executable(fields.command.into()),
            Runtime::Python => ExecutionCommand::Python(fields.command.into()),
            Runtime::PythonModule => ExecutionCommand::PythonModule(fields.command),
        };
        Ok(Self {
            command,
            working_directory: fields.working_directory,
        })
    }
}
impl JsonSchema for ExecutionSpec {
    /// Shared schema name for declared and resolved execution configuration.
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ExecutionSpec".into()
    }
    /// Reuse the flat validated wire schema, including runtime defaulting.
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        ExecutionFields::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Each runtime gives the same command field a typed meaning, preserving wire round trips.
    #[test]
    fn command_defaults_and_runtimes_round_trip() {
        for (document, expected) in [
            (
                json!({"command":"./run"}),
                ExecutionCommand::Executable("./run".into()),
            ),
            (
                json!({"runtime":"executable","command":"./run","workingDirectory":"./work"}),
                ExecutionCommand::Executable("./run".into()),
            ),
            (
                json!({"runtime":"python","command":"./plugin.py"}),
                ExecutionCommand::Python("./plugin.py".into()),
            ),
            (
                json!({"runtime":"pythonModule","command":"plugin.main"}),
                ExecutionCommand::PythonModule("plugin.main".into()),
            ),
        ] {
            let execution: ExecutionSpec = serde_json::from_value(document.clone()).unwrap();
            assert_eq!(execution.command, expected);
            let encoded = serde_json::to_value(&execution).unwrap();
            assert_eq!(encoded["command"], document["command"]);
            assert_eq!(
                serde_json::from_value::<ExecutionSpec>(encoded.clone()).unwrap(),
                execution
            );
            if document.get("runtime").is_none() {
                assert_eq!(encoded["runtime"], "executable");
            }
        }
    }

    /// Legacy fields, unsupported runtimes, and incomplete commands fail at deserialization.
    #[test]
    fn rejects_invalid_and_legacy_execution_fields() {
        for document in [
            json!({}),
            json!({"runtime":"python"}),
            json!({"runtime":"pythonModule"}),
            json!({"runtime":"unknown","command":"./run"}),
            json!({"command":""}),
            json!({"command":null}),
            json!({"command":42}),
            json!({"command":"./run","workingDirectory":""}),
            json!({"command":"./run","arbitraryField":true}),
            json!({"executable":"./run"}),
            json!({"runtime":"python","script":"./plugin.py"}),
            json!({"runtime":"python","module":"plugin"}),
            json!({"command":"./run","script":"./plugin.py"}),
        ] {
            assert!(
                serde_json::from_value::<ExecutionSpec>(document.clone()).is_err(),
                "{document}"
            );
        }
    }
}
