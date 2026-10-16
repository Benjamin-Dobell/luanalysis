//! Immutable Lua annotation type scopes and original-source parameter recipes.
//!
//! This module resolves names and source identity only. The native core owns
//! constraint/default resolution, cycle detection, relations and inference.

use super::Diagnostic;
use super::native_named::{NamedGenericScopeRecipe, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{NativeFunctionKey, NativeTypeParameterKey, Type};
use luanalysis_syntax::{Name, Span, TypeExpr, TypeExprKind};
use luanalysis_engine::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::Cell;
use std::rc::Rc;

pub(super) struct TypeParameterRecipe {
    pub name: Name,
    pub constraint: Option<TypeExpr>,
}

pub(super) struct TypeScopeRecipe {
    pub owner: NativeFunctionKey,
    pub parent: Option<NativeFunctionKey>,
    pub parameters: Vec<TypeParameterRecipe>,
}

pub(super) struct TypeParameterSource {
    pub key: NativeTypeParameterKey,
    pub name: Name,
    pub owner: NativeTypeContext,
    pub order: u32,
    pub constraint: Option<TypeExpr>,
    /// A real semantic class-self parameter. Its constraint is the original
    /// declared head, not a fabricated generic/constraint AST node.
    pub semantic_self: Option<NativeNamedKey>,
}

#[derive(Clone)]
struct TypeScope {
    // Imported scopes share owner preparation changes and their source epoch.
    metadata_epoch: Rc<Cell<u64>>,
    // A written generic function-type head supplies body-visible names without
    // changing their original Signature owner or creating a parent cycle.
    annotation_scope: Option<NativeTypeContext>,
    // Imported copies retain this original scope's key-only preparation state.
    method_owner: Rc<Cell<Option<NativeTypeContext>>>,
    self_parameter: Option<NativeTypeParameterKey>,
    parent: Option<NativeTypeContext>,
    documentation_parents: Vec<NativeTypeContext>,
    names: HashMap<String, NativeTypeParameterKey>,
    parameters: Vec<NativeTypeParameterKey>,
}

pub(super) struct NativeTypes {
    snapshot: u64,
    scopes: HashMap<NativeTypeContext, TypeScope>,
    parameters: Vec<TypeParameterSource>,
    diagnostics: Vec<Diagnostic>,
    metadata_epochs: Vec<Rc<Cell<u64>>>,
}

/// Immutable original identities visible only through an attached owner scope.
/// Importing these scopes does not introduce any global generic parameter name.
#[derive(Clone)]
pub(super) struct OriginalTypeScopes(HashMap<NativeTypeContext,TypeScope>, Vec<Rc<Cell<u64>>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TypeNameResolution {
    Missing,
    Parameter(NativeTypeParameterKey),
    /// Original lexical candidate parameters, preserved for a binding diagnostic.
    Ambiguous(Vec<NativeTypeParameterKey>),
}

impl NativeTypes {
    pub(super) fn original_scopes(&self) -> OriginalTypeScopes {
        let mut pending:Vec<_>=self.scopes.keys().filter(|owner|matches!(owner,NativeTypeContext::NamedType(_))).copied().collect();
        let mut scopes=HashMap::default();
        while let Some(owner)=pending.pop() {
            if scopes.contains_key(&owner) {continue;}
            let scope=self.scopes.get(&owner).expect("original named enclosing scope");
            pending.extend(scope.parent);
            pending.extend(scope.method_owner.get());
            pending.extend(scope.annotation_scope);
            pending.extend(scope.documentation_parents.iter().copied());
            scopes.insert(owner,scope.clone());
        }
        OriginalTypeScopes(scopes, self.metadata_epochs.clone())
    }
    pub(super) fn import_original_scopes(&mut self, metadata: &OriginalTypeScopes) {
        let mut changed = false;
        for (&owner,scope) in &metadata.0 {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.scopes.entry(owner) {
                entry.insert(scope.clone());
                changed = true;
            }
        }
        for epoch in &metadata.1 {
            if !self.metadata_epochs.iter().any(|known| Rc::ptr_eq(known, epoch)) {
                self.metadata_epochs.push(Rc::clone(epoch));
                changed = true;
            }
        }
        if changed { Self::advance_metadata(&self.metadata_epochs[0]); }
    }
    pub(super) fn attach_documentation_owner(&self, signature:luanalysis_engine::NativeSignatureKey,owner:NativeNamedKey) {
        let scope = &self.scopes[&NativeTypeContext::Signature(signature)];
        let owner = Some(NativeTypeContext::NamedType(owner));
        if scope.method_owner.replace(owner) != owner { Self::advance_metadata(&scope.metadata_epoch); }
    }
    pub(super) fn clear_documentation_owner(&self, signature:luanalysis_engine::NativeSignatureKey) {
        let scope = &self.scopes[&NativeTypeContext::Signature(signature)];
        if scope.method_owner.replace(None).is_some() { Self::advance_metadata(&scope.metadata_epoch); }
    }
    fn advance_metadata(epoch: &Cell<u64>) {
        epoch.set(epoch.get().checked_add(1).expect("original scope metadata epoch fits u64"));
    }
    /// Original scope revisions only; these never name inferred checker types.
    pub(super) fn metadata_revisions(&self) -> Vec<u64> {
        self.metadata_epochs.iter().map(|epoch| epoch.get()).collect()
    }
    pub(super) fn own_metadata_revision(&self) -> u64 { self.metadata_epochs[0].get() }
    pub(super) fn new(snapshot: u64, recipes: Vec<TypeScopeRecipe>) -> Self {
        Self::new_with_named(snapshot, recipes, Vec::new())
    }

    /// One allocator owns every function and named parameter identity. Keeping
    /// function recipes first preserves their existing source parameter indices.
    pub(super) fn new_with_named(
        snapshot: u64,
        recipes: Vec<TypeScopeRecipe>,
        named: Vec<NamedGenericScopeRecipe>,
    ) -> Self {
        let mut result = Self {
            snapshot,
            scopes: HashMap::default(),
            parameters: Vec::new(),
            diagnostics: Vec::new(),
            metadata_epochs: vec![Rc::new(Cell::new(0))],
        };
        // Every sibling identity exists before any callback can ask for a
        // constraint, including valid forward references between parameters.
        let recipes = recipes
            .into_iter()
            .map(|recipe| {
                (
                    NativeTypeContext::Function(recipe.owner),
                    recipe.parent.map(NativeTypeContext::Function),
                    Vec::new(),
                    recipe.parameters,
                )
            })
            .chain(named.into_iter().map(|recipe| {
                (
                    recipe.owner,
                    recipe.parent,
                    recipe
                        .documentation_parents
                        .into_iter()
                        .map(NativeTypeContext::NamedType)
                        .collect(),
                    recipe
                        .parameters
                        .into_iter()
                        .map(|parameter| TypeParameterRecipe {
                            name: parameter.name,
                            constraint: parameter.constraint,
                        })
                        .collect(),
                )
            }));
        for (owner, parent, documentation_parents, parameters) in recipes {
            assert_eq!(context_snapshot(owner), snapshot);
            assert!(parent.is_none_or(|context| context_snapshot(context) == snapshot));
            assert!(
                documentation_parents
                    .iter()
                    .all(|&context| context_snapshot(context) == snapshot)
            );
            let mut scope = TypeScope {
                metadata_epoch: Rc::clone(&result.metadata_epochs[0]),
                annotation_scope: None,
                method_owner: Rc::new(Cell::new(None)),
                self_parameter: None,
                parent,
                documentation_parents,
                names: HashMap::default(),
                parameters: Vec::new(),
            };
            for (order, parameter) in parameters.into_iter().enumerate() {
                let key = NativeTypeParameterKey {
                    snapshot,
                    index: u32::try_from(result.parameters.len())
                        .expect("source type parameter index fits in u32"),
                };
                if scope.names.contains_key(&parameter.name.text) {
                    result.diagnostics.push(Diagnostic {
                        code: "TS2300",
                        span: parameter.name.span,
                        message: format!("Duplicate identifier '{}'.", parameter.name.text),
                    });
                } else {
                    scope.names.insert(parameter.name.text.clone(), key);
                }
                scope.parameters.push(key);
                result.parameters.push(TypeParameterSource {
                    key,
                    name: parameter.name,
                    owner,
                    order: u32::try_from(order).expect("type parameter order fits in u32"),
                    constraint: parameter.constraint,
                    semantic_self: None,
                });
            }
            assert!(
                result.scopes.insert(owner, scope).is_none(),
                "one type scope per original generic owner"
            );
        }
        result
    }

    pub(super) fn register_self_parameter(&mut self, owner: NativeNamedKey, span: Span) -> NativeTypeParameterKey {
        let context = NativeTypeContext::NamedType(owner);
        if let Some(key) = self.scopes[&context].self_parameter { return key }
        let key = NativeTypeParameterKey {
            snapshot: self.snapshot,
            index: self.parameters.len().try_into().expect("semantic self parameter index fits u32"),
        };
        self.parameters.push(TypeParameterSource {
            key,
            name: Name { text: "self".into(), span },
            owner: context,
            order: 0,
            constraint: None,
            semantic_self: Some(owner),
        });
        self.scopes.get_mut(&context).expect("original class type scope").self_parameter = Some(key);
        Self::advance_metadata(&self.metadata_epochs[0]);
        key
    }

    pub(super) fn self_parameter(&self, owner: NativeNamedKey) -> Option<NativeTypeParameterKey> {
        self.scopes.get(&NativeTypeContext::NamedType(owner)).and_then(|scope| scope.self_parameter)
    }

    /// Imported scopes preserve their original canonical self key. Its owning
    /// parameter recipe remains in the original file, not this local allocator.
    pub(super) fn is_semantic_self_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.scopes.values().any(|scope| scope.self_parameter == Some(key))
    }

    pub(crate) fn lexical_function_parent(&self, function: NativeFunctionKey) -> Option<NativeFunctionKey> {
        match self.scopes.get(&NativeTypeContext::Function(function))?.parent? {
            NativeTypeContext::Function(parent) => Some(parent),
            _ => None,
        }
    }

    pub(super) fn attach_method_scope(&mut self, function: NativeFunctionKey, owner: NativeNamedKey) {
        self.attach_owner_scope(NativeTypeContext::Function(function), owner);
    }

    pub(super) fn attach_owner_scope(&mut self, member: NativeTypeContext, owner: NativeNamedKey) {
        let context = NativeTypeContext::NamedType(owner);
        assert!(self.scopes.contains_key(&context), "original class scope");
        let scope = &self.scopes[&member];
        if scope.method_owner.replace(Some(context)) != Some(context) { Self::advance_metadata(&scope.metadata_epoch); }
    }

    pub(super) fn attach_signature_scope(&mut self,function:NativeFunctionKey,signature:luanalysis_engine::NativeSignatureKey) {
        let context=NativeTypeContext::Signature(signature);
        assert!(self.scopes.contains_key(&context),"original annotation signature scope");
        let scope = self.scopes.get_mut(&NativeTypeContext::Function(function))
            .expect("original runtime function scope");
        if scope.annotation_scope != Some(context) {
            scope.annotation_scope = Some(context);
            Self::advance_metadata(&scope.metadata_epoch);
        }
    }

    pub(super) fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub(super) fn contains(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == self.snapshot && (key.index as usize) < self.parameters.len()
    }

    pub(super) fn parameter(&self, key: NativeTypeParameterKey) -> &TypeParameterSource {
        assert!(
            self.contains(key),
            "foreign or missing source type parameter"
        );
        let parameter = &self.parameters[key.index as usize];
        debug_assert_eq!(parameter.key, key);
        parameter
    }

    pub(super) fn own_parameters(&self, owner: NativeFunctionKey) -> Vec<NativeTypeParameterKey> {
        self.own_parameters_in_context(NativeTypeContext::Function(owner))
    }

    pub(super) fn own_parameters_in_context(
        &self,
        owner: NativeTypeContext,
    ) -> Vec<NativeTypeParameterKey> {
        self.scopes
            .get(&owner)
            .expect("original generic owner scope")
            .parameters
            .clone()
    }

    pub(super) fn siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        if self.parameter(key).semantic_self.is_some() { return vec![key] }
        self.own_parameters_in_context(self.parameter(key).owner)
    }

    pub(super) fn resolve_name(
        &self,
        context: Option<NativeFunctionKey>,
        name: &str,
    ) -> Option<Type> {
        match self.resolve_name_in_context(context.map(NativeTypeContext::Function), name) {
            TypeNameResolution::Parameter(key) => Some(Type::NativeTypeParameter(key)),
            TypeNameResolution::Missing | TypeNameResolution::Ambiguous(_) => None,
        }
    }

    pub(super) fn resolve_name_in_context(
        &self,
        context: Option<NativeTypeContext>,
        name: &str,
    ) -> TypeNameResolution {
        let mut pending: Vec<_> = context.into_iter().collect();
        let mut visited = HashSet::default();
        while let Some(owner) = pending.pop() {
            if !visited.insert(owner) { continue }
            let Some(scope) = self.scopes.get(&owner) else {
                return TypeNameResolution::Missing;
            };
            if let Some(key) = own_name(scope, name) {
                return TypeNameResolution::Parameter(key);
            }
            if let Some(annotation)=scope.annotation_scope {
                if let Some(key)=own_name(&self.scopes[&annotation], name) {
                    return TypeNameResolution::Parameter(key);
                }
            }
            // These are sibling documentation scopes, not recursive alias
            // edges. Only their own original parameters are lookup candidates.
            let mut candidates: Vec<_> = scope
                .documentation_parents
                .iter()
                .filter_map(|owner| self.scopes.get(owner))
                .filter_map(|scope| own_name(scope, name))
                .collect();
            candidates.sort_by_key(|key| key.index);
            candidates.dedup();
            match candidates.len() {
                0 => {}
                1 => return TypeNameResolution::Parameter(candidates[0]),
                _ => return TypeNameResolution::Ambiguous(candidates),
            }
            if let Some(parent) = scope.parent { pending.push(parent); }
            if let Some(owner) = scope.method_owner.get() { pending.push(owner); }
        }
        TypeNameResolution::Missing
    }

    pub(super) fn annotation_dependencies(
        &self,
        context: Option<NativeFunctionKey>,
        ty: &TypeExpr,
        used: &mut HashSet<NativeTypeParameterKey>,
    ) {
        self.annotation_dependencies_in_context(context.map(NativeTypeContext::Function), ty, used)
    }

    pub(super) fn annotation_dependencies_in_context(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
        used: &mut HashSet<NativeTypeParameterKey>,
    ) {
        match &ty.kind {
            TypeExprKind::Named(name) => {
                if let TypeNameResolution::Parameter(key) =
                    self.resolve_name_in_context(context, name)
                {
                    used.insert(key);
                }
            }
            TypeExprKind::Union(types) => {
                for ty in types {
                    self.annotation_dependencies_in_context(context, ty, used);
                }
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    if let luanalysis_syntax::TypeFieldKey::Indexed(key) = &field.key {
                        self.annotation_dependencies_in_context(context, key, used);
                    }
                    self.annotation_dependencies_in_context(context, &field.ty, used);
                }
            }
            TypeExprKind::Array { element, .. } => {
                self.annotation_dependencies_in_context(context, element, used)
            }
            TypeExprKind::Generic { base, args, .. } => {
                if let TypeNameResolution::Parameter(key) =
                    self.resolve_name_in_context(context, &base.text)
                {
                    used.insert(key);
                }
                for argument in args {
                    self.annotation_dependencies_in_context(context, argument, used);
                }
            }
            TypeExprKind::Parenthesized(ty) => {
                self.annotation_dependencies_in_context(context, ty, used)
            }
            TypeExprKind::Function(function) => {
                for param in &function.params {
                    if let Some(ty) = &param.ty {
                        self.annotation_dependencies_in_context(context, ty, used);
                    }
                }
                if let Some(vararg) = &function.vararg {
                    self.annotation_dependencies_in_context(context, &vararg.ty, used);
                }
                for returned in &function.returns {
                    self.annotation_dependencies_in_context(context, returned, used);
                }
            }
            TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::Error => {}
        }
    }

    /// Keep only source parameters actually used by this signature/captures.
    /// Mere visibility of an ambient generic must not manufacture a mapper.
    pub(super) fn outer_parameters(
        &self,
        owner: NativeFunctionKey,
        used: &HashSet<NativeTypeParameterKey>,
    ) -> Vec<NativeTypeParameterKey> {
        self.outer_parameters_in_context(NativeTypeContext::Function(owner), used)
    }

    pub(super) fn outer_parameters_in_context(
        &self,
        owner: NativeTypeContext,
        used: &HashSet<NativeTypeParameterKey>,
    ) -> Vec<NativeTypeParameterKey> {
        let scope = self
            .scopes
            .get(&owner)
            .expect("original generic owner scope");
        let mut pending: Vec<_> = scope.parent.into_iter().chain(scope.method_owner.get()).chain(scope.documentation_parents.iter().copied()).collect();
        let mut visited = HashSet::default();
        let mut parameters = Vec::new();
        while let Some(owner) = pending.pop() {
            if !visited.insert(owner) { continue }
            let scope = self.scopes.get(&owner).expect("original enclosing generic owner scope");
            parameters.extend(scope.parameters.iter().copied().chain(scope.self_parameter));
            pending.extend(scope.parent);
            pending.extend(scope.method_owner.get());
            pending.extend(scope.documentation_parents.iter().copied());
        }
        parameters.retain(|key| used.contains(key));
        parameters.sort_by_key(|key| key.index);
        parameters.dedup();
        parameters
    }
}

fn own_name(scope: &TypeScope, name: &str) -> Option<NativeTypeParameterKey> {
    scope.names.get(name).copied().or_else(|| (name == "self").then_some(scope.self_parameter).flatten())
}

fn context_snapshot(context: NativeTypeContext) -> u64 {
    match context {
        NativeTypeContext::Function(key) => key.snapshot,
        NativeTypeContext::NamedType(key) => key.snapshot,
        NativeTypeContext::Signature(key) => key.snapshot,
    }
}
