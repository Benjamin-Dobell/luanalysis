//! Original namespace member identities and pure declaration-root contracts.
//! This index performs no type expansion, alias solving, or stub-body inference.
use super::{GlobalDeclarationSource, NativeRegistry};
use crate::Diagnostic;
use luanalysis_engine::{
    LuaKey, NativeTypeDeclarationKey, NativeTypeMemberKey, Type, TypeMemberHeader,
    TypeMemberKeyRecipe, TypeMemberOwner, TypeMemberVisibility, TypeQueries, TableField, TableType,
};
use luanalysis_project::SourceRole;
use luanalysis_syntax::{Expr, ExprKind, Name, Span};

pub(in crate::native_functions) struct NamespaceTarget {
    pub root: Name,
    pub path: Vec<Name>,
    pub span: Span,
    pub member: NativeTypeMemberKey,
}

pub(super) fn declaration_target(
    expression: &Expr,
    snapshot: u64,
    member_start: u32,
    ordinal: usize,
) -> Option<(Name, Option<NamespaceTarget>)> {
    fn path(expression: &Expr, members: &mut Vec<Name>) -> Option<Name> {
        match &expression.kind {
            ExprKind::Name(name) => Some(name.clone()),
            ExprKind::Parenthesized(inner) => path(inner, members),
            ExprKind::Field { base, name } => {
                let root = path(base, members)?;
                members.push(name.clone());
                Some(root)
            }
            _ => None,
        }
    }
    let mut members = Vec::new();
    let root = path(expression, &mut members)?;
    let Some(name) = members.last().cloned() else {
        return Some((root, None));
    };
    let index = member_start
        .checked_add(ordinal.try_into().expect("member ordinal fits u32"))
        .expect("namespace member identities fit u32");
    Some((
        name,
        Some(NamespaceTarget {
            root,
            path: members,
            span: expression.span,
            member: NativeTypeMemberKey { snapshot, index },
        }),
    ))
}

