//! Immutable original annotation signatures; the backend owns applicability.
use super::Diagnostic;
use super::native_functions::NativeRegistry;
use super::native_named::{
    NamedGenericParameterRecipe, NamedGenericScopeRecipe, NativeNamed, NativeNamedKey,
    NativeTypeContext,
};
use super::native_packs::AnnotationPack;
use luanalysis_engine::{
    NativeFunctionKey, NativeSignatureKey, NativeTypeDeclarationKey, SignatureHeader,
    SignatureOwner, SignatureSite,
};
use luanalysis_engine::{NativeTypeParameterKey, Parameter, Type, TypeQueries, ValuePack};
use luanalysis_syntax::{
    DocFunctionDeclarationId, DocFunctionOwner, File, Function, FunctionType, NamedTypeDeclarationKind, Span, Stmt, StmtKind, TypeExpr,
    TypeExprKind, TypeFieldKey,
};
use crate::collections::{HashMap, HashSet};

/// Immutable annotation edges contain original parameter identities, rather
/// than expanded types or names substituted into another declaration's scope.
#[derive(Clone)]
pub(super) struct ResultPackAliasRecipe {
    parameters: Vec<NativeTypeParameterKey>,
    target: ResultPackSourceType,
}

#[derive(Clone)]
enum ResultPackSourceType {
    Parameter(NativeTypeParameterKey),
    Alias {
        name: String,
        arguments: Vec<Self>,
    },
    Callable {
        constraints: Vec<Self>,
        parameters: Vec<Self>,
        returns: Vec<Self>,
        scalar_row: bool,
        homogeneous_last: bool,
    },
    Parts(Vec<Self>),
}

#[derive(Clone, Default, PartialEq, Eq)]
struct ResultPackProducer {
    inputs: HashSet<NativeTypeParameterKey>,
    scalar_row: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct ResultPackSourceSummary {
    identity: Option<NativeTypeParameterKey>,
    dependencies: HashSet<NativeTypeParameterKey>,
    producers: HashMap<NativeTypeParameterKey, ResultPackProducer>,
}

impl ResultPackSourceSummary {
    fn absorb(&mut self, other: &Self) {
        self.dependencies.extend(other.dependencies.iter().copied());
        for (&key, producer) in &other.producers {
            let merged = self.producers.entry(key).or_default();
            merged.inputs.extend(producer.inputs.iter().copied());
            merged.scalar_row |= producer.scalar_row;
        }
    }
}

struct ResultPackSourceAnalysis<'a> {
    aliases: &'a HashMap<NativeTypeDeclarationKey, ResultPackAliasRecipe>,
    registry: &'a NativeRegistry,
}

impl ResultPackSourceAnalysis<'_> {
    /// Every summary grows over a finite set of original parameter keys.
    /// Recursive alias edges therefore converge without expanding an AST or
    /// assuming a cycle denotes its written generic argument.
    fn alias_summaries(&self) -> HashMap<NativeTypeDeclarationKey, ResultPackSourceSummary> {
        let mut summaries = HashMap::default();
        loop {
            let mut changed = false;
            for (&key, recipe) in self.aliases {
                let next = self.summarize(&recipe.target, &summaries);
                if summaries.get(&key) != Some(&next) {
                    summaries.insert(key, next);
                    changed = true;
                }
            }
            if !changed {
                return summaries;
            }
        }
    }

    fn summarize(
        &self,
        source: &ResultPackSourceType,
        aliases: &HashMap<NativeTypeDeclarationKey, ResultPackSourceSummary>,
    ) -> ResultPackSourceSummary {
        let mut summary = ResultPackSourceSummary::default();
        match source {
            ResultPackSourceType::Parameter(key) => {
                summary.identity = Some(*key);
                summary.dependencies.insert(*key);
            }
            ResultPackSourceType::Parts(parts) => {
                for part in parts {
                    summary.absorb(&self.summarize(part, aliases));
                }
            }
            ResultPackSourceType::Callable {
                constraints,
                parameters,
                returns,
                scalar_row,
                homogeneous_last,
            } => {
                let mut inputs = HashSet::default();
                for parameter in parameters {
                    let parameter = self.summarize(parameter, aliases);
                    inputs.extend(parameter.dependencies.iter().copied());
                    summary.absorb(&parameter);
                }
                for constraint in constraints {
                    summary.absorb(&self.summarize(constraint, aliases));
                }
                let returns: Vec<_> = returns
                    .iter()
                    .map(|ty| self.summarize(ty, aliases))
                    .collect();
                for result in &returns {
                    summary.absorb(result);
                }
                if !homogeneous_last
                    && let Some(key) = returns.last().and_then(|result| result.identity)
                {
                    let producer = summary.producers.entry(key).or_default();
                    producer.inputs.extend(inputs);
                    producer.scalar_row |= *scalar_row;
                }
            }
            ResultPackSourceType::Alias { name, arguments } => {
                let arguments: Vec<_> = arguments
                    .iter()
                    .map(|ty| self.summarize(ty, aliases))
                    .collect();
                let candidates = self.registry.named_candidates(name);
                let [key] = candidates.as_slice() else {
                    for argument in &arguments {
                        summary
                            .dependencies
                            .extend(argument.dependencies.iter().copied());
                    }
                    return summary;
                };
                let Some(recipe) = self.aliases.get(key) else {
                    // Named records keep their written type arguments. Alias
                    // arguments below contribute only through used formals.
                    for argument in &arguments {
                        summary.absorb(argument);
                    }
                    return summary;
                };
                let Some(target) = aliases.get(key) else {
                    return summary;
                };
                let identity =
                    |key| match recipe.parameters.iter().position(|&formal| formal == key) {
                        Some(index) => arguments.get(index).and_then(|argument| argument.identity),
                        None => Some(key),
                    };
                let dependencies =
                    |key| match recipe.parameters.iter().position(|&formal| formal == key) {
                        Some(index) => arguments
                            .get(index)
                            .map_or_else(HashSet::default, |argument| {
                                argument.dependencies.clone()
                            }),
                        None => HashSet::from_iter([key]),
                    };
                summary.identity = target.identity.and_then(identity);
                for &key in &target.dependencies {
                    summary.dependencies.extend(dependencies(key));
                    if let Some(index) = recipe.parameters.iter().position(|&formal| formal == key)
                        && let Some(argument) = arguments.get(index)
                    {
                        summary.absorb(argument);
                    }
                }
                for (&key, producer) in &target.producers {
                    let Some(key) = identity(key) else { continue };
                    let mapped = summary.producers.entry(key).or_default();
                    for &input in &producer.inputs {
                        mapped.inputs.extend(dependencies(input));
                    }
                    mapped.scalar_row |= producer.scalar_row;
                }
            }
        }
        summary
    }
}

impl super::Checker<'_> {
    fn declared_type_predicate(
        &mut self,
        source: &TypeExpr,
        parameters: &[Parameter],
    ) -> Option<luanalysis_engine::TypePredicate> {
        let TypeExprKind::TypePredicate { parameter, ty, asserts } = &source.kind else { return None };
        let target = ty.as_ref().map(|ty| self.annotation(ty));
        let Some(index) = parameters.iter().rposition(|candidate| candidate.name == parameter.text) else {
            self.error("TS1225", parameter.span, format!("Cannot find parameter '{}'.", parameter.text));
            return None;
        };
        let parameter_type = if parameters[index].optional {
            Type::Union(vec![parameters[index].ty.clone(), Type::Nil])
        } else {
            parameters[index].ty.clone()
        };
        if let (Some(ty), Some(target)) = (ty, &target)
            && !self.engine.is_assignable(target, &parameter_type)
        {
            self.error_types(
                "TS2677", ty.span,
                "A type predicate's type '{}' must be assignable to its parameter's type '{}'.",
                &[target, &parameter_type],
            );
            return None;
        }
        Some(luanalysis_engine::TypePredicate { parameter: index, ty: target, asserts: asserts.is_some() })
    }
}

impl NativeRegistry {
    /// Retain source binding identities while following callable aliases. The
    /// native core still owns type expansion, constraints and pack inference.
    pub(super) fn source_result_pack_alias_recipes(
        &self,
    ) -> HashMap<NativeTypeDeclarationKey, ResultPackAliasRecipe> {
        self.named_metadata()
            .declarations()
            .iter()
            .filter_map(|source| {
                let NamedTypeDeclarationKind::Alias { target } = &source.syntax.kind else {
                    return None;
                };
                let context = NativeTypeContext::NamedType(source.key);
                Some((
                    NativeTypeDeclarationKey {
                        snapshot: source.key.snapshot,
                        index: source.key.index,
                    },
                    ResultPackAliasRecipe {
                        parameters: self.types.own_parameters_in_context(context),
                        target: self.result_pack_source_type(Some(context), target),
                    },
                ))
            })
            .collect()
    }

