use super::{TextEdit, normalize_words, phonetic_normalize, split_identifier};
use crate::{
    intent::{Intent, parse},
    vocabulary::ProjectVocabulary,
};
use regex::Regex;
use std::collections::HashSet;
use strsim::normalized_levenshtein;

pub(super) fn apply_validated_edits(
    original: &str,
    edits: Vec<TextEdit>,
    vocabulary: &ProjectVocabulary,
    max_edits: usize,
    protect_intent: bool,
) -> (String, usize, usize) {
    let known = vocabulary
        .terms
        .iter()
        .map(|term| term.to_lowercase())
        .collect::<HashSet<_>>();
    let mut output = original.to_string();
    let mut applied = 0usize;
    let mut rejected = 0usize;
    let mut all_known = true;
    for edit in edits.into_iter().take(max_edits) {
        let known_replacement = known.contains(&edit.replacement.trim().to_lowercase());
        if !valid_edit(&output, &edit, known_replacement) {
            rejected += 1;
            continue;
        }
        output = output.replacen(&edit.source, &edit.replacement, 1);
        applied += 1;
        all_known &= known_replacement;
    }
    if (protect_intent && !matches!(parse(&output), Intent::Insert(_)))
        || (!all_known && normalized_levenshtein(original, &output) < 0.70)
    {
        return (original.into(), 0, rejected + applied);
    }
    (output, applied, rejected)
}

fn valid_edit(text: &str, edit: &TextEdit, known_replacement: bool) -> bool {
    let source = edit.source.trim();
    let replacement = edit.replacement.trim();
    if source.is_empty()
        || replacement.is_empty()
        || source == replacement
        || source.contains('\n')
        || replacement.contains('\n')
        || text.match_indices(&edit.source).count() != 1
        || replacement.len() > source.len().saturating_mul(3).max(64)
    {
        return false;
    }
    preserve_numbers(source, replacement)
        && preserve_protected_words(source, replacement)
        && (!known_replacement || plausible_vocabulary_edit(source, replacement))
        && (preserve_code_tokens(source, replacement)
            || (known_replacement && preserve_code_tokens(source, "")))
}

fn plausible_vocabulary_edit(source: &str, replacement: &str) -> bool {
    let source = normalize_words(source);
    let target = normalize_words(&split_identifier(replacement));
    // A known spelling must not allow swallowing neighboring verbs or entire sentences.
    source.split_whitespace().count() <= target.split_whitespace().count()
        && normalized_levenshtein(&phonetic_normalize(&source), &phonetic_normalize(&target))
            >= 0.55
}

pub(super) fn preserve_numbers(source: &str, replacement: &str) -> bool {
    let regex = Regex::new(r"\d+(?:[.,]\d+)?").expect("valid number regex");
    regex
        .find_iter(source)
        .map(|item| item.as_str())
        .collect::<Vec<_>>()
        == regex
            .find_iter(replacement)
            .map(|item| item.as_str())
            .collect::<Vec<_>>()
}

pub(super) fn preserve_protected_words(source: &str, replacement: &str) -> bool {
    const WORDS: &[&str] = &[
        "не",
        "нет",
        "никогда",
        "только",
        "сначала",
        "после",
        "not",
        "never",
        "only",
        "before",
        "after",
        "without",
        "don't",
        "dont",
    ];
    let protected = |text: &str| {
        normalize_words(text)
            .split_whitespace()
            .filter(|word| WORDS.contains(word))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    };
    protected(source) == protected(replacement)
}

fn preserve_code_tokens(source: &str, replacement: &str) -> bool {
    let regex = Regex::new(
        r"\b(?:[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9_]+|[A-Za-z]+[A-Z][A-Za-z0-9]*|[A-Za-z0-9_./-]+\.[A-Za-z0-9]{1,8})\b",
    )
    .expect("valid code token regex");
    regex
        .find_iter(source)
        .map(|item| item.as_str())
        .collect::<Vec<_>>()
        == regex
            .find_iter(replacement)
            .map(|item| item.as_str())
            .collect::<Vec<_>>()
}