impl NativeRegistry {
    fn namespace_source(&self, key: NativeTypeMemberKey) -> Option<&GlobalDeclarationSource> {
        self.globals.declarations.iter().find(|source| {
            source
                .namespace
                .as_ref()
                .is_some_and(|target| target.member == key)
        })
    }
    fn namespace_owner(
        &self,
        source: &GlobalDeclarationSource,
    ) -> Result<NativeTypeDeclarationKey, &'static str> {
        if self.source_role() != SourceRole::Declaration {
            return Err("namespace declarations require declaration source role");
        }
        let target = source.namespace.as_ref().expect("original dotted target");
        if target.path.len() != 1 {
            return Err("nested namespace paths require explicit intermediate owner contracts");
        }
        let owners: Vec<_> = self
            .globals
            .declarations
            .iter()
            .filter(|candidate| {
                candidate.namespace.is_none() && candidate.name.text == target.root.text
            })
            .filter_map(|candidate| self.global_attached_type(candidate))
            .collect();
        match owners.as_slice() {
            [owner] => Ok(*owner),
            [] => Err(
                "namespace root needs an original attached named declaration in this source file",
            ),
            _ => Err("namespace root has multiple original attached declaration owners"),
        }
    }
    fn plain_namespace_root(
        &self,
        source: &GlobalDeclarationSource,
    ) -> Result<&GlobalDeclarationSource, &'static str> {
        if self.source_role() != SourceRole::Declaration {
            return Err("namespace declarations require declaration source role");
        }
        let target = source.namespace.as_ref().expect("original dotted target");
        if target.path.len() != 1 {
            return Err("nested namespace paths require explicit intermediate owner contracts");
        }
        let mut roots = self.globals.declarations.iter().filter(|candidate| {
            candidate.namespace.is_none() && candidate.name.text == target.root.text
        });
        let root = roots.next().ok_or("namespace has no original root declaration")?;
        if roots.next().is_some() {
            return Err("namespace has multiple original root declarations");
        }
        if root.annotation.is_some()
            || self.named.declarations().iter().any(|named| named.syntax.attachment == Some(root.statement))
            || !root.initializer.is_some_and(|key| empty_table(&self.expressions.get(key).expression))
        {
            return Err("plain namespace root requires an original unannotated empty-table declaration");
        }
        if source.function.is_some_and(|key| self.implicit_receiver(self.function(key)).is_some()) {
            return Err("colon namespace declarations require an original named receiver contract");
        }
        Ok(root)
    }
    pub(super) fn plain_namespace_contract(
        &self,
        root: &GlobalDeclarationSource,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        if root.namespace.is_some() { return None; }
        let members: Vec<_> = self.globals.declarations.iter().filter(|source| {
            source.namespace.as_ref().is_some_and(|target| target.root.text == root.name.text)
                && self.plain_namespace_root(source).is_ok_and(|owner| owner.key == root.key)
        }).collect();
        if members.is_empty() { return None; }
        let mut fields = Vec::with_capacity(members.len());
        for source in members {
            let value = match queries.value_declaration_type(source.key) {
                Ok(value) => value,
                // The guarded value query has already reported the original
                // failed contract. Do not replace it with an open table or any.
                Err(_) => return Some(Type::Unknown),
            };
            fields.push(TableField::declared(
                LuaKey::String(source.name.text.as_bytes().to_vec()), value,
            ));
        }
        Some(match queries.table_type(&TableType { fields, ..Default::default() }) {
            Ok(table) => table,
            Err(error) => {
                let (template, types) = crate::native_tables::table_error_description(&error);
                self.report.borrow_mut().push_type_diagnostic("LUA2038", root.name.span,
                    format!("Namespace has an invalid declared member schema: {template}"), &types.iter().collect::<Vec<_>>());
                Type::Unknown
            }
        })
    }
    pub(crate) fn namespace_contains_member(&self, key: NativeTypeMemberKey) -> bool {
        self.namespace_source(key)
            .is_some_and(|source| self.namespace_owner(source).is_ok())
    }
    pub(crate) fn namespace_member_header(
        &self,
        key: NativeTypeMemberKey,
    ) -> Option<TypeMemberHeader> {
        let source = self.namespace_source(key)?;
        Some(TypeMemberHeader {
            owner: TypeMemberOwner::Declared(self.namespace_owner(source).ok()?),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(source.name.text.as_bytes().to_vec())),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        })
    }
    pub(crate) fn namespace_member_type(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.namespace_source(key)?;
        self.namespace_owner(source).ok()?;
        Some(
            queries
                .value_declaration_type(source.key)
                .unwrap_or(Type::Any),
        )
    }
    pub(crate) fn namespace_member_origin(&self, key: NativeTypeMemberKey) -> Option<Span> {
        self.namespace_source(key).map(|source| source.name.span)
    }
    pub(crate) fn namespace_members_for(
        &self,
        owner: NativeTypeDeclarationKey,
    ) -> Vec<NativeTypeMemberKey> {
        self.globals
            .declarations
            .iter()
            .filter_map(|source| {
                let target = source.namespace.as_ref()?;
                (self.namespace_owner(source) == Ok(owner)).then_some(target.member)
            })
            .collect()
    }
    pub(super) fn check_namespace_declaration(
        &self,
        source: &GlobalDeclarationSource,
        queries: &mut dyn TypeQueries,
    ) {
        if self.namespace_owner(source).is_err()
            && let Err(reason) = self.plain_namespace_root(source)
        {
            self.report.borrow_mut().diagnostics.push(Diagnostic {
                code: "LUA2038",
                span: source.namespace.as_ref().unwrap().span,
                message: format!("Namespace member declaration is unsupported: {reason}."),
            });
        }
        // Even an unresolved owner cannot hide malformed written contracts.
        let _ = queries.value_declaration_type(source.key);
    }
}

