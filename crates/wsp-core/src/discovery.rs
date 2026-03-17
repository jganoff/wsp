use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
use crate::template;
use crate::util::read_stdin_line;

/// Describes a template file found in a repo directory or bare mirror.
#[derive(Debug, Clone)]
pub struct DiscoveredTemplate {
    pub name: String,
    pub file_path: PathBuf,
    pub repo_identity: String,
    pub status: DiscoveryStatus,
    /// Raw YAML content (populated for bare mirror discoveries).
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryStatus {
    /// Not yet imported locally.
    New,
    /// Imported but source file content differs from local template.
    Changed,
    /// Imported and unchanged.
    AlreadyImported,
}

/// Scan a repo directory root for *.wsp.yaml files.
/// Compares against imported templates to determine status.
pub fn scan_repo_dir(
    repo_dir: &Path,
    repo_identity: &str,
    templates_dir: &Path,
) -> Vec<DiscoveredTemplate> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(repo_dir) {
        Ok(e) => e,
        Err(_) => return found,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };
        if !filename.ends_with(".wsp.yaml") {
            continue;
        }
        // Skip .wsp.yaml (workspace metadata)
        if filename == ".wsp.yaml" {
            continue;
        }

        let tmpl = match template::load_from_file(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let name = template::derive_name_from_file(&path, &tmpl);
        if template::validate_name(&name).is_err() {
            continue;
        }
        let status = determine_status(templates_dir, &name, &path);

        found.push(DiscoveredTemplate {
            name,
            file_path: path,
            repo_identity: repo_identity.to_string(),
            status,
            content: None,
        });
    }

    found
}

/// Scan a bare mirror for *.wsp.yaml files (git ls-tree + git show).
pub fn scan_bare_mirror(
    mirror_path: &Path,
    repo_identity: &str,
    templates_dir: &Path,
) -> Vec<DiscoveredTemplate> {
    let mut found = Vec::new();

    let filenames = match git::ls_tree_names(mirror_path, "HEAD") {
        Ok(names) => names,
        Err(_) => return found,
    };

    for filename in filenames {
        if !filename.ends_with(".wsp.yaml") || filename == ".wsp.yaml" {
            continue;
        }

        let content = match git::show_file(mirror_path, "HEAD", &filename) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        let tmpl: template::Template = match serde_yaml_ng::from_str(&content) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if tmpl.repos.is_empty() {
            continue;
        }

        let name = template::derive_name_from_file(Path::new(&filename), &tmpl);
        if template::validate_name(&name).is_err() {
            continue;
        }
        let synthetic_source = format!("mirror:{}:{}", repo_identity, filename);
        let status =
            determine_status_from_content(templates_dir, &name, &synthetic_source, &content);

        found.push(DiscoveredTemplate {
            name,
            file_path: PathBuf::from(&filename),
            repo_identity: repo_identity.to_string(),
            status,
            content: Some(content),
        });
    }

    found
}

/// Determine status by comparing file content with stored template.
fn determine_status(templates_dir: &Path, name: &str, file_path: &Path) -> DiscoveryStatus {
    if !template::exists(templates_dir, name) {
        return DiscoveryStatus::New;
    }

    // Check source metadata to see if this file was the import source.
    // If no sidecar exists, the template was manually created — don't overwrite it.
    let source = match template::load_source(templates_dir, name) {
        Ok(Some(s)) => s,
        _ => return DiscoveryStatus::AlreadyImported,
    };

    let file_str = file_path.to_string_lossy();
    if source.source_path != *file_str {
        return DiscoveryStatus::AlreadyImported; // imported from different source
    }

    // Compare content
    let file_content = match crate::util::read_yaml_file(file_path) {
        Ok(c) => c,
        Err(_) => return DiscoveryStatus::Changed,
    };

    let stored = match template::load(templates_dir, name) {
        Ok(t) => match template::to_yaml(&t) {
            Ok(y) => y,
            Err(_) => return DiscoveryStatus::Changed,
        },
        Err(_) => return DiscoveryStatus::Changed,
    };

    // Parse the file content as a template and re-serialize for comparison
    let file_yaml = match serde_yaml_ng::from_str::<template::Template>(&file_content) {
        Ok(t) => match template::to_yaml(&t) {
            Ok(y) => y,
            Err(_) => return DiscoveryStatus::Changed,
        },
        Err(_) => return DiscoveryStatus::Changed,
    };

    if stored == file_yaml {
        DiscoveryStatus::AlreadyImported
    } else {
        DiscoveryStatus::Changed
    }
}

