//! Gitignore patterns, which directory discovery skips files and directories by.
use std::path::Path;

/// What a directory's discovery skips: `patterns`, as lines of a `.gitignore` in the directory
/// discovered (LuaLS's `workspace.ignoreDir`), and with `git_ignore`, what the `.gitignore` files
/// in it and its `.git/info/exclude` ignore, as Git does (LuaLS's `workspace.useGitIgnore`).
/// A path named explicitly, rather than found in a directory, is never skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    pub patterns: Vec<String>,
    pub git_ignore: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    tokens: Vec<Token>,
    negated: bool,
    directory: bool,
    anchored: bool,
}

#[derive(Clone, Debug)]
enum Token {
    Byte(u8),
    /// `?`
    One,
    /// `*`
    Star,
    /// `**/`: no directories or any.
    Directories,
    /// A final `**`: everything.
    Rest,
    Class { negated: bool, ranges: Vec<(u8, u8)> },
}

/// The patterns of a `.gitignore` file's text.
pub(crate) fn patterns(text: &str) -> Vec<Pattern> {
    text.lines().filter_map(pattern).collect()
}

/// A `.gitignore` line's pattern: none for a blank line or a comment.
pub(crate) fn pattern(line: &str) -> Option<Pattern> {
    let mut line = line.trim_end_matches('\r');
    // Trailing spaces are not part of a pattern unless escaped.
    while line.ends_with(' ') && !line.ends_with("\\ ") {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let negated = line.starts_with('!');
    if negated {
        line = &line[1..];
    }
    let directory = line.ends_with('/');
    let line = line.trim_end_matches('/');
    if line.is_empty() {
        return None;
    }
    // A separator anywhere but at the end anchors a pattern to its directory; otherwise it
    // names a file or directory at any depth.
    let anchored = line.contains('/');
    let line = line.strip_prefix('/').unwrap_or(line);
    Some(Pattern { tokens: tokens(line.as_bytes()), negated, directory, anchored })
}

fn tokens(pattern: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < pattern.len() {
        let component = at == 0 || pattern[at - 1] == b'/';
        match pattern[at] {
            b'*' if component && pattern.get(at + 1) == Some(&b'*') && matches!(pattern.get(at + 2), None | Some(b'/')) => {
                if at + 2 == pattern.len() {
                    tokens.push(Token::Rest);
                    at += 2;
                } else {
                    tokens.push(Token::Directories);
                    at += 3;
                }
            }
            b'*' => {
                tokens.push(Token::Star);
                while pattern.get(at) == Some(&b'*') {
                    at += 1;
                }
            }
            b'?' => {
                tokens.push(Token::One);
                at += 1;
            }
            b'[' => match class(&pattern[at + 1..]) {
                Some((token, length)) => {
                    tokens.push(token);
                    at += 1 + length;
                }
                None => {
                    tokens.push(Token::Byte(b'['));
                    at += 1;
                }
            },
            b'\\' if at + 1 < pattern.len() => {
                tokens.push(Token::Byte(pattern[at + 1]));
                at += 2;
            }
            byte => {
                tokens.push(Token::Byte(byte));
                at += 1;
            }
        }
    }
    tokens
}

/// A bracket expression after its `[`, and its length to its `]`.
fn class(pattern: &[u8]) -> Option<(Token, usize)> {
    let mut at = 0;
    let negated = matches!(pattern.first(), Some(b'!' | b'^'));
    if negated {
        at += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let mut byte = *pattern.get(at)?;
        if byte == b']' && !first {
            return Some((Token::Class { negated, ranges }, at + 1));
        }
        first = false;
        if byte == b'\\' {
            at += 1;
            byte = *pattern.get(at)?;
        }
        at += 1;
        if pattern.get(at) == Some(&b'-') && pattern.get(at + 1).is_some_and(|end| *end != b']') {
            let mut end = pattern[at + 1];
            at += 2;
            if end == b'\\' {
                end = *pattern.get(at)?;
                at += 1;
            }
            ranges.push((byte, end));
        } else {
            ranges.push((byte, byte));
        }
    }
}

fn matches(tokens: &[Token], text: &[u8]) -> bool {
    let Some((token, rest)) = tokens.split_first() else { return text.is_empty() };
    match token {
        Token::Byte(byte) => text.first() == Some(byte) && matches(rest, &text[1..]),
        Token::One => text.first().is_some_and(|byte| *byte != b'/') && matches(rest, &text[1..]),
        Token::Class { negated, ranges } => text.first().is_some_and(|byte| {
            *byte != b'/' && ranges.iter().any(|(start, end)| (start..=end).contains(&byte)) != *negated
        }) && matches(rest, &text[1..]),
        Token::Star => {
            let mut at = 0;
            loop {
                if matches(rest, &text[at..]) {
                    return true;
                }
                if at == text.len() || text[at] == b'/' {
                    return false;
                }
                at += 1;
            }
        }
        Token::Directories => {
            matches(rest, text)
                || text.iter().enumerate().any(|(at, byte)| *byte == b'/' && matches(rest, &text[at + 1..]))
        }
        Token::Rest => true,
    }
}

impl Pattern {
    /// Whether this pattern names `relative`, a path below its directory with `/` separators.
    fn names(&self, relative: &str, directory: bool) -> bool {
        if self.directory && !directory {
            return false;
        }
        let text = if self.anchored { relative } else { relative.rsplit('/').next().unwrap_or(relative) };
        matches(&self.tokens, text.as_bytes())
    }
}

/// Whether `patterns`, of one directory, ignore `relative`: as the last of them that names it
/// says, or None when none does.
pub(crate) fn decision(patterns: &[Pattern], relative: &str, directory: bool) -> Option<bool> {
    patterns.iter().rev().find(|pattern| pattern.names(relative, directory)).map(|pattern| !pattern.negated)
}

/// `path` below `base`, with `/` separators.
pub(crate) fn relative(path: &Path, base: &Path) -> Option<String> {
    let tail = path.strip_prefix(base).ok()?;
    let parts: Vec<_> = tail.components().map(|component| component.as_os_str().to_string_lossy()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(lines: &str, path: &str, directory: bool) -> Option<bool> {
        decision(&patterns(lines), path, directory)
    }

    #[test]
    fn unanchored_patterns_name_files_and_directories_at_any_depth() {
        assert_eq!(ignored("build", "build", true), Some(true));
        assert_eq!(ignored("build", "src/build", true), Some(true));
        assert_eq!(ignored("build", "src/build.lua", false), None);
        assert_eq!(ignored("*.lua", "a/b/c.lua", false), Some(true));
        assert_eq!(ignored(".vscode", "x/.vscode", true), Some(true));
    }

    #[test]
    fn a_separator_anchors_and_a_trailing_one_names_only_directories() {
        assert_eq!(ignored("/build/", "build", true), Some(true));
        assert_eq!(ignored("/build/", "build", false), None);
        assert_eq!(ignored("/build/", "src/build", true), None);
        assert_eq!(ignored("src/gen", "src/gen", true), Some(true));
        assert_eq!(ignored("src/gen", "lib/src/gen", true), None);
        assert_eq!(ignored("src/*.lua", "src/a.lua", false), Some(true));
        assert_eq!(ignored("src/*.lua", "src/a/b.lua", false), None);
    }

    #[test]
    fn double_stars_name_any_directories() {
        assert_eq!(ignored("**/gen", "gen", true), Some(true));
        assert_eq!(ignored("**/gen", "a/b/gen", true), Some(true));
        assert_eq!(ignored("a/**/b", "a/b", true), Some(true));
        assert_eq!(ignored("a/**/b", "a/x/y/b", true), Some(true));
        assert_eq!(ignored("a/**", "a/x/y", false), Some(true));
        assert_eq!(ignored("a/**", "a", true), None);
    }

    #[test]
    fn the_last_naming_pattern_decides_and_comments_and_escapes_are_read() {
        assert_eq!(ignored("*.lua\n!keep.lua", "keep.lua", false), Some(false));
        assert_eq!(ignored("!keep.lua\n*.lua", "keep.lua", false), Some(true));
        assert_eq!(ignored("# build\n\n", "build", true), None);
        assert_eq!(ignored("\\#build", "#build", true), Some(true));
        assert_eq!(ignored("[a-c]?.lua", "b1.lua", false), Some(true));
        assert_eq!(ignored("[!a-c]?.lua", "b1.lua", false), None);
        assert_eq!(ignored("lo?", "log", true), Some(true));
        assert_eq!(ignored("lo?", "lo/g", true), None);
        assert_eq!(ignored("build   ", "build", true), Some(true));
    }
}