fn empty_table(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Table(fields) => fields.is_empty(),
        ExprKind::Parenthesized(inner) => empty_table(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luanalysis_engine::{Engine, GlobalValueKind, NativeFunctionKey, NativeTypeProvider};

    #[test]
    fn plain_namespace_schemas_query_original_member_values_without_named_declarations() {
        let source = "Api={}\n---@param value number\n---@return string\nfunction Api.read(value) return unresolved_placeholder(value) end\nApi.label=''\n";
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        Engine::new().with_native_provider(
            move || {
                let mut registry = NativeRegistry::new(file, bound);
                registry.set_source_role(SourceRole::Declaration);
                registry
            },
            |registry, queries| {
                assert!(registry.named.declarations().is_empty());
                let root = &registry.globals.declarations[0];
                let member = &registry.globals.declarations[1];
                assert_ne!(root.key, member.key);
                assert!(member.function.is_some());
                assert_eq!(registry.plain_namespace_root(member).unwrap().key, root.key);
                let table = queries.value_declaration_type(root.key).unwrap();
                let callable = queries.property_type(&table, "read").unwrap();
                assert!(queries.call(&callable, &[Type::Number]).errors.is_empty());
                assert!(!queries.call(&callable, &[Type::String]).errors.is_empty());
                let label = queries.property_type(&table, "label").unwrap();
                assert_eq!(queries.format(&label), "string");
                assert!(registry.take_report().diagnostics.is_empty());
            },
        );
    }

    #[test]
    fn namespace_and_constructor_member_recipes_have_disjoint_original_keys() {
        let source = "---@class Namespace\nnamespace = {}\n---@return number\nfunction namespace.read() return 1 end\n---@class Example\nlocal Example = {value = 1}\n";
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        let registry = NativeRegistry::new(file, bound);
        let namespace = registry.globals.declarations.iter()
            .find_map(|source| source.namespace.as_ref()).expect("original dotted declaration");
        let originals = registry.members.sources();
        assert!(!originals.is_empty(), "original constructor members are retained");
        for source in originals {
            assert_ne!(source.key, namespace.member);
            assert!(source.key.index > namespace.member.index);
        }
        assert!(registry.members.get(namespace.member).is_none());
    }

    #[test]
    fn namespace_keys_are_original_value_declarations_and_disjoint_type_members() {
        let source = "---@class std__Math\n---@field config {name:string}\nmath = {}\n---@param value number\n---@return number\nfunction math.abs(value) return unresolved_placeholder(value) end\n";
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        Engine::new().with_native_provider(
            move || {
                let mut registry = NativeRegistry::new(file, bound);
                registry.set_source_role(SourceRole::Declaration);
                registry
            },
            |registry, queries| {
                let declaration = registry
                    .globals
                    .declarations
                    .iter()
                    .find(|source| source.namespace.is_some())
                    .unwrap();
                let target = declaration.namespace.as_ref().unwrap();
                assert_eq!(
                    registry.global_header(declaration.key).kind,
                    GlobalValueKind::Member
                );
                assert_eq!(declaration.key.snapshot, registry.snapshot);
                assert_ne!(
                    declaration.key.index,
                    declaration.function.unwrap().index,
                    "member values do not borrow function IDs"
                );
                assert_eq!(target.member.index, registry.named.next_member_index());
                assert!(registry.contains_type_member(target.member));
                assert!(registry.global_binding("math.abs").is_none());
                assert!(registry.global_binding("abs").is_none());
                let owner = registry.namespace_owner(declaration).unwrap();
                assert_eq!(
                    registry
                        .namespace_member_header(target.member)
                        .unwrap()
                        .owner,
                    TypeMemberOwner::Declared(owner)
                );
                let ty = queries.value_declaration_type(declaration.key).unwrap();
                assert!(queries.call(&ty, &[Type::Number]).errors.is_empty());
                assert!(!queries.call(&ty, &[Type::String]).errors.is_empty());
                let function = NativeFunctionKey {
                    snapshot: registry.snapshot,
                    index: declaration.function.unwrap().index,
                };
                assert!(registry.contains_function(function));
                assert_eq!(
                    &source[declaration.name.span.start..declaration.name.span.end],
                    "abs"
                );
                assert!(
                    registry.take_report().diagnostics.is_empty(),
                    "placeholder bodies must not be checked"
                );
            },
        );
    }
}
