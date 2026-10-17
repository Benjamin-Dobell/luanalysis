use super::*;
use crate::native_flow::SignatureFrame;
use luanalysis_engine::{Engine, FunctionType, NativeReferenceKey};
use std::sync::Arc;

fn input(
    index: u32,
    source: &str,
    standard_library: Option<StandardLibrarySource>,
) -> RegistryInput {
    let syntax = luanalysis_syntax::parse(source);
    assert!(syntax.diagnostics.is_empty(), "{:#?}", syntax.diagnostics);
    let bound = crate::bind::bind(&syntax);
    RegistryInput {
        id: FileId {
            project: 97_431,
            index,
        },
        role: if standard_library.is_some() {
            SourceRole::Declaration
        } else {
            SourceRole::Implementation
        },
        standard_library,
        syntax: syntax.into(),
        bound: bound.into(),
    }
}

fn snapshot() -> ProjectRegistrySnapshot {
    ProjectRegistrySnapshot::new(
        vec![
            input(
                0,
                "local prefix=1\nlocal callback=function(value) return prefix+value end\nlocal target={}\ntarget.first=1\nif unknown then target.second=2 end\nreturn callback\n",
                None,
            ),
            input(
                1,
                include_str!("../stdlib/Lua52/global.def.lua"),
                Some(StandardLibrarySource::Base),
            ),
            input(
                2,
                include_str!("../stdlib/Lua52/builtin.def.lua"),
                Some(StandardLibrarySource::Builtin),
            ),
        ],
        Vec::new(),
        NativeLoaderPolicy::Lua52StandardFile,
        StandardEnvironmentProfile::Lua52,
        crate::OverloadMode::V2,
    )
}

fn fingerprint(provider: &ProjectRegistry) -> Vec<String> {
    let mut result = Vec::new();
    for file in &provider.files {
        let source = &file.source;
        result.push(format!("{:?}:{:?}", file.id, source.module_key()));
        result.extend(
            source
                .functions
                .iter()
                .map(|function| format!("{:?}", source.key_for(function))),
        );
        result.extend(
            source
                .expressions
                .sources()
                .map(|(key, source)| format!("{key:?}:{:?}", source.expression.span)),
        );
        for graph in &source.flow.program.graphs {
            result.extend(
                graph
                    .nodes
                    .iter()
                    .map(|node| format!("{:?}:{:?}", node.id, node.kind)),
            );
            let graph = source.flow.graph(graph.index, 0);
            for reference in 0.. {
                let key = NativeReferenceKey { graph, reference };
                let Some(original) = source.flow.source_reference(key) else {
                    break;
                };
                result.push(format!("{key:?}:{original:?}"));
            }
        }
    }
    result
}

#[test]
fn prepared_worker_flows_share_original_storage_and_preserve_all_source_keys() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProjectRegistrySnapshot>();
    let mut snapshot = snapshot();
    let expected = fingerprint(&snapshot.worker_registry());
    snapshot.prepare_shared_flows();
    let prepared = snapshot.worker_registry();
    assert_eq!(fingerprint(&prepared), expected);
    let recipes: Vec<_> = prepared
        .files
        .iter()
        .map(|file| file.source.flow.shared_recipes())
        .collect();
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let snapshot = &snapshot;
                let expected = &expected;
                let recipes = &recipes;
                let barrier = &barrier;
                scope.spawn(move || {
                    let registry = snapshot.worker_registry();
                    barrier.wait();
                    assert_eq!(fingerprint(&registry), *expected);
                    for (file, original) in registry.files.iter().zip(recipes) {
                        assert!(Arc::ptr_eq(&file.source.flow.shared_recipes(), original));
                        assert_eq!(file.source.flow.context_state_counts(), (0, 0, 0));
                    }
                })
            })
            .collect();
        for task in tasks {
            task.join().unwrap();
        }
    });
    assert_eq!(fingerprint(&prepared), expected);
}

