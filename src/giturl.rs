use std::path::PathBuf;

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

fn validate_component(s: &str, label: &str) -> Result<()> {
    if s.is_empty() {
        bail!("{} cannot be empty", label);
    }
    if s.contains("..") {
        bail!("{} contains unsafe path component \"..\": {:?}", label, s);
    }
    if s.starts_with('/') || s.ends_with('/') {
        bail!("{} contains leading/trailing slash: {:?}", label, s);
    }
    if s.contains('\0') {
        bail!("{} contains null byte: {:?}", label, s);
    }
    Ok(())
}

fn validate_parsed(p: &Parsed) -> Result<()> {
    validate_component(&p.host, "host")?;
    validate_component(&p.owner, "owner")?;
    validate_component(&p.repo, "repo")?;
    Ok(())
}

impl Parsed {
    pub fn identity(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
    }

    /// Parses an identity string (host/owner/repo) directly without URL round-trip.
    pub fn from_identity(identity: &str) -> Result<Self> {
        let parts: Vec<&str> = identity.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            bail!("invalid identity format: {}", identity);
        }
        let rest = parts[1];
        // Split owner from repo: last segment is repo, everything before is owner
        let parsed = match rest.rfind('/') {
            Some(i) => Parsed {
                host: parts[0].to_string(),
                owner: rest[..i].to_string(),
                repo: rest[i + 1..].to_string(),
            },
            None => bail!("invalid identity format (missing owner): {}", identity),
        };
        validate_parsed(&parsed)?;
        Ok(parsed)
    }

    pub fn mirror_path(&self) -> PathBuf {
        PathBuf::from(&self.host)
            .join(&self.owner)
            .join(format!("{}.git", self.repo))
    }

}

pub fn parse(raw_url: &str) -> Result<Parsed> {
    if raw_url.starts_with("git@") {
        parse_ssh(raw_url)
    } else {
        parse_https(raw_url)
    }
}

fn parse_ssh(raw: &str) -> Result<Parsed> {
    let without_prefix = raw.strip_prefix("git@").unwrap_or(raw);
    let parts: Vec<&str> = without_prefix.splitn(2, ':').collect();
    if parts.len() != 2 {
        bail!("invalid SSH URL: {}", raw);
    }

    let host = parts[0];
    let path = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 {
        bail!("invalid SSH URL path: {}", raw);
    }

    let parsed = Parsed {
        host: host.to_string(),
        owner: segments[..segments.len() - 1].join("/"),
        repo: segments[segments.len() - 1].to_string(),
    };
    validate_parsed(&parsed)?;
    Ok(parsed)
}

fn parse_https(raw: &str) -> Result<Parsed> {
    let u: url::Url = raw
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid URL: {}", e))?;

    let path = u.path().trim_start_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 {
        bail!("invalid URL path: {}", raw);
    }

    let parsed = Parsed {
        host: u.host_str().unwrap_or("").to_string(),
        owner: segments[..segments.len() - 1].join("/"),
        repo: segments[segments.len() - 1].to_string(),
    };
    validate_parsed(&parsed)?;
    Ok(parsed)
}

/// Computes the shortest unique suffix for each identity.
pub fn shortnames(identities: &[String]) -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();

    // Pre-split all identities once to avoid repeated allocations
    let split: Vec<Vec<&str>> = identities
        .iter()
        .map(|id| id.split('/').collect())
        .collect();

    for (idx, parts) in split.iter().enumerate() {
        let mut found = false;
        // Try progressively longer suffixes starting from just the repo name
        for depth in 1..=parts.len() {
            let candidate = &parts[parts.len() - depth..];
            let unique = split.iter().enumerate().all(|(j, other)| {
                j == idx || other.len() < depth || other[other.len() - depth..] != *candidate
            });
            if unique {
                result.insert(identities[idx].clone(), candidate.join("/"));
                found = true;
                break;
            }
        }
        if !found {
            result.insert(identities[idx].clone(), identities[idx].clone());
        }
    }

    result
}

