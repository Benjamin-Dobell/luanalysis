//! Immutable Lua annotation type scopes and original-source parameter recipes.
//!
//! This module resolves names and source identity only. The native core owns
//! constraint/default resolution, cycle detection, relations and inference.

use super::Diagnostic;
use super::native_named::{NamedGenericScopeRecipe, NativeTypeContext};
use luanalysis_engine::{NativeFunctionKey, NativeTypeParameterKey, Type};
use luanalysis_syntax::{Name, TypeExpr, TypeExprKind};
use std::collections::{HashMap, HashSet};

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
}

struct TypeScope {
    // A written generic function-type head supplies body-visible names without
    // changing their original Signature owner or creating a parent cycle.
    annotation_scope: Option<NativeTypeContext>,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TypeNameResolution {
    Missing,
    Parameter(NativeTypeParameterKey),
    /// Original lexical candidate parameters, preserved for a binding diagnostic.
    Ambiguous(Vec<NativeTypeParameterKey>),
}

impl NativeTypes {
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
            scopes: HashMap::new(),
            parameters: Vec::new(),
            diagnostics: Vec::new(),
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
                annotation_scope: None,
                parent,
                documentation_parents,
                names: HashMap::new(),
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
                });
            }
            assert!(
                result.scopes.insert(owner, scope).is_none(),
                "one type scope per original generic owner"
            );
        }
        result
    }

    pub(super) fn attach_signature_scope(&mut self,function:NativeFunctionKey,signature:luanalysis_engine::NativeSignatureKey) {
        let context=NativeTypeContext::Signature(signature);
        assert!(self.scopes.contains_key(&context),"original annotation signature scope");
        self.scopes.get_mut(&NativeTypeContext::Function(function))
            .expect("original runtime function scope").annotation_scope=Some(context);
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
        mut context: Option<NativeTypeContext>,
        name: &str,
    ) -> TypeNameResolution {
        let mut remaining = self.scopes.len();
        while let Some(owner) = context {
            assert!(remaining > 0, "cyclic lexical generic-owner metadata");
            remaining -= 1;
            let Some(scope) = self.scopes.get(&owner) else {
                return TypeNameResolution::Missing;
            };
            if let Some(key) = scope.names.get(name) {
                return TypeNameResolution::Parameter(*key);
            }
            if let Some(annotation)=scope.annotation_scope {
                if let Some(key)=self.scopes[&annotation].names.get(name) {
                    return TypeNameResolution::Parameter(*key);
                }
            }
            // These are sibling documentation scopes, not recursive alias
            // edges. Only their own original parameters are lookup candidates.
            let mut candidates: Vec<_> = scope
                .documentation_parents
                .iter()
                .filter_map(|owner| self.scopes.get(owner))
                .filter_map(|scope| scope.names.get(name).copied())
                .collect();
            candidates.sort_by_key(|key| key.index);
            candidates.dedup();
            match candidates.len() {
                0 => {}
                1 => return TypeNameResolution::Parameter(candidates[0]),
                _ => return TypeNameResolution::Ambiguous(candidates),
            }
            context = scope.parent;
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
            TypeExprKind::Literal(_) | TypeExprKind::Error => {}
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
        let mut parents = Vec::new();
        let scope = self
            .scopes
            .get(&owner)
            .expect("original generic owner scope");
        let mut parent = scope.parent;
        let documentation_parents: Vec<_> = scope
            .documentation_parents
            .iter()
            .map(|owner| {
                self.scopes
                    .get(owner)
                    .expect("original documentation generic owner scope")
            })
            .collect();
        let mut remaining = self.scopes.len();
        while let Some(owner) = parent {
            assert!(remaining > 0, "cyclic lexical generic-owner metadata");
            remaining -= 1;
            let scope = self
                .scopes
                .get(&owner)
                .expect("enclosing generic owner scope");
            parents.push(scope);
            parent = scope.parent;
        }
        let mut seen = HashSet::new();
        parents
            .into_iter()
            .rev()
            .chain(documentation_parents)
            .flat_map(|scope| scope.parameters.iter().copied())
            .filter(|key| used.contains(key) && seen.insert(*key))
            .collect()
    }
}

fn context_snapshot(context: NativeTypeContext) -> u64 {
    match context {
        NativeTypeContext::Function(key) => key.snapshot,
        NativeTypeContext::NamedType(key) => key.snapshot,
        NativeTypeContext::Signature(key) => key.snapshot,
    }
}
