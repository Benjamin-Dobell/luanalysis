//! Every one of v1's own cases (`legacy/cases.tsv`), checked as the legacy suite checks them. A
//! case matches v1, or differs from it in just the ways `legacy/v2-differences.txt` lists, each
//! difference under a comment that says why v2 behaves as it does.
#[allow(dead_code)] // The shared host also serves the full-suite example.
#[path = "support/legacy.rs"]
mod legacy;
use std::collections::BTreeMap;

const DIFFERENCES: &str = include_str!("legacy/v2-differences.txt");

/// Each case's listed differences, sorted. Every difference is under a comment of its case.
fn listed() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut cases = BTreeMap::new();
    let mut case = None;
    let mut explained = false;
    for line in DIFFERENCES.lines() {
        if let Some(id) = line.strip_prefix("## ") {
            assert!(cases.insert(id, Vec::new()).is_none(), "{id} is listed twice");
            case = Some(id);
            explained = false;
        } else if line.starts_with('#') {
            explained = case.is_some();
        } else if !line.trim().is_empty() {
            let id = case.unwrap_or_else(|| panic!("{line:?} is under no case"));
            assert!(explained, "{id}: {line:?} has no comment saying why it differs");
            cases.get_mut(id).unwrap().push(line);
        }
    }
    for differences in cases.values_mut() {
        differences.sort_unstable();
    }
    cases
}

/// The lines of `from` that `other` lacks, as many times as it lacks them.
fn lacking<'a>(from: &'a [impl AsRef<str>], other: &[impl AsRef<str>]) -> Vec<&'a str> {
    let mut remaining: Vec<&str> = other.iter().map(AsRef::as_ref).collect();
    from.iter()
        .map(AsRef::as_ref)
        .filter(|line| match remaining.iter().position(|other| other == line) {
            Some(index) => {
                remaining.swap_remove(index);
                false
            }
            None => true,
        })
        .collect()
}

#[test]
fn every_v1_case_matches_v1_or_differs_only_as_listed() {
    let listed = listed();
    let cases = legacy::cases();
    for id in listed.keys() {
        assert!(cases.iter().any(|case| case.id == *id), "{id} is listed but is no case");
    }
    let mut wrong = String::new();
    for case in &cases {
        let directory = legacy::Temp::new();
        let mut observed = legacy::run(case, &directory.0).differences;
        observed.sort_unstable();
        let expected = listed.get(case.id).map_or(&[][..], Vec::as_slice);
        let (unlisted, gone) = (lacking(&observed, expected), lacking(expected, &observed));
        if !unlisted.is_empty() || !gone.is_empty() {
            wrong.push_str(&format!("## {}\n", case.id));
            gone.iter().for_each(|line| wrong.push_str(&format!("- {line}\n")));
            unlisted.iter().for_each(|line| wrong.push_str(&format!("+ {line}\n")));
        }
    }
    assert!(
        wrong.is_empty(),
        "v2 now differs from v1 otherwise than legacy/v2-differences.txt lists (- listed, no \
         longer seen; + seen, not listed). Say why under a comment for each new difference:\n{wrong}"
    );
}

#[test]
fn a_table_iterated_with_pairs_completes_the_members_of_its_values() {
    // v1's testAnnotationDict iterates with `pair`, which no Lua declares: its loop value is
    // unknown, with no members. Iterated with `pairs`, the value is an `Emmy`, and they complete.
    let case = legacy::cases().into_iter().find(|case| case.id == "TestCompletion/testAnnotationDict").unwrap();
    let directory = legacy::Temp::new();
    let result = legacy::run_with_sources(&case, &directory.0, |_, source| source.replace("pair(", "pairs("));
    assert!(result.passed, "{}", result.detail);
}