    fn result_pack_source_type(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
    ) -> ResultPackSourceType {
        let ty = ty.value_syntax();
        match &ty.kind {
            TypeExprKind::Named(name)
                if matches!(
                    name.as_str(),
                    "any"
                        | "unknown"
                        | "never"
                        | "nil"
                        | "boolean"
                        | "number"
                        | "integer"
                        | "string"
                        | "void"
                        | "table"
                        | "function"
                ) =>
            {
                ResultPackSourceType::Parts(Vec::new())
            }
            TypeExprKind::Named(name) => match self.types.resolve_name_in_context(context, name) {
                super::native_types::TypeNameResolution::Parameter(key) => {
                    ResultPackSourceType::Parameter(key)
                }
                _ => ResultPackSourceType::Alias {
                    name: name.clone(),
                    arguments: Vec::new(),
                },
            },
            TypeExprKind::Generic { base, args, .. } => {
                let arguments: Vec<_> = args
                    .iter()
                    .map(|ty| self.result_pack_source_type(context, ty))
                    .collect();
                if base.text == "table" {
                    return ResultPackSourceType::Parts(arguments);
                }
                match self.types.resolve_name_in_context(context, &base.text) {
                    super::native_types::TypeNameResolution::Parameter(key) => {
                        let mut parts = vec![ResultPackSourceType::Parameter(key)];
                        parts.extend(arguments);
                        ResultPackSourceType::Parts(parts)
                    }
                    _ => ResultPackSourceType::Alias {
                        name: base.text.clone(),
                        arguments,
                    },
                }
            }
            TypeExprKind::Parenthesized(inner) => self.result_pack_source_type(context, inner),
            TypeExprKind::Function(function) => {
                let context = self
                    .signature_key_for(function.span)
                    .map(NativeTypeContext::Signature)
                    .or(context);
                ResultPackSourceType::Callable {
                    constraints: function
                        .generics
                        .iter()
                        .filter_map(|parameter| parameter.constraint.as_ref())
                        .map(|ty| self.result_pack_source_type(context, ty))
                        .collect(),
                    parameters: function
                        .params
                        .iter()
                        .filter_map(|parameter| parameter.ty.as_ref())
                        .chain(function.vararg.iter().map(|parameter| &parameter.ty))
                        .map(|ty| self.result_pack_source_type(context, ty))
                        .collect(),
                    returns: function
                        .returns
                        .iter()
                        .map(|ty| self.result_pack_source_type(context, ty))
                        .collect(),
                    scalar_row: function.returns.len() != 1
                        || function
                            .packs
                            .as_ref()
                            .is_some_and(|packs| packs.alternatives.len() != 1),
                    homogeneous_last: function.packs.as_ref().is_some_and(|packs| {
                        packs.alternatives.iter().any(|row| {
                            row.variadic
                                .is_some_and(|tail| tail.type_index + 1 == function.returns.len())
                        })
                    }),
                }
            }
            TypeExprKind::Union(types) | TypeExprKind::Intersection(types) => {
                ResultPackSourceType::Parts(
                    types
                        .iter()
                        .map(|ty| self.result_pack_source_type(context, ty))
                        .collect(),
                )
            }
            TypeExprKind::Array { element, .. }
            | TypeExprKind::TypePredicate { ty: Some(element), .. }
            | TypeExprKind::KeyOf(element) => {
                ResultPackSourceType::Parts(vec![self.result_pack_source_type(context, element)])
            }
            TypeExprKind::IndexedAccess { object, index } => ResultPackSourceType::Parts(vec![
                self.result_pack_source_type(context, object),
                self.result_pack_source_type(context, index),
            ]),
            TypeExprKind::Record(fields) => {
                let mut parts = Vec::new();
                for field in fields {
                    parts.push(self.result_pack_source_type(context, &field.ty));
                    if let TypeFieldKey::Indexed(key) = &field.key {
                        parts.push(self.result_pack_source_type(context, key));
                    }
                }
                ResultPackSourceType::Parts(parts)
            }
            TypeExprKind::TemplateLiteral { types, .. } => {
                ResultPackSourceType::Parts(types.iter().map(|ty| self.result_pack_source_type(context, ty)).collect())
            }
            TypeExprKind::Conditional { check, extends, yes, no } => {
                ResultPackSourceType::Parts([check, extends, yes, no].iter().map(|ty| self.result_pack_source_type(context, ty)).collect())
            }
            TypeExprKind::Mapped(mapped) => ResultPackSourceType::Parts(
                [&mapped.constraint].into_iter().chain(&mapped.name).chain([&mapped.ty])
                    .map(|ty| self.result_pack_source_type(context, ty)).collect(),
            ),
            TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::TypePredicate { ty: None, .. }
            | TypeExprKind::TypeQuery(_) | TypeExprKind::Infer { .. } | TypeExprKind::Error => {
                ResultPackSourceType::Parts(Vec::new())
            }
        }
    }

    /// A free producer introduces a physical result sequence. Scalar
    /// transformers and written tuple members retain their scalar contracts.
    /// Whether the type parameter's owner writes it as varargs, `...: T...`: a sequence of values,
    /// as a result list's final type parameter is.
    pub(crate) fn source_type_parameter_is_spread(&self, parameter: NativeTypeParameterKey) -> bool {
        let source = self.types.parameter(parameter);
        let name = source.name.text.as_str();
        let signature = |signature: &luanalysis_syntax::FunctionType| {
            signature.params.iter().filter_map(|parameter| parameter.ty.as_ref()).any(|ty| ty.spreads(name))
                || signature.vararg.as_ref().is_some_and(|vararg| vararg.spreads(name) || vararg.ty.spreads(name))
                || signature.returns.iter().any(|ty| ty.spreads(name))
        };
        let annotations = |annotations: &luanalysis_syntax::FunctionAnnotations| {
            annotations.params.iter().any(|parameter| parameter.ty.spreads(name))
                || annotations.vararg.as_ref().is_some_and(|vararg| vararg.spreads(name) || vararg.ty.spreads(name))
                || annotations.returns.iter().chain(&annotations.type_annotation).any(|annotation| annotation.types.iter().any(|ty| ty.spreads(name)))
        };
        match source.owner {
            NativeTypeContext::Function(function) => annotations(&self.function(function).annotations),
            NativeTypeContext::Signature(owner) => {
                self.signatures.get(owner).signature.as_ref().is_some_and(signature)
                    || self.documentation_signature(owner).is_some_and(|documentation| annotations(&documentation.annotations))
            }
            NativeTypeContext::Annotation(key) => self.types.annotation(key).is_some_and(|annotation| matches!(&annotation.syntax,
                luanalysis_syntax::AnnotationDeclaration::Conditional(conditional) if conditional.extends_type.spreads(name))),
            NativeTypeContext::NamedType(_) => false,
        }
    }