fn determine_status_from_content(
    templates_dir: &Path,
    name: &str,
    synthetic_source: &str,
    content: &str,
) -> DiscoveryStatus {
    if !template::exists(templates_dir, name) {
        return DiscoveryStatus::New;
    }

    let source = match template::load_source(templates_dir, name) {
        Ok(Some(s)) => s,
        _ => return DiscoveryStatus::AlreadyImported,
    };

    if source.source_path != synthetic_source {
        return DiscoveryStatus::AlreadyImported;
    }

    let stored = match template::load(templates_dir, name) {
        Ok(t) => match template::to_yaml(&t) {
            Ok(y) => y,
            Err(_) => return DiscoveryStatus::Changed,
        },
        Err(_) => return DiscoveryStatus::Changed,
    };

    let file_yaml = match serde_yaml_ng::from_str::<template::Template>(content) {
        Ok(t) => match template::to_yaml(&t) {
            Ok(y) => y,
            Err(_) => return DiscoveryStatus::Changed,
        },
        Err(_) => return DiscoveryStatus::Changed,
    };

    if stored == file_yaml {
        DiscoveryStatus::AlreadyImported
    } else {
        DiscoveryStatus::Changed
    }
}

// ---------------------------------------------------------------------------
// Interactive prompts
// ---------------------------------------------------------------------------

/// Prompt the user about discovered templates and import as requested.
/// Returns the number of templates imported.
pub fn prompt_and_import(discovered: &[DiscoveredTemplate], templates_dir: &Path) -> Result<usize> {
    let actionable: Vec<&DiscoveredTemplate> = discovered
        .iter()
        .filter(|d| d.status != DiscoveryStatus::AlreadyImported)
        .collect();

    if actionable.is_empty() {
        return Ok(0);
    }

    let is_tty = std::io::stdin().is_terminal();
    let mut imported = 0;

    for dt in &actionable {
        if is_tty {
            imported += prompt_single(dt, templates_dir)?;
        } else {
            hint_single(dt);
        }
    }

    Ok(imported)
}

fn prompt_single(dt: &DiscoveredTemplate, templates_dir: &Path) -> Result<usize> {
    let repo_short = dt
        .repo_identity
        .rsplit('/')
        .next()
        .unwrap_or(&dt.repo_identity);

    match dt.status {
        DiscoveryStatus::New => {
            eprintln!(
                "  found template {:?} in {}/ ({})",
                dt.name,
                repo_short,
                dt.file_path.display()
            );
            eprintln!("    [1] Import template (default)");
            eprintln!("    [2] Skip");
            eprint!("  choice [1]: ");
        }
        DiscoveryStatus::Changed => {
            eprintln!(
                "  template {:?} has changed in {}/ ({})",
                dt.name,
                repo_short,
                dt.file_path.display()
            );
            eprintln!("    [1] Update template (default)");
            eprintln!("    [2] Skip");
            eprint!("  choice [1]: ");
        }
        DiscoveryStatus::AlreadyImported => return Ok(0),
    }

    let choice = read_stdin_line();
    if choice.trim() == "2" {
        return Ok(0);
    }

    do_import(dt, templates_dir)?;
    Ok(1)
}

fn hint_single(dt: &DiscoveredTemplate) {
    match dt.status {
        DiscoveryStatus::New => {
            eprintln!(
                "hint: found template {:?} in {} ({})",
                dt.name,
                dt.repo_identity,
                dt.file_path.display()
            );
            eprintln!("  wsp template import {}", dt.file_path.display());
        }
        DiscoveryStatus::Changed => {
            eprintln!(
                "hint: template {:?} has changed in {} ({})",
                dt.name,
                dt.repo_identity,
                dt.file_path.display()
            );
            eprintln!("  wsp template import {} --update", dt.file_path.display());
        }
        DiscoveryStatus::AlreadyImported => {}
    }
}

