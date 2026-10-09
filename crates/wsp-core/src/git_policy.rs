//! Invocation-owned Git presentation policy, independent of Git configuration.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

/// Parallel capture or exclusive native terminal interaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Parallel,
    Native,
}

impl Mode {
    pub const VALUES: [&'static str; 2] = ["parallel", "native"];

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "parallel" => Ok(Self::Parallel),
            "native" => Ok(Self::Native),
            _ => anyhow::bail!("progress mode must be 'parallel' or 'native'"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parallel => "parallel",
            Self::Native => "native",
        }
    }
}

#[derive(Clone)]
struct Policy {
    default: Mode,
    repositories: BTreeMap<String, Mode>,
    invocation: Option<Mode>,
    launcher: PathBuf,
    json: bool,
}

impl Policy {
    fn resolve(&self, repository: Option<&str>) -> Mode {
        self.invocation
            .or_else(|| repository.and_then(|id| self.repositories.get(id).copied()))
            .unwrap_or(self.default)
    }
}

static POLICY: OnceLock<Mutex<Option<Arc<Policy>>>> = OnceLock::new();
static NATIVE_TERMINAL: Mutex<()> = Mutex::new(());
thread_local! {
    static REPOSITORY: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn slot() -> &'static Mutex<Option<Arc<Policy>>> {
    POLICY.get_or_init(|| Mutex::new(None))
}

/// Install once at the CLI invocation boundary. Scoped workers share the snapshot.
pub fn install(
    default: Mode,
    repositories: BTreeMap<String, Mode>,
    invocation: Option<Mode>,
    launcher: PathBuf,
    json: bool,
) -> Installation {
    let policy = Arc::new(Policy {
        default,
        repositories,
        invocation,
        launcher,
        json,
    });
    let previous = slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(policy);
    Installation { previous }
}

pub struct Installation {
    previous: Option<Arc<Policy>>,
}
impl Drop for Installation {
    fn drop(&mut self) {
        *slot().lock().unwrap_or_else(|e| e.into_inner()) = self.previous.take();
    }
}

/// Select a repository from wsp's own identity at the operation boundary.
/// This scope must be installed inside a worker, since thread context is local.
pub fn repository(identity: &str) -> RepositoryScope {
    let previous = REPOSITORY.with(|slot| slot.replace(Some(identity.to_owned())));
    RepositoryScope {
        previous,
        thread: PhantomData,
    }
}

pub struct RepositoryScope {
    previous: Option<String>,
    thread: PhantomData<Rc<()>>,
}
impl Drop for RepositoryScope {
    fn drop(&mut self) {
        REPOSITORY.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

pub(crate) struct Execution {
    pub mode: Mode,
    pub launcher: Option<PathBuf>,
    pub detached: bool,
}

pub(crate) fn execution() -> Execution {
    let policy = slot().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match policy {
        Some(policy) => {
            let mode = REPOSITORY.with(|repo| policy.resolve(repo.borrow().as_deref()));
            Execution {
                mode,
                launcher: Some(policy.launcher.clone()),
                detached: mode == Mode::Parallel || policy.json,
            }
        }
        None => Execution {
            mode: Mode::Native,
            launcher: None,
            detached: false,
        },
    }
}

/// Native subprocesses take exclusive terminal ownership before clearing the UI.
pub(crate) fn native_terminal(execution: &Execution) -> Option<MutexGuard<'static, ()>> {
    (execution.mode == Mode::Native && execution.launcher.is_some())
        .then(|| NATIVE_TERMINAL.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_mode_precedes_repository_and_global_defaults() {
        for (default, repository, invocation, expected) in [
            (Mode::Parallel, None, None, Mode::Parallel),
            (Mode::Native, None, None, Mode::Native),
            (Mode::Parallel, Some(Mode::Native), None, Mode::Native),
            (Mode::Native, Some(Mode::Parallel), None, Mode::Parallel),
            (
                Mode::Parallel,
                Some(Mode::Native),
                Some(Mode::Parallel),
                Mode::Parallel,
            ),
            (
                Mode::Native,
                Some(Mode::Parallel),
                Some(Mode::Native),
                Mode::Native,
            ),
        ] {
            let policy = Policy {
                default,
                repositories: repository
                    .map(|mode| BTreeMap::from([("host/owner/repo".into(), mode)]))
                    .unwrap_or_default(),
                invocation,
                launcher: PathBuf::from("wsp"),
                json: false,
            };
            assert_eq!(policy.resolve(Some("host/owner/repo")), expected);
            assert_eq!(
                policy.resolve(Some("other/repo")),
                invocation.unwrap_or(default)
            );
        }
    }

    #[test]
    fn repository_scopes_restore_context_and_do_not_cross_threads() {
        assert!(REPOSITORY.with(|repo| repo.borrow().is_none()));
        let _outer = repository("outer");
        {
            let _inner = repository("inner");
            assert_eq!(
                REPOSITORY.with(|repo| repo.borrow().clone()),
                Some("inner".into())
            );
        }
        assert_eq!(
            REPOSITORY.with(|repo| repo.borrow().clone()),
            Some("outer".into())
        );
        std::thread::scope(|scope| {
            scope.spawn(|| assert!(REPOSITORY.with(|repo| repo.borrow().is_none())));
        });
    }
}