    pub(super) fn source_type_parameter_is_result_pack(
        &self,
        parameter: NativeTypeParameterKey,
    ) -> bool {
        if self.source_type_parameter_is_spread(parameter) { return true; }
        let source = self.types.parameter(parameter);
        let project_metadata = self.project_result_pack_alias_metadata();
        let local_aliases;
        let aliases = if let Some((aliases, _)) = project_metadata {
            aliases
        } else {
            local_aliases = self.source_result_pack_alias_recipes();
            &local_aliases
        };
        let analysis = ResultPackSourceAnalysis {
            aliases,
            registry: self,
        };
        let local_summaries;
        let aliases = if let Some((_, summaries)) = project_metadata {
            summaries.get_or_init(|| analysis.alias_summaries())
        } else {
            local_summaries = analysis.alias_summaries();
            &local_summaries
        };
        let input_types: Vec<_> = match source.owner {
            NativeTypeContext::Function(function) => {
                let function = self.function(function);
                function
                    .annotations
                    .params
                    .iter()
                    .map(|parameter| &parameter.ty)
                    .chain(
                        function
                            .annotations
                            .vararg
                            .iter()
                            .map(|parameter| &parameter.ty),
                    )
                    .collect()
            }
            NativeTypeContext::Signature(owner) => self
                .signatures
                .get(owner)
                .signature
                .as_ref()
                .map_or_else(Vec::new, |signature| {
                    signature
                        .params
                        .iter()
                        .filter_map(|parameter| parameter.ty.as_ref())
                        .chain(signature.vararg.iter().map(|parameter| &parameter.ty))
                        .collect()
                }),
            NativeTypeContext::NamedType(owner) => match &self
                .named_metadata()
                .declaration(owner)
                .expect("original generic owner")
                .syntax
                .kind
            {
                NamedTypeDeclarationKind::Alias { target } => vec![target],
                NamedTypeDeclarationKind::Record { .. } => Vec::new(),
            },
            // An `infer` type parameter is a value's type.
            NativeTypeContext::Annotation(_) => return false,
        };
        let mut summary = ResultPackSourceSummary::default();
        for ty in input_types {
            summary.absorb(&analysis.summarize(
                &self.result_pack_source_type(Some(source.owner), ty),
                &aliases,
            ));
        }
        summary
            .producers
            .get(&parameter)
            .is_some_and(|producer| !producer.scalar_row && !producer.inputs.contains(&parameter))
    }
    pub(super) fn source_function_may_have_type_predicate(&self, key: NativeFunctionKey) -> bool {
        let function = self.function(key);
        if let Some(annotation) = &function.annotations.returns {
            return matches!(annotation.types.as_slice(), [TypeExpr { kind: TypeExprKind::TypePredicate { .. }, .. }]);
        }
        let Some(annotation) = &function.annotations.type_annotation else { return false };
        let [ty] = annotation.types.as_slice() else { return false };
        let mut ty = ty.value_syntax();
        while let TypeExprKind::Parenthesized(inner) = &ty.kind { ty = inner.value_syntax(); }
        let TypeExprKind::Function(signature) = &ty.kind else { return false };
        self.signature_key_for(signature.span).is_some()
            && matches!(signature.returns.as_slice(), [TypeExpr { kind: TypeExprKind::TypePredicate { .. }, .. }])
    }

    pub(super) fn source_signature_may_have_type_predicate(&self, key: NativeSignatureKey) -> bool {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function { return self.source_function_may_have_type_predicate(function); }
        let Some(signature) = &recipe.signature else { return false };
        let returns = if signature.returns_span.is_none() {
            self.documentation_signature(key).and_then(|doc| doc.annotations.returns.as_ref()).map(|annotation| annotation.types.as_slice())
        } else { Some(signature.returns.as_slice()) };
        matches!(returns, Some([TypeExpr { kind: TypeExprKind::TypePredicate { .. }, .. }]))
    }

