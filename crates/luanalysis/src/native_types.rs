//! Immutable Lua annotation type scopes and original-source parameter recipes.
//!
//! This module resolves names and source identity only. The native core owns
//! constraint/default resolution, cycle detection, relations and inference.

use super::Diagnostic;
use super::native_named::{NamedGenericScopeRecipe, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{NativeAnnotationKey, NativeFunctionKey, NativeSignatureKey, NativeTypeParameterKey};
use luanalysis_syntax::{AnnotationDeclaration, Name, Span, TypeExpr};
use std::cell::RefCell;
use luanalysis_engine::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct TypeParameterRecipe {
    pub name: Name,
    pub constraint: Option<TypeExpr>,
    pub default: Option<TypeExpr>,
    pub constant: bool,
}

#[derive(Clone)]
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
    pub default: Option<TypeExpr>,
    /// `---@generic const T` (`GenericParameter::constant`).
    pub constant: bool,
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
    /// While a registry no query has read is built (`unobserved`).
    static UNOBSERVED: Cell<u32> = const { Cell::new(0) };
}

pub(super) fn metadata_clock() -> u64 {
    METADATA_CLOCK.get()
}

pub(super) fn advance_metadata_clock() {
    if UNOBSERVED.get() > 0 { return; }
    METADATA_CLOCK.set(METADATA_CLOCK.get().checked_add(1).expect("original metadata clock fits u64"));
}

/// Runs `build` without advancing the metadata clock: for building a source's registry that no
/// query can have read, as no metadata cached before can have read its scopes. A project builds
/// a registry when a check first reads it, and advancing the clock would compute every header
/// cached until then again.
pub(super) fn unobserved<R>(build: impl FnOnce() -> R) -> R {
    struct Observed;
    impl Drop for Observed {
        fn drop(&mut self) { UNOBSERVED.set(UNOBSERVED.get() - 1); }
    }
    UNOBSERVED.set(UNOBSERVED.get() + 1);
    let _observed = Observed;
    build()
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
    /// The conditional and mapped types of the documentation comments, by `NativeAnnotationKey`.
    annotations: Vec<AnnotationSource>,
    /// Those that declare type parameters.
    scoped_annotations: Vec<u32>,
    /// The type parameter that each `infer P` and mapped type's `[P in K]` declares, by the
    /// start of `P`.
    parameter_declarations: HashMap<usize, NativeTypeParameterKey>,
    /// How many pieces of conditional and mapped types are being lowered.
    lowering_pieces: Cell<u32>,
    /// Where a check that publishes nothing first saw a limit that the checker could not
    /// place (`Checker::report_unplaced_limit`), and how many had been hit when it began and
    /// ended.
    unplaced_limit_claim: Cell<Option<(u64, u64, Span)>>,
}

/// The contexts that lower an annotation (`Checker::type_context` and its kin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnnotationContext {
    pub function: Option<NativeFunctionKey>,
    pub named: Option<NativeNamedKey>,
    pub signature: Option<NativeSignatureKey>,
}

impl AnnotationContext {
    /// As the checker carries it with a conditional type, for any task that reads its branches.
    pub(super) fn encode(self) -> luanalysis_engine::NativeAnnotationContext {
        [
            self.function.map(|key| (key.snapshot, key.index)),
            self.named.map(|key| (key.snapshot, key.index)),
            self.signature.map(|key| (key.snapshot, key.index)),
        ]
    }

    pub(super) fn decode([function, named, signature]: luanalysis_engine::NativeAnnotationContext) -> Self {
        Self {
            function: function.map(|(snapshot, index)| NativeFunctionKey { snapshot, index }),
            named: named.map(|(snapshot, index)| NativeNamedKey { snapshot, index }),
            signature: signature.map(|(snapshot, index)| NativeSignatureKey { snapshot, index }),
        }
    }
}

/// A conditional or mapped type, whose pieces (a conditional type's branches, a mapped type's
/// template and `as` clause) are lowered when first read, in the context that lowered the rest
/// of it.
pub(super) struct AnnotationSource {
    pub syntax: AnnotationDeclaration,
    /// The type parameters it declares: a conditional type's `infer` ones (one declared twice is
    /// one), a mapped type's key.
    parameters: Vec<(String, NativeTypeParameterKey)>,
    /// Where they are seen: a conditional type's extends type and true branch, a mapped type's
    /// `as` clause and template.
    regions: Vec<Span>,
    /// Where this source lowered it, or read a piece of it, first: the context of its
    /// parameters' constraints, and of the pieces that `check_unread_annotation_pieces` lowers.
    pub context: RefCell<Option<AnnotationContext>>,
    /// Which pieces have been lowered: true branch or template, false branch or `as` clause.
    pub lowered: Cell<[bool; 2]>,
}

