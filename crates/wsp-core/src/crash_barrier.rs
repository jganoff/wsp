//! Private, test-only process barriers for crash-recovery integration tests.
//!
//! This module is compiled only by the `just crash-test` configuration. It is
//! intentionally absent from ordinary product artifacts.

use std::collections::BTreeMap;
use std::env;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
const CATALOG_VERSION: u32 = 4;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_SELECTIONS: usize = 16;
const MAX_TEXT_BYTES: usize = 256;
const WATCHDOG: Duration = Duration::from_secs(90);

const ADDR: &str = "WSP_TEST_CRASH_ADDR";
const TOKEN: &str = "WSP_TEST_CRASH_TOKEN";
const SESSION: &str = "WSP_TEST_CRASH_SESSION";

static SESSION_STATE: OnceLock<Option<Session>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Add,
    Remove,
    Describe,
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Point {
    StageCreated,
    CloneStaged,
    AddRechecked,
    ClonePublished,
    AdoptionValidated,
    MembershipCommitted,
    MemberAlreadyPresent,
    RemoveRechecked,
    CloneQuarantined,
    CloneDeleted,
    MissingCloneConfirmed,
    RemovalCommitted,
    DescriptionCommitted,
    GuidanceSnapshot,
    GuidanceAgentsCommitted,
    GuidanceComplete,
    AddAdmitted,
    MirrorPrepared,
    RegistryCommitted,
    RefreshSelected,
    RefreshComplete,
    MetadataReplacePending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub operation: Operation,
    pub repository: String,
    pub point: Point,
    pub occurrence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reached {
    pub session: String,
    pub sequence: u64,
    pub operation: Operation,
    pub repository: String,
    pub point: Point,
    pub occurrence: u32,
    pub lock_held: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        version: u32,
        catalog_version: u32,
        session: String,
        token: String,
        pid: u32,
    },
    Select {
        version: u32,
        session: String,
        token: String,
        selections: Vec<Selection>,
    },
    Reached(Reached),
    Continue {
        session: String,
        sequence: u64,
    },
    Fail {
        session: String,
        sequence: u64,
    },
}

struct Session {
    stream: std::sync::Mutex<TcpStream>,
    session: String,
    selections: Vec<Selection>,
    sequence: std::sync::Mutex<u64>,
    occurrences: std::sync::Mutex<BTreeMap<(Operation, String, Point), u32>>,
}

/// Initializes the optional controller session before CLI mutation. With none
/// of the private variables present this is a no-op; partial configuration is
/// rejected before the command reaches product code.
pub fn initialize() -> Result<()> {
    if SESSION_STATE.get().is_some() {
        return Ok(());
    }
    let configured = [ADDR, TOKEN, SESSION]
        .into_iter()
        .map(|name| (name, env::var(name).ok()))
        .collect::<Vec<_>>();
    let populated = configured
        .iter()
        .filter(|(_, value)| value.is_some())
        .count();
    if populated == 0 {
        let _ = SESSION_STATE.set(None);
        return Ok(());
    }
    if populated != configured.len() {
        bail!("incomplete test crash-barrier configuration");
    }
    let address = configured[0].1.as_deref().unwrap();
    let token = configured[1].1.as_deref().unwrap();
    let session = configured[2].1.as_deref().unwrap();
    validate_text("token", token)?;
    validate_text("session", session)?;
    let address: SocketAddr = address
        .parse()
        .context("invalid test crash-barrier address")?;
    if !matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback()) {
        bail!("test crash-barrier address must be a literal IPv4 loopback address");
    }
    let mut stream = TcpStream::connect_timeout(&address, WATCHDOG)
        .context("connecting test crash-barrier controller")?;
    stream
        .set_read_timeout(Some(WATCHDOG))
        .context("setting test crash-barrier read timeout")?;
    stream
        .set_write_timeout(Some(WATCHDOG))
        .context("setting test crash-barrier write timeout")?;
    write_message(
        &mut stream,
        &Message::Hello {
            version: PROTOCOL_VERSION,
            catalog_version: CATALOG_VERSION,
            session: session.to_owned(),
            token: token.to_owned(),
            pid: std::process::id(),
        },
    )?;
    let Message::Select {
        version,
        session: selected_session,
        token: selected_token,
        selections,
    } = read_message(&mut stream).context("reading test crash-barrier selection")?
    else {
        bail!("test crash-barrier controller did not send a selection");
    };
    if version != PROTOCOL_VERSION || selected_session != session || selected_token != token {
        bail!("test crash-barrier controller selection did not match this session");
    }
    if selections.len() > MAX_SELECTIONS {
        bail!("test crash-barrier controller selected too many barriers");
    }
    for selected in &selections {
        validate_repository(&selected.repository)?;
        if selected.occurrence == 0 {
            bail!("test crash-barrier occurrence must be non-zero");
        }
    }
    let _ = SESSION_STATE.set(Some(Session {
        stream: std::sync::Mutex::new(stream),
        session: session.to_owned(),
        selections,
        sequence: std::sync::Mutex::new(0),
        occurrences: std::sync::Mutex::new(BTreeMap::new()),
    }));
    Ok(())
}

/// Announces a completed persistent boundary and waits only when the controller
/// selected this operation/repository/point/occurrence.
pub fn reach(operation: Operation, repository: &str, point: Point, lock_held: bool) -> Result<()> {
    let Some(Some(state)) = SESSION_STATE.get() else {
        return Ok(());
    };
    let occurrence = {
        let mut occurrences = state
            .occurrences
            .lock()
            .expect("crash barrier occurrence lock poisoned");
        let key = (operation, repository.to_owned(), point);
        let entry = occurrences.entry(key).or_insert(0);
        *entry += 1;
        *entry
    };
    if !state.selections.iter().any(|selection| {
        selection.operation == operation
            && selection.repository == repository
            && selection.point == point
            && selection.occurrence == occurrence
    }) {
        return Ok(());
    }
    let sequence = {
        let mut sequence = state
            .sequence
            .lock()
            .expect("crash barrier sequence lock poisoned");
        *sequence += 1;
        *sequence
    };
    let reached = Reached {
        session: state.session.clone(),
        sequence,
        operation,
        repository: repository.to_owned(),
        point,
        occurrence,
        lock_held,
    };
    let mut stream = state
        .stream
        .lock()
        .expect("crash barrier stream lock poisoned");
    write_message(&mut stream, &Message::Reached(reached))?;
    match read_message(&mut stream).context("reading test crash-barrier response")? {
        Message::Continue {
            session,
            sequence: reply,
        } if session == state.session && reply == sequence => Ok(()),
        Message::Fail {
            session,
            sequence: reply,
        } if session == state.session && reply == sequence => {
            if !allows_fail(point) {
                bail!("test crash-barrier Fail is not permitted at {point:?}");
            }
            bail!("synthetic test crash-barrier failure at {point:?}")
        }
        _ => bail!("invalid test crash-barrier response"),
    }
}

fn allows_fail(point: Point) -> bool {
    matches!(
        point,
        Point::MetadataReplacePending | Point::RefreshSelected
    )
}

pub fn write_message(stream: &mut TcpStream, message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        bail!("test crash-barrier message is too large");
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

pub fn read_message(stream: &mut TcpStream) -> Result<Message> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_MESSAGE_BYTES {
        bail!("invalid test crash-barrier message size");
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_text(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_TEXT_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid test crash-barrier {name}");
    }
    Ok(())
}

fn validate_repository(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_TEXT_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        bail!("invalid test crash-barrier repository");
    }
    Ok(())
}