    pub(super) fn source_function_type_predicate(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<luanalysis_engine::TypePredicate> {
        let function = self.function(key);
        let Some(annotation) = &function.annotations.returns else {
            return self.implementation_annotation_signature(key)
                .and_then(|signature| self.source_signature_type_predicate(signature, queries));
        };
        let [source] = annotation.types.as_slice() else { return None };
        if !matches!(source.kind, TypeExprKind::TypePredicate { .. }) { return None; }
        let parameters = self.metadata(key, queries).0;
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        let predicate = checker.declared_type_predicate(source, &parameters);
        self.merge_report(checker.report);
        predicate
    }

    pub(super) fn source_signature_type_predicate(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<luanalysis_engine::TypePredicate> {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function {
            return self.source_function_type_predicate(function, queries);
        }
        let signature = recipe.signature.as_ref()?;
        let returns = if signature.returns_span.is_none() {
            &self.documentation_signature(key)?.annotations.returns.as_ref()?.types
        } else {
            &signature.returns
        };
        let [source] = returns.as_slice() else { return None };
        if !matches!(source.kind, TypeExprKind::TypePredicate { .. }) { return None; }
        let parameters = self.source_signature_parameters(key, queries);
        let mut checker = self.signature_checker(key, queries);
        let predicate = checker.declared_type_predicate(source, &parameters);
        self.merge_report(checker.report);
        predicate
    }

    /// A missing doc-function result is distinct from an explicit any/void row.
    /// Runtime implementation signatures keep their actual inferred result.
    pub(super) fn signature_has_omitted_result(&self, key: NativeSignatureKey) -> bool {
        let recipe = self.signatures.get(key);
        recipe.ambient_function.is_none()
            && recipe.signature.as_ref().is_some_and(|signature| signature.returns_span.is_none())
            && self.documentation_signature(key).is_none_or(|doc| doc.annotations.returns.is_none())
    }

    pub(super) fn signature_has_method_origin(&self, key: NativeSignatureKey) -> bool {
        let recipe = self.signatures.get(key);
        if let Some(source) = self.documentation_signature(key) {
            return source.owner.is_some();
        }
        let SignatureOwner::Function(function) = recipe.header.owner else { return false };
        luanalysis_engine::NativeTypeProvider::function_is_method(self, function)
            && (recipe.ambient_function == Some(function)
                || self.signatures.function_heads(function).contains(&key)
                || self.implementation_annotation_signature(function) == Some(key))
    }
    /// Whether `function` is the `__call` field of a table constructor, as a metatable's is.
    fn is_call_metamethod(&self, function: luanalysis_engine::NativeFunctionKey) -> bool {
        let span = self.function(function).span;
        self.expressions().sources().any(|(_, source)| {
            let luanalysis_syntax::ExprKind::Table(fields) = &source.expression.kind else { return false };
            fields.iter().any(|field| {
                let luanalysis_syntax::TableFieldKind::Named(name) = &field.kind else { return false };
                let mut value = &field.value;
                while let luanalysis_syntax::ExprKind::Parenthesized(inner) = &value.kind { value = inner; }
                name.text == "__call" && matches!(&value.kind, luanalysis_syntax::ExprKind::Function(f) if f.span == span)
            })
        })
    }
    pub(super) fn signature_available(&self, key: NativeSignatureKey) -> bool {
        if !self.signatures.contains(key) { return false; }
        self.signatures.get(key).documentation.and_then(|id| self.doc_members.for_declaration(id))
            .is_none_or(|source| matches!(source.scope.get(), crate::native_doc_members::DocOwnerScope::Scoped(_) | crate::native_doc_members::DocOwnerScope::Ready(_)))
    }
    pub(crate) fn prepare_documentation_scope_keys(&self, queries: &mut dyn TypeQueries) {
        use crate::native_doc_members::DocOwnerScope;
        for source in self.doc_members.sources() {
            if source.scope.get() != DocOwnerScope::Unprepared { continue; }
            let signature = self.doc_signature_key(source.declaration).expect("original documentation signature");
            self.types.clear_documentation_owner(signature);
            match queries.named_member_scope_owner(source.key) {
                Ok(owner) => {
                    let owner = NativeNamedKey { snapshot: owner.snapshot, index: owner.index };
                    let doc = self.documentation_function(source.declaration).unwrap();
                    if doc.method_colon.is_some() && self.types.self_parameter(owner).is_none()
                        && !doc.annotations.params.iter().any(|tag| tag.name.text == "self")
                    {
                        self.merge_report(super::CheckReport { diagnostics: vec![Diagnostic {
                            code: "LUA2034", span: source.owner_span,
                            message: "Bodyless colon declaration has no original class or shape receiver scope.".into(),
                        }], ..Default::default() });
                        source.scope.set(DocOwnerScope::Failed);
                        self.invalidate_header_metadata();
                        continue;
                    }
                    self.types.attach_documentation_owner(signature, owner);
                    source.scope.set(DocOwnerScope::Scoped(owner));
                    self.invalidate_header_metadata();
                }
                Err(_) => {
                    source.scope.set(DocOwnerScope::Failed);
                    self.invalidate_header_metadata();
                },
            }
        }
    }
    pub(crate) fn validate_documentation_scope_owners(&self, queries: &mut dyn TypeQueries) {
        use crate::native_doc_members::DocOwnerScope;
        for source in self.doc_members.sources() {
            let DocOwnerScope::Scoped(expected) = source.scope.get() else { continue; };
            match queries.named_member_owner(source.key) {
                Ok(owner) if owner.declaration.snapshot == expected.snapshot && owner.declaration.index == expected.index => {
                    source.scope.set(DocOwnerScope::Ready(expected));
                    self.invalidate_header_metadata();
                }
                _ => {
                    source.scope.set(DocOwnerScope::Failed);
                    self.invalidate_header_metadata();
                    self.types.clear_documentation_owner(self.doc_signature_key(source.declaration).unwrap());
                }
            }
        }
    }
    pub(crate) fn prepare_documentation_scopes(&self, queries: &mut dyn TypeQueries) {
        self.prepare_documentation_scope_keys(queries);
        self.validate_documentation_scope_owners(queries);
    }
    pub(crate) fn doc_signature_key(&self, id: DocFunctionDeclarationId) -> Option<NativeSignatureKey> {
        self.signatures.documentation.get(&id).copied()
    }
    fn documentation_signature(&self, key: NativeSignatureKey) -> Option<&luanalysis_syntax::DocFunctionDeclaration> {
        let id = self.signatures.get(key).documentation?;
        self.documentation_function(id)
    }
    pub(super) fn documentation_signature_owner(&self, key: NativeSignatureKey) -> Option<NativeNamedKey> {
        if let Some(source) = self.signatures.get(key).documentation.and_then(|id| self.doc_members.for_declaration(id)) {
            match source.scope.get() {
                crate::native_doc_members::DocOwnerScope::Scoped(owner) | crate::native_doc_members::DocOwnerScope::Ready(owner) => return Some(owner),
                crate::native_doc_members::DocOwnerScope::Failed => return None,
                crate::native_doc_members::DocOwnerScope::Unprepared => {}
            }
        }
        match self.documentation_signature(key)?.owner.as_ref()? {
            DocFunctionOwner::Contextual(id) => self.named_metadata().key_for_syntax(*id),
            DocFunctionOwner::Explicit(name) => {
                let candidates = self.named_candidates(&name.text);
                let [key] = candidates.as_slice() else { return None };
                Some(NativeNamedKey { snapshot:key.snapshot,index:key.index })
            }
        }
    }
    pub(super) fn attach_documentation_signature_owners(&mut self) {
        let owners:Vec<_> = self.signatures.documentation.values().filter_map(|&key| {
            Some((key,self.documentation_signature_owner(key)?))
        }).collect();
        for (signature, owner) in owners { self.types.attach_documentation_owner(signature,owner); }
    }
    pub(super) fn implementation_annotation_signature(
        &self,
        key: NativeFunctionKey,
    ) -> Option<NativeSignatureKey> {
        let annotation = self.function(key).annotations.type_annotation.as_ref()?;
        let mut ty = match annotation.types.as_slice() {
            [ty] => ty,
            _ => return None,
        };
        ty = ty.value_syntax();
        while let TypeExprKind::Parenthesized(inner) = &ty.kind {
            ty = inner.value_syntax();
        }
        match &ty.kind {
            TypeExprKind::Function(signature) => self.signature_key_for(signature.span),
            _ => None,
        }
    }
    pub(super) fn implementation_type_parameters(
        &self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        let mut parameters = self.types.own_parameters(key);
        if let Some(signature) = self.implementation_annotation_signature(key) {
            parameters.extend(
                self.types
                    .own_parameters_in_context(NativeTypeContext::Signature(signature)),
            );
        }
        parameters
    }
    pub(crate) fn signature_context_for_annotation(
        &self,
        span: Span,
    ) -> Option<NativeSignatureKey> {
        self.signatures
            .type_contexts
            .get(&(span.start, span.end))
            .copied()
    }
    pub(super) fn signature_key_for(&self, span: Span) -> Option<NativeSignatureKey> {
        self.signatures.key_for(span)
    }
    /// The original long-form declaration remains private when overloads exist.
    /// Compatibility checks can demand it without executing a placeholder body.
    pub(crate) fn ambient_implementation_signature(&self, function: NativeFunctionKey) -> Option<NativeSignatureKey> {
        (self.source_role() == luanalysis_project::SourceRole::Declaration)
            .then(|| self.signatures.ambient_mains.get(&function).copied()).flatten()
    }
    fn signature_checker<'a>(
        &'a self,
        key: NativeSignatureKey,
        queries: &'a mut dyn TypeQueries,
    ) -> super::Checker<'a> {
        let mut checker = self.checker(queries);
        checker.signature_type_context = Some(key);
        match self.signatures.get(key).context {
            Some(NativeTypeContext::Function(owner)) => checker.type_context = Some(owner),
            Some(NativeTypeContext::NamedType(owner)) => checker.named_type_context = Some(owner),
            _ => {}
        }
        checker
    }
    pub(super) fn source_signature_parameters(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function {
            return self.metadata(function, queries).0;
        }
        let source = recipe.signature.as_ref().expect("original function-type annotation");
        let mut checker = self.signature_checker(key, queries);
        let mut optional = false;
        let mut parameters: Vec<_> = source
            .params
            .iter()
            .map(|parameter| {
                let tag = self.documentation_signature(key).and_then(|doc| doc.annotations.params.iter().find(|tag| tag.name.text == parameter.name.text));
                let parameter_optional = parameter.optional || tag.is_some_and(|tag| tag.optional);
                if optional && !parameter_optional {
                    checker.error(
                        "TS1016",
                        parameter.span,
                        "A required parameter cannot follow an optional parameter.",
                    );
                }
                optional |= parameter_optional;
                let ty = parameter
                    .ty
                    .as_ref()
                    .or_else(|| tag.map(|tag| &tag.ty))
                    .map(|ty| checker.annotation(ty))
                    .unwrap_or_else(|| {
                        checker.implicit_parameter_any(
                            "TS7006",
                            parameter.name.span,
                            "",
                            "Function-type parameter implicitly has an any type.",
                        );
                        Type::Any
                    });
                Parameter {
                    name: parameter.name.text.clone(),
                    ty,
                    optional: parameter_optional,
                }
            })
            .collect();
        if self.documentation_signature(key).is_some_and(|source| source.method_colon.is_some()) {
            let explicit = self.documentation_signature(key).unwrap().annotations.params.iter().find(|tag| tag.name.text == "self");
            let ty = explicit.map(|tag| checker.annotation(&tag.ty)).or_else(|| {
                self.documentation_signature_owner(key).and_then(|owner| self.types.self_parameter(owner)).map(Type::NativeTypeParameter)
            }).expect("a published bodyless colon signature has a prepared original receiver contract");
            parameters.insert(0,Parameter {name:"self".into(),ty,optional:false});
        }
        if let Some(function) = self.signature_method_function(key) {
            let original = self.function(function);
            let explicit = (!original.params.iter().any(|parameter| parameter.text == "self"))
                .then(|| original.annotations.params.iter().find(|parameter| parameter.name.text == "self"))
                .flatten();
            let receiver = explicit.map(|parameter| checker.annotation(&parameter.ty))
                .or_else(|| self.default_receiver_type(original, checker.engine))
                .unwrap_or_else(|| checker.engine.error_type());
            parameters.insert(0, Parameter { name: "self".into(), ty: receiver, optional: false });
        } else if let SignatureOwner::Function(function) = self.signatures.get(key).header.owner
            && self.signatures.function_heads(function).contains(&key)
            && self.is_call_metamethod(function)
        {
            // A `__call` metamethod's overloads describe calling its table, which Lua passes
            // first; its implementation names that receiver itself.
            let name = self.function(function).params.first().map_or("self", |parameter| &parameter.text).into();
            parameters.insert(0, Parameter { name, ty: Type::Any, optional: false });
        }
        self.merge_report(checker.report);
        parameters
    }
    pub(super) fn source_signature_rest(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function {
            return self.source_rest_pack(function, queries);
        }
        let signature = recipe.signature.as_ref().expect("original function-type annotation");
        let Some(source) = signature.vararg.as_ref().or_else(|| self.documentation_signature(key).and_then(|doc| doc.annotations.vararg.as_ref())) else {
            return signature.parameters_span.is_none().then(|| ValuePack::variadic(Type::Any));
        };
        let mut checker = self.signature_checker(key, queries);
        let pack = checker.rest_annotation(source);
        self.merge_report(checker.report);
        Some(pack)
    }
    pub(super) fn source_signature_return(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function {
            let contract = self.declared_pack_contract(function, queries)
                .or_else(|| self.function(function).body.is_empty().then(ValuePack::empty))
                .expect("public ambient main has a written or syntactically empty contract");
            return queries.declared_result_pack(&contract);
        }
        let source = recipe.signature.as_ref().expect("original function-type annotation");
        if source.returns_span.is_none() && let Some(annotation) = self.documentation_signature(key).and_then(|doc| doc.annotations.returns.as_ref()) {
            let mut checker=self.signature_checker(key,queries);
            let pack=AnnotationPack::annotation(annotation).lower(&mut |ty| checker.annotation(ty));
            self.merge_report(checker.report);
            return pack;
        }
        if source.returns_span.is_none() {
            return ValuePack::variadic(Type::Any);
        }
        let mut checker = self.signature_checker(key, queries);
        let pack = AnnotationPack::new(&source.returns, source.packs.as_deref(), source.span)
            .lower(&mut |ty| checker.annotation(ty));
        self.merge_report(checker.report);
        pack
    }
    pub(super) fn check_signature_declarations(&self, queries: &mut dyn TypeQueries) {
        for recipe in &self.signatures.recipes {
            if !self.signature_available(recipe.key) { continue; }
            if recipe.ambient_function.is_some() && self.source_role() != luanalysis_project::SourceRole::Declaration {
                continue;
            }
            for parameter in self.signature_parameters_keys(recipe.key) {
                queries.type_parameter_constraint(parameter);
            }
            // This demands the backend's lazy parameter and result recipes,
            // including declarations that were never used by a call.
            let ty = queries.signature_type(recipe.key);
            queries.format(&ty);
            self.source_signature_type_predicate(recipe.key, queries);
            if let Some(source) = self.documentation_signature(recipe.key) {
                let mut checker=self.signature_checker(recipe.key,queries);
                let mut names=crate::collections::HashSet::default();
                for tag in &source.annotations.params {
                    if !names.insert(&tag.name.text) || (!source.signature.params.iter().any(|p|p.name.text==tag.name.text) && !(tag.name.text=="self" && source.method_colon.is_some())) {
                        checker.error("LUA2011",tag.name.span,"Parameter annotation has a duplicate or unknown original parameter owner.");
                    }
                    checker.annotation(&tag.ty);
                }
                if !source.annotations.overloads.is_empty() || source.annotations.type_annotation.is_some() {
                    checker.error("LUA2034",source.tag_span,"Bodyless function declarations use their original header and long-form tags; @overload and @type on this head are unsupported.");
                }
                self.merge_report(checker.report);
            }
        }
    }
    pub(super) fn check_all_signatures(&self, queries: &mut dyn TypeQueries) {
        self.check_signature_declarations(queries);
        if self.source_role() == luanalysis_project::SourceRole::Declaration {
            // The original annotated main is a private declaration contract.
            // Compare pure signature recipes; placeholder bodies stay opaque.
            for (&function, heads) in &self.signatures.functions {
                let Some(main) = self.ambient_implementation_signature(function) else { continue };
                let implementation = queries.signature_type(main);
                for &head in heads {
                    if !queries.signature_implementation_compatible(head, &implementation) {
                        self.report_signature_incompatibility(head, &implementation, queries);
                    }
                }
            }
            return;
        }
        for (&function, heads) in &self.signatures.functions {
            let selected = self.expressions().final_facets(function);
            let implementations = if selected.is_empty() {
                vec![Type::NativeFunction(function)]
            } else {
                selected
            };
            for implementation in implementations {
                let implementation = queries
                    .function_implementation_type(&implementation)
                    .expect("an actual runtime declaration has a private implementation signature");
                for &head in heads {
                    if !queries.signature_implementation_compatible(head, &implementation) {
                        self.report_signature_incompatibility(head, &implementation, queries);
                    }
                }
            }
        }
    }
    fn report_signature_incompatibility(
        &self,
        head: NativeSignatureKey,
        implementation: &Type,
        queries: &mut dyn TypeQueries,
    ) {
        let overload = queries.signature_type(head);
        let mut report = super::CheckReport::default();
        report.push_type_diagnostic(
            "TS2394",
            self.signatures.get(head).span,
            "Overload signature '{}' is not compatible with implementation signature '{}'.",
            &[&overload, implementation],
        );
        self.merge_report(report);
    }
    pub(super) fn signature_parameters_keys(
        &self,
        key: NativeSignatureKey,
    ) -> Vec<NativeTypeParameterKey> {
        if let Some(function) = self.signatures.get(key).ambient_function {
            return self.implementation_type_parameters(function);
        }
        let mut parameters = self
            .types
            .own_parameters_in_context(NativeTypeContext::Signature(key));
        if let Some(function) = self.ambient_annotation_function(key) {
            let mut original = self.types.own_parameters(function);
            original.extend(parameters);
            parameters = original;
        }
        parameters
    }
    pub(super) fn signature_outer_keys(
        &self,
        key: NativeSignatureKey,
    ) -> Vec<NativeTypeParameterKey> {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function {
            let source = self.function(function);
            let context = Some(NativeTypeContext::Function(function));
            let mut used = crate::collections::HashSet::default();
            for constraint in source.annotations.generics.iter().filter_map(|parameter| parameter.constraint.as_ref()) {
                self.annotation_parameter_dependencies(context, constraint, &mut used);
            }
            for parameter in &source.annotations.params {
                self.annotation_parameter_dependencies(context, &parameter.ty, &mut used);
            }
            if let Some(vararg) = &source.annotations.vararg {
                self.annotation_parameter_dependencies(context, &vararg.ty, &mut used);
            }
            for annotation in source.annotations.returns.iter().chain(source.annotations.type_annotation.iter()) {
                for ty in &annotation.types { self.annotation_parameter_dependencies(context, ty, &mut used); }
            }
            if self.implicit_receiver(source).is_some()
                && let Some(owner) = self.method_owner(function)
            {
                if let Some(parameter) = self.types.self_parameter(owner) { used.insert(parameter); }
                used.extend(self.types.own_parameters_in_context(NativeTypeContext::NamedType(owner)));
            }
            if self.implicit_receiver(source).is_some()
                && self.method_owner(function).is_none()
                && let Some(parameter) = self.contextual_self_dependency(function)
            { used.insert(parameter); }
            let mut outer = self.types.outer_parameters(function, &used);
            self.extend_contextual_self_dependencies(NativeTypeContext::Function(function), &used, &mut outer);
            return outer;
        }
        let source = recipe.signature.as_ref().expect("original function-type annotation");
        let mut used = crate::collections::HashSet::default();
        if let Some(function) = self.signature_method_function(key)
            && let Some(parameter) = self.method_owner(function)
                .and_then(|owner| self.types.self_parameter(owner))
                .or_else(|| self.contextual_self_dependency(function))
        {
            used.insert(parameter);
        }
        let context = Some(NativeTypeContext::Signature(key));
        if let Some(doc) = self.documentation_signature(key) {
            for generic in &doc.annotations.generics { if let Some(ty)=&generic.constraint { self.annotation_parameter_dependencies(context,ty,&mut used); } }
            for parameter in &doc.annotations.params { self.annotation_parameter_dependencies(context,&parameter.ty,&mut used); }
            if let Some(vararg)=&doc.annotations.vararg { self.annotation_parameter_dependencies(context,&vararg.ty,&mut used); }
            if let Some(returns)=&doc.annotations.returns { for ty in &returns.types { self.annotation_parameter_dependencies(context,ty,&mut used); } }
            if doc.method_colon.is_some() && let Some(owner)=self.documentation_signature_owner(key) && let Some(parameter)=self.types.self_parameter(owner) { used.insert(parameter); }
        }
        for parameter in &source.generics {
            if let Some(ty) = &parameter.constraint {
                self.annotation_parameter_dependencies(context, ty, &mut used)
            }
        }
        for parameter in &source.params {
            if let Some(ty) = &parameter.ty {
                self.annotation_parameter_dependencies(context, ty, &mut used)
            }
        }
        if let Some(vararg) = &source.vararg {
            self.annotation_parameter_dependencies(context, &vararg.ty, &mut used)
        }
        for returned in &source.returns {
            self.annotation_parameter_dependencies(context, returned, &mut used)
        }
        let mut outer = self
            .types
            .outer_parameters_in_context(NativeTypeContext::Signature(key), &used);
        // A function type written in a conditional type sees its `infer` type parameters.
        outer.extend(self.types.visible_annotation_parameters(recipe.span).filter(|parameter| used.contains(parameter) && !outer.contains(parameter)).collect::<Vec<_>>());
        self.extend_contextual_self_dependencies(NativeTypeContext::Signature(key), &used, &mut outer);
        if self.ambient_annotation_function(key).is_some() {
            let formals = self.signature_parameters_keys(key);
            outer.retain(|key| !formals.contains(key));
        }
        outer
    }
    fn ambient_annotation_function(&self, key: NativeSignatureKey) -> Option<NativeFunctionKey> {
        if self.source_role() != luanalysis_project::SourceRole::Declaration {
            return None;
        }
        let SignatureOwner::Function(function) = self.signatures.get(key).header.owner else {
            return None;
        };
        (self.implementation_annotation_signature(function) == Some(key)).then_some(function)
    }