impl AnnotationSource {
    /// The region of it that sees its type parameters and has `span`, if any.
    fn region(&self, span: Span) -> Option<Span> {
        if self.parameters.is_empty() { return None }
        self.regions.iter().copied().find(|region| region.start <= span.start && span.end <= region.end)
    }
}

/// Immutable original identities visible only through an attached owner scope.
/// Importing these scopes does not introduce any global generic parameter name.
#[derive(Clone)]
pub(super) struct OriginalTypeScopes(HashMap<NativeTypeContext,TypeScope>, Vec<Rc<Cell<u64>>>);

impl OriginalTypeScopes {
    /// Whether they reach a scope other than a named type's (a class declared in a function has
    /// the function's scope as its parent), through which another source can read its source's
    /// function and signature scopes.
    pub(super) fn reach_member_scopes(&self) -> bool {
        self.0.keys().any(|owner| !matches!(owner, NativeTypeContext::NamedType(_)))
    }
    /// The scopes of every one of `sources`, the first's of an owner first, and their epochs.
    pub(super) fn merge(sources: impl IntoIterator<Item = OriginalTypeScopes>) -> Self {
        let mut merged = OriginalTypeScopes(HashMap::default(), Vec::new());
        let mut epochs = HashSet::default();
        for source in sources {
            for (owner, scope) in source.0 { merged.0.entry(owner).or_insert(scope); }
            for epoch in source.1 {
                if epochs.insert(Rc::as_ptr(&epoch)) { merged.1.push(epoch); }
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
        let merged = OriginalTypeScopes::merge(self.imported.iter().map(|imported| (**imported).clone()).chain([metadata.clone()]));
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
    fn advance_metadata(epoch: &Cell<u64>) {
        epoch.set(epoch.get().checked_add(1).expect("original scope metadata epoch fits u64"));
        advance_metadata_clock();
    }
    /// Original scope revisions only; these never name inferred checker types.
    #[cfg(test)]
    pub(super) fn metadata_revisions(&self) -> Vec<u64> {
        self.all_epochs().iter().map(|epoch| epoch.get()).collect()
    }
    /// One allocator owns every function and named parameter identity. Keeping
    /// function recipes first preserves their existing source parameter indices.
    pub(super) fn new_with_named(
        snapshot: u64,
        recipes: Vec<TypeScopeRecipe>,
        named: Vec<NamedGenericScopeRecipe>,
        annotations: Vec<AnnotationDeclaration>,
    ) -> Self {
        let mut result = Self {
            snapshot,
            scopes: HashMap::default(),
            imported: None,
            parameters: Vec::new(),
            diagnostics: Vec::new(),
            metadata_epochs: vec![Rc::new(Cell::new(0))],
            annotations: Vec::new(),
            scoped_annotations: Vec::new(),
            parameter_declarations: HashMap::default(),
            lowering_pieces: Cell::new(0),
            unplaced_limit_claim: Cell::new(None),
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
                            default: parameter.default,
                            constant: false,
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
                    default: parameter.default,
                    constant: parameter.constant,
                    semantic_self: None,
                });
            }
            assert!(
                result.scopes.insert(owner, scope).is_none(),
                "one type scope per original generic owner"
            );
        }
        // After every written generic, so that their keys do not depend on these.
        for (index, syntax) in annotations.into_iter().enumerate() {
            let index = u32::try_from(index).expect("annotation type index fits in u32");
            let (declarations, regions): (Vec<(Name, Option<TypeExpr>)>, Vec<Span>) = match &syntax {
                AnnotationDeclaration::Conditional(conditional) => (
                    conditional.infer.iter().map(|declaration| (declaration.name.clone(), declaration.constraint.clone())).collect(),
                    vec![conditional.extends, conditional.yes.span],
                ),
                AnnotationDeclaration::Mapped(mapped) => (
                    vec![(mapped.parameter.clone(), Some(mapped.constraint.clone()))],
                    mapped.name.iter().map(|name| name.span).chain([mapped.ty.span]).collect(),
                ),
            };
            let mut source = AnnotationSource {
                parameters: Vec::new(),
                regions,
                context: RefCell::new(None),
                lowered: Cell::new([false; 2]),
                syntax,
            };
            if declarations.is_empty() {
                result.annotations.push(source);
                continue;
            }
            let owner = NativeTypeContext::Annotation(NativeAnnotationKey { snapshot, index });
            let mut scope = TypeScope {
                metadata_epoch: Rc::clone(&result.metadata_epochs[0]),
                annotation_scope: None,
                method_owner: Rc::new(Cell::new(None)),
                self_parameter: None,
                parent: None,
                documentation_parents: Vec::new(),
                names: HashMap::default(),
                parameters: Vec::new(),
            };
            for (name, constraint) in declarations {
                let key = match scope.names.get(&name.text) {
                    // One declared again is one parameter, constrained by the first constraint.
                    Some(&key) => {
                        let parameter = &mut result.parameters[key.index as usize];
                        if parameter.constraint.is_none() { parameter.constraint = constraint; }
                        key
                    }
                    None => {
                        let key = NativeTypeParameterKey {
                            snapshot,
                            index: u32::try_from(result.parameters.len())
                                .expect("source type parameter index fits in u32"),
                        };
                        scope.names.insert(name.text.clone(), key);
                        scope.parameters.push(key);
                        result.parameters.push(TypeParameterSource {
                            key,
                            name: name.clone(),
                            owner,
                            order: u32::try_from(scope.parameters.len() - 1).expect("type parameter order fits in u32"),
                            constraint,
                            default: None,
                            constant: false,
                            semantic_self: None,
                        });
                        key
                    }
                };
                result.parameter_declarations.insert(name.span.start, key);
            }
            let mut names: Vec<_> = scope.names.iter().map(|(name, &key)| (name.clone(), key)).collect();
            names.sort_by_key(|(_, key)| key.index);
            source.parameters = names;
            result.annotations.push(source);
            result.scoped_annotations.push(index);
            result.scopes.insert(owner, scope);
        }
        result
    }

