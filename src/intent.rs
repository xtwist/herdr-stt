use std::collections::BTreeMap;

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use strsim::jaro_winkler;

use crate::config::WorkspaceConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Empty,
    Insert(String),
    SendPrompt(String),
    SubmitBuffer,
    EnablePlanMode(String),
    FocusWorkspace(String),
}

pub fn parse(text: &str) -> Intent {
    let clean = text.trim();
    if clean.is_empty() {
        return Intent::Empty;
    }

    let planning = Regex::new(r"(?iu)^планирование(?:[^\p{L}\p{N}]+(?P<body>.*))?$")
        .expect("valid planning prefix regex");
    if let Some(captures) = planning.captures(clean) {
        let body = captures
            .name("body")
            .map(|value| value.as_str().trim())
            .unwrap_or_default();
        return Intent::EnablePlanMode(body.to_string());
    }

    let normalized_command = normalize(clean);
    if matches!(
        normalized_command.as_str(),
        "режим планирования"
            | "включи режим планирования"
            | "переключись в режим планирования"
            | "plan mode"
            | "enable plan mode"
    ) {
        return Intent::EnablePlanMode(String::new());
    }

    let prompt = Regex::new(
        r"(?iu)^(?:отправь|запусти)\s+промпт(?:\s*[:—–,-]\s*|\s+)?(?P<body>.*)$|^(?:send|run)\s+prompt(?:\s*[:—–,-]\s*|\s+)?(?P<body_en>.*)$",
    )
    .expect("valid prompt regex");
    if let Some(captures) = prompt.captures(clean) {
        let body = capture_body(&captures);
        return if body.is_empty() {
            Intent::SubmitBuffer
        } else {
            Intent::SendPrompt(body.to_string())
        };
    }

    let workspace = Regex::new(
        r"(?iu)^(?:переключись[^\p{L}\p{N}]+на[^\p{L}\p{N}]+(?:воркспейс|workspace|workspice)|(?:переключи|открой)[^\p{L}\p{N}]+(?:воркспейс|workspace|workspice)|(?:workspace|workspice)|switch[^\p{L}\p{N}]+to[^\p{L}\p{N}]+(?:workspace|workspice))[^\p{L}\p{N}]+(?P<name>.+)$",
    )
    .expect("valid workspace regex");
    if let Some(captures) = workspace.captures(clean) {
        let name = captures
            .name("name")
            .map(|value| {
                value
                    .as_str()
                    .trim()
                    .trim_matches(|character: char| !character.is_alphanumeric())
            })
            .unwrap_or_default();
        if !name.is_empty() {
            return Intent::FocusWorkspace(name.to_string());
        }
    }

    Intent::Insert(clean.to_string())
}

fn capture_body<'a>(captures: &'a Captures<'a>) -> &'a str {
    captures
        .name("body")
        .or_else(|| captures.name("body_en"))
        .map(|value| value.as_str().trim())
        .unwrap_or_default()
}

