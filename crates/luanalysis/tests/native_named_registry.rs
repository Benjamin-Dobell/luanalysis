//! Test-only module import keeps registry development out of the shared
//! production wiring until the lazy named provider/parameter owner API lands.
use luanalysis::bind;
use luanalysis_engine::NativeFunctionKey;
use luanalysis_syntax::{
    ExprKind, NamedRecordKind, NamedTypeDeclarationKind, Span, StmtKind, TypeExprKind,
    TypeFieldKey, TypeLiteral,
};
use luanalysis_engine::FxHashMap as HashMap;

#[allow(dead_code)]
#[path = "../src/native_named.rs"]
mod native_named;
use native_named::*;

const SNAPSHOT: u64 = 401;

fn file(source: &str) -> luanalysis_syntax::File {
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    file
}

fn registry(source: &str) -> NativeNamed {
    let file = file(source);
    let bound = bind::bind(&file);
    NativeNamed::new(SNAPSHOT, &file, &bound, &HashMap::default()).unwrap()
}

#[test]
fn every_identity_exists_before_forward_and_recursive_target_demands() {
    let named = registry(
        "---@alias First Later\n---@alias Later First[]\n---@shape Node\n---@field child First",
    );
    assert_eq!(named.declarations().len(), 3);
    let first = named.candidates("First")[0];
    let later = named.candidates("Later")[0];
    assert_ne!(first, later);
    assert_eq!(first.snapshot, SNAPSHOT);
    let declaration = named.declaration(first).unwrap();
    let NamedTypeDeclarationKind::Alias { target } = &declaration.syntax.kind else {
        panic!("alias")
    };
    assert!(matches!(&target.kind, TypeExprKind::Named(name) if name=="Later"));
    assert_eq!(named.key_for_syntax(declaration.syntax.id), Some(first));
    // The second body remains a reference, not an expanded recursive shape.
    let NamedTypeDeclarationKind::Alias { target } = &named.declaration(later).unwrap().syntax.kind
    else {
        panic!("alias")
    };
    assert!(
        matches!(&target.kind, TypeExprKind::Array {element,..} if matches!(&element.kind,TypeExprKind::Named(name) if name=="First"))
    );
}

#[test]
fn duplicate_names_and_duplicate_fields_remain_distinct_for_core_diagnostics() {
    let named = registry(
        "---@alias Duplicate number\n---@alias Duplicate string\n---@class Record\n---@field value number\n---@field value string",
    );
    let aliases = named.candidates("Duplicate");
    assert_eq!(aliases.len(), 2);
    assert_ne!(aliases[0], aliases[1]);
    let record = named.declaration(named.candidates("Record")[0]).unwrap();
    assert_eq!(record.members.len(), 2);
    assert_ne!(record.members[0], record.members[1]);
    let member = named.member(record.members[0]).unwrap();
    assert!(matches!(member.owner, NamedMemberOwner::Declared(key) if key==record.key));
    assert!(matches!(&member.field.key,TypeFieldKey::Named(name) if name.text=="value"));
}

#[test]
fn named_generic_owners_are_not_manufactured_function_identities() {
    let named =
        registry("---@alias Alias<T:string,T:number> T\n---@shape Record<T>\n---@field value T");
    let alias = named.candidates("Alias")[0];
    let scope = named
        .generic_scope(NativeTypeContext::NamedType(alias))
        .unwrap();
    assert_eq!(scope.owner, NativeTypeContext::NamedType(alias));
    assert_eq!(scope.parent, None);
    assert_eq!(scope.parameters.len(), 2);
    assert_eq!(scope.parameters[0].order, 0);
    assert_eq!(scope.parameters[1].order, 1);
    assert!(
        scope
            .parameters
            .iter()
            .all(|parameter| parameter.name.text == "T" && parameter.owner == scope.owner)
    );
    assert!(
        matches!(&scope.parameters[0].constraint.as_ref().unwrap().kind,TypeExprKind::Named(name) if name=="string")
    );
    let function = NativeTypeContext::Function(NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: alias.index,
    });
    assert_ne!(scope.owner, function);
    assert!(named.generic_scope(function).is_none());
}

