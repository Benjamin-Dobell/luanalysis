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
    File, Function, FunctionType, NamedTypeDeclarationKind, Span, Stmt, StmtKind, TypeExpr,
    TypeExprKind, TypeFieldKey,
};
use std::collections::HashMap;

impl NativeRegistry {
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
        let source = &self.signatures.get(key).signature;
        let mut checker = self.signature_checker(key, queries);
        let mut optional = false;
        let parameters = source
            .params
            .iter()
            .map(|parameter| {
                if optional && !parameter.optional {
                    checker.error(
                        "TS1016",
                        parameter.span,
                        "A required parameter cannot follow an optional parameter.",
                    );
                }
                optional |= parameter.optional;
                let ty = parameter
                    .ty
                    .as_ref()
                    .map(|ty| checker.annotation(ty))
                    .unwrap_or_else(|| {
                        checker.error(
                            "TS7006",
                            parameter.name.span,
                            "Function-type parameter implicitly has an any type.",
                        );
                        Type::Any
                    });
                Parameter {
                    name: parameter.name.text.clone(),
                    ty,
                    optional: parameter.optional,
                }
            })
            .collect();
        self.merge_report(checker.report);
        parameters
    }
    pub(super) fn source_signature_rest(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        let signature = &self.signatures.get(key).signature;
        let Some(source) = signature.vararg.as_ref() else {
            return signature.parameters_span.is_none().then(|| ValuePack::variadic(Type::Any));
        };
        let mut checker = self.signature_checker(key, queries);
        let pack = ValuePack::variadic(checker.annotation(&source.ty));
        self.merge_report(checker.report);
        Some(pack)
    }
    pub(super) fn source_signature_return(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let source = &self.signatures.get(key).signature;
        if source.parameters_span.is_none() && source.returns.is_empty() {
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
            for parameter in self.signature_parameters_keys(recipe.key) {
                queries.type_parameter_constraint(parameter);
            }
            // This demands the backend's lazy parameter and result recipes,
            // including declarations that were never used by a call.
            let ty = queries.signature_type(recipe.key);
            queries.format(&ty);
        }
    }
    pub(super) fn check_all_signatures(&self, queries: &mut dyn TypeQueries) {
        self.check_signature_declarations(queries);
        if self.source_role() == luanalysis_project::SourceRole::Declaration {
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
                        self.merge_report(super::CheckReport {diagnostics:vec![Diagnostic {
                            code:"TS2394",span:self.signatures.get(head).span,
                            message:"This overload signature is not compatible with its implementation signature.".into(),
                        }],..Default::default()});
                    }
                }
            }
        }
    }
    pub(super) fn signature_parameters_keys(
        &self,
        key: NativeSignatureKey,
    ) -> Vec<NativeTypeParameterKey> {
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
        let source = &self.signatures.get(key).signature;
        let mut used = std::collections::HashSet::new();
        let context = Some(NativeTypeContext::Signature(key));
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
}

pub(super) struct SignatureRecipe {
    pub key: NativeSignatureKey,
    pub header: SignatureHeader,
    pub context: Option<NativeTypeContext>,
    pub signature: FunctionType,
    pub span: Span,
}

pub(super) struct NativeSignatures {
    snapshot: u64,
    recipes: Vec<SignatureRecipe>,
    spans: HashMap<(usize, usize), NativeSignatureKey>,
    type_contexts: HashMap<(usize, usize), NativeSignatureKey>,
    functions: HashMap<NativeFunctionKey, Vec<NativeSignatureKey>>,
    named: HashMap<NativeNamedKey, Vec<NativeSignatureKey>>,
    sites: HashMap<(usize, usize), SignatureSite>,
    pub diagnostics: Vec<Diagnostic>,
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
            spans: HashMap::new(),
            type_contexts: HashMap::new(),
            functions: HashMap::new(),
            named: HashMap::new(),
            sites: HashMap::new(),
            diagnostics: Vec::new(),
        };
        for head in &file.overload_annotations {
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
            let key = result.insert(
                owner,
                context,
                None,
                &head.signature,
                head.span,
                head.group_span,
            );
            if let Some(named) = named_key {
                result.named.entry(named).or_default().push(key);
            } else if let Some(function) = function {
                result.functions.entry(function).or_default().push(key);
            }
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
                NamedTypeDeclarationKind::Record { base, fields, .. } => {
                    if let Some(base) = base {
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
            let enclosing = functions
                .iter()
                .enumerate()
                .filter(|(_, function)| {
                    function.body_span.start <= span.start
                        && span.end <= function.body_span.end
                })
                .min_by_key(|(_, function)| function.body_span.end - function.body_span.start)
                .map(|(index, _)| NativeFunctionKey {
                    snapshot,
                    index: index as u32,
                });
            let owner = enclosing.map_or(SignatureOwner::Standalone, SignatureOwner::Function);
            let context = enclosing.map(NativeTypeContext::Function);
            for ty in &annotation.types {
                result.visit_type(owner, context, None, ty);
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
                StmtKind::Block { body, .. }
                | StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. }
                | StmtKind::GenericFor { body, .. } => self.visit_statements(owner, context, body),
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
            signature: signature.clone(),
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
            TypeExprKind::Union(types) | TypeExprKind::Generic { args: types, .. } => {
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
            TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element) => {
                self.visit_type(owner, context, parent, element)
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
    pub(super) fn named_heads(&self, key: NativeNamedKey) -> Vec<NativeSignatureKey> {
        self.named.get(&key).cloned().unwrap_or_default()
    }
    pub(super) fn scopes(&self) -> Vec<NamedGenericScopeRecipe> {
        self.recipes
            .iter()
            .map(|recipe| NamedGenericScopeRecipe {
                owner: NativeTypeContext::Signature(recipe.key),
                parent: recipe.context,
                documentation_parents: Vec::new(),
                parameters: recipe
                    .signature
                    .generics
                    .iter()
                    .enumerate()
                    .map(|(index, generic)| NamedGenericParameterRecipe {
                        owner: NativeTypeContext::Signature(recipe.key),
                        order: index as u32,
                        name: generic.name.clone(),
                        constraint: generic.constraint.clone(),
                    })
                    .collect(),
            })
            .collect()
    }
}
