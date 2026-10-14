//! Immutable Lua annotation type scopes and original-source parameter recipes.
//!
//! This module resolves names and source identity only. The native core owns
//! constraint/default resolution, cycle detection, relations and inference.

use super::Diagnostic;
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
    pub owner: NativeFunctionKey,
    pub order: u32,
    pub constraint: Option<TypeExpr>,
}

struct TypeScope {
    parent: Option<NativeFunctionKey>,
    names: HashMap<String, NativeTypeParameterKey>,
    parameters: Vec<NativeTypeParameterKey>,
}

pub(super) struct NativeTypes {
    snapshot: u64,
    scopes: HashMap<NativeFunctionKey, TypeScope>,
    parameters: Vec<TypeParameterSource>,
    diagnostics: Vec<Diagnostic>,
}

impl NativeTypes {
    pub(super) fn new(snapshot: u64, recipes: Vec<TypeScopeRecipe>) -> Self {
        let mut result = Self {
            snapshot,
            scopes: HashMap::new(),
            parameters: Vec::new(),
            diagnostics: Vec::new(),
        };
        // Every sibling identity exists before any callback can ask for a
        // constraint, including valid forward references between parameters.
        for recipe in recipes {
            assert_eq!(recipe.owner.snapshot, snapshot);
            let mut scope = TypeScope {
                parent: recipe.parent,
                names: HashMap::new(),
                parameters: Vec::new(),
            };
            for (order, parameter) in recipe.parameters.into_iter().enumerate() {
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
                    owner: recipe.owner,
                    order: u32::try_from(order).expect("type parameter order fits in u32"),
                    constraint: parameter.constraint,
                });
            }
            assert!(
                result.scopes.insert(recipe.owner, scope).is_none(),
                "one type scope per original function"
            );
        }
        result
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
        self.scopes
            .get(&owner)
            .expect("original function type scope")
            .parameters
            .clone()
    }

    pub(super) fn siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        self.own_parameters(self.parameter(key).owner)
    }

    pub(super) fn resolve_name(
        &self,
        mut context: Option<NativeFunctionKey>,
        name: &str,
    ) -> Option<Type> {
        while let Some(owner) = context {
            let scope = self.scopes.get(&owner)?;
            if let Some(key) = scope.names.get(name) {
                return Some(Type::NativeTypeParameter(*key));
            }
            context = scope.parent;
        }
        None
    }

    pub(super) fn annotation_dependencies(
        &self,
        context: Option<NativeFunctionKey>,
        ty: &TypeExpr,
        used: &mut HashSet<NativeTypeParameterKey>,
    ) {
        match &ty.kind {
            TypeExprKind::Named(name) => {
                if let Some(Type::NativeTypeParameter(key)) = self.resolve_name(context, name) {
                    used.insert(key);
                }
            }
            TypeExprKind::Union(types) => {
                for ty in types {
                    self.annotation_dependencies(context, ty, used);
                }
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    self.annotation_dependencies(context, &field.ty, used);
                }
            }
            TypeExprKind::Parenthesized(ty) => self.annotation_dependencies(context, ty, used),
            TypeExprKind::Function(function) => {
                for param in &function.params {
                    if let Some(ty) = &param.ty {
                        self.annotation_dependencies(context, ty, used);
                    }
                }
                for returned in &function.returns {
                    self.annotation_dependencies(context, returned, used);
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
        let mut parents = Vec::new();
        let mut parent = self
            .scopes
            .get(&owner)
            .expect("original function type scope")
            .parent;
        while let Some(owner) = parent {
            let scope = self
                .scopes
                .get(&owner)
                .expect("enclosing function type scope");
            parents.push(scope);
            parent = scope.parent;
        }
        parents
            .into_iter()
            .rev()
            .flat_map(|scope| scope.parameters.iter().copied())
            .filter(|key| used.contains(key))
            .collect()
    }
}
