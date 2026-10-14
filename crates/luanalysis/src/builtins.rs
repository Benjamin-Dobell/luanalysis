//! Source-independent signatures for Lua's standard runtime intrinsics.
//! Intrinsic recognition still follows lexical binding, so locals can shadow
//! these names. Runtime predicate transfer belongs to the native flow worker.

use luanalysis_engine::{FunctionType, Parameter, Type};

pub(super) fn value(name: &str) -> Option<Type> {
    match name {
        "type" => Some(Type::Function(FunctionType {
            params: vec![Parameter {
                name: "value".into(),
                ty: Type::Any,
                optional: false,
            }],
            returns: Box::new(Type::String),
        })),
        _ => None,
    }
}
