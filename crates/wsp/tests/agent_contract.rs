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

/// Every source that defines a shape an agent can receive. `help.rs` counts:
/// `wsp help <topic> --json` serializes its own structs, whose fields are not
/// `pub`, so scoping this to one file or to public fields would leave a blind
/// spot rather than a registered gap.
const SOURCES: &[(&str, &str)] = &[
    (
        "wsp-core/output",
        include_str!("../../wsp-core/src/output.rs"),
    ),
    ("cli/help", include_str!("../src/cli/help.rs")),
];

/// What agents are told to read.
const CONTRACT: &str = include_str!("../../../skills/wsp-manage/SKILL.md");

/// Fields no sample shows. **Empty, and worth keeping that way**: a field agents
/// never see is a field they will not handle.
///
/// Only add an entry if a sample genuinely cannot reach the field, and say why.
/// Every one of the 23 that were here got there for one of three reasons, and
/// all three were fixable by editing a sample rather than by registering it:
///
/// 1. the type had no sample at all
/// 2. the type sat inside a collection its parent's sample left empty
/// 3. `skip_serializing_if` hid the field because the sample used the default
const INVISIBLE: &[&str] = &[];

/// The direct fields of each `Serialize` struct, keyed by type name.
///
/// Direct only: following `Vec<RepoStatusEntry>` into its element type would
/// need a type graph, and a nested type that has its own section is checked
/// there instead. One that has no section is still covered by the file-wide
/// check, which is why both exist.
fn fields_by_type(source: &str) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    let mut by_type: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for entry in contract_fields(source) {
        let (ty, field) = entry.split_once('.').expect("Struct.field");
        by_type
            .entry(ty.to_string())
            .or_default()
            .insert(field.to_string());
    }
    by_type
}

/// Each `<!-- type: T -->` marker and the JSON keys of the block that follows.
fn sections(contract: &str) -> Vec<(String, BTreeSet<String>)> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in contract.lines() {
        if let Some(rest) = line.trim().strip_prefix("<!-- type: ")
            && let Some(ty) = rest.strip_suffix(" -->")
        {
            if let Some((ty, body)) = current.take() {
                out.push((
                    ty,
                    documented_keys(&body)
                        .iter()
                        .map(|k| k.to_string())
                        .collect(),
                ));
            }
            current = Some((ty.to_string(), String::new()));
            continue;
        }
        if line.starts_with("### ")
            && let Some((ty, body)) = current.take()
        {
            out.push((
                ty,
                documented_keys(&body)
                    .iter()
                    .map(|k| k.to_string())
                    .collect(),
            ));
        }
        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((ty, body)) = current {
        out.push((
            ty,
            documented_keys(&body)
                .iter()
                .map(|k| k.to_string())
                .collect(),
        ));
    }
    out
}

/// `Struct.field` for every field of every `Serialize` struct.
///
/// Keyed on the derive rather than on `pub`, because what makes a field part of
/// the contract is that serde writes it. Fields marked `#[serde(skip)]` are not
/// written, so they are not part of it.
fn contract_fields(source: &str) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    let mut serializable = false;
    let mut current: Option<&str> = None;
    let mut skip_next = false;
    let mut rename_next: Option<String> = None;

    for line in source.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("#[derive") {
            serializable = trimmed.contains("Serialize");
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("pub struct ")
            .or_else(|| trimmed.strip_prefix("struct "))
        {
            current = if serializable {
                rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .filter(|n| !n.is_empty())
            } else {
                None
            };
            serializable = false;
            continue;
        }
        if line == "}" {
            current = None;
            continue;
        }
        if trimmed.starts_with("#[serde(skip)]") {
            skip_next = true;
            continue;
        }
        // The JSON key is the rename, not the Rust name, so comparing the field
        // name would report a documented field as missing.
        if let Some(rest) = trimmed.strip_prefix("#[serde(rename = \"")
            && let Some(name) = rest.split('"').next()
        {
            rename_next = Some(name.to_string());
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.starts_with("//") {
            continue;
        }

        if let Some(struct_name) = current {
            let field = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
            if let Some(name) = field.split(':').next()
                && !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && field.contains(':')
            {
                if !skip_next {
                    let key = rename_next.as_deref().unwrap_or(name);
                    fields.insert(format!("{struct_name}.{key}"));
                }
                skip_next = false;
                rename_next = None;
            }
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
    let fields: BTreeSet<String> = SOURCES
        .iter()
        .flat_map(|(_, src)| contract_fields(src))
        .collect();
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

/// A field must be visible in *its own* type's section, not merely somewhere in
/// the file. Without this, moving a field between types keeps the file-wide
/// check happy while the section an agent actually reads loses it.
///
/// Direct fields only. A nested type either has its own section, and is checked
/// here, or has none, and is covered by the file-wide check above.
#[test]
fn each_section_shows_its_own_types_fields() {
    let by_type: std::collections::BTreeMap<String, BTreeSet<String>> = SOURCES
        .iter()
        .flat_map(|(_, src)| fields_by_type(src))
        .collect();

    let found = sections(CONTRACT);
    assert!(
        found.len() > 15,
        "expected the type markers, found {} sections",
        found.len()
    );

    let mut missing = Vec::new();
    for (ty, keys) in &found {
        let Some(expected) = by_type.get(ty) else {
            continue;
        };
        for field in expected.difference(keys) {
            // A type with more than one sample shows different shapes in each,
            // so a field absent here may be present in a sibling section.
            let elsewhere = found
                .iter()
                .any(|(other_ty, other)| other_ty == ty && other.contains(field));
            if !elsewhere {
                missing.push(format!("{ty}.{field}"));
            }
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "these fields are missing from their own type's section: {missing:?}\n  \
         A field documented only under another type is one an agent reading this \
         command's output will not find."
    );
}

/// The register is only meaningful if the parser applies serde's rules. A parser
/// that silently matches nothing, or that reports fields serde never writes,
/// would make the guard above pass while proving nothing.
#[test]
fn the_parser_applies_serdes_rules() {
    let found = contract_fields(
        "#[derive(Serialize)]\n\
         pub struct Shown {\n\
         \x20   pub plain: String,\n\
         \x20   #[serde(rename = \"renamed_key\")]\n\
         \x20   pub renamed: String,\n\
         \x20   #[serde(skip)]\n\
         \x20   pub never_written: String,\n\
         }\n\
         #[derive(Debug)]\n\
         pub struct NotSerialized {\n\
         \x20   pub invisible: String,\n\
         }\n",
    );
    let found: Vec<&str> = found.iter().map(String::as_str).collect();

    assert!(found.contains(&"Shown.plain"), "{found:?}");
    // The JSON key is the rename, so that is what must be looked for.
    assert!(found.contains(&"Shown.renamed_key"), "{found:?}");
    assert!(
        !found.contains(&"Shown.renamed"),
        "used the Rust name: {found:?}"
    );
    // serde never writes these, so they are not part of the contract.
    assert!(!found.contains(&"Shown.never_written"), "{found:?}");
    // Nor is a struct serde does not serialize at all.
    assert!(!found.contains(&"NotSerialized.invisible"), "{found:?}");
}

#[test]
fn the_key_parser_reads_json_keys_only() {
    let keys = documented_keys("{\n  \"visible\": 1,\n  \"nested\": {\"deep\": 2}\n}");
    assert!(
        keys.contains("visible") && keys.contains("deep"),
        "{keys:?}"
    );
    // A quoted string that is not a key must not count.
    assert!(!documented_keys("\"just a value\"").contains("just a value"));
}