#[test]
fn member_key_visibility_and_name_origins_are_original_metadata() {
    let source = "-- café\r\n---@class Box<T>\r\n---@field private [1] T\r\n---@field [\"1\"] string\r\nlocal Box={}\r\n";
    let named = registry(source);
    let declaration = named.declaration(named.candidates("Box")[0]).unwrap();
    assert!(matches!(
        &declaration.syntax.kind,
        NamedTypeDeclarationKind::Record {
            flavor: NamedRecordKind::Class,
            ..
        }
    ));
    let first = named.member(declaration.members[0]).unwrap();
    let second = named.member(declaration.members[1]).unwrap();
    assert_eq!(
        &source[first.field.span.start..first.field.span.end],
        "@field private [1] T"
    );
    assert_eq!(
        &source
            [first.field.visibility.unwrap().span.start..first.field.visibility.unwrap().span.end],
        "private"
    );
    assert_eq!(
        first.lexical_context,
        Some(NativeTypeContext::NamedType(declaration.key))
    );
    assert!(
        matches!(&first.field.key,TypeFieldKey::Indexed(key) if matches!(&key.kind,TypeExprKind::Literal(TypeLiteral::Number(value)) if value=="1"))
    );
    assert!(
        matches!(&second.field.key,TypeFieldKey::Indexed(key) if matches!(&key.kind,TypeExprKind::Literal(TypeLiteral::String(value)) if value==b"1"))
    );
}

#[test]
fn augmentations_retain_all_forward_owner_candidates_without_declaring_them() {
    let named = registry(
        "---@field <Owner> x number\n---@class Owner\n---@class Owner\n---@alias Alias Owner\n---@field <Alias> y string\n---@field <Missing> z boolean",
    );
    assert_eq!(named.declarations().len(), 3);
    assert_eq!(named.augmentations().len(), 3);
    let owner = &named.augmentations()[0];
    assert_eq!(owner.candidates, named.candidates("Owner"));
    assert_eq!(owner.candidates.len(), 2);
    let member = named.member(owner.members[0]).unwrap();
    assert!(
        matches!(&member.owner,NamedMemberOwner::Reference {name,candidates,augmentation} if name.text=="Owner"&&candidates.len()==2&&*augmentation==owner.key)
    );
    let alias = &named.augmentations()[1];
    assert_eq!(alias.candidates, named.candidates("Alias"));
    // Alias owners remain alias candidates, not eagerly redirected to Owner.
    assert_eq!(alias.candidates.len(), 1);
    assert!(named.augmentations()[2].candidates.is_empty());
    assert!(named.candidates("Missing").is_empty());
    assert_eq!(named.augmentation(owner.key).unwrap().key, owner.key);
}

#[test]
fn runtime_attachments_keep_all_bindings_and_original_assignment_targets() {
    let source = "---@class Pair\nlocal first,second={},{}\n---@class Nested\nfirst.child={}\n";
    let parsed = file(source);
    let bound = bind::bind(&parsed);
    let named = NativeNamed::new(SNAPSHOT, &parsed, &bound, &HashMap::default()).unwrap();
    let local = named.attachment(parsed.statements[0].span).unwrap();
    let RuntimeAttachmentKind::Local { bindings } = &local.kind else {
        panic!("local")
    };
    assert_eq!(bindings.len(), 2);
    assert_eq!(local.declarations, named.candidates("Pair"));
    assert_eq!(bound.declaration(bindings[0]).name, "first");
    assert_eq!(bound.declaration(bindings[1]).name, "second");
    let assignment = named.attachment(parsed.statements[1].span).unwrap();
    let RuntimeAttachmentKind::Assignment { targets } = &assignment.kind else {
        panic!("assignment")
    };
    assert_eq!(
        &source[targets[0].span.start..targets[0].span.end],
        "first.child"
    );
    assert_eq!(assignment.declarations, named.candidates("Nested"));
}