pub fn apply_replacements(text: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut output = text.to_string();
    for (from, to) in replacements {
        if from.trim().is_empty() {
            continue;
        }
        let pattern = format!(r"(?iu)\b{}\b", regex::escape(from.trim()));
        if let Ok(regex) = Regex::new(&pattern) {
            output = regex
                .replace_all(&output, regex::NoExpand(to.as_str()))
                .into_owned();
        }
    }
    output
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
    pub number: u64,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceResolution {
    Match(Workspace),
    Ambiguous(Vec<Workspace>),
    NotFound,
}

pub fn resolve_workspace(
    query: &str,
    workspaces: &[Workspace],
    config: &WorkspaceConfig,
) -> WorkspaceResolution {
    if let Some(number) = workspace_number(query) {
        return workspaces
            .iter()
            .find(|workspace| workspace.number == number)
            .cloned()
            .map(WorkspaceResolution::Match)
            .unwrap_or(WorkspaceResolution::NotFound);
    }

    let query = normalize(query);
    if query.is_empty() {
        return WorkspaceResolution::NotFound;
    }

    let exact: Vec<_> = workspaces
        .iter()
        .filter(|workspace| {
            normalize(&workspace.label) == query
                || config
                    .aliases
                    .get(&workspace.label)
                    .is_some_and(|aliases| aliases.iter().any(|alias| normalize(alias) == query))
        })
        .cloned()
        .collect();
    match exact.len() {
        1 => return WorkspaceResolution::Match(exact[0].clone()),
        2.. => return WorkspaceResolution::Ambiguous(exact),
        _ => {}
    }

    let mut scored: Vec<(f64, Workspace)> = workspaces
        .iter()
        .map(|workspace| {
            let mut score = jaro_winkler(&query, &normalize(&workspace.label));
            if let Some(aliases) = config.aliases.get(&workspace.label) {
                for alias in aliases {
                    score = score.max(jaro_winkler(&query, &normalize(alias)));
                }
            }
            (score, workspace.clone())
        })
        .collect();
    scored.sort_by(|left, right| right.0.total_cmp(&left.0));

    let Some((best_score, best)) = scored.first() else {
        return WorkspaceResolution::NotFound;
    };
    if *best_score < config.fuzzy_threshold {
        return WorkspaceResolution::NotFound;
    }
    if let Some((second_score, second)) = scored.get(1)
        && best_score - second_score < config.fuzzy_margin
    {
        return WorkspaceResolution::Ambiguous(vec![best.clone(), second.clone()]);
    }
    WorkspaceResolution::Match(best.clone())
}

fn workspace_number(query: &str) -> Option<u64> {
    let regex = Regex::new(r"(?iu)^(?:номер|number)\s+(\d+)$").expect("valid number regex");
    regex
        .captures(query.trim())
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspaces() -> Vec<Workspace> {
        vec![
            Workspace {
                workspace_id: "w1".into(),
                label: "ProxyResponse API".into(),
                number: 1,
                focused: true,
            },
            Workspace {
                workspace_id: "w2".into(),
                label: "Papaproxy Rust Proxy".into(),
                number: 2,
                focused: false,
            },
        ]
    }

    #[test]
    fn parses_safe_default_as_insert() {
        assert_eq!(
            parse("проверь router и запусти тесты"),
            Intent::Insert("проверь router и запусти тесты".into())
        );
        assert_eq!(
            parse("проверь workspace Accessor"),
            Intent::Insert("проверь workspace Accessor".into())
        );
        assert_eq!(
            parse("составь план реализации"),
            Intent::Insert("составь план реализации".into())
        );
    }

    #[test]
    fn parses_prompt_commands() {
        assert_eq!(
            parse("Отправь промпт: запусти тесты"),
            Intent::SendPrompt("запусти тесты".into())
        );
        assert_eq!(parse("запусти промпт"), Intent::SubmitBuffer);
        assert_eq!(
            parse("send prompt review the diff"),
            Intent::SendPrompt("review the diff".into())
        );
    }

    #[test]
    fn parses_plan_mode_commands() {
        for command in [
            "Режим планирования",
            "Включи режим планирования",
            "Переключись в режим планирования",
            "PLAN MODE",
            "Enable plan mode",
            "ВКЛЮЧИ: РЕЖИМ-ПЛАНИРОВАНИЯ!",
        ] {
            assert_eq!(
                parse(command),
                Intent::EnablePlanMode(String::new()),
                "failed to parse: {command}"
            );
        }
    }

    #[test]
    fn planning_prefix_enables_plan_mode_and_removes_it_from_text() {
        for (command, expected) in [
            (
                "Планирование проверь текущую реализацию",
                "проверь текущую реализацию",
            ),
            (
                "ПЛАНИРОВАНИЕ, добавь тесты для router",
                "добавь тесты для router",
            ),
            ("Планирование — сравни два подхода.", "сравни два подхода."),
        ] {
            assert_eq!(
                parse(command),
                Intent::EnablePlanMode(expected.into()),
                "failed to parse: {command}"
            );
        }
    }

    #[test]
    fn parses_workspace_command() {
        for (command, expected) in [
            (
                "переключись на воркспейс ProxyResponse API",
                "ProxyResponse API",
            ),
            ("Переключи workspace ProxyResponse API", "ProxyResponse API"),
            ("Переключи воркспейс ProxyResponse API", "ProxyResponse API"),
            ("Открой workspace ProxyResponse API", "ProxyResponse API"),
            ("Workspace ProxyResponse API", "ProxyResponse API"),
            ("switch to workspace ProxyResponse API", "ProxyResponse API"),
            ("Workspace, Telegram", "Telegram"),
            ("Workspice Telegram", "Telegram"),
            ("wOrKsPaCe ::: [TeLeGrAm] !!!", "TeLeGrAm"),
            ("Переключи, WORKSPICE: Telegram!!!", "Telegram"),
        ] {
            assert_eq!(
                parse(command),
                Intent::FocusWorkspace(expected.into()),
                "failed to parse: {command}"
            );
        }
    }

    #[test]
    fn resolves_alias_and_number() {
        let mut config = WorkspaceConfig::default();
        config.aliases.insert(
            "Papaproxy Rust Proxy".into(),
            vec!["папа прокси раст".into()],
        );
        assert!(matches!(
            resolve_workspace("папа прокси раст", &workspaces(), &config),
            WorkspaceResolution::Match(workspace) if workspace.workspace_id == "w2"
        ));
        assert!(matches!(
            resolve_workspace("номер 1", &workspaces(), &config),
            WorkspaceResolution::Match(workspace) if workspace.workspace_id == "w1"
        ));
    }

    #[test]
    fn parses_and_resolves_workspace_ignoring_case_and_symbols() {
        let Intent::FocusWorkspace(query) = parse("WORKSPICE: [PAPAPROXY---RUST_PROXY]!!!") else {
            panic!("expected a workspace command");
        };

        assert!(matches!(
            resolve_workspace(&query, &workspaces(), &WorkspaceConfig::default()),
            WorkspaceResolution::Match(workspace) if workspace.workspace_id == "w2"
        ));
    }

    #[test]
    fn refuses_duplicate_exact_labels() {
        let mut items = workspaces();
        items.push(Workspace {
            workspace_id: "w3".into(),
            label: "Papaproxy Rust Proxy".into(),
            number: 3,
            focused: false,
        });
        assert!(matches!(
            resolve_workspace("Papaproxy Rust Proxy", &items, &WorkspaceConfig::default()),
            WorkspaceResolution::Ambiguous(matches) if matches.len() == 2
        ));
    }

    #[test]
    fn applies_case_insensitive_word_replacements() {
        let replacements = BTreeMap::from([("редис".into(), "Redis".into())]);
        assert_eq!(
            apply_replacements("проверь Редис lookup", &replacements),
            "проверь Redis lookup"
        );
    }
}