    /// The conditional or mapped type written at `span`.
    pub(super) fn annotation_at(&self, span: Span) -> Option<NativeAnnotationKey> {
        let index = self.annotations.binary_search_by_key(&(span.start, span.end), |annotation| (annotation.syntax.span().start, annotation.syntax.span().end)).ok()?;
        Some(NativeAnnotationKey { snapshot: self.snapshot, index: index as u32 })
    }

    pub(super) fn annotation(&self, key: NativeAnnotationKey) -> Option<&AnnotationSource> {
        (key.snapshot == self.snapshot).then(|| self.annotations.get(key.index as usize)).flatten()
    }

    /// Every conditional and mapped type of the documentation comments.
    pub(super) fn annotations(&self) -> &[AnnotationSource] {
        &self.annotations
    }

    /// Whether a piece of a conditional or mapped type is being lowered, which another type's
    /// evaluation reads.
    pub(super) fn lowering_annotation_piece(&self) -> bool {
        self.lowering_pieces.get() != 0
    }

    /// Claims `span` for the limits hit since `since`, until `hits`, unless a check within it
    /// has claimed them.
    pub(super) fn claim_unplaced_limit(&self, since: u64, hits: u64, span: Span) {
        if self.unplaced_limit_claim.get().is_some_and(|(start, end, _)| since <= start && end <= hits && start < end) { return }
        self.unplaced_limit_claim.set(Some((since, hits, span)));
    }

    /// The place claimed for limits hit since `since`.
    pub(super) fn claimed_unplaced_limit(&self, since: u64) -> Option<Span> {
        let (start, _, span) = self.unplaced_limit_claim.take()?;
        (since <= start).then_some(span)
    }

    /// Lowers a piece of a conditional or mapped type with `lower`.
    pub(super) fn lower_annotation_piece<T>(&self, lower: impl FnOnce() -> T) -> T {
        self.lowering_pieces.set(self.lowering_pieces.get() + 1);
        let lowered = lower();
        self.lowering_pieces.set(self.lowering_pieces.get() - 1);
        lowered
    }

    /// The type parameter that `infer P`, or a mapped type's `[P in K]`, declares, where `name`
    /// is `P`.
    pub(super) fn declared_parameter(&self, name: Span) -> Option<NativeTypeParameterKey> {
        self.parameter_declarations.get(&name.start).copied()
    }

    /// The type parameters that the conditional or mapped type `key` declares.
    pub(super) fn declared_annotation_parameters(&self, key: NativeAnnotationKey) -> Vec<NativeTypeParameterKey> {
        self.annotation(key).map(|annotation| annotation.parameters.iter().map(|(_, key)| *key).collect()).unwrap_or_default()
    }

