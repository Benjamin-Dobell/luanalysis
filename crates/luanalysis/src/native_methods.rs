//! Immutable original named-function/assignment origins. This indexes ownership
//! evidence; receiver typing, substitution and relations remain native queries.
use crate::bind::{BindingId, BoundFile};
use crate::native_named::{NativeNamed, NativeNamedKey, RuntimeAttachmentKind};
use crate::native_members::{OriginalMemberInput, OriginalMemberKey, OriginalMemberOrigin, OriginalMemberValue};
use luanalysis_engine::{LuaKey, NativeFunctionKey, Type, TypeQueries, NativeCheckMode};
use luanalysis_syntax::{
    Expr, ExprKind, File, Function, NamedRecordKind, NamedTypeDeclarationKind, Span, Stmt, StmtKind,
};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(super) struct MethodSource {
    pub function: NativeFunctionKey,
    pub target: Expr,
    pub receiver: Expr,
    pub member: LuaKey,
    pub colon: Option<Span>,
    pub implicit_receiver: Option<BindingId>,
    /// Exact original attached class binding, never a matching runtime name.
    pub class_owner: Option<NativeNamedKey>,
}

pub(super) struct NativeMethods {
    snapshot: u64,
    sources: Vec<Option<MethodSource>>,
}
impl NativeMethods {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        functions: &[Function],
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        named: &NativeNamed,
    ) -> Self {
        let mut owners: HashMap<BindingId, Vec<NativeNamedKey>> = HashMap::new();
        for declaration in named.declarations() {
            if !matches!(
                declaration.syntax.kind,
                NamedTypeDeclarationKind::Record {
                    flavor: NamedRecordKind::Class,
                    ..
                }
            ) {
                continue;
            }
            let Some(attachment) = declaration
                .syntax
                .attachment
                .and_then(|span| named.attachment(span))
            else {
                continue;
            };
            let RuntimeAttachmentKind::Local { bindings } = &attachment.kind else {
                continue;
            };
            let [binding] = bindings.as_slice() else {
                continue;
            };
            if attachment.declarations.len() == 1
                && named.candidates(&declaration.syntax.name.text).len() == 1
            {
                owners.entry(*binding).or_default().push(declaration.key);
            }
        }
        let mut methods = Self {
            snapshot,
            sources: vec![None; functions.len()],
        };
        methods.statements(&file.statements, spans, bound, &owners);
        // Each original function body is indexed once; expression nesting was
        // already traversed by the original function identity collector.
        for function in functions {
            methods.statements(&function.body, spans, bound, &owners);
        }
        methods
    }
    pub(super) fn get(&self, key: NativeFunctionKey) -> Option<&MethodSource> {
        (key.snapshot == self.snapshot)
            .then(|| self.sources.get(key.index as usize))
            .flatten()
            .and_then(Option::as_ref)
    }
    pub(super) fn sources(&self) -> impl Iterator<Item = &MethodSource> {
        self.sources.iter().filter_map(Option::as_ref)
    }
    pub(super) fn original_members(&self) -> Vec<OriginalMemberInput> {
        self.sources().filter_map(|method| {
            let owner = method.class_owner?;
            Some(OriginalMemberInput {
                owner,
                key: OriginalMemberKey::Fixed(method.member.clone()),
                origin: OriginalMemberOrigin::Method(method.function),
                value: OriginalMemberValue::Function(method.function),
                span: method.target.span,
                lexical_context: Some(crate::native_named::NativeTypeContext::Function(method.function)),
                documentation_contexts: vec![owner],
            })
        }).collect()
    }
    fn statements(
        &mut self,
        statements: &[Stmt],
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        owners: &HashMap<BindingId, Vec<NativeNamedKey>>,
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::FunctionDeclaration {
                    target,
                    function,
                    method_span,
                } => self.record(target, function, *method_span, spans, bound, owners),
                StmtKind::Assign {
                    targets, values, ..
                } if targets.len() == values.len() => {
                    for (target, value) in targets.iter().zip(values) {
                        if let Some(function) = function_literal(value) {
                            self.record(target, function, None, spans, bound, owners);
                        }
                    }
                }
                StmtKind::Block { body, .. }
                | StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. }
                | StmtKind::GenericFor { body, .. } => self.statements(body, spans, bound, owners),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.statements(&branch.body, spans, bound, owners);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body, spans, bound, owners);
                    }
                }
                _ => {}
            }
        }
    }
    fn record(
        &mut self,
        target: &Expr,
        function: &Function,
        colon: Option<Span>,
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        owners: &HashMap<BindingId, Vec<NativeNamedKey>>,
    ) {
        let (receiver, member) = match &target.kind {
            ExprKind::Field { base, name } => {
                (base.as_ref(), LuaKey::String(name.text.as_bytes().to_vec()))
            }
            ExprKind::Index { base, index } => match crate::native_tables::literal_key(index) {
                Some(key) => (base.as_ref(), key),
                None => return,
            },
            _ => return,
        };
        let index = *spans
            .get(&(function.span.start, function.span.end))
            .expect("original method function was allocated first");
        let key = NativeFunctionKey {
            snapshot: self.snapshot,
            index,
        };
        let class_owner = match &receiver.kind {
            ExprKind::Name(name) => bound
                .binding_at(name.span.start)
                .and_then(|binding| owners.get(&binding))
                .and_then(|owners| match owners.as_slice() {
                    [owner] => Some(*owner),
                    _ => None,
                }),
            _ => None,
        };
        self.sources[index as usize] = Some(MethodSource {
            function: key,
            target: target.clone(),
            receiver: receiver.clone(),
            member,
            colon,
            implicit_receiver: bound.implicit_receiver(function.span),
            class_owner,
        });
    }
}
fn function_literal(value: &Expr) -> Option<&Function> {
    match &value.kind {
        ExprKind::Function(function) => Some(function),
        ExprKind::Parenthesized(value) => function_literal(value),
        _ => None,
    }
}

