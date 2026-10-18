//! Immutable allocation-site and member-store recipes from original Lua.
//!
//! A constructor key identifies syntax, not one globally unique runtime object:
//! native flow/capture contexts distinguish evaluations of that allocation site.
//! No binding name, alias guess, inferred type or mutable member cache is stored.
use crate::flow;
use crate::native_expressions::NativeExpressions;
use crate::native_flow::NativeFlowSources;
use luanalysis_engine::{
    LuaKey, NativeExpressionKey, NativeFlowKey, NativeFunctionKey, NativeTypeDeclarationKey,
    NativeTypeMemberKey, NativeValueKey,
};
use luanalysis_syntax::{
    Expr, ExprKind, ExpressionAssertionKind, File, NamedTypeDeclarationId,
    NamedTypeDeclarationKind, Span, Stmt, StmtKind, TableField, TypeAnnotation,
};
use crate::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum WrittenTableContext {
    Annotation {
        annotation: TypeAnnotation,
        slot: usize,
    },
    NamedDeclaration {
        declaration: NamedTypeDeclarationId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeTableConstructor {
    pub key: NativeExpressionKey,
    pub owner: Option<NativeFunctionKey>,
    pub input: NativeFlowKey,
    pub span: Span,
    pub fields: Vec<TableField>,
    /// Only syntactically attached contracts are indexed here. A constructor
    /// in a call/return/assignment can receive a contextual contract later;
    /// the native expression query must decide eligibility in that context.
    pub written_contexts: Vec<WrittenTableContext>,
}
impl RuntimeTableConstructor {
    pub(super) fn is_lexically_uncontracted(&self) -> bool {
        self.written_contexts.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum RuntimeTableStoreKey {
    Fixed(LuaKey),
    Computed(NativeExpressionKey),
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeTableStore {
    pub key: NativeFlowKey,
    pub receiver: NativeExpressionKey,
    pub member: RuntimeTableStoreKey,
    pub value: NativeValueKey,
    pub epoch: NativeFlowKey,
    pub span: Span,
    pub statement_end: usize,
    pub annotation: Option<(TypeAnnotation, usize)>,
}

pub(super) struct NativeRuntimeTables {
    constructors: Vec<RuntimeTableConstructor>,
    constructor_indices: HashMap<NativeExpressionKey, usize>,
    stores: Vec<RuntimeTableStore>,
    store_indices: HashMap<NativeFlowKey, usize>,
    target_stores: HashMap<(usize, usize), usize>,
    schema_keys: Vec<(NativeTypeDeclarationKey, NativeExpressionKey)>,
    schema_indices: HashMap<NativeExpressionKey, usize>,
    member_keys: Vec<NativeTypeMemberKey>,
    /// The registered key namespace: (snapshot, declaration base, member base).
    bases: Option<(u64, u32, u32)>,
}
impl NativeRuntimeTables {
    pub(super) fn new(
        file: &File,
        expressions: &NativeExpressions,
        sources: &NativeFlowSources,
    ) -> Self {
        let mut contexts: HashMap<(usize, usize), Vec<WrittenTableContext>> = HashMap::default();
        let mut annotations = HashMap::default();
        let mut statements = Vec::new();
        collect_statements(&file.statements, &mut statements);
        // Function-expression bodies are original source roots too. Their
        // statement spans are deduplicated below, independent from traversal.
        for (_, source) in expressions.sources() {
            if let ExprKind::Function(function) = &source.expression.kind {
                collect_statements(&function.body, &mut statements);
            }
            if let ExprKind::Table(fields) = &source.expression.kind {
                for field in fields {
                    if let Some(annotation) = &field.annotation {
                        attach_annotation(&mut contexts, &field.value, annotation, 0);
                    }
                }
            }
        }
        statements.sort_by_key(|statement| (statement.span.start, statement.span.end));
        statements.dedup_by_key(|statement| (statement.span.start, statement.span.end));
        for statement in statements {
            let (values, written) = match &statement.kind {
                StmtKind::Local {
                    values, annotation, ..
                } => (values, annotation.as_ref()),
                StmtKind::Return(values) => (
                    values,
                    file.return_annotations
                        .iter()
                        .find(|annotation| annotation.statement == statement.span)
                        .map(|annotation| &annotation.annotation),
                ),
                StmtKind::Assign {
                    targets,
                    values,
                    annotation,
                } => {
                    if let Some(annotation) = annotation {
                        for (slot, target) in targets.iter().enumerate() {
                            annotations.insert(
                                (target.span.start, target.span.end),
                                (annotation.clone(), slot),
                            );
                        }
                    }
                    (values, annotation.as_ref())
                }
                _ => continue,
            };
            if let Some(annotation) = written {
                for (slot, value) in values.iter().enumerate() {
                    attach_annotation(&mut contexts, value, annotation, slot);
                }
            }
            for declaration in &file.type_declarations {
                if declaration.attachment == Some(statement.span)
                    && matches!(declaration.kind, NamedTypeDeclarationKind::Record { .. })
                {
                    for value in values {
                        if let Some(table) = constructor(value) {
                            contexts
                                .entry((table.span.start, table.span.end))
                                .or_default()
                                .push(WrittenTableContext::NamedDeclaration {
                                    declaration: declaration.id,
                                    span: declaration.span,
                                });
                        }
                    }
                }
            }
        }
        for assertion in &file.expression_assertions {
            if assertion.kind != ExpressionAssertionKind::Type {
                continue;
            }
            let expression = expressions.key_for_span(assertion.expression);
            attach_annotation(
                &mut contexts,
                &expressions.get(expression).expression,
                &assertion.annotation,
                0,
            );
        }
        let mut constructors = Vec::new();
        let mut constructor_indices = HashMap::default();
        let mut evaluations = HashMap::default();
        for evaluation in &sources.program.evaluations {
            let span = sources
                .program
                .expression(evaluation.expression)
                .expression
                .span;
            evaluations.insert((span.start, span.end), evaluation);
        }
        for (key, source) in expressions.sources() {
            let ExprKind::Table(fields) = &source.expression.kind else {
                continue;
            };
            let span = source.expression.span;
            let evaluation = evaluations
                .get(&(span.start, span.end))
                .expect("every original constructor has a flow evaluation");
            constructor_indices.insert(key, constructors.len());
            constructors.push(RuntimeTableConstructor {
                key,
                owner: source.owner,
                input: sources.flow_key(evaluation.input, 0),
                span,
                fields: fields.clone(),
                written_contexts: contexts.remove(&(span.start, span.end)).unwrap_or_default(),
            });
        }
        let mut stores = Vec::new();
        let mut store_indices = HashMap::default();
        let mut target_stores = HashMap::default();
        for graph in &sources.program.graphs {
            for node in &graph.nodes {
                let flow::NodeKind::Write {
                    address,
                    value: _,
                    ..
                } = &node.kind
                else {
                    continue;
                };
                let Some(receiver_id) = address.receiver else {
                    continue;
                };
                let receiver = sources.program.evaluation(receiver_id);
                let receiver_expression =
                    &sources.program.expression(receiver.expression).expression;
                let member = match address
                    .reference
                    .path
                    .last()
                    .expect("member store has an original key")
                {
                    flow::Key::String(bytes) => {
                        RuntimeTableStoreKey::Fixed(LuaKey::String(bytes.clone()))
                    }
                    flow::Key::Boolean(value) => {
                        RuntimeTableStoreKey::Fixed(LuaKey::Boolean(*value))
                    }
                    flow::Key::Integer(value) => {
                        RuntimeTableStoreKey::Fixed(LuaKey::Integer(*value))
                    }
                    flow::Key::Float(bits) => {
                        RuntimeTableStoreKey::Fixed(LuaKey::Number(f64::from_bits(*bits)))
                    }
                    flow::Key::Dynamic(id) => RuntimeTableStoreKey::Computed(
                        expressions.key_for_span(sources.program.expression(*id).expression.span),
                    ),
                };
                let key = sources.flow_key(node.id, 0);
                store_indices.insert(key, stores.len());
                target_stores.insert((address.span.start, address.span.end), stores.len());
                stores.push(RuntimeTableStore {
                    key,
                    receiver: expressions.key_for_span(receiver_expression.span),
                    member,
                    value: sources
                        .write_value(node)
                        .expect("every original write has a value recipe"),
                    epoch: sources.flow_key(address.epoch, 0),
                    span: address.span,
                    statement_end: address.statement_end,
                    annotation: annotations
                        .get(&(address.span.start, address.span.end))
                        .cloned(),
                });
            }
        }
        Self {
            constructors,
            constructor_indices,
            stores,
            store_indices,
            target_stores,
            schema_keys: Vec::new(),
            schema_indices: HashMap::default(),
            member_keys: Vec::new(),
            bases: None,
        }
    }
    #[cfg(test)]
    pub(super) fn constructors(&self) -> &[RuntimeTableConstructor] {
        &self.constructors
    }
    pub(super) fn register_keys(&mut self, snapshot: u64, declaration_base: u32, member_base: u32) {
        self.bases = Some((snapshot, declaration_base, member_base));
        self.schema_keys = self
            .constructors
            .iter()
            .filter(|source| source.is_lexically_uncontracted())
            .enumerate()
            .map(|(index, source)| {
                (
                    NativeTypeDeclarationKey {
                        snapshot,
                        index: declaration_base
                            .checked_add(index.try_into().expect("runtime schema count fits u32"))
                            .expect("original declaration namespace fits u32"),
                    },
                    source.key,
                )
            })
            .collect();
        self.schema_indices = self.schema_keys.iter().enumerate().map(|(index, (_, constructor))| (*constructor, index)).collect();
        self.member_keys = self
            .stores
            .iter()
            .enumerate()
            .map(|(index, _)| NativeTypeMemberKey {
                snapshot,
                index: member_base
                    .checked_add(index.try_into().expect("runtime store count fits u32"))
                    .expect("original member namespace fits u32"),
            })
            .collect();
    }
    pub(super) fn schema(&self, key: NativeTypeDeclarationKey) -> Option<&RuntimeTableConstructor> {
        let (snapshot, base, _) = self.bases?;
        if key.snapshot != snapshot { return None; }
        let (candidate, constructor) = self.schema_keys.get(key.index.checked_sub(base)? as usize)?;
        (*candidate == key).then(|| self.constructor(*constructor)).flatten()
    }
    #[cfg(test)]
    pub(super) fn schemas(&self) -> impl Iterator<Item = NativeTypeDeclarationKey> + '_ {
        self.schema_keys.iter().map(|(key, _)| *key)
    }
    pub(super) fn schema_key(
        &self,
        constructor: NativeExpressionKey,
    ) -> Option<NativeTypeDeclarationKey> {
        self.schema_indices.get(&constructor).map(|&index| self.schema_keys[index].0)
    }
    pub(super) fn member(&self, key: NativeTypeMemberKey) -> Option<&RuntimeTableStore> {
        self.member_store_index(key).map(|index| &self.stores[index])
    }
    pub(super) fn member_store_index(&self, key: NativeTypeMemberKey) -> Option<usize> {
        let (snapshot, _, base) = self.bases?;
        if key.snapshot != snapshot { return None; }
        let index = key.index.checked_sub(base)? as usize;
        (self.member_keys.get(index) == Some(&key)).then_some(index)
    }
    /// Each member key with the index of its original store.
    pub(super) fn members_with_stores(&self) -> impl Iterator<Item = (NativeTypeMemberKey, usize)> + '_ {
        self.member_keys.iter().copied().enumerate().map(|(index, key)| (key, index))
    }
    pub(super) fn constructor(&self, key: NativeExpressionKey) -> Option<&RuntimeTableConstructor> {
        self.constructor_indices
            .get(&key)
            .map(|&index| &self.constructors[index])
    }
    pub(super) fn stores(&self) -> &[RuntimeTableStore] {
        &self.stores
    }
    pub(super) fn store_for_target(&self, span: Span) -> Option<&RuntimeTableStore> {
        self.target_stores.get(&(span.start, span.end)).map(|&index| &self.stores[index])
    }
    /// Recipes use context zero. Contextual/captured instances retain these
    /// same original nodes and acquire their native query context separately.
    pub(super) fn store(&self, key: NativeFlowKey) -> Option<&RuntimeTableStore> {
        let mut original = key;
        original.graph.context = 0;
        self.store_indices
            .get(&original)
            .map(|&index| &self.stores[index])
    }
    pub(super) fn store_index(&self, key: NativeFlowKey) -> Option<usize> {
        let mut original = key;
        original.graph.context = 0;
        self.store_indices.get(&original).copied()
    }
}
fn constructor(mut expression: &Expr) -> Option<&Expr> {
    while let ExprKind::Parenthesized(inner) = &expression.kind {
        expression = inner;
    }
    matches!(expression.kind, ExprKind::Table(_)).then_some(expression)
}
fn attach_annotation(
    contexts: &mut HashMap<(usize, usize), Vec<WrittenTableContext>>,
    expression: &Expr,
    annotation: &TypeAnnotation,
    slot: usize,
) {
    if let Some(table) = constructor(expression) {
        contexts
            .entry((table.span.start, table.span.end))
            .or_default()
            .push(WrittenTableContext::Annotation {
                annotation: annotation.clone(),
                slot,
            });
    }
}
pub(super) fn collect_statements<'a>(statements: &'a [Stmt], result: &mut Vec<&'a Stmt>) {
    for statement in statements {
        result.push(statement);
        match &statement.kind {
            StmtKind::Block { body, .. }
            | StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. }
            | StmtKind::NumericFor { body, .. }
            | StmtKind::GenericFor { body, .. } => collect_statements(body, result),
            StmtKind::LocalFunction { function, .. }
            | StmtKind::FunctionDeclaration { function, .. } => {
                collect_statements(&function.body, result)
            }
            StmtKind::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    collect_statements(&branch.body, result);
                }
                if let Some(body) = else_branch {
                    collect_statements(&body.body, result);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bind, native_functions::NativeRegistry};

    fn indexed(source: &str) -> (NativeRegistry, NativeRuntimeTables) {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let registry = NativeRegistry::new(file.clone(), bind::bind(&file));
        let tables =
            NativeRuntimeTables::new(&file, registry.expressions(), registry.iterator_sources());
        (registry, tables)
    }
    fn text(source: &str, span: Span) -> &str {
        &source[span.start..span.end]
    }

    #[test]
    fn plain_module_function_stores_keep_original_function_graphs() {
        let source = "local License={}\n---@param name string\n---@return boolean\nfunction License.add(name) return true end\n---@return string\nfunction License.get() return 'license' end\nreturn License\n";
        let (registry, tables) = indexed(source);
        assert_eq!(tables.constructors().len(), 1);
        assert!(tables.constructors()[0].is_lexically_uncontracted());
        assert_eq!(tables.stores().len(), 2);
        for store in tables.stores() {
            assert_eq!(
                text(
                    source,
                    registry.expressions().get(store.receiver).expression.span
                ),
                "License"
            );
            assert!(matches!(
                registry.iterator_sources().value(store.value),
                Some(crate::native_flow::ValueRecipe::Function(_))
            ));
            assert!(store.annotation.is_none());
        }
        assert_eq!(
            tables.stores()[0].member,
            RuntimeTableStoreKey::Fixed(LuaKey::String(b"add".to_vec()))
        );
        assert_eq!(
            tables.stores()[1].member,
            RuntimeTableStoreKey::Fixed(LuaKey::String(b"get".to_vec()))
        );
    }

    #[test]
    fn aliases_rebindings_and_nested_aliases_retain_values_without_inventing_owners() {
        let source = "local object={}\nlocal alias=object\nobject={}\nalias.old=1\nobject.new=2\nobject.Name={value=3}\nobject.Tag=object.Name\n";
        let (registry, tables) = indexed(source);
        assert_eq!(tables.constructors().len(), 3);
        assert_eq!(tables.stores().len(), 4);
        let old = &tables.stores()[0];
        let new = &tables.stores()[1];
        assert_eq!(
            text(
                source,
                registry.expressions().get(old.receiver).expression.span
            ),
            "alias"
        );
        assert_eq!(
            text(
                source,
                registry.expressions().get(new.receiver).expression.span
            ),
            "object"
        );
        let tag = &tables.stores()[3];
        let Some(crate::native_flow::ValueRecipe::Evaluation(evaluation)) =
            registry.iterator_sources().value(tag.value)
        else {
            panic!("original alias RHS evaluation")
        };
        let program = &registry.iterator_sources().program;
        let expression = &program
            .expression(program.evaluation(*evaluation).expression)
            .expression;
        assert_eq!(text(source, expression.span), "object.Name");
        // The index records the alias expression; it never decides which
        // allocation receives the store from lexical spelling or source order.
    }

    #[test]
    fn parallel_assignment_keeps_old_receiver_and_frozen_transaction() {
        let source = "local t={}\nt,t.value={},1\n";
        let (registry, tables) = indexed(source);
        assert_eq!(tables.stores().len(), 1);
        let store = &tables.stores()[0];
        let flow = registry.iterator_sources();
        let node = flow.source_node(store.key).unwrap();
        let flow::NodeKind::Write { address, store_order, .. } = &node.kind else {
            panic!("write")
        };
        // Lua's right-to-left commit order is retained from the original graph.
        assert_eq!(*store_order, 0);
        let receiver_input = flow.program.evaluation(address.receiver.unwrap()).input.index;
        let transaction_nodes: Vec<_> = flow.program.graphs[store.key.graph.region as usize].nodes.iter().filter(|node| {
            matches!(&node.kind, flow::NodeKind::Write { address: other, .. } if other.transaction == address.transaction)
        }).collect();
        assert_eq!(transaction_nodes.len(), 2);
        assert!(receiver_input < transaction_nodes[0].id.index);
        assert!(store.epoch.node <= receiver_input);
        assert_eq!(tables.constructors().len(), 2);
        assert!(tables.constructors()[1].input.node >= receiver_input);
    }

    #[test]
    fn computed_keys_literal_key_evaluations_and_written_store_contracts_survive() {
        let source =
            "local t={}\nlocal key='field'\n---@type number\nt[key]=1\nt[1.0]=2\nt[-0.0]=3\n";
        let (registry, tables) = indexed(source);
        let stores = tables.stores();
        assert_eq!(stores.len(), 3);
        let RuntimeTableStoreKey::Computed(key) = stores[0].member else {
            panic!("computed key")
        };
        assert_eq!(
            text(source, registry.expressions().get(key).expression.span),
            "key"
        );
        let flow = registry.iterator_sources();
        assert!(stores.iter().all(|store| matches!(&flow.source_node(store.key).unwrap().kind,
            flow::NodeKind::Write { address, .. } if address.key.is_some())));
        assert_eq!(
            stores[1].member,
            RuntimeTableStoreKey::Fixed(LuaKey::Integer(1))
        );
        assert_eq!(
            stores[2].member,
            RuntimeTableStoreKey::Fixed(LuaKey::Integer(0))
        );
        let (annotation, slot) = stores[0].annotation.as_ref().unwrap();
        assert_eq!(*slot, 0);
        assert_eq!(text(source, annotation.types[0].span), "number");
        assert!(stores[1].annotation.is_none());
    }

    #[test]
    fn constructor_contracts_are_syntactic_and_original_keys_survive_context_lookup() {
        let source = "---@class C\nlocal c={}\n---@type table\nlocal typed=({})\nlocal free={child={}}\nlocal cast=--[[---@type table]]{}\n---@type table\nreturn {}\n";
        let (_, tables) = indexed(source);
        assert_eq!(tables.constructors().len(), 6);
        assert!(matches!(
            tables.constructors()[0].written_contexts.as_slice(),
            [WrittenTableContext::NamedDeclaration { .. }]
        ));
        assert!(matches!(
            tables.constructors()[1].written_contexts.as_slice(),
            [WrittenTableContext::Annotation { slot: 0, .. }]
        ));
        assert!(tables.constructors()[2].is_lexically_uncontracted());
        assert!(tables.constructors()[3].is_lexically_uncontracted());
        assert!(matches!(
            tables.constructors()[4].written_contexts.as_slice(),
            [WrittenTableContext::Annotation { slot: 0, .. }]
        ));
        assert!(matches!(
            tables.constructors()[5].written_contexts.as_slice(),
            [WrittenTableContext::Annotation { slot: 0, .. }]
        ));
        let key = tables.constructors()[0].key;
        assert!(tables.constructor(key).is_some());
        assert!(
            tables
                .constructor(NativeExpressionKey {
                    snapshot: key.snapshot + 1,
                    ..key
                })
                .is_none()
        );
        let (_, other) = indexed("local x={}\nx.value=1\n");
        let store = &other.stores()[0];
        let mut contextual = store.key;
        contextual.graph.context = 42;
        assert_eq!(other.store(contextual).unwrap().key, store.key);
        let mut foreign = store.key;
        foreign.graph.snapshot += 1;
        assert!(other.store(foreign).is_none());
    }
}
