// apcore-cli -- Filesystem-based module discoverer.
// Scans a directory recursively for module.json descriptor files and
// produces DiscoveredModule entries for registration in the apcore Registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{DiscoveredModule, Discoverer, ModuleDescriptor};

/// Placeholder `Module` carried in `DiscoveredModule.module` for subprocess-based
/// modules discovered on the filesystem. It holds the module's schemas so the
/// registry can report them for validation and description, but `execute()`
/// intentionally fails — actual invocation goes through the subprocess dispatch
/// path in `main.rs` which resolves the executable via
/// [`FsDiscoverer::executables_snapshot`].
struct SubprocessPlaceholderModule {
    module_id: String,
    input_schema: serde_json::Value,
    output_schema: serde_json::Value,
    description: String,
}

#[async_trait]
impl Module for SubprocessPlaceholderModule {
    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn output_schema(&self) -> serde_json::Value {
        self.output_schema.clone()
    }

    async fn execute(
        &self,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        Err(ModuleError::new(
            ErrorCode::ModuleExecuteError,
            format!(
                "Module '{}' is a subprocess module; in-process execute() is \
                 unsupported. Invoke via the CLI subprocess dispatcher.",
                self.module_id
            ),
        ))
    }
}

/// Intermediate struct for deserializing module.json files.
///
/// Fields that are optional in the JSON map to defaults suitable for
/// constructing a full `ModuleDescriptor`.
#[derive(Debug, serde::Deserialize)]
struct ModuleJson {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_schema")]
    input_schema: serde_json::Value,
    #[serde(default = "default_schema")]
    output_schema: serde_json::Value,
    /// Optional relative path to an executable script (e.g. "run.sh").
    #[serde(default)]
    executable: Option<String>,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({})
}

/// Filesystem-based module discoverer.
///
/// Recursively walks `root` looking for files named `module.json`, parses each
/// one into a `DiscoveredModule`, and returns them all from `discover()`.
pub struct FsDiscoverer {
    root: PathBuf,
    /// Map of module name to resolved executable path (built during discovery).
    executables: std::sync::Mutex<HashMap<String, PathBuf>>,
}

impl FsDiscoverer {
    /// Create a new discoverer rooted at the given directory path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            executables: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Return the resolved executable path for a module, if one was declared.
    pub fn get_executable(&self, module_name: &str) -> Option<PathBuf> {
        match self.executables.lock() {
            Ok(map) => map.get(module_name).cloned(),
            Err(_poisoned) => {
                tracing::warn!("Executables mutex poisoned — returning None for '{module_name}'");
                None
            }
        }
    }

    /// Return a snapshot of all executable paths discovered so far.
    pub fn executables_snapshot(&self) -> HashMap<String, PathBuf> {
        self.executables
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default()
    }

    /// Scan the extensions directory and return a map of module name to description.
    ///
    /// This is a convenience method for populating description metadata that
    /// `ModuleDescriptor` does not carry. Non-parseable files are silently skipped.
    pub fn load_descriptions(&self) -> std::collections::HashMap<String, String> {
        let paths = Self::collect_module_jsons(&self.root);
        let mut map = std::collections::HashMap::new();
        for path in paths {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(mj) = serde_json::from_str::<ModuleJson>(&content) {
                    if !mj.description.is_empty() {
                        map.insert(mj.name, mj.description);
                    }
                }
            }
        }
        map
    }

    /// Recursively collect all `module.json` paths under `dir`.
    ///
    /// Skips symlinked directories to prevent infinite recursion when the
    /// extensions tree contains a symlink that points back into an ancestor.
    fn collect_module_jsons(dir: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return result,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Do not follow symlinked directories — avoids infinite recursion
            // when a symlink under the extensions root points back into an
            // ancestor directory (common in monorepo / workspace layouts).
            let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            if path.is_dir() && !is_symlink {
                result.extend(Self::collect_module_jsons(&path));
            } else if path.file_name().and_then(|n| n.to_str()) == Some("module.json") {
                result.push(path);
            }
        }
        result
    }
}