    /// The type parameter named `name` that a reference at `at` sees: that of the innermost
    /// conditional or mapped type that declares it and has `at` where it is seen.
    pub(super) fn visible_annotation_parameter(&self, name: &str, at: Span) -> Option<NativeTypeParameterKey> {
        self.scoped_annotations.iter()
            .map(|&index| &self.annotations[index as usize])
            .filter_map(|annotation| {
                let region = annotation.region(at)?;
                let (_, key) = annotation.parameters.iter().find(|(declared, _)| declared == name)?;
                Some((region.end - region.start, *key))
            })
            .min_by_key(|(width, _)| *width)
            .map(|(_, key)| key)
    }

    /// The type parameters of conditional and mapped types that what is written at `span` sees,
    /// as outer type parameters.
    pub(super) fn visible_annotation_parameters(&self, span: Span) -> impl Iterator<Item = NativeTypeParameterKey> + '_ {
        self.scoped_annotations.iter()
            .map(|&index| &self.annotations[index as usize])
            .filter(move |annotation| annotation.region(span).is_some())
            .flat_map(|annotation| annotation.parameters.iter().map(|(_, key)| *key))
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
            default: None,
            constant: false,
            semantic_self: Some(owner),
        });
        self.scope_mut(&context).expect("original class type scope").self_parameter = Some(key);
        Self::advance_metadata(&self.metadata_epochs[0]);
        key
    }

    /// The type parameters that the named type `owner` declares, in order, with their names.
    pub(super) fn named_parameters(&self, owner: NativeNamedKey) -> Vec<(String, NativeTypeParameterKey)> {
        let scope = self.scope(&NativeTypeContext::NamedType(owner)).expect("original named type scope");
        scope.parameters.iter().map(|&key| (self.parameter(key).name.text.clone(), key)).collect()
    }

    /// Makes the named type `owner`, a declaration of a class declared more than once, declare
    /// that class's type parameters: `parameters` is every declaration's list as one, and each
    /// name `owner` declares is the parameter of that name in `names`, which may belong to
    /// another declaration. As TypeScript's merged declarations see only the type parameters they
    /// declare themselves, the other names stay unseen here. Its `self` is the class's,
    /// `self_parameter`.
    pub(super) fn merge_named_parameters(
        &mut self,
        owner: NativeNamedKey,
        names: &HashMap<String, NativeTypeParameterKey>,
        parameters: &[NativeTypeParameterKey],
        self_parameter: Option<NativeTypeParameterKey>,
    ) {
        let scope = self.scope_mut(&NativeTypeContext::NamedType(owner)).expect("original named type scope");
        for (name, key) in &mut scope.names {
            if let Some(&merged) = names.get(name) { *key = merged; }
        }
        scope.parameters = parameters.to_vec();
        if self_parameter.is_some() { scope.self_parameter = self_parameter; }
        let epoch = Rc::clone(&scope.metadata_epoch);
        Self::advance_metadata(&epoch);
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

    /// Every parameter's source, in declaration order within each owner.
    pub(super) fn parameter_sources(&self) -> &[TypeParameterSource] {
        &self.parameters
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

    /// The parameters that a written generic function-type head supplies to `context`'s body,
    /// which its own scope does not declare.
    pub(super) fn annotation_parameters_in_context(&self, context: NativeTypeContext) -> Vec<NativeTypeParameterKey> {
        self.scope(&context).map(|scope| self.annotation_parameters(scope).collect()).unwrap_or_default()
    }

    fn annotation_parameters<'a>(&'a self, scope: &'a TypeScope) -> impl Iterator<Item = NativeTypeParameterKey> + 'a {
        scope.annotation_scope
            .and_then(|annotation| self.scope(&annotation))
            .into_iter()
            .flat_map(|annotation| annotation.parameters.iter().copied())
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
            parameters.extend(self.annotation_parameters(scope));
            pending.extend(scope.parent);
            pending.extend(scope.method_owner.get());
            pending.extend(scope.documentation_parents.iter().copied());
        }
        // A signature's scope encloses the function it types, whose head the signature is.
        parameters.retain(|key| used.contains(key) && !scope.parameters.contains(key));
        parameters.sort_by_key(|key| key.index);
        // A merged class's scopes hold the parameters of its other declarations' sources too,
        // so one key may not follow itself.
        let mut seen = HashSet::default();
        parameters.retain(|key| seen.insert(*key));
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
        NativeTypeContext::Annotation(key) => key.snapshot,
    }
}
