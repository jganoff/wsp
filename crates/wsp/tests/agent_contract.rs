//! Every field an agent can receive must be visible in the contract it reads.
//!
//! `AGENTS.md` points agents at `skills/wsp-manage/SKILL.md` as the source of
//! truth for CLI output, and "structured output is the contract" is a design
//! tenet. A field that never appears in a sample is invisible: an agent cannot
//! guess a shape it has never seen, so it will not look for the field and will
//! not handle it.
//!
//! Fields hidden by `skip_serializing_if` are the easy ones to lose, because
//! they disappear whenever the sample happens to use the default. Three have
//! already shipped that way: `state` and `size_bytes` on the workspace listing,
//! and `signal` on `wsp exec`. Each was found by accident.
//!
//! Read against the committed `SKILL.md` rather than a freshly generated one, so
//! this runs without the `codegen` feature. `just ci` already fails when the
//! committed file is stale, which makes the two equivalent.

use std::collections::BTreeSet;

/// The JSON contract: every `pub` field of every struct in `wsp-core`'s output
/// module.
const OUTPUT_SOURCE: &str = include_str!("../../wsp-core/src/output.rs");

/// What agents are told to read.
const CONTRACT: &str = include_str!("../../../skills/wsp-manage/SKILL.md");

/// Fields no sample currently shows, and why. A debt register, not an allowance:
/// the assertion below is an equality, so it fails both when a new field goes
/// missing and when a listed one becomes visible. It only shrinks.
///
/// Three causes, all fixable by enriching a sample:
///
/// 1. The type has no sample at all, so none of its fields appear.
/// 2. The type is nested inside a collection its parent's sample leaves empty.
/// 3. `skip_serializing_if` hides the field because the sample uses the default
///    (`None`, `false`, an empty `Vec`).
const INVISIBLE: &[&str] = &[
    // `wsp config ls` has no sample, so neither the output nor its entries appear.
    "ConfigListEntry.experimental",
    "ConfigListOutput.entries",
    // `wsp repo setup-commands` has no sample.
    "SetupCommandEntry.command",
    "SetupCommandsOutput.commands",
    "SetupCommandsOutput.repo",
    // Sampled parents whose collections are empty, so the nested shape is unseen.
    "ImportOutput.failed",
    "SyncRepoResult.repo_dir",
    "SyncRepoResult.strategy",
    "SyncRepoResult.target",
    // Present only in the shape the sample does not use: `wsp log --oneline`
    // sets `raw`, the default sets the structured commit fields.
    "LogCommit.timestamp",
    "LogOutput.oneline",
    "RepoLogEntry.raw",
    // Skipped because the sample uses the default.
    "ExecRepoResult.stderr",
    "MutationOutput.duration_ms",
    "RepoStatusEntry.expected_branch",
    "RepoStatusEntry.files",
    // Pull-request detail, absent unless PR integration is on.
    "PrInfo.is_draft",
    "PrInfo.number",
    "PrInfo.title",
    "RepoStatusEntry.pr",
    "StatusOutput.pr_enabled",
    "StatusOutput.root",
    "StatusOutput.verbose",
];

/// `pub <name>:` for every struct in the output module, as `Struct.field`.
fn contract_fields(source: &str) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    let mut current: Option<&str> = None;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            current = rest.split_whitespace().next();
            continue;
        }
        // Struct bodies are the only place indented `pub <name>:` appears; a
        // closing brace at column zero ends one.
        if line == "}" {
            current = None;
            continue;
        }
        if let Some(struct_name) = current
            && let Some(rest) = trimmed.strip_prefix("pub ")
            && let Some(field) = rest.split(':').next()
            && !field.is_empty()
            && field.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            fields.insert(format!("{struct_name}.{field}"));
        }
    }
    fields
}

/// Field names appearing as JSON keys anywhere in the contract.
fn documented_keys(contract: &str) -> BTreeSet<&str> {
    let mut keys = BTreeSet::new();
    let mut rest = contract;
    while let Some(at) = rest.find('"') {
        rest = &rest[at + 1..];
        let Some(end) = rest.find('"') else { break };
        let (name, after) = (&rest[..end], &rest[end + 1..]);
        if after.starts_with(':') && !name.is_empty() {
            keys.insert(name);
        }
        rest = after;
    }
    keys
}

#[test]
fn every_output_field_is_visible_in_the_agent_contract() {
    let fields = contract_fields(OUTPUT_SOURCE);
    assert!(
        fields.len() > 100,
        "expected to find the output structs, found {} fields; the parser has \
         probably drifted from the source layout",
        fields.len()
    );

    let documented = documented_keys(CONTRACT);
    let invisible: Vec<&str> = fields
        .iter()
        .filter(|f| {
            let field = f.split('.').nth(1).expect("Struct.field");
            !documented.contains(field)
        })
        .map(String::as_str)
        .collect();

    let expected: BTreeSet<&str> = INVISIBLE.iter().copied().collect();
    let found: BTreeSet<&str> = invisible.iter().copied().collect();

    let new_gaps: Vec<&&str> = found.difference(&expected).collect();
    let now_visible: Vec<&&str> = expected.difference(&found).collect();

    assert!(
        new_gaps.is_empty() && now_visible.is_empty(),
        "the agent contract and the INVISIBLE register disagree.\n  \
         no sample shows these, and they are not registered: {new_gaps:?}\n  \
         registered but now visible, so delete their lines: {now_visible:?}\n  \
         A field agents never see is a field they will not handle. Give it a \
         value in a `sample()` in wsp-core/src/output.rs and run `just skill`."
    );
}

/// The register is only meaningful if the parser can actually find fields and
/// keys. Both halves are asserted so a parser that silently matches nothing
/// cannot make this suite pass.
#[test]
fn the_parsers_find_what_they_are_looking_for() {
    let fields =
        contract_fields("pub struct Thing {\n    pub visible: String,\n    pub other: u64,\n}\n");
    assert_eq!(
        fields.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["Thing.other", "Thing.visible"]
    );

    let keys = documented_keys("{\n  \"visible\": 1,\n  \"nested\": {\"deep\": 2}\n}");
    assert!(
        keys.contains("visible") && keys.contains("deep"),
        "{keys:?}"
    );
    // A quoted string that is not a key must not count.
    assert!(!documented_keys("\"just a value\"").contains("just a value"));
}
