//! Immutable Lua annotation type scopes and original-source parameter recipes.
//!
//! This module resolves names and source identity only. The native core owns
//! constraint/default resolution, cycle detection, relations and inference.

use super::Diagnostic;
use super::native_named::{NamedGenericScopeRecipe, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{NativeFunctionKey, NativeTypeParameterKey};
use luanalysis_syntax::{Name, Span, TypeExpr};
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

thread_local! {
    /// Advanced with every original scope epoch and header metadata generation
    /// on this thread. A project's registries live on one thread, so the clock
    /// is unchanged exactly when none of their revisions changed: a merged
    /// metadata view is validated in O(1) instead of comparing every file's.
    static METADATA_CLOCK: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn metadata_clock() -> u64 {
    METADATA_CLOCK.get()
}

pub(super) fn advance_metadata_clock() {
    METADATA_CLOCK.set(METADATA_CLOCK.get().checked_add(1).expect("original metadata clock fits u64"));
}

pub(super) struct NativeTypes {
    snapshot: u64,
    /// This source's own scopes, and imported ones it has changed.
    scopes: HashMap<NativeTypeContext, TypeScope>,
    /// The other sources' scopes, shared by every source of a project rather than copied into
    /// each: a project's sources would otherwise each copy every named type's scope.
    imported: Option<Rc<OriginalTypeScopes>>,
    parameters: Vec<TypeParameterSource>,
    diagnostics: Vec<Diagnostic>,
    metadata_epochs: Vec<Rc<Cell<u64>>>,
}

/// Immutable original identities visible only through an attached owner scope.
/// Importing these scopes does not introduce any global generic parameter name.
#[derive(Clone)]
pub(super) struct OriginalTypeScopes(HashMap<NativeTypeContext,TypeScope>, Vec<Rc<Cell<u64>>>);

impl OriginalTypeScopes {
    /// The scopes of every one of `sources`, the first's of an owner first, and their epochs.
    pub(super) fn merge<'a>(sources: impl IntoIterator<Item = &'a OriginalTypeScopes>) -> Self {
        let mut merged = OriginalTypeScopes(HashMap::default(), Vec::new());
        let mut epochs = HashSet::default();
        for source in sources {
            for (&owner, scope) in &source.0 { merged.0.entry(owner).or_insert_with(|| scope.clone()); }
            for epoch in &source.1 {
                if epochs.insert(Rc::as_ptr(epoch)) { merged.1.push(Rc::clone(epoch)); }
            }
        }
        merged
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TypeNameResolution {
    Missing,
    Parameter(NativeTypeParameterKey),
    /// Original lexical candidate parameters, preserved for a binding diagnostic.
    Ambiguous(Vec<NativeTypeParameterKey>),
}

impl NativeTypes {
    /// The scope of `owner`: this source's own, or an imported one.
    fn scope(&self, owner: &NativeTypeContext) -> Option<&TypeScope> {
        self.scopes.get(owner).or_else(|| self.imported.as_ref()?.0.get(owner))
    }
    /// The scope of `owner`, to change: an imported one is copied into this source's own first,
    /// as importing used to copy every one.
    fn scope_mut(&mut self, owner: &NativeTypeContext) -> Option<&mut TypeScope> {
        if !self.scopes.contains_key(owner) {
            let copy = self.imported.as_ref()?.0.get(owner)?.clone();
            self.scopes.insert(*owner, copy);
        }
        self.scopes.get_mut(owner)
    }
    /// Every scope this source sees: its own, then imported ones it has not changed.
    fn all_scopes(&self) -> impl Iterator<Item = (&NativeTypeContext, &TypeScope)> {
        self.scopes.iter().chain(self.imported.iter().flat_map(|imported| imported.0.iter()).filter(|(owner, _)| !self.scopes.contains_key(owner)))
    }
    /// Every metadata epoch this source observes: its own, then imported ones.
    fn all_epochs(&self) -> Vec<Rc<Cell<u64>>> {
        let mut epochs = self.metadata_epochs.clone();
        for epoch in self.imported.iter().flat_map(|imported| imported.1.iter()) {
            if !epochs.iter().any(|known| Rc::ptr_eq(known, epoch)) { epochs.push(Rc::clone(epoch)); }
        }
        epochs
    }
    pub(super) fn original_scopes(&self) -> OriginalTypeScopes {
        let mut pending:Vec<_>=self.all_scopes().map(|(owner, _)| *owner).filter(|owner|matches!(owner,NativeTypeContext::NamedType(_))).collect();
        let mut scopes=HashMap::default();
        while let Some(owner)=pending.pop() {
            if scopes.contains_key(&owner) {continue;}
            let scope=self.scope(&owner).expect("original named enclosing scope");
            pending.extend(scope.parent);
            pending.extend(scope.method_owner.get());
            pending.extend(scope.annotation_scope);
            pending.extend(scope.documentation_parents.iter().copied());
            scopes.insert(owner,scope.clone());
        }
        OriginalTypeScopes(scopes, self.all_epochs())
    }
    #[cfg(test)]
    pub(super) fn import_original_scopes(&mut self, metadata: &OriginalTypeScopes) {
        let merged = OriginalTypeScopes::merge(self.imported.iter().map(|imported| &**imported).chain([metadata]));
        self.share_original_scopes(Rc::new(merged));
    }
    /// Sees `imported`, the scopes of a project's sources merged once for all of them, behind
    /// this source's own.
    pub(super) fn share_original_scopes(&mut self, imported: Rc<OriginalTypeScopes>) {
        let changed = imported.0.keys().any(|owner| !self.scopes.contains_key(owner))
            || imported.1.iter().any(|epoch| !self.metadata_epochs.iter().any(|known| Rc::ptr_eq(known, epoch)));
        self.imported = Some(imported);
        if changed { Self::advance_metadata(&self.metadata_epochs[0]); }
    }
    pub(super) fn attach_documentation_owner(&self, signature:luanalysis_engine::NativeSignatureKey,owner:NativeNamedKey) {
        let scope = self.scope(&NativeTypeContext::Signature(signature)).expect("original signature scope");
        let owner = Some(NativeTypeContext::NamedType(owner));
        if scope.method_owner.replace(owner) != owner { Self::advance_metadata(&scope.metadata_epoch); }
    }
    pub(super) fn clear_documentation_owner(&self, signature:luanalysis_engine::NativeSignatureKey) {
        let scope = self.scope(&NativeTypeContext::Signature(signature)).expect("original signature scope");
        if scope.method_owner.replace(None).is_some() { Self::advance_metadata(&scope.metadata_epoch); }
    }
    fn advance_metadata(epoch: &Cell<u64>) {
        epoch.set(epoch.get().checked_add(1).expect("original scope metadata epoch fits u64"));
        advance_metadata_clock();
    }
    /// Original scope revisions only; these never name inferred checker types.
    #[cfg(test)]
    pub(super) fn metadata_revisions(&self) -> Vec<u64> {
        self.all_epochs().iter().map(|epoch| epoch.get()).collect()
    }
    #[cfg(test)]
    pub(super) fn own_metadata_revision(&self) -> u64 { self.metadata_epochs[0].get() }
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
            imported: None,
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
        if let Some(key) = self.scope(&context).expect("original class type scope").self_parameter { return key }
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
        self.scope_mut(&context).expect("original class type scope").self_parameter = Some(key);
        Self::advance_metadata(&self.metadata_epochs[0]);
        key
    }

    pub(super) fn self_parameter(&self, owner: NativeNamedKey) -> Option<NativeTypeParameterKey> {
        self.scope(&NativeTypeContext::NamedType(owner)).and_then(|scope| scope.self_parameter)
    }

    /// Imported scopes preserve their original canonical self key. Its owning
    /// parameter recipe remains in the original file, not this local allocator.
    pub(super) fn is_semantic_self_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.all_scopes().any(|(_, scope)| scope.self_parameter == Some(key))
    }

    pub(crate) fn lexical_function_parent(&self, function: NativeFunctionKey) -> Option<NativeFunctionKey> {
        match self.scope(&NativeTypeContext::Function(function))?.parent? {
            NativeTypeContext::Function(parent) => Some(parent),
            _ => None,
        }
    }

    pub(super) fn attach_method_scope(&mut self, function: NativeFunctionKey, owner: NativeNamedKey) {
        self.attach_owner_scope(NativeTypeContext::Function(function), owner);
    }

    pub(super) fn attach_owner_scope(&mut self, member: NativeTypeContext, owner: NativeNamedKey) {
        let context = NativeTypeContext::NamedType(owner);
        assert!(self.scope(&context).is_some(), "original class scope");
        let scope = self.scope(&member).expect("original member scope");
        if scope.method_owner.replace(Some(context)) != Some(context) { Self::advance_metadata(&scope.metadata_epoch); }
    }

    pub(super) fn attach_signature_scope(&mut self,function:NativeFunctionKey,signature:luanalysis_engine::NativeSignatureKey) {
        let context=NativeTypeContext::Signature(signature);
        assert!(self.scope(&context).is_some(),"original annotation signature scope");
        let scope = self.scope_mut(&NativeTypeContext::Function(function))
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
        self.scope(&owner)
            .expect("original generic owner scope")
            .parameters
            .clone()
    }

    pub(super) fn siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        if self.parameter(key).semantic_self.is_some() { return vec![key] }
        self.own_parameters_in_context(self.parameter(key).owner)
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
            let Some(scope) = self.scope(&owner) else {
                return TypeNameResolution::Missing;
            };
            if let Some(key) = own_name(scope, name) {
                return TypeNameResolution::Parameter(key);
            }
            if let Some(annotation)=scope.annotation_scope {
                if let Some(key)=own_name(self.scope(&annotation).expect("original annotation scope"), name) {
                    return TypeNameResolution::Parameter(key);
                }
            }
            // These are sibling documentation scopes, not recursive alias
            // edges. Only their own original parameters are lookup candidates.
            let mut candidates: Vec<_> = scope
                .documentation_parents
                .iter()
                .filter_map(|owner| self.scope(owner))
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

    /// Keep only source parameters actually used by this signature/captures.
    /// Mere visibility of an ambient generic must not manufacture a mapper.
    pub(super) fn outer_parameters(
        &self,
        owner: NativeFunctionKey,
        used: &HashSet<NativeTypeParameterKey>,
    ) -> Vec<NativeTypeParameterKey> {
        self.outer_parameters_in_context(NativeTypeContext::Function(owner), used)
    }

    /// Whether `owner` or any context enclosing it declares a type parameter (or a self type):
    /// otherwise nothing it holds can depend on one.
    pub(super) fn any_parameter_in_scope(&self, owner: NativeFunctionKey) -> bool {
        self.any_parameter_from(vec![NativeTypeContext::Function(owner)])
    }

    /// Whether any context enclosing `owner`, but not `owner` itself, declares a type parameter
    /// (or a self type): otherwise `owner` has no outer type parameters.
    pub(super) fn any_outer_parameter(&self, owner: NativeFunctionKey) -> bool {
        let Some(scope) = self.scope(&NativeTypeContext::Function(owner)) else { return true };
        self.any_parameter_from(scope.parent.into_iter().chain(scope.method_owner.get()).chain(scope.documentation_parents.iter().copied()).collect())
    }

    fn any_parameter_from(&self, mut pending: Vec<NativeTypeContext>) -> bool {
        let mut visited = HashSet::default();
        while let Some(owner) = pending.pop() {
            if !visited.insert(owner) { continue }
            let Some(scope) = self.scope(&owner) else { continue };
            if !scope.parameters.is_empty() || scope.self_parameter.is_some() { return true }
            pending.extend(scope.parent);
            pending.extend(scope.method_owner.get());
            pending.extend(scope.documentation_parents.iter().copied());
        }
        false
    }

    pub(super) fn outer_parameters_in_context(
        &self,
        owner: NativeTypeContext,
        used: &HashSet<NativeTypeParameterKey>,
    ) -> Vec<NativeTypeParameterKey> {
        let scope = self
            .scope(&owner)
            .expect("original generic owner scope");
        let mut pending: Vec<_> = scope.parent.into_iter().chain(scope.method_owner.get()).chain(scope.documentation_parents.iter().copied()).collect();
        let mut visited = HashSet::default();
        let mut parameters = Vec::new();
        while let Some(owner) = pending.pop() {
            if !visited.insert(owner) { continue }
            let scope = self.scope(&owner).expect("original enclosing generic owner scope");
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