#[test]
fn prepared_global_keys_and_identical_profile_setters_do_not_copy_original_storage() {
    let mut snapshot = snapshot();
    snapshot.prepare_shared_flows();
    let mut worker = snapshot.worker_registry();
    let names: Vec<_> = worker
        .globals
        .bindings()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.iter().any(|name| name == b"require"));
    assert!(names.iter().any(|name| name == b"type"));
    let expected = fingerprint(&worker);
    for file in &mut worker.files {
        let flow = &mut file.source.flow;
        let before = flow.shared_recipes();
        assert!(flow.has_original_require_capability());
        // The selected full profile supplies these exact original capabilities.
        // Repeating setup must leave the frozen recipe allocation shared.
        flow.set_original_require_capability(true);
        flow.set_runtime_type_intrinsic_enabled(true);
        flow.set_opaque_runtime_type_enabled(NativeOpaqueRuntimeKind::Userdata, true);
        flow.set_opaque_runtime_type_enabled(NativeOpaqueRuntimeKind::Thread, true);
        assert!(Arc::ptr_eq(&before, &flow.shared_recipes()));
        for name in &names {
            for root in [
                crate::flow::ReferenceRoot::EnvironmentObject,
                crate::flow::ReferenceRoot::EnvironmentCell,
            ] {
                let reference = crate::flow::Reference {
                    root,
                    path: vec![crate::flow::Key::String(name.clone())],
                };
                assert!(flow.reference_key(flow.graph(0, 0), &reference).is_some());
            }
        }
    }
    assert_eq!(fingerprint(&worker), expected);
}

#[test]
fn real_capture_contexts_and_ancestry_remain_worker_local_across_sessions() {
    let mut snapshot = snapshot();
    snapshot.prepare_shared_flows();
    let original = snapshot.worker_registry().files[0]
        .source
        .flow
        .shared_recipes();
    for _ in 0..2 {
        Engine::new().with_native_provider(
            || snapshot.worker_registry(),
            |first, queries| {
                let second = snapshot.worker_registry();
                let first_source = &first.files[0].source;
                let first_flow = &first_source.flow;
                let second_flow = &second.files[0].source.flow;
                assert!(Arc::ptr_eq(&original, &first_flow.shared_recipes()));
                assert!(Arc::ptr_eq(&original, &second_flow.shared_recipes()));
                assert_eq!(first_flow.context_state_counts(), (0, 0, 0));
                assert_eq!(second_flow.context_state_counts(), (0, 0, 0));
                let function = first_source.key(0);
                let prefix = first_source
                    .bound()
                    .declarations
                    .iter()
                    .find(|binding| binding.name == "prefix")
                    .unwrap()
                    .id;
                let captured = queries.captured_function(
                    function,
                    &[(first_source.source_binding_key(prefix), Type::Number)],
                );
                let expected = Type::Function(FunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: Type::Number,
                        optional: false,
                    }],
                    returns: Box::new(Type::Number),
                });
                let expression =
                    first_source.expression_key_for_span(first_source.function(function).span);
                let selected = queries.contextual_function_value(
                    &captured,
                    expression,
                    &expected,
                    NativeCheckMode::CONTEXTUAL,
                );
                let capture = queries
                    .function_capture_context(&selected)
                    .expect("the selected function has a real native capture");
                let context = queries
                    .captured_function_context(&capture)
                    .expect("the untyped parameter receives its native contextual frame");
                first_flow.retain_signature_ancestry(vec![SignatureFrame {
                    function,
                    contextual: Some(context),
                    captured: capture,
                }]);
                let region = first_flow
                    .graph_for_function(function, None)
                    .unwrap()
                    .region;
                let graph = first_flow.graph_with_contexts(region, Some(&context), Some(&capture));
                assert_eq!(first_flow.captured(graph), Some(capture));
                assert_eq!(first_flow.contextual(graph), Some(context));
                let (contexts, ancestry, owners) = first_flow.context_state_counts();
                assert!(contexts >= 1 && ancestry >= 1 && owners >= 1);
                assert_eq!(second_flow.context_state_counts(), (0, 0, 0));
                assert!(!second_flow.contains_graph(graph));
                // Registering the same ordinal in another worker must not import
                // its capture ancestry or contextual-owner attestation.
                let second_graph =
                    second_flow.graph_with_contexts(region, Some(&context), Some(&capture));
                assert_eq!(second_flow.captured(second_graph), None);
                assert_eq!(second_flow.contextual(second_graph), None);
                assert_eq!(second_flow.context_state_counts(), (1, 0, 0));
                assert_eq!(first_flow.captured(graph), Some(capture));
                assert!(Arc::ptr_eq(&original, &first_flow.shared_recipes()));
                assert!(Arc::ptr_eq(&original, &second_flow.shared_recipes()));
            },
        );
        // Only original recipes cross the session boundary; each next provider
        // must start without handles, contexts, or ancestry from the prior run.
        assert_eq!(
            snapshot.worker_registry().files[0]
                .source
                .flow
                .context_state_counts(),
            (0, 0, 0)
        );
    }
}
