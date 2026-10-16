//! Original bodyless documentation members. This is an immutable source index:
//! native named queries resolve owner aliases, compose mappers, merge callable
//! declarations and check inheritance. No executable function is synthesized.
use crate::native_named::{NativeNamed, NativeNamedKey};
use luanalysis_engine::{
    LuaKey, NativeSignatureKey, NativeTypeDeclarationKey, NativeTypeMemberKey,
    NativeTypeParameterKey, Type, TypeMemberHeader, TypeMemberKeyRecipe, TypeMemberOwner,
    TypeMemberVisibility, TypeQueries,
};
use luanalysis_syntax::{DocFunctionDeclarationId, DocFunctionOwner, File, Span};
use crate::collections::HashSet;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DocOwnerScope { Unprepared, Scoped(NativeNamedKey), Ready(NativeNamedKey), Failed }

#[derive(Clone, Debug)]
pub(super) struct DocMemberSource {
    pub key: NativeTypeMemberKey,
    pub declaration: DocFunctionDeclarationId,
    pub owner: TypeMemberOwner,
    pub name: String,
    /// Unknown/ambiguous owner errors point to the original owner spelling.
    pub owner_span: Span,
    /// Duplicate-member errors point to the original declared member name.
    pub name_span: Span,
    /// Override/signature errors retain the complete original header span.
    pub signature_span: Span,
    pub scope: Cell<DocOwnerScope>,
}

