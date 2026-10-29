//! What a check must redo after an edit, as TypeScript's builder decides it: files whose exported
//! shape is unchanged affect no other file's check. A Lua file reaches others through the modules
//! it is required as, and also through globals and annotation types, which are shared by name.

use crate::bind::{BoundFile, OccurrenceKind};
use luanalysis_syntax::{Expr, ExprKind, File, StmtKind, TypeNameKind};
use std::collections::BTreeSet;

/// The names through which one file reaches the others.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileNames {
    /// The globals it assigns, the types it declares, and the global tables it adds fields to.
    pub touched: BTreeSet<String>,
    /// The globals it reads and the types it refers to.
    pub referenced: BTreeSet<String>,
    /// Its top-level stores into global tables it does not assign, as written: other files see
    /// what they add through those globals, so they are part of its signature.
    pub augmentations: Vec<String>,
}

impl FileNames {
    pub(crate) fn of(syntax: &File, bound: &BoundFile, source: &str) -> Self {
        let mut names = Self::default();
        for occurrence in bound.occurrences.iter().filter(|occurrence| occurrence.binding.is_none()) {
            match occurrence.kind {
                OccurrenceKind::Write | OccurrenceKind::Declaration => { names.touched.insert(occurrence.name.clone()); }
                OccurrenceKind::Read | OccurrenceKind::TypeQuery => { names.referenced.insert(occurrence.name.clone()); }
                OccurrenceKind::Annotation => {}
            }
        }
        for name in luanalysis_syntax::type_names(syntax, source) {
            match name.kind {
                TypeNameKind::Declaration => { names.touched.insert(name.name); }
                TypeNameKind::Reference => { names.referenced.insert(name.name); }
                TypeNameKind::Generic => {}
            }
        }
        for statement in &syntax.statements {
            let targets: Vec<&Expr> = match &statement.kind {
                StmtKind::Assign { targets, .. } => targets.iter().collect(),
                StmtKind::FunctionDeclaration { target, .. } => vec![target],
                _ => continue,
            };
            for target in targets {
                let Some(root) = stored_table_root(target) else { continue };
                let global = bound.occurrences.binary_search_by_key(&root.span.start, |occurrence| occurrence.span.start)
                    .ok()
                    .is_some_and(|index| bound.occurrences[index].binding.is_none());
                if global && !names.touched.contains(&root.text) {
                    names.touched.insert(root.text.clone());
                    names.augmentations.push(source.get(statement.span.start..statement.span.end).unwrap_or_default().to_owned());
                }
            }
        }
        names
    }

    /// Whether a file that refers to `self`'s names or shares them depends on what `other` touches.
    pub(crate) fn depends_on(&self, touched: &BTreeSet<String>) -> bool {
        self.referenced.iter().chain(&self.touched).any(|name| touched.contains(name))
    }
}

/// The name a store such as `A.b.c = v` or `function A:m()` begins at, when it stores into a field.
fn stored_table_root(target: &Expr) -> Option<&luanalysis_syntax::Name> {
    let mut expression = match &target.kind {
        ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => base,
        _ => return None,
    };
    loop {
        match &expression.kind {
            ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => expression = base,
            ExprKind::Parenthesized(inner) => expression = inner,
            ExprKind::Name(name) => return Some(name),
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> FileNames {
        let syntax = luanalysis_syntax::parse(source);
        let bound = crate::bind::bind(&syntax);
        FileNames::of(&syntax, &bound, source)
    }

    #[test]
    fn a_file_touches_its_globals_and_types_and_refers_to_others() {
        let found = names("---@class Point\n---@field x number\nOrigin = {}\nfunction Origin.at() return 1 end\nlocal p = Shared.value\n---@type Vector\nlocal v\n");
        assert_eq!(found.touched.iter().map(String::as_str).collect::<Vec<_>>(), ["Origin", "Point"]);
        assert!(found.referenced.contains("Shared") && found.referenced.contains("Vector"), "{found:?}");
        // `Origin` is assigned here, so storing into it adds nothing another file sees beside it.
        assert!(found.augmentations.is_empty(), "{found:?}");
    }

    #[test]
    fn a_store_into_another_files_global_table_is_an_augmentation() {
        let found = names("function string.shout(s) return s end\nlocal M = require('m')\nM.extra = true\n");
        assert!(found.touched.contains("string"), "{found:?}");
        assert_eq!(found.augmentations, ["function string.shout(s) return s end"]);
    }
}
