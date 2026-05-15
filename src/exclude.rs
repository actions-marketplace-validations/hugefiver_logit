//! --exclude rule parsing and commit/contribution filtering.
//!
//! Format: `--exclude REPO[:QUAL:VALUE[,...]]`
//!
//! Examples:
//! - `--exclude my-repo`            → exclude entire repo
//! - `--exclude my-repo:lang:rust`  → exclude Rust in my-repo
//! - `--exclude my-repo:l:rust`     → same, short form
//! - `--exclude my-repo:path:docs/**` → exclude files under docs/ in my-repo
//! - `--exclude my-repo:l:md,p:*.md`  → exclude Markdown language AND *.md paths
//! - `--exclude repo1,repo2:lang:js`  → exclude repo1 entirely, plus JS in repo2

use glob::Pattern;
use regex::Regex;

use crate::stats::models::CommitStats;

/// A single exclusion rule parsed from one `--exclude` value.
#[derive(Debug, Clone)]
pub struct ExcludeRule {
    pub repo: Option<String>,
    pub lang: Option<String>,
    path_glob_raw: Option<String>,
    path_pattern: Option<Pattern>,
    path_regex: Option<Regex>,
}

impl ExcludeRule {
    /// Parse a single `--exclude` value into one or more rules.
    ///
    /// Comma-separated segments at top level create independent rules.
    /// Segments starting with a qualifier keyword (`lang:`, `l:`, `path:`, `p:`)
    /// attach to the preceding repo rule as an additional constraint.
    pub fn parse_many(value: &str) -> Vec<ExcludeRule> {
        let segments: Vec<&str> = value.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return Vec::new();
        }

        let mut rules: Vec<ExcludeRule> = Vec::new();

        for seg in &segments {
            if let Some(rest) = seg.strip_prefix("lang:")
                .or_else(|| seg.strip_prefix("language:"))
                .or_else(|| seg.strip_prefix("l:"))
            {
                if let Some(rule) = rules.last_mut() {
                    rule.lang = Some(rest.to_string());
                } else {
                    rules.push(ExcludeRule::new(None, Some(rest), None));
                }
            } else if let Some(rest) = seg.strip_prefix("path:")
                .or_else(|| seg.strip_prefix("p:"))
            {
                if let Some(rule) = rules.last_mut() {
                    rule.set_path(rest);
                } else {
                    rules.push(ExcludeRule::new(None, None, Some(rest)));
                }
            } else {
                let (repo_name, qualifiers) = split_first_colon(seg);
                let repo = if repo_name.is_empty() { None } else { Some(repo_name) };
                let mut rule = ExcludeRule::new(repo, None, None);
                parse_inline_qualifiers(&mut rule, &qualifiers);
                rules.push(rule);
            }
        }

        rules
    }

    fn new(repo: Option<&str>, lang: Option<&str>, path_glob: Option<&str>) -> Self {
        let mut rule = ExcludeRule {
            repo: repo.map(|s| s.to_string()),
            lang: lang.map(|s| s.to_string()),
            path_glob_raw: None,
            path_pattern: None,
            path_regex: None,
        };
        if let Some(g) = path_glob {
            rule.set_path(g);
        }
        rule
    }

    fn set_path(&mut self, glob_str: &str) {
        self.path_glob_raw = Some(glob_str.to_string());
        self.path_pattern = Pattern::new(glob_str).ok();
        self.path_regex = glob_to_regex(glob_str);
    }

    fn matches_repo(&self, repo_name: &str) -> bool {
        match &self.repo {
            Some(r) => repo_name == r.as_str()
                || repo_name.starts_with(&format!("{}/", r))
                || repo_name.ends_with(&format!("/{}", r)),
            None => true,
        }
    }

    fn matches_lang(&self, lang: &str) -> bool {
        self.lang.as_deref().is_some_and(|l| lang.eq_ignore_ascii_case(l))
    }

    fn matches_path(&self, path: &str) -> bool {
        if self.path_pattern.is_none() && self.path_regex.is_none() {
            return false;
        }
        let normalized = path.replace('\\', "/");
        if let Some(ref pat) = self.path_pattern {
            if pat.matches(&normalized) {
                return true;
            }
        }
        if let Some(ref re) = self.path_regex {
            if re.is_match(&normalized) {
                return true;
            }
        }
        false
    }

    pub fn is_repo_exclusion(&self) -> bool {
        self.lang.is_none() && self.path_pattern.is_none() && self.path_regex.is_none()
    }

    pub fn has_lang(&self) -> bool {
        self.lang.is_some()
    }

    pub fn has_path(&self) -> bool {
        self.path_pattern.is_some() || self.path_regex.is_some()
    }
}

fn split_first_colon(s: &str) -> (&str, &str) {
    match s.find(':') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    }
}

fn parse_inline_qualifiers(rule: &mut ExcludeRule, qualifiers: &str) {
    if qualifiers.is_empty() {
        return;
    }
    for part in qualifiers.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(rest) = part.strip_prefix("lang:")
            .or_else(|| part.strip_prefix("language:"))
            .or_else(|| part.strip_prefix("l:"))
        {
            rule.lang = Some(rest.to_string());
        } else if let Some(rest) = part.strip_prefix("path:")
            .or_else(|| part.strip_prefix("p:"))
        {
            rule.set_path(rest);
        } else if let Some((key, value)) = part.split_once(':') {
            match key {
                "lang" | "language" | "l" => rule.lang = Some(value.to_string()),
                "path" | "p" => rule.set_path(value),
                _ => {}
            }
        }
    }
}