/// Import a single discovered template.
fn do_import(dt: &DiscoveredTemplate, templates_dir: &Path) -> Result<()> {
    let tmpl = if let Some(ref content) = dt.content {
        serde_yaml_ng::from_str::<template::Template>(content)?
    } else {
        template::load_from_file(&dt.file_path)?
    };

    template::save(templates_dir, &dt.name, &tmpl)?;

    let source_path = if dt.content.is_some() {
        // Bare mirror discovery uses synthetic path
        format!("mirror:{}:{}", dt.repo_identity, dt.file_path.display())
    } else {
        dt.file_path.to_string_lossy().to_string()
    };

    template::save_source(
        templates_dir,
        &dt.name,
        &template::ImportSource {
            source_path,
            imported_at: chrono::Utc::now(),
        },
    )?;

    eprintln!("  imported template {:?}", dt.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_repo_dir_finds_wsp_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        // Create a .wsp.yaml template file in the repo
        std::fs::write(
            repo_dir.join("dash.wsp.yaml"),
            "repos:\n  - url: git@github.com:acme/api.git\n",
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "dash");
        assert_eq!(found[0].status, DiscoveryStatus::New);
    }

    #[test]
    fn scan_repo_dir_skips_workspace_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        // .wsp.yaml is workspace metadata, not a template
        std::fs::write(
            repo_dir.join(".wsp.yaml"),
            "name: my-ws\nbranch: main\nrepos: {}\ncreated: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert!(found.is_empty());
    }

    #[test]
    fn scan_repo_dir_detects_already_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        let template_content = "repos:\n  - url: git@github.com:acme/api.git\n";
        let file_path = repo_dir.join("dash.wsp.yaml");
        std::fs::write(&file_path, template_content).unwrap();

        // Import it first
        let tmpl = template::load_from_file(&file_path).unwrap();
        template::save(&templates_dir, "dash", &tmpl).unwrap();
        template::save_source(
            &templates_dir,
            "dash",
            &template::ImportSource {
                source_path: file_path.to_string_lossy().to_string(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].status, DiscoveryStatus::AlreadyImported);
    }

    #[test]
    fn scan_repo_dir_detects_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        let file_path = repo_dir.join("dash.wsp.yaml");
        std::fs::write(&file_path, "repos:\n  - url: git@github.com:acme/api.git\n").unwrap();

        // Import the original
        let tmpl = template::load_from_file(&file_path).unwrap();
        template::save(&templates_dir, "dash", &tmpl).unwrap();
        template::save_source(
            &templates_dir,
            "dash",
            &template::ImportSource {
                source_path: file_path.to_string_lossy().to_string(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        // Now change the file
        std::fs::write(
            &file_path,
            "repos:\n  - url: git@github.com:acme/api.git\n  - url: git@github.com:acme/web.git\n",
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].status, DiscoveryStatus::Changed);
    }

    #[test]
    fn derive_name_from_file_uses_yaml_name() {
        let tmpl = template::Template {
            name: Some("my-template".into()),
            description: None,
            wsp_version: None,
            repos: vec![],
            config: None,
            agent_md: None,
        };
        let name = template::derive_name_from_file(Path::new("dash.wsp.yaml"), &tmpl);
        assert_eq!(name, "my-template");
    }

    #[test]
    fn derive_name_from_file_uses_filename_stem() {
        let tmpl = template::Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![],
            config: None,
            agent_md: None,
        };

        let cases = vec![
            ("dash.wsp.yaml", "dash"),
            ("backend.yaml", "backend"),
            ("my-template", "my-template"),
        ];

        for (filename, expected) in cases {
            let name = template::derive_name_from_file(Path::new(filename), &tmpl);
            assert_eq!(name, expected, "filename: {}", filename);
        }
    }

    #[test]
    fn scan_repo_dir_skips_invalid_names() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        // Template with path-traversal name inside YAML
        std::fs::write(
            repo_dir.join("evil.wsp.yaml"),
            "name: \"../../etc/passwd\"\nrepos:\n  - url: git@github.com:acme/api.git\n",
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert!(found.is_empty(), "should skip template with invalid name");
    }

    #[test]
    fn scan_repo_dir_skips_manually_created_template() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        // Create a template locally (no sidecar — manually created)
        let tmpl_content = "repos:\n  - url: git@github.com:acme/api.git\n";
        let tmpl: template::Template = serde_yaml_ng::from_str(tmpl_content).unwrap();
        template::save(&templates_dir, "dash", &tmpl).unwrap();

        // Same-named template exists in repo
        std::fs::write(repo_dir.join("dash.wsp.yaml"), tmpl_content).unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].status,
            DiscoveryStatus::AlreadyImported,
            "manually-created template should not be overwritten"
        );
    }

    #[test]
    fn scan_repo_dir_skips_different_source_template() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        let tmpl_content = "repos:\n  - url: git@github.com:acme/api.git\n";
        let file_path = repo_dir.join("dash.wsp.yaml");
        std::fs::write(&file_path, tmpl_content).unwrap();

        // Import from a different source path
        let tmpl: template::Template = serde_yaml_ng::from_str(tmpl_content).unwrap();
        template::save(&templates_dir, "dash", &tmpl).unwrap();
        template::save_source(
            &templates_dir,
            "dash",
            &template::ImportSource {
                source_path: "/some/other/path/dash.wsp.yaml".to_string(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        let found = scan_repo_dir(repo_dir, "github.com/acme/repo", &templates_dir);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].status,
            DiscoveryStatus::AlreadyImported,
            "template imported from different source should not be overwritten"
        );
    }

    #[test]
    fn prompt_and_import_filters_already_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let templates_dir = tmp.path().join("templates");

        let discovered = vec![DiscoveredTemplate {
            name: "already".to_string(),
            file_path: PathBuf::from("already.wsp.yaml"),
            repo_identity: "github.com/acme/repo".to_string(),
            status: DiscoveryStatus::AlreadyImported,
            content: None,
        }];

        // Should return 0 imports (all filtered out)
        let count = prompt_and_import(&discovered, &templates_dir).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn do_import_writes_template_and_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let templates_dir = tmp.path().join("templates");

        let tmpl_content = "repos:\n  - url: git@github.com:acme/api.git\n";
        let file_path = repo_dir.join("dash.wsp.yaml");
        std::fs::write(&file_path, tmpl_content).unwrap();

        let dt = DiscoveredTemplate {
            name: "dash".to_string(),
            file_path: file_path.clone(),
            repo_identity: "github.com/acme/repo".to_string(),
            status: DiscoveryStatus::New,
            content: None,
        };

        do_import(&dt, &templates_dir).unwrap();

        // Template should exist
        assert!(template::exists(&templates_dir, "dash"));

        // Sidecar should exist with correct source path
        let source = template::load_source(&templates_dir, "dash")
            .unwrap()
            .expect("sidecar should exist");
        assert_eq!(source.source_path, file_path.to_string_lossy().to_string());
    }

    #[test]
    fn do_import_from_bare_mirror_content() {
        let tmp = tempfile::tempdir().unwrap();
        let templates_dir = tmp.path().join("templates");

        let tmpl_content = "repos:\n  - url: git@github.com:acme/api.git\n";

        let dt = DiscoveredTemplate {
            name: "dash".to_string(),
            file_path: PathBuf::from("dash.wsp.yaml"),
            repo_identity: "github.com/acme/repo".to_string(),
            status: DiscoveryStatus::New,
            content: Some(tmpl_content.to_string()),
        };

        do_import(&dt, &templates_dir).unwrap();

        assert!(template::exists(&templates_dir, "dash"));

        let source = template::load_source(&templates_dir, "dash")
            .unwrap()
            .expect("sidecar should exist");
        assert_eq!(
            source.source_path,
            "mirror:github.com/acme/repo:dash.wsp.yaml"
        );
    }
}