pub(super) struct NativeDocMembers {
    snapshot: u64,
    base: u32,
    sources: Vec<DocMemberSource>,
}
impl NativeDocMembers {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        named: &NativeNamed,
        base: u32,
        mut candidates: impl FnMut(&str) -> Vec<NativeTypeDeclarationKey>,
    ) -> Self {
        assert!(
            base >= named.member_namespace_end(),
            "original member namespaces are disjoint"
        );
        let mut sources = Vec::new();
        for source in &file.function_declarations {
            if source.invalid_owner {
                continue;
            }
            let Some(owner) = &source.owner else {
                continue;
            };
            let (owner, owner_span) = match owner {
                DocFunctionOwner::Contextual(id) => {
                    let owner = named
                        .key_for_syntax(*id)
                        .expect("original contextual declaration exists");
                    (
                        TypeMemberOwner::Declared(NativeTypeDeclarationKey {
                            snapshot: owner.snapshot,
                            index: owner.index,
                        }),
                        source.name.span,
                    )
                }
                DocFunctionOwner::Explicit(owner) => (
                    TypeMemberOwner::Reference {
                        name: owner.text.clone(),
                        candidates: candidates(&owner.text),
                    },
                    owner.span,
                ),
            };
            let index = base
                .checked_add(
                    sources
                        .len()
                        .try_into()
                        .expect("documentation member count fits u32"),
                )
                .expect("documentation member namespace fits u32");
            sources.push(DocMemberSource {
                key: NativeTypeMemberKey { snapshot, index },
                declaration: source.id,
                owner,
                name: source.name.text.clone(),
                owner_span,
                name_span: source.name.span,
                signature_span: source.span,
                scope: Cell::new(DocOwnerScope::Unprepared),
            });
        }
        Self {
            snapshot,
            base,
            sources,
        }
    }
    pub(super) fn get(&self, key: NativeTypeMemberKey) -> Option<&DocMemberSource> {
        if key.snapshot != self.snapshot {
            return None;
        }
        self.sources.get(key.index.checked_sub(self.base)? as usize)
    }
    pub(super) fn sources(&self) -> &[DocMemberSource] {
        &self.sources
    }
    pub(super) fn for_declaration(&self, id: DocFunctionDeclarationId) -> Option<&DocMemberSource> {
        self.sources.iter().find(|source| source.declaration == id)
    }
    /// Reference ownership must be resolved by the native core. Do not select
    /// an apparent source owner or filter aliases in this immutable index.
    pub(super) fn keys(&self) -> impl Iterator<Item = NativeTypeMemberKey> + '_ {
        self.sources.iter().map(|source| source.key)
    }
    pub(super) fn namespace_end(&self) -> u32 {
        self.base
            .checked_add(
                self.sources
                    .len()
                    .try_into()
                    .expect("documentation member count fits u32"),
            )
            .expect("documentation member namespace fits u32")
    }
    pub(super) fn header(&self, key: NativeTypeMemberKey) -> Option<TypeMemberHeader> {
        let source = self.get(key)?;
        Some(TypeMemberHeader {
            owner: source.owner.clone(),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(source.name.as_bytes().to_vec())),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        })
    }
    pub(super) fn signature_keys(
        &self,
        key: NativeTypeMemberKey,
        lookup: impl FnOnce(DocFunctionDeclarationId) -> Option<NativeSignatureKey>,
    ) -> Option<Vec<NativeSignatureKey>> {
        let source = self.get(key)?;
        Some(vec![lookup(source.declaration).expect(
            "every original documentation function has a signature key",
        )])
    }
    pub(super) fn ty(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
        lookup: impl FnOnce(DocFunctionDeclarationId) -> Option<NativeSignatureKey>,
    ) -> Option<Type> {
        let keys = self.signature_keys(key, lookup)?;
        Some(queries.signature_group_type(&keys))
    }
    pub(super) fn dependencies(
        &self,
        owner: NativeNamedKey,
        used: &mut HashSet<NativeTypeParameterKey>,
        mut lookup: impl FnMut(DocFunctionDeclarationId) -> Option<NativeSignatureKey>,
        mut outer: impl FnMut(NativeSignatureKey) -> Vec<NativeTypeParameterKey>,
    ) {
        let owner = NativeTypeDeclarationKey {
            snapshot: owner.snapshot,
            index: owner.index,
        };
        for source in &self.sources {
            let applicable = match source.scope.get() {
                DocOwnerScope::Scoped(canonical) | DocOwnerScope::Ready(canonical) => canonical.snapshot == owner.snapshot && canonical.index == owner.index,
                DocOwnerScope::Failed => false,
                DocOwnerScope::Unprepared => match &source.owner {
                TypeMemberOwner::Declared(declaration) => *declaration == owner,
                TypeMemberOwner::Reference { candidates, .. } => candidates.contains(&owner),
                TypeMemberOwner::RuntimeStore(_) => false,
                },
            };
            if applicable {
                let signature =
                    lookup(source.declaration).expect("original documentation signature key");
                used.extend(outer(signature));
            }
        }
    }
    pub(super) fn validate(&self, queries: &mut dyn TypeQueries) {
        for key in self.keys() {
            if self.get(key).is_some_and(|source| source.scope.get() == DocOwnerScope::Failed) { continue; }
            let _ = queries.validate_named_member(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::HashMap;

    fn indexed(source: &str, snapshot: u64) -> (File, NativeNamed) {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let named =
            NativeNamed::new(snapshot, &file, &crate::bind::bind(&file), &HashMap::default()).unwrap();
        (file, named)
    }

    #[test]
    fn repeated_original_heads_keep_independent_keys_and_real_spans() {
        let source = "---@class Api\n---@function convert(value:number):string\n---@function convert(value:string):number\n";
        let (file, named) = indexed(source, 91);
        let base = named.member_namespace_end();
        let members =
            NativeDocMembers::new(91, &file, &named, base, |_| panic!("contextual owner"));
        assert_eq!(members.sources().len(), 2);
        let first = &members.sources()[0];
        let second = &members.sources()[1];
        assert_ne!(first.key, second.key);
        assert_ne!(first.declaration, second.declaration);
        assert_eq!(first.owner, second.owner);
        assert_eq!(
            &source[first.name_span.start..first.name_span.end],
            "convert"
        );
        assert_eq!(first.signature_span, file.function_declarations[0].span);
        assert_eq!(members.namespace_end(), base + 2);
        assert!(
            members
                .get(NativeTypeMemberKey {
                    snapshot: 92,
                    index: first.key.index
                })
                .is_none()
        );
    }

    #[test]
    fn unknown_explicit_owner_keeps_native_reference_and_exact_original_owner_span() {
        let source = "-- α\r\n---@function Missing.convert(value:number):string\r\n";
        let (file, named) = indexed(source, 17);
        let members =
            NativeDocMembers::new(17, &file, &named, named.member_namespace_end(), |_| {
                Vec::new()
            });
        let member = &members.sources()[0];
        assert!(
            matches!(&member.owner, TypeMemberOwner::Reference { name, candidates } if name == "Missing" && candidates.is_empty())
        );
        assert_eq!(
            &source[member.owner_span.start..member.owner_span.end],
            "Missing"
        );
        assert_eq!(
            &source[member.name_span.start..member.name_span.end],
            "convert"
        );
        assert_eq!(
            members.header(member.key).unwrap().key,
            TypeMemberKeyRecipe::Fixed(LuaKey::String(b"convert".to_vec()))
        );
    }

    #[test]
    fn globals_and_invalid_contextual_headers_do_not_become_member_recipes() {
        let source = "---@function global(value:number):number\n---@class A\n---@class B\n---@function :ambiguous(value:number):number\n";
        let file = luanalysis_syntax::parse(source);
        assert!(
            file.function_declarations
                .iter()
                .any(|source| source.invalid_owner)
        );
        let named =
            NativeNamed::new(13, &file, &crate::bind::bind(&file), &HashMap::default()).unwrap();
        let members =
            NativeDocMembers::new(13, &file, &named, named.member_namespace_end(), |_| {
                Vec::new()
            });
        assert!(members.sources().is_empty());
    }

    #[test]
    fn doc_signature_containers_are_indexed_at_original_nested_type_spans() {
        let source =
            "---@function create<T:{value:number}>(rows:T[],...:table<string,T>):{result:T[]}\n";
        let (file, named) = indexed(source, 81);
        let doc = &file.function_declarations[0];
        let heads = named.anonymous_heads();
        assert_eq!(heads.len(), 5, "{heads:#?}");
        for text in ["{value:number}", "T[]", "table<string,T>", "{result:T[]}"] {
            assert!(
                heads
                    .iter()
                    .any(|head| &source[head.syntax.span.start..head.syntax.span.end] == text),
                "missing {text}"
            );
        }
        for head in heads {
            assert!(
                head.origins.contains(
                    &crate::native_named::AnonymousHeadOrigin::RuntimeAnnotation(doc.span)
                )
            );
        }
    }

    #[test]
    fn long_form_doc_tags_index_their_original_container_nodes() {
        let source = "---@generic T:{value:number}\n---@param rows T[]\n---@vararg table<string,T>\n---@return {result:T[]}\n---@function create(rows)\n";
        let (file, named) = indexed(source, 82);
        assert_eq!(named.anonymous_heads().len(), 5);
        assert!(
            named
                .anonymous_heads()
                .iter()
                .all(|head| head.origins.contains(
                    &crate::native_named::AnonymousHeadOrigin::RuntimeAnnotation(
                        file.function_declarations[0].span
                    )
                ))
        );
    }
}