#[test]
fn enclosing_function_context_uses_original_spans_not_following_attachment() {
    let parsed = file(
        "---@alias Outer number\nlocal function enclosing()\n---@alias Inner<T> T\nlocal value=1\nend",
    );
    let bound = bind::bind(&parsed);
    let StmtKind::LocalFunction { function, .. } = &parsed.statements[0].kind else {
        panic!("function")
    };
    let key = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    };
    let functions = HashMap::from_iter([((function.span.start, function.span.end), key)]);
    let named = NativeNamed::new(SNAPSHOT, &parsed, &bound, &functions).unwrap();
    assert_eq!(
        named
            .declaration(named.candidates("Outer")[0])
            .unwrap()
            .generic_scope
            .parent,
        None
    );
    let inner = named.declaration(named.candidates("Inner")[0]).unwrap();
    assert_eq!(
        inner.generic_scope.parent,
        Some(NativeTypeContext::Function(key))
    );
    assert_eq!(
        inner.generic_scope.owner,
        NativeTypeContext::NamedType(inner.key)
    );
    assert!(inner.scope.is_some());
}

#[test]
fn embedded_function_body_attachments_are_indexed_in_control_headers() {
    let source =
        "if (function()\n---@alias HeaderLocal number\nlocal value=1\nreturn true\nend)() then end";
    let parsed = file(source);
    let bound = bind::bind(&parsed);
    let StmtKind::If { branches, .. } = &parsed.statements[0].kind else {
        panic!("if")
    };
    let ExprKind::Call { callee, .. } = &branches[0].condition.kind else {
        panic!("call")
    };
    let ExprKind::Parenthesized(expression) = &callee.kind else {
        panic!("parentheses")
    };
    let ExprKind::Function(function) = &expression.kind else {
        panic!("function")
    };
    let key = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    };
    let functions = HashMap::from_iter([((function.span.start, function.span.end), key)]);
    let named = NativeNamed::new(SNAPSHOT, &parsed, &bound, &functions).unwrap();
    let declaration = named
        .declaration(named.candidates("HeaderLocal")[0])
        .unwrap();
    assert_eq!(
        declaration.generic_scope.parent,
        Some(NativeTypeContext::Function(key))
    );
    assert!(
        named
            .attachment(declaration.syntax.attachment.unwrap())
            .is_some()
    );
}

#[test]
fn foreign_snapshot_queries_and_invalid_origin_metadata_do_not_fallback() {
    let mut parsed = file("---@alias Alias number\nlocal value=1");
    let bound = bind::bind(&parsed);
    let named = NativeNamed::new(SNAPSHOT, &parsed, &bound, &HashMap::default()).unwrap();
    let key = named.candidates("Alias")[0];
    assert!(
        named
            .declaration(NativeNamedKey {
                snapshot: SNAPSHOT + 1,
                ..key
            })
            .is_none()
    );
    assert!(
        named
            .member(NativeNamedMemberKey {
                snapshot: SNAPSHOT,
                index: 999
            })
            .is_none()
    );
    let functions = HashMap::from_iter([(
        (0, parsed.span.end),
        NativeFunctionKey {
            snapshot: SNAPSHOT + 1,
            index: 0,
        },
    )]);
    assert!(matches!(
        NativeNamed::new(SNAPSHOT, &parsed, &bound, &functions),
        Err(NamedRegistryError::ForeignFunction(_))
    ));
    parsed.type_declarations[0].attachment = Some(Span::new(999, 1000));
    assert!(matches!(
        NativeNamed::new(SNAPSHOT, &parsed, &bound, &HashMap::default()),
        Err(NamedRegistryError::MissingAttachment(_))
    ));
}

#[test]
fn malformed_binding_and_repeated_syntax_identity_are_explicit_registry_errors() {
    let mut parsed = file("---@class Attached\nlocal first,second={},{}\n");
    let mut bound = bind::bind(&parsed);
    let second = bound.declarations[1].span;
    bound
        .occurrences
        .retain(|occurrence| occurrence.span != second);
    assert!(matches!(
        NativeNamed::new(SNAPSHOT, &parsed, &bound, &HashMap::default()),
        Err(NamedRegistryError::MissingAttachmentBinding(span)) if span==second
    ));
    let bound = bind::bind(&parsed);
    parsed
        .type_declarations
        .push(parsed.type_declarations[0].clone());
    assert!(matches!(
        NativeNamed::new(SNAPSHOT, &parsed, &bound, &HashMap::default()),
        Err(NamedRegistryError::DuplicateSyntaxId(_))
    ));
}

