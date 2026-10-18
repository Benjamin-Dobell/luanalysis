//! The captures of a literal Lua pattern, so that the standard pattern functions' results are typed
//! by the pattern they are given: a capture is a string, and an empty capture, `()`, a position.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capture {
    String,
    Position,
}

/// The captures of `pattern` in order of their opening parentheses, or `None` when it is malformed.
pub(crate) fn captures(pattern: &[u8]) -> Option<Vec<Capture>> {
    let mut captures = Vec::new();
    let mut open = 0usize;
    let mut index = 0;
    while index < pattern.len() {
        match pattern[index] {
            b'%' => {
                let class = *pattern.get(index + 1)?;
                index += 2;
                match class {
                    b'b' => {
                        pattern.get(index + 1)?;
                        index += 2;
                    }
                    b'f' => {
                        if pattern.get(index) != Some(&b'[') { return None }
                        index = set_end(pattern, index)?;
                    }
                    _ => {}
                }
            }
            b'[' => index = set_end(pattern, index)?,
            b'(' => {
                if pattern.get(index + 1) == Some(&b')') {
                    captures.push(Capture::Position);
                    index += 2;
                } else {
                    captures.push(Capture::String);
                    open += 1;
                    index += 1;
                }
            }
            b')' => {
                open = open.checked_sub(1)?;
                index += 1;
            }
            _ => index += 1,
        }
    }
    (open == 0).then_some(captures)
}

/// The index after the set that starts at `start` (a `[`).
fn set_end(pattern: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if pattern.get(index) == Some(&b'^') { index += 1 }
    // A `]` first in the set is one of its characters.
    if pattern.get(index) == Some(&b']') { index += 1 }
    loop {
        match *pattern.get(index)? {
            b'%' => { pattern.get(index + 1)?; index += 2 }
            b']' => return Some(index + 1),
            _ => index += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Capture::*, captures};

    #[test]
    fn captures_are_strings_and_empty_captures_positions() {
        assert_eq!(captures(b"^local%s"), Some(vec![]));
        assert_eq!(captures(b"(%w+)=(%w+)"), Some(vec![String, String]));
        assert_eq!(captures(b"()(%a+)()"), Some(vec![Position, String, Position]));
        assert_eq!(captures(b"((a)(b))"), Some(vec![String, String, String]));
        assert_eq!(captures(b"[()]+"), Some(vec![]));
        assert_eq!(captures(b"[]()]"), Some(vec![]));
        assert_eq!(captures(b"%(x%)"), Some(vec![]));
        assert_eq!(captures(b"%b()(.)"), Some(vec![String]));
        assert_eq!(captures(b"%f[%w](%w+)"), Some(vec![String]));
        assert_eq!(captures(b"(a"), None);
        assert_eq!(captures(b"a)"), None);
        assert_eq!(captures(b"[a"), None);
        assert_eq!(captures(b"%"), None);
    }
}
