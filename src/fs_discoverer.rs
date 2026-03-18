// apcore-cli -- Filesystem-based module discoverer.
// Scans a directory recursively for module.json descriptor files and
// produces DiscoveredModule entries for registration in the apcore Registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use apcore::errors::ModuleError;
use apcore::module::ModuleAnnotations;
use apcore::registry::registry::{DiscoveredModule, Discoverer, ModuleDescriptor};

/// Intermediate struct for deserializing module.json files.
///
/// Fields that are optional in the JSON map to defaults suitable for
/// constructing a full `ModuleDescriptor`.
#[derive(Debug, serde::Deserialize)]
struct ModuleJson {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
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
        self.executables
            .lock()
            .ok()
            .and_then(|map| map.get(module_name).cloned())
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
    fn collect_module_jsons(dir: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return result,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
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
    async fn discover(&self) -> Result<Vec<DiscoveredModule>, ModuleError> {
        let paths = Self::collect_module_jsons(&self.root);
        let mut modules = Vec::new();

        for path in paths {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                ModuleError::new(
                    apcore::errors::ErrorCode::ModuleLoadError,
                    format!("Failed to read {}: {}", path.display(), e),
                )
            })?;

            let mj: ModuleJson = serde_json::from_str(&content).map_err(|e| {
                ModuleError::new(
                    apcore::errors::ErrorCode::ModuleLoadError,
                    format!("Failed to parse {}: {}", path.display(), e),
                )
            })?;

            // Resolve executable path relative to module.json directory.
            if let Some(ref exec_rel) = mj.executable {
                if let Some(parent) = path.parent() {
                    let exec_path = parent.join(exec_rel);
                    if exec_path.exists() {
                        if let Ok(mut map) = self.executables.lock() {
                            map.insert(mj.name.clone(), exec_path);
                        }
                    }
                }
            }

            let descriptor = ModuleDescriptor {
                name: mj.name.clone(),
                annotations: ModuleAnnotations::default(),
                input_schema: mj.input_schema,
                output_schema: mj.output_schema,
                enabled: true,
                tags: mj.tags,
                dependencies: vec![],
            };

            modules.push(DiscoveredModule {
                name: mj.name,
                source: path.display().to_string(),
                descriptor,
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
        let modules = discoverer.discover().await.unwrap();
        assert_eq!(modules.len(), 2);

        let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"math.add"));
        assert!(names.contains(&"text.upper"));
    }

    #[tokio::test]
    async fn test_discover_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let discoverer = FsDiscoverer::new(tmp.path());
        let modules = discoverer.discover().await.unwrap();
        assert!(modules.is_empty());
    }

    #[tokio::test]
    async fn test_discover_nonexistent_dir() {
        let discoverer = FsDiscoverer::new("/nonexistent/path/xxx");
        let modules = discoverer.discover().await.unwrap();
        assert!(modules.is_empty());
    }

    #[tokio::test]
    async fn test_discover_invalid_json_returns_error() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("bad");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("module.json"), "not valid json").unwrap();

        let discoverer = FsDiscoverer::new(tmp.path());
        let result = discoverer.discover().await;
        assert!(result.is_err());
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
        let modules = discoverer.discover().await.unwrap();
        assert_eq!(modules.len(), 1);

        let m = &modules[0];
        assert_eq!(m.name, "test.mod");
        assert_eq!(m.descriptor.name, "test.mod");
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
        let mut registry = apcore::Registry::new();
        let names = registry.discover(&discoverer).await.unwrap();

        assert_eq!(names, vec!["math.add"]);
        assert!(registry.get_definition("math.add").is_some());
    }
}