    /// Only public method overload heads omit the source language's implicit
    /// physical receiver. Nested function-type annotations keep their own slots.
    fn signature_method_function(&self, key: NativeSignatureKey) -> Option<NativeFunctionKey> {
        let SignatureOwner::Function(function) = self.signatures.get(key).header.owner else { return None };
        (self.signatures.function_heads(function).contains(&key)
            && self.implicit_receiver(self.function(function)).is_some()).then_some(function)
    }
    pub(crate) fn signature_applied_owner_function(&self, key: NativeSignatureKey) -> Option<NativeFunctionKey> {
        let recipe = self.signatures.get(key);
        if let Some(function) = recipe.ambient_function { return Some(function); }
        let SignatureOwner::Function(function) = recipe.header.owner else { return None };
        self.signatures.function_heads(function).contains(&key).then_some(function)
    }
}

pub(super) struct SignatureRecipe {
    pub key: NativeSignatureKey,
    pub header: SignatureHeader,
    pub context: Option<NativeTypeContext>,
    pub signature: Option<FunctionType>,
    /// A real ambient function declaration's written parameter/result tags.
    /// It has an original key and source span, not a synthesized function AST.
    pub ambient_function: Option<NativeFunctionKey>,
    pub documentation: Option<DocFunctionDeclarationId>,
    pub span: Span,
}

