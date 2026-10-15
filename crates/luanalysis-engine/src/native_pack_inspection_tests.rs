use crate::*;

#[test]
fn whole_any_display_does_not_invent_a_variadic_row_or_change_pack_checks() {
    Engine::new().with_session(|q| {
        let whole = q.call_pack(&Type::Any, &[], &[], None).return_pack;
        assert!(q.inspect_pack(&whole).is_none());
        assert!(!q.pack_assignable(&whole, &ValuePack::never()));
        let PackInspection::Whole(exact) = q.inspect_pack_view(&whole) else {
            panic!("whole Any carrier")
        };
        assert!(matches!(exact, Type::SessionType(_)));
        assert_eq!(q.format(&exact), "any");
        assert_eq!(q.pack_slot(&whole, 0), Type::Any);
        assert_eq!(q.pack_slot(&whole, 100), Type::Any);
        assert!(!q.pack_assignable(&whole, &ValuePack::never()));
        assert!(q.inspect_pack(&whole).is_none());
        let variadic = q.retain_pack(&ValuePack::variadic(Type::Any));
        assert_ne!(whole, variadic);
        let PackInspection::Alternatives(rows) = q.inspect_pack_view(&variadic) else {
            panic!("actual variadic row")
        };
        assert_eq!(rows.len(), 1);
        assert!(rows[0].fixed.is_empty());
        assert_eq!(rows[0].tail.as_deref(), Some(&Type::Any));
        let invalid = q.call_pack(&Type::Unknown, &[], &[], None);
        assert!(!invalid.errors.is_empty());
        let PackInspection::Whole(recovery) = q.inspect_pack_view(&invalid.return_pack) else {
            panic!("original call error recovery carrier")
        };
        assert_eq!(q.format(&recovery), "any");
    });
}

#[test]
fn whole_unknown_carrier_is_distinct_from_an_unknown_element_row() {
    use bun_sema::types::{SigData, SigParam, TypeId};
    with_program(|program, handles| {
        let mut checker = program.checker();
        checker.enable_native_arrays();
        let rest = SigParam {
            name: checker.atoms().intern(b"values"),
            ty: TypeId::UNKNOWN,
            optional: false,
            rest: true,
            is_required_rest: false,
            declaration: None,
        };
        let signature = checker.types().intern_sig(SigData::Synth {
            type_params: checker.list(&[]),
            params: checker.list(&[rest]),
            ret: TypeId::UNKNOWN,
            this: None,
            of: checker.list(&[]),
            is_union: false,
        });
        let original = checker.signature_rest_pack(signature).unwrap();
        let pack = handles.retain_pack(&checker, original);
        let mut q = SessionQueries {
            checker: &mut checker,
            handles,
        };
        assert!(q.inspect_pack(&pack).is_none());
        let PackInspection::Whole(exact) = q.inspect_pack_view(&pack) else {
            panic!("whole unknown carrier")
        };
        assert_eq!(q.format(&exact), "unknown");
        assert_eq!(import_type(q.checker, handles, &exact), TypeId::UNKNOWN);
        let row = q.retain_pack(&ValuePack::variadic(Type::Unknown));
        assert!(matches!(
            q.inspect_pack_view(&row),
            PackInspection::Alternatives(_)
        ));
        assert_ne!(row, pack);
    });
}

#[test]
fn display_inspection_keeps_correlated_rows_optional_slots_and_physical_cardinality() {
    Engine::new().with_session(|q| {
        let rows = vec![
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(true)),
                    PackSlot::required(Type::Number),
                ],
                tail: None,
            },
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(false)),
                    PackSlot::required(Type::String),
                ],
                tail: None,
            },
        ];
        let pack = q.retain_pack(&ValuePack::Alternatives(rows));
        let before = q.inspect_pack(&pack).unwrap();
        let PackInspection::Alternatives(displayed) = q.inspect_pack_view(&pack) else {
            panic!("correlated rows")
        };
        assert_eq!(displayed, before);
        assert_eq!(q.inspect_pack(&pack).unwrap(), before);
        let empty = q.retain_pack(&ValuePack::empty());
        let never = q.retain_pack(&ValuePack::never());
        let PackInspection::Alternatives(empty_rows) = q.inspect_pack_view(&empty) else {
            panic!("zero result row")
        };
        let PackInspection::Alternatives(never_rows) = q.inspect_pack_view(&never) else {
            panic!("no reachable rows")
        };
        assert_eq!(empty_rows.len(), 1);
        assert!(empty_rows[0].fixed.is_empty() && empty_rows[0].tail.is_none());
        assert!(never_rows.is_empty());
    });
}