impl super::NativeRegistry {
    pub(crate) fn implicit_receiver(&self, function: &Function) -> Option<BindingId> {
        self.bound.implicit_receiver(function.span)
    }

    pub(crate) fn default_receiver_type(&self, function: &Function, queries: &mut dyn TypeQueries) -> Option<Type> {
        let key = self.key_for(function);
        let method = self.methods.get(key)?;
        if let Some(owner) = method.class_owner {
            return self.types.self_parameter(owner).map(Type::NativeTypeParameter);
        }
        // A plain Lua receiver is read at its original pre-write creation
        // point. The native flow walker remains the only value-flow solver.
        let graph = self.flow.graph_for_function(key, None)?;
        let creation = self.flow.graph_creation(graph)?;
        let mut checker = self.checker_at_flow(creation.input, queries);
        checker.check_mode |= NativeCheckMode::INFERENTIAL;
        let receiver = checker.expression(&method.receiver);
        Some(receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_functions::NativeRegistry;
    fn registry(source: &str) -> NativeRegistry {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        NativeRegistry::new(file, bound)
    }
    #[test]
    fn method_owners_follow_original_attached_binding_not_spelling() {
        let native = registry(
            "---@class DifferentName\nlocal object={}\nfunction object:method(value) return self end",
        );
        let method = native.methods.sources().next().unwrap();
        assert!(method.colon.is_some());
        assert!(method.implicit_receiver.is_some());
        assert!(method.class_owner.is_some());
        assert_eq!(native.function(method.function).params.len(), 1);
    }
    #[test]
    fn dot_lambda_and_colon_origins_keep_independent_actual_function_keys() {
        let native = registry(
            "---@class C\nlocal object={}\nfunction object.dot() end\nobject.lambda=function() end\nfunction object:colon() end",
        );
        let methods: Vec<_> = native.methods.sources().collect();
        assert_eq!(methods.len(), 3);
        assert!(
            methods
                .iter()
                .all(|method| method.class_owner == methods[0].class_owner)
        );
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.implicit_receiver.is_some())
                .count(),
            1
        );
        assert_ne!(methods[0].function, methods[1].function);
    }
    #[test]
    fn lexical_shadows_aliases_and_instance_extensions_do_not_invent_class_ownership() {
        let native = registry(
            "---@class C\nlocal object={}\nlocal alias=object\nfunction alias:method() end\ndo local object={} function object:method() end end\n---@type C\nlocal instance\nfunction instance:method() end",
        );
        assert_eq!(native.methods.sources().count(), 3);
        assert!(
            native
                .methods
                .sources()
                .all(|method| method.class_owner.is_none())
        );
    }
    #[test]
    fn semantic_self_keeps_written_arity_and_generic_shadowing_separate() {
        use crate::native_named::NativeTypeContext;
        use crate::native_types::TypeNameResolution;
        let native = registry("---@generic T\nlocal function outer()\n---@class C<U>\nlocal object={}\n---@generic T\n---@param value T\n---@return self\nfunction object:method(value) return self end\nreturn object\nend");
        let method = native.methods.sources().next().unwrap();
        let owner = method.class_owner.unwrap();
        let own = native.types.own_parameters(method.function);
        let class = native.types.own_parameters_in_context(NativeTypeContext::NamedType(owner));
        let self_key = native.types.self_parameter(owner).unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(class.len(), 1);
        assert!(!class.contains(&self_key));
        assert_eq!(native.function(method.function).params.len(), 1);
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "T"), TypeNameResolution::Parameter(own[0]));
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "U"), TypeNameResolution::Parameter(class[0]));
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "self"), TypeNameResolution::Parameter(self_key));
        assert_ne!(own[0], class[0]);
        assert_eq!(native.types.parameter(self_key).semantic_self, Some(owner));
        assert!(native.types.parameter(self_key).constraint.is_none());
    }
    #[test]
    fn member_recipes_retain_original_function_and_target_identities() {
        use crate::native_members::{OriginalMemberOrigin, OriginalMemberValue};
        let native = registry("---@class C\nlocal object={}\nfunction object:method() end\nobject.lambda=function() end\nlocal alias=object\nfunction alias:ignored() end");
        let members = native.methods.original_members();
        assert_eq!(members.len(), 2);
        for member in members {
            let OriginalMemberOrigin::Method(function) = member.origin else { panic!("method origin") };
            assert!(matches!(member.value, OriginalMemberValue::Function(key) if key==function));
            let source = native.methods.get(function).unwrap();
            assert_eq!(member.span, source.target.span);
            assert_eq!(member.documentation_contexts, vec![member.owner]);
            assert!(source.class_owner.is_some());
        }
    }
}