#[async_trait]
impl Discoverer for FsDiscoverer {
    async fn discover(&self, _roots: &[String]) -> Result<Vec<DiscoveredModule>, ModuleError> {
        let paths = Self::collect_module_jsons(&self.root);
        let mut modules = Vec::new();

        for path in paths {
            // Skip a single unreadable / malformed module.json with a warning
            // rather than aborting the whole pass — the sibling
            // load_descriptions() already tolerates the same failures, and
            // dropping every later module on the floor because of one typo
            // produces a confusing "registry shrink" symptom for users.
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Failed to read module.json '{}': {} — skipping",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            let mj: ModuleJson = match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse module.json '{}': {} — skipping",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            // Resolve executable path relative to module.json directory.
            // Security: validate the resolved path stays within the extensions root.
            if let Some(ref exec_rel) = mj.executable {
                if let Some(parent) = path.parent() {
                    let exec_path = parent.join(exec_rel);
                    if exec_path.exists() {
                        // Canonicalize both paths to prevent traversal via ../../
                        // Store the canonicalized form so consumers hold the
                        // vetted, symlink-resolved path rather than the raw one.
                        let (exec_canon_res, root_canon_res) =
                            (exec_path.canonicalize(), self.root.canonicalize());
                        let safe = match (&exec_canon_res, &root_canon_res) {
                            (Ok(ec), Ok(rc)) => ec.starts_with(rc),
                            _ => false,
                        };
                        if safe {
                            let exec_canon = exec_canon_res.unwrap();
                            match self.executables.lock() {
                                Ok(mut map) => {
                                    map.insert(mj.name.clone(), exec_canon);
                                }
                                Err(_poisoned) => {
                                    tracing::warn!(
                                        "Executables mutex poisoned during discover() — '{}' not registered",
                                        mj.name
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                "Executable '{}' for module '{}' escapes extensions root — skipped",
                                exec_path.display(),
                                mj.name
                            );
                        }
                    }
                }
            }

            let module_id = mj.name.clone();
            let descriptor = ModuleDescriptor {
                module_id: module_id.clone(),
                name: None,
                description: mj.description.clone(),
                documentation: None,
                input_schema: mj.input_schema.clone(),
                output_schema: mj.output_schema.clone(),
                version: "1.0.0".to_string(),
                tags: mj.tags,
                annotations: Some(ModuleAnnotations::default()),
                examples: vec![],
                metadata: HashMap::new(),
                display: None,
                sunset_date: None,
                dependencies: vec![],
                enabled: true,
            };

            let module: Arc<dyn Module> = Arc::new(SubprocessPlaceholderModule {
                module_id: module_id.clone(),
                input_schema: mj.input_schema,
                output_schema: mj.output_schema,
                description: mj.description,
            });

            modules.push(DiscoveredModule {
                name: module_id,
                source: path.display().to_string(),
                descriptor,
                module,
            });
        }

        Ok(modules)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_module_json(dir: &Path, name: &str, description: &str, tags: &[&str]) {
        let tags_json: Vec<String> = tags.iter().map(|t| format!("\"{}\"", t)).collect();
        let content = format!(
            r#"{{
  "name": "{}",
  "description": "{}",
  "tags": [{}],
  "input_schema": {{"type": "object"}},
  "output_schema": {{"type": "object"}}
}}"#,
            name,
            description,
            tags_json.join(", ")
        );
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("module.json"), content).unwrap();
    }

    #[tokio::test]
    async fn test_discover_finds_modules() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_module_json(&root.join("math/add"), "math.add", "Add numbers", &["math"]);
        write_module_json(
            &root.join("text/upper"),
            "text.upper",
            "Uppercase text",
            &["text"],
        );

        let discoverer = FsDiscoverer::new(root);
        let modules = discoverer.discover(&[]).await.unwrap();
        assert_eq!(modules.len(), 2);

        let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"math.add"));
        assert!(names.contains(&"text.upper"));
    }

    #[tokio::test]
    async fn test_discover_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let discoverer = FsDiscoverer::new(tmp.path());
        let modules = discoverer.discover(&[]).await.unwrap();
        assert!(modules.is_empty());
    }

    #[tokio::test]
    async fn test_discover_nonexistent_dir() {
        let discoverer = FsDiscoverer::new("/nonexistent/path/xxx");
        let modules = discoverer.discover(&[]).await.unwrap();
        assert!(modules.is_empty());
    }

    #[tokio::test]
    async fn test_discover_invalid_json_is_skipped_not_aborting() {
        // Per review #14: a malformed module.json must produce a tracing
        // warning and be skipped, not abort the whole discovery pass.
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("module.json"), "not valid json").unwrap();
        write_module_json(
            &tmp.path().join("good"),
            "good.mod",
            "still loads",
            &["demo"],
        );

        let discoverer = FsDiscoverer::new(tmp.path());
        let modules = discoverer
            .discover(&[])
            .await
            .expect("malformed sibling must not abort the pass");
        let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"good.mod"),
            "well-formed module must still load alongside malformed sibling, got {names:?}"
        );
    }

    #[tokio::test]
    async fn test_discover_sets_descriptor_fields() {
        let tmp = TempDir::new().unwrap();
        write_module_json(
            &tmp.path().join("a"),
            "test.mod",
            "A test module",
            &["demo", "test"],
        );

        let discoverer = FsDiscoverer::new(tmp.path());
        let modules = discoverer.discover(&[]).await.unwrap();
        assert_eq!(modules.len(), 1);

        let m = &modules[0];
        assert_eq!(m.name, "test.mod");
        assert_eq!(m.descriptor.module_id, "test.mod");
        assert!(m.descriptor.enabled);
        assert_eq!(m.descriptor.tags, vec!["demo", "test"]);
        assert!(m.descriptor.dependencies.is_empty());
    }

    #[tokio::test]
    async fn test_discover_and_register_populates_registry() {
        let tmp = TempDir::new().unwrap();
        write_module_json(
            &tmp.path().join("math/add"),
            "math.add",
            "Add numbers",
            &["math"],
        );

        let discoverer = FsDiscoverer::new(tmp.path());
        let registry = apcore::Registry::new();
        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 1);
        assert!(registry.get_definition("math.add").is_some());
    }
}
