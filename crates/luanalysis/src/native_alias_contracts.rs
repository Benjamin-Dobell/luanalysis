//! Original written bounds for native alias disjointness. No initializer,
//! actual capture value, narrowed binding, or inferred constructor is queried.
use super::{AnnotationPack, BindingId, DeclarationSource, NativeRegistry, Type, TypeQueries};

impl NativeRegistry {
    pub(super) fn original_written_binding_contract(
        &self,
        binding: BindingId,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.declarations.get(&binding)?;
        let mut checker = self.checker(queries);
        match source {
            DeclarationSource::Local { annotation, initializer_assertion, owner, .. } => {
                checker.type_context = owner.map(|owner| self.key(owner));
                if let Some(annotation) = annotation {
                    return Some(checker.annotation(annotation));
                }
                let (annotation, slot) = initializer_assertion.as_ref()?;
                let pack = AnnotationPack::annotation(annotation).lower(&mut |ty| checker.annotation(ty));
                Some(checker.engine.pack_slot(&pack, *slot))
            }
            DeclarationSource::Parameter { function, index } => {
                let owner = self.key(*function);
                checker.type_context = Some(owner);
                let function = self.function(owner);
                let offset = usize::from(self.bound.implicit_receiver(function.span).is_some());
                let name = if *index < offset {
                    if function.params.iter().any(|parameter| parameter.text == "self") { return None }
                    "self"
                } else {
                    function.params.get(*index - offset)?.text.as_str()
                };
                if let Some(tag) = function.annotations.params.iter().find(|tag| tag.name.text == name) {
                    let ty = checker.annotation(&tag.ty);
                    return Some(if tag.optional { Type::Union(vec![ty, Type::Nil]) } else { ty });
                }
                let annotation = function.annotations.type_annotation.as_ref()?.types.first()?;
                let ty = checker.annotation(annotation);
                let signature = checker.engine.function_signature(&ty)?;
                let parameter = signature.params.get(*index)?;
                Some(if parameter.optional { Type::Union(vec![parameter.ty.clone(), Type::Nil]) } else { parameter.ty.clone() })
            }
            DeclarationSource::Function(_) | DeclarationSource::NumericLoop | DeclarationSource::GenericLoop => None,
        }
    }
}