pub(super) struct NativeSignatures {
    snapshot: u64,
    recipes: Vec<SignatureRecipe>,
    spans: HashMap<(usize, usize), NativeSignatureKey>,
    type_contexts: HashMap<(usize, usize), NativeSignatureKey>,
    functions: HashMap<NativeFunctionKey, Vec<NativeSignatureKey>>,
    ambient_mains: HashMap<NativeFunctionKey, NativeSignatureKey>,
    named: HashMap<NativeNamedKey, Vec<NativeSignatureKey>>,
    operators: HashMap<(NativeNamedKey, luanalysis_engine::NativeOperator), Vec<NativeSignatureKey>>,
    documentation: HashMap<DocFunctionDeclarationId,NativeSignatureKey>,
    sites: HashMap<(usize, usize), SignatureSite>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Which function's body each function, and each span, is innermost in. Bodies nest as the
/// syntax does, so one sweep in source order finds every function's, where comparing every
/// pair of functions was quadratic in a file's functions.
struct FunctionNesting {
    /// Function indices by their bodies' starts.
    by_body: Vec<usize>,
    /// The innermost function whose body contains each function.
    parents: Vec<Option<usize>>,
}

impl FunctionNesting {
    fn new(functions: &[Function]) -> Self {
        let mut order: Vec<usize> = (0..functions.len()).collect();
        order.sort_by_key(|&index| (functions[index].span.start, std::cmp::Reverse(functions[index].span.end)));
        let mut parents = vec![None; functions.len()];
        let mut open: Vec<usize> = Vec::new();
        for index in order {
            let span = functions[index].span;
            while open.last().is_some_and(|&outer| functions[outer].body_span.end < span.end) { open.pop(); }
            parents[index] = open.last().copied().filter(|&outer| {
                functions[outer].span != span && functions[outer].body_span.start <= span.start
            });
            open.push(index);
        }
        let mut by_body: Vec<usize> = (0..functions.len()).collect();
        by_body.sort_by_key(|&index| (functions[index].body_span.start, std::cmp::Reverse(functions[index].body_span.end)));
        Self { by_body, parents }
    }

