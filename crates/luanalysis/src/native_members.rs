//! Finite original class member declarations. These are immutable source
//! recipes; native named queries own type evaluation, cycles and substitution.
use crate::native_named::{NativeNamed, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{LuaKey, NativeExpressionKey, NativeFunctionKey, NativeTypeMemberKey};
use luanalysis_syntax::{Span, TypeExpr};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OriginalMemberKey {
    Fixed(LuaKey),
    Computed(NativeExpressionKey),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OriginalMemberOrigin {
    ConstructorField(Span),
    Method(NativeFunctionKey),
}
#[derive(Clone, Debug)]
pub(super) enum OriginalMemberValue {
    Written(TypeExpr),
    Expression(NativeExpressionKey),
    Function(NativeFunctionKey),
}
#[derive(Clone, Debug)]
pub(super) struct OriginalMemberInput {
    pub owner: NativeNamedKey,
    pub key: OriginalMemberKey,
    pub origin: OriginalMemberOrigin,
    pub value: OriginalMemberValue,
    pub span: Span,
    pub lexical_context: Option<NativeTypeContext>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}