fn glob_to_regex(glob: &str) -> Option<Regex> {
    if Pattern::new(glob).is_ok() {
        return None;
    }

    let mut regex_str = String::from("^");
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    regex_str.push_str(".*");
                    i += 2;
                    if i < chars.len() && chars[i] == '/' {
                        regex_str.push('/');
                        i += 1;
                    }
                    continue;
                }
                regex_str.push_str("[^/]*");
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                regex_str.push('\\');
                regex_str.push(chars[i]);
            }
            c => regex_str.push(c),
        }
        i += 1;
    }
    regex_str.push('$');
    Regex::new(&regex_str).ok()
}

pub fn filter_commits(commits: Vec<CommitStats>, rules: &[ExcludeRule]) -> Vec<CommitStats> {
    if rules.is_empty() {
        return commits;
    }

    commits
        .into_iter()
        .filter_map(|mut commit| {
            for rule in rules {
                if !rule.matches_repo(&commit.repo) {
                    continue;
                }
                if rule.is_repo_exclusion() {
                    return None;
                }
                if rule.has_lang() || rule.has_path() {
                    let before = commit.file_changes.len();
                    commit.file_changes.retain(|fc| {
                        let lang_match = rule.has_lang()
                            && fc.language.as_deref().is_some_and(|l| rule.matches_lang(l));
                        let path_match = rule.has_path() && rule.matches_path(&fc.path);
                        !(lang_match || path_match)
                    });
                    if commit.file_changes.is_empty() && before > 0 {
                        return None;
                    }
                }
            }
            Some(commit)
        })
        .collect()
}

pub fn is_repo_excluded(repo_name: &str, rules: &[ExcludeRule]) -> bool {
    rules.iter().any(|r| r.matches_repo(repo_name) && r.is_repo_exclusion())
}

pub fn excluded_langs_for_repo(repo_name: &str, rules: &[ExcludeRule]) -> Vec<String> {
    rules.iter()
        .filter(|r| r.matches_repo(repo_name) && r.has_lang())
        .filter_map(|r| r.lang.clone())
        .collect()
}

pub fn any_path_rules(rules: &[ExcludeRule]) -> bool {
    rules.iter().any(|r| r.has_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_repo() {
        let rules = ExcludeRule::parse_many("my-repo");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert!(rules[0].lang.is_none());
        assert!(rules[0].path_pattern.is_none());
    }

    #[test]
    fn parse_repo_with_lang() {
        let rules = ExcludeRule::parse_many("my-repo:lang:rust");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert_eq!(rules[0].lang.as_deref(), Some("rust"));
    }

    #[test]
    fn parse_repo_with_short_lang() {
        let rules = ExcludeRule::parse_many("my-repo:l:rust");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert_eq!(rules[0].lang.as_deref(), Some("rust"));
    }

    #[test]
    fn parse_repo_with_path() {
        let rules = ExcludeRule::parse_many("my-repo:path:src/**");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert!(rules[0].lang.is_none());
        assert!(rules[0].path_pattern.is_some());
    }

    #[test]
    fn parse_multiple_repos() {
        let rules = ExcludeRule::parse_many("repo1,repo2:lang:js");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].repo.as_deref(), Some("repo1"));
        assert!(rules[0].is_repo_exclusion());
        assert_eq!(rules[1].repo.as_deref(), Some("repo2"));
        assert_eq!(rules[1].lang.as_deref(), Some("js"));
    }

    #[test]
    fn parse_global_lang() {
        let rules = ExcludeRule::parse_many(":lang:markdown");
        assert_eq!(rules.len(), 1);
        assert!(rules[0].repo.is_none());
        assert_eq!(rules[0].lang.as_deref(), Some("markdown"));
    }

    #[test]
    fn parse_global_path() {
        let rules = ExcludeRule::parse_many(":p:**/*.md");
        assert_eq!(rules.len(), 1);
        assert!(rules[0].repo.is_none());
        assert!(rules[0].path_pattern.is_some());
    }

    #[test]
    fn repo_match_exact() {
        let rule = ExcludeRule::new(Some("my-repo"), None, None);
        assert!(rule.matches_repo("my-repo"));
        assert!(!rule.matches_repo("other-repo"));
    }

    #[test]
    fn repo_match_prefix() {
        let rule = ExcludeRule::new(Some("owner"), None, None);
        assert!(rule.matches_repo("owner/repo-name"));
        assert!(!rule.matches_repo("other/repo"));
    }

    #[test]
    fn global_rule_matches_all() {
        let rule = ExcludeRule::new(None, Some("rust"), None);
        assert!(rule.matches_repo("any-repo"));
        assert!(rule.matches_repo("owner/other"));
    }

    #[test]
    fn path_match_glob() {
        let mut rule = ExcludeRule::new(Some("repo"), None, None);
        rule.set_path("src/**");
        assert!(rule.matches_path("src/main.rs"));
        assert!(rule.matches_path("src/lib/mod.rs"));
        assert!(!rule.matches_path("tests/main.rs"));
    }

    #[test]
    fn path_match_wildcard() {
        let mut rule = ExcludeRule::new(Some("repo"), None, None);
        rule.set_path("*.md");
        assert!(rule.matches_path("README.md"));
        assert!(rule.matches_path("docs/guide.md"));
        assert!(!rule.matches_path("src/main.rs"));
    }
}