    /// The innermost function whose body contains `span`.
    fn innermost(&self, functions: &[Function], span: Span) -> Option<usize> {
        let before = self.by_body.partition_point(|&index| functions[index].body_span.start <= span.start);
        let mut candidate = before.checked_sub(1).map(|position| self.by_body[position]);
        while let Some(index) = candidate {
            let body = functions[index].body_span;
            if body.start <= span.start && span.end <= body.end { return Some(index); }
            candidate = self.parents[index];
        }
        None
    }
}

impl NativeSignatures {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        functions: &[Function],
        named: &NativeNamed,
    ) -> Self {
        let mut result = Self {
            snapshot,
            recipes: Vec::new(),
            spans: HashMap::default(),
            type_contexts: HashMap::default(),
            functions: HashMap::default(),
            ambient_mains: HashMap::default(),
            named: HashMap::default(),
            operators: HashMap::default(),
            documentation: HashMap::default(),
            sites: HashMap::default(),
            diagnostics: Vec::new(),
        };
        let nesting = FunctionNesting::new(functions);
        for declaration in &file.function_declarations {
            if declaration.invalid_owner { continue; }
            let context = match declaration.owner.as_ref() {
                Some(DocFunctionOwner::Contextual(id)) => named.key_for_syntax(*id).map(NativeTypeContext::NamedType),
                Some(DocFunctionOwner::Explicit(name)) => {
                    let mut candidates=named.declarations().iter().filter(|source|source.syntax.name.text==name.text);
                    let candidate=candidates.next();
                    if candidates.next().is_none() { candidate.map(|source|NativeTypeContext::NamedType(source.key)) } else { None }
                }
                None => None,
            };
            let key=result.insert(SignatureOwner::Standalone,context,None,&declaration.signature,declaration.span,declaration.group_span);
            result.recipes[key.index as usize].documentation=Some(declaration.id);
            result.recipes[key.index as usize].header.has_direct_literal_parameter |= declaration.signature.params.iter().any(|parameter| {
                parameter.ty.as_ref().or_else(||declaration.annotations.params.iter().find(|tag|tag.name.text==parameter.name.text).map(|tag|&tag.ty)).is_some_and(|ty| {
                    let ty=ty.value_syntax();
                    matches!(ty.kind,TypeExprKind::Literal(_)) || matches!(&ty.kind,TypeExprKind::Named(name) if name=="nil")
                })
            });
            result.documentation.insert(declaration.id,key);
            let context=Some(NativeTypeContext::Signature(key));
            for generic in &declaration.annotations.generics { if let Some(ty)=&generic.constraint { result.visit_type(SignatureOwner::Standalone,context,Some(key),ty); } }
            for parameter in &declaration.annotations.params { result.visit_type(SignatureOwner::Standalone,context,Some(key),&parameter.ty); }
            if let Some(vararg)=&declaration.annotations.vararg { result.visit_type(SignatureOwner::Standalone,context,Some(key),&vararg.ty); }
            if let Some(returns)=&declaration.annotations.returns { for ty in &returns.types { result.visit_type(SignatureOwner::Standalone,context,Some(key),ty); } }
        }
        // An overload written into a documented local's type is that type's call signature.
        for head in file.overload_annotations.iter().filter(|head| !head.in_type) {
            let named_key = match head.named_candidates.as_slice() {
                [] => None,
                [id] => named.key_for_syntax(*id),
                _ => {
                    result.diagnostics.push(Diagnostic {
                        code: "LUA2034",
                        span: head.tag_span,
                        message: "overload documentation has multiple candidate named owners"
                            .into(),
                    });
                    continue;
                }
            };
            let function = functions
                .iter()
                .enumerate()
                .find(|(_, function)| {
                    function
                        .annotations
                        .overloads
                        .iter()
                        .any(|reference| reference.id == head.id)
                })
                .map(|(index, _)| NativeFunctionKey {
                    snapshot,
                    index: index as u32,
                });
            let (owner, context) = if let Some(key) = named_key {
                (
                    SignatureOwner::NamedType(NativeTypeDeclarationKey {
                        snapshot: key.snapshot,
                        index: key.index,
                    }),
                    Some(NativeTypeContext::NamedType(key)),
                )
            } else if let Some(key) = function {
                (
                    SignatureOwner::Function(key),
                    Some(NativeTypeContext::Function(key)),
                )
            } else {
                result.diagnostics.push(Diagnostic {
                    code: "LUA2034",
                    span: head.tag_span,
                    message: "overload has no original function or named declaration owner".into(),
                });
                continue;
            };
            // A function's `---@generic` parameters are each of its overloads' own, as LuaLS and
            // EmmyLua Analyzer read them and as TypeScript gives each overload its own: a call
            // infers them for the overload it selects.
            let generic_overload;
            let signature = match function {
                Some(key) if named_key.is_none() && head.signature.generics.is_empty() => {
                    let generics = &functions[key.index as usize].annotations.generics;
                    if generics.is_empty() { head.signature.as_ref() } else {
                        generic_overload = FunctionType { generics: generics.clone(), ..(*head.signature).clone() };
                        &generic_overload
                    }
                }
                _ => head.signature.as_ref(),
            };
            let key = result.insert(
                owner,
                context,
                None,
                signature,
                head.span,
                head.group_span,
            );
            if let Some(named) = named_key {
                result.named.entry(named).or_default().push(key);
            } else if let Some(function) = function {
                result.functions.entry(function).or_default().push(key);
            }
        }
        for head in &file.operator_annotations {
            let named_key = match head.named_candidates.as_slice() {
                [id] => named.key_for_syntax(*id),
                candidates => {
                    result.diagnostics.push(Diagnostic {
                        code: "LUA2034",
                        span: head.tag_span,
                        message: if candidates.is_empty() {
                            "@operator has no class, shape or alias in its documentation group".into()
                        } else {
                            "operator documentation has multiple candidate named owners".into()
                        },
                    });
                    continue;
                }
            };
            let Some(named_key) = named_key else { continue };
            let key = result.insert(
                SignatureOwner::NamedType(NativeTypeDeclarationKey { snapshot: named_key.snapshot, index: named_key.index }),
                Some(NativeTypeContext::NamedType(named_key)),
                None,
                &head.signature,
                head.span,
                head.group_span,
            );
            result.operators.entry((named_key, native_operator(head.operator))).or_default().push(key);
        }
        for (index, function) in functions.iter().enumerate() {
            let key = NativeFunctionKey {
                snapshot,
                index: index as u32,
            };
            let context = Some(NativeTypeContext::Function(key));
            let owner = SignatureOwner::Function(key);
            for parameter in &function.annotations.params {
                result.visit_type(owner, context, None, &parameter.ty);
            }
            for generic in &function.annotations.generics {
                if let Some(ty) = &generic.constraint {
                    result.visit_type(owner, context, None, ty);
                }
            }
            if let Some(vararg) = &function.annotations.vararg {
                result.visit_type(owner, context, None, &vararg.ty);
            }
            for annotation in function
                .annotations
                .type_annotation
                .iter()
                .chain(function.annotations.returns.iter())
            {
                for ty in &annotation.types {
                    result.visit_type(owner, context, None, ty);
                }
            }
        }
        for declaration in &file.type_declarations {
            let Some(key) = named.key_for_syntax(declaration.id) else {
                continue;
            };
            let owner = SignatureOwner::NamedType(NativeTypeDeclarationKey {
                snapshot: key.snapshot,
                index: key.index,
            });
            let context = Some(NativeTypeContext::NamedType(key));
            for parameter in &declaration.generics {
                if let Some(ty) = &parameter.constraint {
                    result.visit_type(owner, context, None, ty)
                }
            }
            match &declaration.kind {
                NamedTypeDeclarationKind::Alias { target } => {
                    result.visit_type(owner, context, None, target)
                }
                NamedTypeDeclarationKind::Record { bases, fields, .. } => {
                    for base in bases {
                        result.visit_type(owner, context, None, base)
                    }
                    for field in fields {
                        if let TypeFieldKey::Indexed(key) = &field.key {
                            result.visit_type(owner, context, None, key)
                        }
                        result.visit_type(owner, context, None, &field.ty)
                    }
                }
            }
        }
        // Statement annotations are independent of the annotated function's
        // own parameter tags. Preserve the actual enclosing function owner;
        // root annotations have a standalone declaration identity.
        result.visit_statements(SignatureOwner::Standalone, None, &file.statements);
        for (index, function) in functions.iter().enumerate() {
            let key = NativeFunctionKey {
                snapshot,
                index: index as u32,
            };
            result.visit_statements(
                SignatureOwner::Function(key),
                Some(NativeTypeContext::Function(key)),
                &function.body,
            );
        }
        for (span, annotation) in file.expression_assertions.iter()
            .map(|assertion| (assertion.expression, &assertion.annotation))
            .chain(file.return_annotations.iter().map(|source| (source.statement, &source.annotation)))
        {
            // Assertions describe the expression at its creation/use site. A
            // function literal's own body scope must not become the parent of
            // the annotation applied to that literal.
            let enclosing = nesting.innermost(functions, span)
                .map(|index| NativeFunctionKey {
                    snapshot,
                    index: index as u32,
                });
            let owner = enclosing.map_or(SignatureOwner::Standalone, SignatureOwner::Function);
            let context = enclosing.map(NativeTypeContext::Function);
            for ty in &annotation.types {
                result.visit_type(owner, context, None, ty);
            }
        }
        for field in named.field_annotations() {
            let owner = match field.lexical_context {
                Some(NativeTypeContext::Function(function)) => SignatureOwner::Function(function),
                Some(NativeTypeContext::NamedType(owner)) => SignatureOwner::NamedType(NativeTypeDeclarationKey { snapshot: owner.snapshot, index: owner.index }),
                _ => SignatureOwner::Standalone,
            };
            for ty in &field.annotation.types {
                result.visit_type(owner, field.lexical_context, None, ty);
            }
        }
        for extension in named.augmentations() {
            let owner = match extension.candidates.as_slice() {
                [key] => SignatureOwner::NamedType(NativeTypeDeclarationKey {
                    snapshot: key.snapshot,
                    index: key.index,
                }),
                _ => SignatureOwner::Standalone,
            };
            // Generic names belong to the lexical/doc group, not automatically
            // to the augmentation target. The existing named registry retains
            // ambiguous documentation candidates for normal binding diagnostics.
            let context = extension.lexical_context.or_else(|| {
                match extension.documentation_contexts.as_slice() {
                    [key] => Some(NativeTypeContext::NamedType(*key)),
                    _ => None,
                }
            });
            for field in &extension.syntax.fields {
                if let TypeFieldKey::Indexed(key) = &field.key {
                    result.visit_type(owner, context, None, key)
                }
                result.visit_type(owner, context, None, &field.ty);
            }
        }
        for (index, function) in functions.iter().enumerate() {
            // Declaration-library bodies are opaque placeholders. An inner
            // function's tags cannot become additional public declarations.
            if nesting.parents[index].is_some() {
                continue;
            }
            let owner = NativeFunctionKey { snapshot, index: index as u32 };
            if let Some(annotation) = function.annotations.type_annotation.as_ref() {
                if let [ty] = annotation.types.as_slice() {
                    let mut ty = ty.value_syntax();
                    while let TypeExprKind::Parenthesized(inner) = &ty.kind { ty = inner.value_syntax(); }
                    if let TypeExprKind::Function(signature) = &ty.kind
                        && signature.parameters_span.is_some()
                        && let Some(key) = result.key_for(signature.span)
                    {
                        result.ambient_mains.insert(owner, key);
                    }
                }
                continue;
            }
            let complete = function.params.iter().all(|parameter| function.annotations.params.iter()
                .any(|tag| tag.name.text == parameter.text))
                && (function.annotations.returns.is_some() || function.body.is_empty());
            if !complete { continue; }
            let key = NativeSignatureKey { snapshot, index: result.recipes.len() as u32 };
            let next_site = SignatureSite { snapshot, index: result.sites.len() as u32 };
            let site = result.functions.get(&owner).and_then(|heads| heads.first())
                .map(|key| result.get(*key).header.site)
                .unwrap_or_else(|| *result.sites.entry((function.span.start, function.span.end)).or_insert(next_site));
            let literal = function.annotations.params.iter().any(|parameter| {
                let ty = parameter.ty.value_syntax();
                matches!(ty.kind, TypeExprKind::Literal(_)) || matches!(&ty.kind, TypeExprKind::Named(name) if name == "nil")
            });
            result.recipes.push(SignatureRecipe {
                key,
                header: SignatureHeader { owner: SignatureOwner::Function(owner), site, parent: None, has_direct_literal_parameter: literal },
                context: Some(NativeTypeContext::Function(owner)),
                signature: None,
                ambient_function: Some(owner),
                documentation: None,
                span: function.span,
            });
            result.ambient_mains.insert(owner, key);
        }
        result
    }