#[test]
fn anonymous_recursive_heads_follow_named_keys_without_expanding_references() {
    let source = "---@alias Node {next:Node|nil}\n---@alias Nodes Node[]\n---@alias Lookup table<string,Node>\n";
    let named = registry(source);
    assert_eq!(named.declarations().len(), 3);
    assert_eq!(named.anonymous_heads().len(), 3);
    for (index, head) in named.anonymous_heads().iter().enumerate() {
        assert_eq!(head.key.index as usize, 3 + index);
        assert_eq!(named.anonymous_key_at(head.syntax.span), Some(head.key));
        assert!(named.anonymous_head(head.key).is_some());
        assert_eq!(head.members.len(), 1);
    }
    let record = &named.anonymous_heads()[0];
    assert_eq!(record.kind, AnonymousHeadKind::Record);
    let field = named.anonymous_member(record.members[0]).unwrap();
    assert_eq!(field.owner, record.key);
    assert!(
        matches!(&field.key_recipe,AnonymousMemberKeyRecipe::Record(TypeFieldKey::Named(name)) if name.text=="next")
    );
    assert!(
        matches!(&field.ty.kind,TypeExprKind::Union(types) if matches!(&types[0].kind,TypeExprKind::Named(name) if name=="Node"))
    );
    assert_eq!(&source[field.ty.span.start..field.ty.span.end], "Node|nil");
    let array = &named.anonymous_heads()[1];
    assert_eq!(array.kind, AnonymousHeadKind::Array);
    let element = named.anonymous_member(array.members[0]).unwrap();
    assert!(matches!(
        element.key_recipe,
        AnonymousMemberKeyRecipe::ArrayNumber
    ));
    assert!(matches!(&element.ty.kind,TypeExprKind::Named(name) if name=="Node"));
    let table = &named.anonymous_heads()[2];
    assert_eq!(table.kind, AnonymousHeadKind::Table);
    let entry = named.anonymous_member(table.members[0]).unwrap();
    assert!(
        matches!(&entry.key_recipe,AnonymousMemberKeyRecipe::Table(ty) if matches!(&ty.kind,TypeExprKind::Named(name) if name=="string"))
    );
    assert!(matches!(&entry.ty.kind,TypeExprKind::Named(name) if name=="Node"));
}

#[test]
fn nested_head_and_member_namespaces_preserve_generic_declaration_space() {
    let named =
        registry("---@shape Box<T>\n---@field child {next:T[], [string]:table<number,T|nil>}\n");
    let owner = named.candidates("Box")[0];
    assert_eq!(named.anonymous_heads().len(), 3);
    assert!(
        named
            .anonymous_heads()
            .iter()
            .all(|head| head.lexical_context == Some(NativeTypeContext::NamedType(owner)))
    );
    for head in named.anonymous_heads() {
        assert!(
            matches!(head.origins.as_slice(),[AnonymousHeadOrigin::Member(key)] if *key==named.declaration(owner).unwrap().members[0])
        );
        for &member in &head.members {
            assert!(member.index > 0); // Named child member already owns index zero.
            assert_eq!(named.anonymous_member(member).unwrap().owner, head.key);
        }
    }
    let record = &named.anonymous_heads()[0];
    let original_field = named.anonymous_member(record.members[0]).unwrap();
    let nested = named.anonymous_key_at(original_field.ty.span).unwrap();
    assert_eq!(
        named.anonymous_head(nested).unwrap().kind,
        AnonymousHeadKind::Array
    );
    assert!(
        named
            .anonymous_head(NativeAnonymousHeadKey {
                snapshot: SNAPSHOT,
                index: 0
            })
            .is_none()
    );
    assert!(
        named
            .anonymous_member(NativeAnonymousMemberKey {
                snapshot: SNAPSHOT,
                index: 0
            })
            .is_none()
    );
    assert!(
        named
            .anonymous_head(NativeAnonymousHeadKey {
                snapshot: SNAPSHOT + 1,
                ..nested
            })
            .is_none()
    );
}

