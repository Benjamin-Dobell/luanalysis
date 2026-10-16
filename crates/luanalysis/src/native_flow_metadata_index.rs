use super::*;

fn fixture(source: &str) -> (NativeFlowSources, crate::bind::BoundFile) {
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let bound = crate::bind::bind(&file);
    let sources = NativeFlowSources::new(&file, &bound, 91_217, &HashMap::default());
    (sources, bound)
}

// Preserve the previous whole-program scan as an independent oracle for this
// source-index optimization. It must agree for every original reference, not
// just references selected by a particular type-checking workload.
fn scanned_declaration(
    sources: &NativeFlowSources,
    reference: NativeReferenceKey,
) -> Option<NativeVariableDeclaration> {
    let source = sources.source_reference(reference)?;
    if matches!(
        source.root,
        flow::ReferenceRoot::LoaderState | flow::ReferenceRoot::EnvironmentObject
    ) {
        let owner = sources.graph(0, reference.graph.context);
        let writes = sources.program.graphs.iter().flat_map(|graph| &graph.nodes).filter_map(|node| {
            let flow::NodeKind::Write { address, .. } = &node.kind else { return None; };
            if matches!(address.reference.root, flow::ReferenceRoot::Binding(_)) && address.reference.path.is_empty() { return None; }
            let candidates = sources.environment_candidate_paths(&address.reference);
            let affects = if source.root == flow::ReferenceRoot::LoaderState {
                sources.loader_address_invalidates(address) || candidates.iter().any(|path| match path.as_slice() {
                    [flow::Key::Dynamic(_), ..] => true,
                    [flow::Key::String(name)] if name == b"require" => !sources.original_require_capability,
                    [flow::Key::String(name), members @ ..] if name == b"package" => crate::bind::modifies_package(&members.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                    _ => false,
                })
            } else {
                candidates.iter().any(|path| matches!((path.as_slice(), source.path.first()), ([written], Some(query)) if written == query || matches!(written, flow::Key::Dynamic(_))))
            };
            affects.then_some(NativeVariableWrite { owner: sources.graph(node.id.graph, reference.graph.context), statement_end: address.statement_end })
        }).collect();
        return Some(NativeVariableDeclaration {
            owner,
            reference: sources.reference_key(owner, source)?,
            writes,
        });
    }
    if !source.path.is_empty() {
        return None;
    }
    let region = match source.root {
        flow::ReferenceRoot::Binding(binding) => *sources.binding_owners.get(&binding)?,
        flow::ReferenceRoot::EnvironmentCell => 0,
        _ => return None,
    };
    let owner = sources.graph(region, reference.graph.context);
    let writes = sources
        .program
        .graphs
        .iter()
        .flat_map(|graph| &graph.nodes)
        .filter_map(|node| {
            let flow::NodeKind::Write { address, .. } = &node.kind else {
                return None;
            };
            (address.reference.root == source.root && address.reference.path.is_empty()).then_some(
                NativeVariableWrite {
                    owner: sources.graph(node.id.graph, reference.graph.context),
                    statement_end: address.statement_end,
                },
            )
        })
        .collect();
    Some(NativeVariableDeclaration {
        owner,
        reference: sources.reference_key(owner, source)?,
        writes,
    })
}

type DeclarationShape = (
    NativeGraphKey,
    NativeReferenceKey,
    Vec<(NativeGraphKey, usize)>,
);
fn declaration_shape(value: Option<NativeVariableDeclaration>) -> Option<DeclarationShape> {
    value.map(|value| {
        (
            value.owner,
            value.reference,
            value
                .writes
                .into_iter()
                .map(|write| (write.owner, write.statement_end))
                .collect(),
        )
    })
}

fn compare_every_reference(sources: &NativeFlowSources, context: u64) {
    for (region, references) in sources.references.iter().enumerate() {
        for index in 0..references.len() {
            let reference = NativeReferenceKey {
                graph: sources.graph(region as u32, context),
                reference: index as u32,
            };
            assert_eq!(
                declaration_shape(sources.reference_declaration(reference)),
                declaration_shape(scanned_declaration(sources, reference)),
                "original reference {:?}",
                references[index]
            );
        }
    }
}

#[test]
fn indexed_declarations_match_all_graph_writes_and_context_owner_remapping() {
    let source = r#"local cell = 1
local object = {value = 1}
local function mutate(flag)
    if flag then cell = 2 else object.value = 3 end
    local function nested() cell = 4 end
    nested()
end
if unknown then cell = 5 end
object[unknown] = 6
_ENV = _ENV
"#;
    let (sources, _) = fixture(source);
    assert!(sources.program.graphs.len() >= 3);
    compare_every_reference(&sources, 0);
    // The transport's context ordinal must be retained on every original
    // owner graph, including writes from nested/captured declarations.
    sources.contexts.borrow_mut().by_id.push(FlowContext {
        contextual: None,
        captured: None,
    });
    compare_every_reference(&sources, 1);
    assert_eq!(
        sources.write_metadata.nodes.len(),
        sources
            .program
            .graphs
            .iter()
            .flat_map(|graph| &graph.nodes)
            .filter(|node| matches!(node.kind, flow::NodeKind::Write { .. }))
            .count()
    );
}

#[test]
fn environment_indices_keep_aliases_dynamic_keys_prefixes_and_both_loader_policies() {
    let source = r#"local env = _G
local alias = unknown and env or _ENV
local namespace = package
local key = unknown
require = function() end
_G.require = function() end
alias[key] = false
alias.type = function() end
alias.package.path = 'nested'
namespace.path = 'direct'
namespace.searchers[1] = function() end
package.preload.changed = function() end
local function captured()
    alias.require = function() end
    env[key] = nil
end
local selected = unknown and _G.type or _G.assert
selected = function() end
"#;
    let (mut sources, _) = fixture(source);
    sources.seed_original_global_reference_keys(&[
        b"unwritten".to_vec(),
        b"type".to_vec(),
        b"assert".to_vec(),
    ]);
    for capability in [false, true, false] {
        sources.set_original_require_capability(capability);
        compare_every_reference(&sources, 0);
    }
    assert!(sources.write_metadata.loader[0].len() > sources.write_metadata.loader[1].len());
    assert!(
        !sources
            .write_metadata
            .dynamic_environment_members
            .is_empty()
    );
}

fn scanned_initializer(
    sources: &NativeFlowSources,
    binding: BindingId,
) -> Option<flow::EvaluationId> {
    let mut writes = sources
        .program
        .graphs
        .iter()
        .flat_map(|graph| &graph.nodes)
        .filter_map(|node| {
            let flow::NodeKind::Write {
                address,
                value,
                declaration,
                ..
            } = &node.kind
            else {
                return None;
            };
            (address.reference.root == flow::ReferenceRoot::Binding(binding)
                && address.reference.path.is_empty())
            .then_some((value, *declaration))
        });
    let (flow::ValueDemand::Evaluation(initializer), true) = writes.next()? else {
        return None;
    };
    if writes.next().is_some() {
        return None;
    }
    Some(*initializer)
}

#[test]
fn immutable_initializer_index_counts_later_branch_and_captured_cell_writes() {
    let source = r#"local stable = {}
local alias = stable
local replaced = {}
local conditional = {}
local captured = {}
local missing
local first, second = function() return {}, {} end
stable.field = 1
replaced = {}
if unknown then conditional = {} end
local function mutate() captured = {} end
"#;
    let (sources, bound) = fixture(source);
    for binding in &bound.declarations {
        assert_eq!(
            sources
                .immutable_initializer(binding.id)
                .map(|evaluation| evaluation.id),
            scanned_initializer(&sources, binding.id),
            "{}",
            binding.name
        );
    }
    for (name, expected) in [
        ("stable", true),
        ("alias", true),
        ("replaced", false),
        ("conditional", false),
        ("captured", false),
        ("missing", false),
        ("mutate", false),
    ] {
        let binding = bound
            .declarations
            .iter()
            .find(|binding| binding.name == name)
            .unwrap();
        assert_eq!(
            sources.immutable_initializer(binding.id).is_some(),
            expected,
            "{name}"
        );
    }
}
