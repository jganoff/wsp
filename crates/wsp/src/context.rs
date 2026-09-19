//! Invocation ownership is resolved before optional machine-global state.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ArgMatches;
use wsp_core::config::{self, Availability, Config, GlobalCapabilities, Paths};
use wsp_core::workspace::{self, Metadata};

pub struct InvocationContext {
    pub workspace: Option<PathBuf>,
    pub metadata: Option<Metadata>,
    pub paths: Option<Paths>,
    pub config: Config,
    pub global_state: Availability,
    pub capabilities: GlobalCapabilities,
    pub global_error: Option<String>,
}

impl InvocationContext {
    pub fn resolve(matches: &ArgMatches) -> Result<Self> {
        let cwd = crate::shellcd::invocation_dir()?;
        let detected = detect_workspace(&cwd)?;
        let (workspace, metadata) = match detected {
            Some((path, meta)) => (Some(path), Some(meta)),
            None => (None, None),
        };
        let mut context = Self {
            workspace,
            metadata,
            paths: None,
            config: Config::default(),
            global_state: Availability::Absent,
            capabilities: GlobalCapabilities::unavailable(Availability::Absent),
            global_error: None,
        };
        let data = match config::data_dir() {
            Ok(data) => data,
            Err(_) => return Ok(context),
        };
        let config_path = data.join("config.yaml");
        let config_present = match std::fs::metadata(&config_path) {
            Ok(_) => true,
            Err(err) if err.kind() == ErrorKind::NotFound => false,
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                context.global_state = Availability::Unavailable;
                context.capabilities = GlobalCapabilities::unavailable(Availability::Unavailable);
                return Ok(context);
            }
            Err(err) => return Err(err).context("inspecting global configuration"),
        };
        context.config = match Config::load_from(&config_path) {
            Ok(cfg) => cfg,
            Err(err)
                if err
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == ErrorKind::PermissionDenied) =>
            {
                context.global_state = Availability::Unavailable;
                context.capabilities = GlobalCapabilities::unavailable(Availability::Unavailable);
                return Ok(context);
            }
            Err(err) if matches.subcommand_name() == Some("doctor") => {
                context.global_error = Some(format!("invalid global configuration: {err:#}"));
                context.global_state = Availability::Malformed;
                context.capabilities = GlobalCapabilities::unavailable(Availability::Unavailable);
                return Ok(context);
            }
            Err(err) => return Err(err).context("invalid global configuration"),
        };
        let validation = (|| -> Result<()> {
            if context.config.version > config::CURRENT_CONFIG_VERSION {
                bail!(
                    "global configuration version {} is unsupported; upgrade wsp before using this configuration",
                    context.config.version
                );
            }
            if let Some(git_config) = &context.config.git_config {
                for key in git_config.keys() {
                    config::validate_git_config_key(key)?;
                }
            }
            Ok(())
        })();
        if let Err(err) = validation {
            if matches.subcommand_name() != Some("doctor") {
                return Err(err).context("invalid global configuration");
            }
            context.global_error = Some(format!("invalid global configuration: {err:#}"));
            context.global_state = Availability::Malformed;
            context.capabilities = GlobalCapabilities::unavailable(Availability::Unavailable);
            context.config = Config::default();
            return Ok(context);
        }
        let workspaces = match &context.config.workspaces_dir {
            Some(dir) => PathBuf::from(dir),
            None => match config::default_workspaces_dir() {
                Ok(dir) => dir,
                Err(_) => return Ok(context),
            },
        };
        let paths = Paths::from_dirs(&data, &workspaces);
        context.capabilities = GlobalCapabilities::inspect(&paths);
        context.global_state = if config_present {
            context.capabilities.registry
        } else {
            Availability::Absent
        };
        context.paths = Some(paths);
        Ok(context)
    }

    pub fn output_context(&self, workspace: &Path) -> wsp_core::output::InvocationContextOutput {
        wsp_core::output::InvocationContextOutput {
            workspace: workspace.display().to_string(),
            mode: if self.is_workspace_local() {
                "workspace_local"
            } else {
                "host"
            }
            .into(),
            global_state: self.global_state,
            global_reason: self.global_error.clone(),
        }
    }

    pub fn direct_transport_reason(&self) -> Option<&'static str> {
        if self.allows_mirror_write() {
            return None;
        }
        Some(match self.capabilities.mirrors {
            Availability::Absent => "mirror_store_absent",
            Availability::Unavailable => "mirror_store_unavailable",
            Availability::ReadOnly => "mirror_store_read_only",
            Availability::Malformed => "mirror_store_malformed",
            Availability::Unknown => "mirror_store_unknown",
            Availability::Available => "mirror_write_not_authorized",
        })
    }

    pub fn is_workspace_local(&self) -> bool {
        self.workspace.is_some() && self.global_state != Availability::Available
    }

    /// A workspace may use a global registry while its mirror store is absent
    /// or inaccessible. Mirror refresh is independently gated because it
    /// writes the shared cache; callers then use the clone's origin directly.
    pub fn allows_mirror_write(&self) -> bool {
        !self.is_workspace_local() && self.capabilities.mirrors == Availability::Available
    }

    pub fn require_host_paths(&self) -> Result<&Paths> {
        if self.is_workspace_local() {
            bail!(
                "this command requires available global wsp state; run it on the host with access to the global registry"
            );
        }
        self.paths.as_ref().context(
            "this command requires global wsp paths; configure HOME or XDG_DATA_HOME on the host",
        )
    }

    pub fn workspace_dir(&self, name: Option<&str>) -> Result<PathBuf> {
        if let Some(name) = name {
            workspace::validate_name(name)?;
            if self.metadata.as_ref().is_some_and(|meta| meta.name == name) {
                return Ok(self.workspace.clone().expect("metadata has workspace"));
            }
            return Ok(workspace::dir(
                &self.require_host_paths()?.workspaces_dir,
                name,
            ));
        }
        self.workspace
            .clone()
            .context("not in a workspace (no .wsp.yaml found)")
    }

    /// Read commands and portable local mutations never persist advice state.
    pub fn allows_global_advice(&self, command: &str) -> bool {
        !self.is_workspace_local()
            && matches!(
                command,
                "new"
                    | "rm"
                    | "rename"
                    | "recover"
                    | "setup"
                    | "init"
                    | "registry"
                    | "template"
                    | "config"
            )
    }
}

fn detect_workspace(start: &Path) -> Result<Option<(PathBuf, Metadata)>> {
    for directory in start.ancestors() {
        let path = directory.join(workspace::METADATA_FILE);
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&data)
            .with_context(|| format!("invalid workspace metadata at {}", path.display()))?;
        // Repository templates share the filename but have no workspace branch.
        if value.get("branch").is_none() && value.get("created").is_none() {
            continue;
        }
        let meta = workspace::load_metadata(directory)?;
        if meta.version > workspace::CURRENT_METADATA_VERSION {
            bail!(
                "workspace metadata version {} is unsupported; upgrade wsp",
                meta.version
            );
        }
        return Ok(Some((directory.to_path_buf(), meta)));
    }
    Ok(None)
}