/// Resolves a shortname/partial name to a full identity.
pub fn resolve(name: &str, identities: &[String]) -> Result<String> {
    // Exact match first
    for id in identities {
        if id == name {
            return Ok(id.clone());
        }
    }

    // Suffix match
    let mut matches = Vec::new();
    for id in identities {
        let parts: Vec<&str> = id.split('/').collect();
        for i in (0..parts.len()).rev() {
            let suffix = parts[i..].join("/");
            if suffix == name {
                matches.push(id.clone());
                break;
            }
        }
    }

    match matches.len() {
        0 => bail!("repo {:?} not found", name),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!(
            "repo {:?} is ambiguous, matches: {}",
            name,
            matches.join(", ")
        ),
    }
}

/// Splits a "repo@ref" argument into the repo name and ref.
/// Splits on the last "@" so repo names with "@" are handled.
pub fn parse_repo_ref(arg: &str) -> (&str, &str) {
    match arg.rfind('@') {
        Some(i) if i < arg.len() - 1 => (&arg[..i], &arg[i + 1..]),
        _ => (arg, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_parse() {
        let cases = vec![
            (
                "SSH github",
                "git@github.com:user/repo-a.git",
                Some(("github.com", "user", "repo-a")),
            ),
            (
                "SSH github without .git",
                "git@github.com:user/repo-a",
                Some(("github.com", "user", "repo-a")),
            ),
            (
                "HTTPS github",
                "https://github.com/user/repo-a.git",
                Some(("github.com", "user", "repo-a")),
            ),
            (
                "HTTPS github without .git",
                "https://github.com/user/repo-a",
                Some(("github.com", "user", "repo-a")),
            ),
            (
                "SSH gitlab",
                "git@gitlab.com:org/project.git",
                Some(("gitlab.com", "org", "project")),
            ),
            (
                "HTTPS bitbucket",
                "https://bitbucket.org/team/repo.git",
                Some(("bitbucket.org", "team", "repo")),
            ),
            (
                "SSH nested group",
                "git@gitlab.com:org/sub/project.git",
                Some(("gitlab.com", "org/sub", "project")),
            ),
            (
                "HTTPS nested path",
                "https://gitlab.com/org/sub/project.git",
                Some(("gitlab.com", "org/sub", "project")),
            ),
            ("invalid no path", "git@github.com:repo.git", None),
            (
                "path traversal SSH",
                "git@evil.com:../../etc/repo.git",
                None,
            ),
        ];
        for (name, url, want) in cases {
            let result = parse(url);
            match want {
                None => assert!(result.is_err(), "{}: expected error", name),
                Some((host, owner, repo)) => {
                    let got =
                        result.unwrap_or_else(|e| panic!("{}: unexpected error: {}", name, e));
                    assert_eq!(got.host, host, "{}", name);
                    assert_eq!(got.owner, owner, "{}", name);
                    assert_eq!(got.repo, repo, "{}", name);
                }
            }
        }
    }

    #[test]
    fn test_parsed_identity() {
        let p = Parsed {
            host: "github.com".into(),
            owner: "user".into(),
            repo: "repo-a".into(),
        };
        assert_eq!(p.identity(), "github.com/user/repo-a");
    }

    #[test]
    fn test_from_identity() {
        let cases = vec![
            (
                "standard",
                "github.com/user/repo-a",
                Some(("github.com", "user", "repo-a")),
            ),
            (
                "nested owner",
                "gitlab.com/org/sub/project",
                Some(("gitlab.com", "org/sub", "project")),
            ),
            ("no slash", "noslash", None),
            ("host only", "github.com/repo", None),
            ("empty", "", None),
            ("path traversal in owner", "github.com/../../etc/repo", None),
            ("path traversal in repo", "github.com/user/..", None),
        ];
        for (name, input, want) in cases {
            let result = Parsed::from_identity(input);
            match want {
                None => assert!(result.is_err(), "{}: expected error", name),
                Some((host, owner, repo)) => {
                    let got =
                        result.unwrap_or_else(|e| panic!("{}: unexpected error: {}", name, e));
                    assert_eq!(got.host, host, "{}", name);
                    assert_eq!(got.owner, owner, "{}", name);
                    assert_eq!(got.repo, repo, "{}", name);
                }
            }
        }
    }

    #[test]
    fn test_from_identity_roundtrip() {
        let identities = vec![
            "github.com/user/repo-a",
            "gitlab.com/org/sub/project",
            "bitbucket.org/team/repo",
        ];
        for id in identities {
            let parsed = Parsed::from_identity(id).unwrap();
            assert_eq!(parsed.identity(), id, "roundtrip failed for {}", id);
        }
    }

    #[test]
    fn test_parsed_mirror_path() {
        let p = Parsed {
            host: "github.com".into(),
            owner: "user".into(),
            repo: "repo-a".into(),
        };
        assert_eq!(
            p.mirror_path().to_str().unwrap(),
            "github.com/user/repo-a.git"
        );
    }

    #[test]
    fn test_shortnames() {
        let cases: Vec<(&str, Vec<&str>, HashMap<&str, &str>)> = vec![
            (
                "all unique repos",
                vec!["github.com/user/repo-a", "github.com/user/repo-b"],
                HashMap::from([
                    ("github.com/user/repo-a", "repo-a"),
                    ("github.com/user/repo-b", "repo-b"),
                ]),
            ),
            (
                "conflicting repo names",
                vec!["github.com/user/repo-a", "github.com/other/repo-a"],
                HashMap::from([
                    ("github.com/user/repo-a", "user/repo-a"),
                    ("github.com/other/repo-a", "other/repo-a"),
                ]),
            ),
            (
                "mixed unique and conflicting",
                vec![
                    "github.com/user/repo-a",
                    "github.com/other/repo-a",
                    "github.com/user/repo-b",
                ],
                HashMap::from([
                    ("github.com/user/repo-a", "user/repo-a"),
                    ("github.com/other/repo-a", "other/repo-a"),
                    ("github.com/user/repo-b", "repo-b"),
                ]),
            ),
            (
                "single repo",
                vec!["github.com/user/repo-a"],
                HashMap::from([("github.com/user/repo-a", "repo-a")]),
            ),
            ("empty", vec![], HashMap::new()),
        ];
        for (name, ids, want) in cases {
            let ids_owned: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            let got = shortnames(&ids_owned);
            let want_owned: HashMap<String, String> = want
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            assert_eq!(got, want_owned, "{}", name);
        }
    }

    #[test]
    fn test_resolve() {
        let identities: Vec<String> = vec![
            "github.com/user/repo-a",
            "github.com/other/repo-a",
            "github.com/user/repo-b",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let cases = vec![
            (
                "exact match",
                "github.com/user/repo-a",
                Ok("github.com/user/repo-a"),
                "",
            ),
            (
                "unique shortname",
                "repo-b",
                Ok("github.com/user/repo-b"),
                "",
            ),
            (
                "disambiguated by owner",
                "user/repo-a",
                Ok("github.com/user/repo-a"),
                "",
            ),
            (
                "disambiguated by other owner",
                "other/repo-a",
                Ok("github.com/other/repo-a"),
                "",
            ),
            ("ambiguous shortname", "repo-a", Err(()), "ambiguous"),
            ("not found", "repo-c", Err(()), "not found"),
        ];
        for (name, input, want, err_contains) in cases {
            let result = resolve(input, &identities);
            match want {
                Ok(expected) => {
                    let got =
                        result.unwrap_or_else(|e| panic!("{}: unexpected error: {}", name, e));
                    assert_eq!(got, expected, "{}", name);
                }
                Err(()) => {
                    let err = result.unwrap_err();
                    assert!(
                        err.to_string().contains(err_contains),
                        "{}: error {:?} should contain {:?}",
                        name,
                        err,
                        err_contains
                    );
                }
            }
        }
    }

    #[test]
    fn test_parse_repo_ref() {
        let cases = vec![
            ("no ref", "api-gateway", "api-gateway", ""),
            ("branch ref", "user-service@main", "user-service", "main"),
            ("tag ref", "proto@v1.0", "proto", "v1.0"),
            ("sha ref", "proto@abc123", "proto", "abc123"),
            (
                "full identity with ref",
                "github.com/acme/api@main",
                "github.com/acme/api",
                "main",
            ),
            ("trailing @", "repo@", "repo@", ""),
            (
                "multiple @ splits on last",
                "user@host/repo@main",
                "user@host/repo",
                "main",
            ),
            (
                "ssh url with ref",
                "git@github.com:user/repo@main",
                "git@github.com:user/repo",
                "main",
            ),
        ];
        for (name, input, want_name, want_ref) in cases {
            let (got_name, got_ref) = parse_repo_ref(input);
            assert_eq!(got_name, want_name, "{}", name);
            assert_eq!(got_ref, want_ref, "{}", name);
        }
    }
}