    fn visit_statements(
        &mut self,
        owner: SignatureOwner,
        context: Option<NativeTypeContext>,
        statements: &[Stmt],
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { annotation, .. } | StmtKind::Assign { annotation, .. } => {
                    if let Some(annotation) = annotation {
                        for ty in &annotation.types {
                            self.visit_type(owner, context, None, ty)
                        }
                    }
                }
                StmtKind::NumericFor { name_type, body, .. } => {
                    if let Some(ty) = name_type { self.visit_type(owner, context, None, ty) }
                    self.visit_statements(owner, context, body)
                }
                StmtKind::GenericFor { name_types, body, .. } => {
                    for ty in name_types.iter().flatten() { self.visit_type(owner, context, None, ty) }
                    self.visit_statements(owner, context, body)
                }
                StmtKind::Block { body, .. }
                | StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. } => self.visit_statements(owner, context, body),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.visit_statements(owner, context, &branch.body)
                    }
                    if let Some(branch) = else_branch {
                        self.visit_statements(owner, context, &branch.body)
                    }
                }
                // Each runtime function body is visited separately under its
                // own original declaration identity, including function literals.
                _ => {}
            }
        }
    }

    fn insert(
        &mut self,
        owner: SignatureOwner,
        context: Option<NativeTypeContext>,
        parent: Option<NativeSignatureKey>,
        signature: &FunctionType,
        span: Span,
        site_span: Span,
    ) -> NativeSignatureKey {
        if let Some(key) = self.spans.get(&(signature.span.start, signature.span.end)) {
            return *key;
        }
        let key = NativeSignatureKey {
            snapshot: self.snapshot,
            index: self.recipes.len() as u32,
        };
        let next = SignatureSite {
            snapshot: self.snapshot,
            index: self.sites.len() as u32,
        };
        let site = *self
            .sites
            .entry((site_span.start, site_span.end))
            .or_insert(next);
        let literal = signature.params.iter().any(|parameter| {
            parameter.ty.as_ref().is_some_and(|ty| {
                let ty = ty.value_syntax();
                matches!(&ty.kind, TypeExprKind::Literal(_))
                    || matches!(&ty.kind,TypeExprKind::Named(name) if name=="nil")
            })
        });
        self.spans
            .insert((signature.span.start, signature.span.end), key);
        self.recipes.push(SignatureRecipe {
            key,
            header: SignatureHeader {
                owner,
                site,
                parent,
                has_direct_literal_parameter: literal,
            },
            context,
            signature: Some(signature.clone()),
            ambient_function: None,
            documentation: None,
            span,
        });
        let nested = Some(NativeTypeContext::Signature(key));
        for parameter in &signature.generics {
            if let Some(ty) = &parameter.constraint {
                self.visit_type(owner, nested, Some(key), ty)
            }
        }
        for parameter in &signature.params {
            if let Some(ty) = &parameter.ty {
                self.visit_type(owner, nested, Some(key), ty)
            }
        }
        if let Some(vararg) = &signature.vararg {
            self.visit_type(owner, nested, Some(key), &vararg.ty)
        }
        for ty in &signature.returns {
            self.visit_type(owner, nested, Some(key), ty)
        }
        key
    }

    fn visit_type(
        &mut self,
        owner: SignatureOwner,
        context: Option<NativeTypeContext>,
        parent: Option<NativeSignatureKey>,
        ty: &TypeExpr,
    ) {
        if let Some(NativeTypeContext::Signature(signature)) = context {
            self.type_contexts
                .insert((ty.span.start, ty.span.end), signature);
        }
        match &ty.kind {
            TypeExprKind::Function(signature) => {
                self.insert(
                    owner,
                    context,
                    parent,
                    signature,
                    signature.span,
                    signature.span,
                );
            }
            TypeExprKind::Union(types) | TypeExprKind::Intersection(types) | TypeExprKind::Generic { args: types, .. } => {
                for ty in types {
                    self.visit_type(owner, context, parent, ty)
                }
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    if let TypeFieldKey::Indexed(key) = &field.key {
                        self.visit_type(owner, context, parent, key)
                    }
                    self.visit_type(owner, context, parent, &field.ty)
                }
            }
            TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element)
            | TypeExprKind::TypePredicate { ty: Some(element), .. } | TypeExprKind::KeyOf(element) => {
                self.visit_type(owner, context, parent, element)
            }
            TypeExprKind::IndexedAccess { object, index } => {
                self.visit_type(owner, context, parent, object);
                self.visit_type(owner, context, parent, index)
            }
            TypeExprKind::Conditional { check, extends, yes, no } => {
                for ty in [check, extends, yes, no] {
                    self.visit_type(owner, context, parent, ty)
                }
            }
            TypeExprKind::Infer { constraint: Some(constraint), .. } => self.visit_type(owner, context, parent, constraint),
            TypeExprKind::Mapped(mapped) => {
                for ty in [&mapped.constraint].into_iter().chain(&mapped.name).chain([&mapped.ty]) {
                    self.visit_type(owner, context, parent, ty)
                }
            }
            _ => {}
        }
    }

    pub(super) fn implementation_scopes(
        &self,
        functions: &[Function],
    ) -> Vec<(NativeFunctionKey, NativeSignatureKey)> {
        functions
            .iter()
            .enumerate()
            .filter_map(|(index, function)| {
                let annotation = function.annotations.type_annotation.as_ref()?;
                let mut ty = match annotation.types.as_slice() {
                    [ty] => ty,
                    _ => return None,
                };
                ty = ty.value_syntax();
                while let TypeExprKind::Parenthesized(inner) = &ty.kind {
                    ty = inner.value_syntax();
                }
                let TypeExprKind::Function(signature) = &ty.kind else {
                    return None;
                };
                Some((
                    NativeFunctionKey {
                        snapshot: self.snapshot,
                        index: index as u32,
                    },
                    self.key_for(signature.span)?,
                ))
            })
            .collect()
    }
    pub(super) fn contains(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == self.snapshot && (key.index as usize) < self.recipes.len()
    }
    pub(super) fn get(&self, key: NativeSignatureKey) -> &SignatureRecipe {
        assert!(self.contains(key), "foreign original signature key");
        &self.recipes[key.index as usize]
    }
    pub(super) fn key_for(&self, span: Span) -> Option<NativeSignatureKey> {
        self.spans.get(&(span.start, span.end)).copied()
    }
    pub(super) fn function_heads(&self, key: NativeFunctionKey) -> Vec<NativeSignatureKey> {
        self.functions.get(&key).cloned().unwrap_or_default()
    }
    pub(super) fn ambient_function_heads(&self, key: NativeFunctionKey, expose_implementation: bool) -> Vec<NativeSignatureKey> {
        let mut heads = self.function_heads(key);
        if !heads.is_empty() && !expose_implementation { return heads; }
        if let Some(&main) = self.ambient_mains.get(&key)
            && !heads.contains(&main)
        {
            heads.push(main);
        }
        heads
    }
    pub(super) fn named_heads(&self, key: NativeNamedKey) -> Vec<NativeSignatureKey> {
        self.named.get(&key).cloned().unwrap_or_default()
    }
    pub(super) fn operator_heads(&self, key: NativeNamedKey, operator: luanalysis_engine::NativeOperator) -> Vec<NativeSignatureKey> {
        self.operators.get(&(key, operator)).cloned().unwrap_or_default()
    }
    pub(super) fn scopes(&self, file: &File) -> Vec<NamedGenericScopeRecipe> {
        self.recipes
            .iter()
            .filter_map(|recipe| Some(NamedGenericScopeRecipe {
                owner: NativeTypeContext::Signature(recipe.key),
                parent: recipe.context,
                documentation_parents: Vec::new(),
                parameters: recipe
                    .signature.as_ref()?
                    .generics
                    .iter()
                    .chain(recipe.documentation.and_then(|id|file.function_declarations.iter().find(|doc|doc.id==id)).into_iter().flat_map(|doc|doc.annotations.generics.iter()))
                    .map(|generic| NamedGenericParameterRecipe {
                        name: generic.name.clone(),
                        constraint: generic.constraint.clone(),
                        default: generic.default.clone(),
                    })
                    .collect(),
            }))
            .collect()
    }
}

fn native_operator(operator: luanalysis_syntax::LuaOperator) -> luanalysis_engine::NativeOperator {
    use luanalysis_engine::NativeOperator as Native;
    use luanalysis_syntax::LuaOperator as Lua;
    match operator {
        Lua::Add => Native::Add,
        Lua::Subtract => Native::Subtract,
        Lua::Multiply => Native::Multiply,
        Lua::Divide => Native::Divide,
        Lua::FloorDivide => Native::FloorDivide,
        Lua::Modulo => Native::Modulo,
        Lua::Power => Native::Power,
        Lua::Concat => Native::Concat,
        Lua::BitwiseAnd => Native::BitwiseAnd,
        Lua::BitwiseOr => Native::BitwiseOr,
        Lua::BitwiseXor => Native::BitwiseXor,
        Lua::ShiftLeft => Native::ShiftLeft,
        Lua::ShiftRight => Native::ShiftRight,
        Lua::Negate => Native::Negate,
        Lua::BitwiseNot => Native::BitwiseNot,
        Lua::Length => Native::Length,
        Lua::Less => Native::Less,
        Lua::LessEqual => Native::LessEqual,
    }
}