#[test]
fn documentation_lexical_owners_are_distinct_from_foreign_augmentation_targets() {
    let named = registry(
        "---@class Lexical<T>\n---@field <Target> value T[]\n---@alias Within<T> T\n\n---@class Target\n",
    );
    let lexical = named.candidates("Lexical")[0];
    let target = named.candidates("Target")[0];
    let extension = &named.augmentations()[0];
    assert_eq!(extension.candidates, vec![target]);
    assert_eq!(extension.documentation_contexts, vec![lexical]);
    let member = named.member(extension.members[0]).unwrap();
    assert_eq!(member.documentation_contexts, vec![lexical]);
    assert!(
        matches!(&member.owner,NamedMemberOwner::Reference{candidates,..} if candidates==&vec![target])
    );
    let head = &named.anonymous_heads()[0];
    assert_eq!(head.documentation_contexts, vec![lexical]);
    let alias = named.declaration(named.candidates("Within")[0]).unwrap();
    assert_eq!(alias.generic_scope.documentation_parents, vec![lexical]);
    // No alias/member type is resolved merely by discovering lexical metadata.
    assert!(
        matches!(&head.syntax.kind,TypeExprKind::Array{element,..} if matches!(&element.kind,TypeExprKind::Named(name) if name=="T"))
    );
}

#[test]
fn ambiguous_documentation_contexts_and_malformed_containers_remain_raw() {
    let named =
        registry("---@class One<T>\n---@class Two<U>\n---@field <Missing> value table<string>\n");
    let contexts = vec![named.candidates("One")[0], named.candidates("Two")[0]];
    assert_eq!(named.augmentations()[0].documentation_contexts, contexts);
    assert!(named.augmentations()[0].candidates.is_empty());
    let head = &named.anonymous_heads()[0];
    assert_eq!(head.kind, AnonymousHeadKind::Table);
    assert!(matches!(&head.syntax.kind,TypeExprKind::Generic{args,..} if args.len()==1));
    assert!(head.members.is_empty()); // No missing value recipe is fabricated.
    assert_eq!(head.documentation_contexts, contexts);
}

#[test]
fn runtime_and_function_annotation_heads_keep_distinct_original_contexts() {
    let source = "---@type {value:number}\nlocal value={}\n---@param input table<string,number[]>\nlocal function use(input) end\n";
    let parsed = file(source);
    let bound = bind::bind(&parsed);
    let StmtKind::LocalFunction { function, .. } = &parsed.statements[1].kind else {
        panic!("function")
    };
    let key = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    };
    let functions = HashMap::from_iter([((function.span.start, function.span.end), key)]);
    let named = NativeNamed::new(SNAPSHOT, &parsed, &bound, &functions).unwrap();
    assert_eq!(named.anonymous_heads().len(), 3);
    assert_eq!(named.anonymous_heads()[0].lexical_context, None);
    assert!(
        matches!(&named.anonymous_heads()[0].origins[..],[AnonymousHeadOrigin::RuntimeAnnotation(span)] if *span==parsed.statements[0].span)
    );
    assert!(
        named.anonymous_heads()[1..]
            .iter()
            .all(
                |head| head.lexical_context == Some(NativeTypeContext::Function(key))
                    && head.origins == vec![AnonymousHeadOrigin::Function(key)]
            )
    );
}

#[test]
fn copied_function_type_annotation_has_one_head_and_retains_both_source_uses() {
    let source = "---@generic T\n---@type fun(value:{item:T}):T\nlocal identity=function(value) return value.item end";
    let parsed = file(source);
    let bound = bind::bind(&parsed);
    let StmtKind::Local { values, .. } = &parsed.statements[0].kind else {
        panic!("local")
    };
    let ExprKind::Function(function) = &values[0].kind else {
        panic!("function")
    };
    let key = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    };
    let named = NativeNamed::new(
        SNAPSHOT,
        &parsed,
        &bound,
        &HashMap::from_iter([((function.span.start, function.span.end), key)]),
    )
    .unwrap();
    assert_eq!(named.anonymous_heads().len(), 1);
    let head = &named.anonymous_heads()[0];
    assert_eq!(head.uses.len(), 2);
    assert!(
        head.uses
            .iter()
            .any(|usage| usage.origin == AnonymousHeadOrigin::Function(key)
                && usage.lexical_context == Some(NativeTypeContext::Function(key)))
    );
    assert!(head.uses.iter().any(|usage| usage.origin
        == AnonymousHeadOrigin::RuntimeAnnotation(parsed.statements[0].span)
        && usage.lexical_context.is_none()));
    assert_eq!(head.members.len(), 1);
    assert_eq!(named.anonymous_key_at(head.syntax.span), Some(head.key));
}
