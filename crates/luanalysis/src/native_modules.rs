//! Original chunk return and require recipes for one native project session.
//! The backend owns export projection, pack rows, caching and query guards.
use super::NativeRegistry;
use crate::module_requests::{RequireCall, RequireTarget};
use crate::native_flow::FlowAdapter;
use crate::{Checker, Diagnostic};
use luanalysis_engine::{
    NativeLoaderPolicy, NativeModuleKey, NativeProjectError, Type, TypeQueries, ValuePack,
};
use luanalysis_syntax::{Expr, Span};
use luanalysis_project::FileId;
use std::cell::RefCell;
use crate::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) enum ImportOutcome {
    Module(NativeModuleKey),
    Missing(String),
    InvalidSource,
    Dynamic,
    NonUtf8,
    Arity(usize),
}
#[derive(Clone)]
pub(crate) struct ImportRecipe {
    pub argument_span: Span,
    pub target: Option<FileId>,
    pub outcome: ImportOutcome,
}
pub(crate) struct ProjectModuleIndex {
    pub policy: NativeLoaderPolicy,
    pub imports: HashMap<(u64, usize, usize), ImportRecipe>,
    /// Checking roots and original targets whose callee/loader were proven.
    /// This schedules source checks; native queries still own all inferred types.
    pub required_files: RefCell<HashSet<FileId>>,
}

impl NativeRegistry {
    pub(crate) fn check_module_requests(&self, queries: &mut dyn TypeQueries) {
        for call in self.module_calls() {
            if !self.module_import(call.span).is_some_and(|(_, recipe)| matches!(recipe.outcome, ImportOutcome::Missing(_))) { continue; }
            let key = self.expression_key_for_span(call.span);
            let expression = &self.expressions().get(key).expression;
            let luanalysis_syntax::ExprKind::Call { callee, args } = &expression.kind else { continue; };
            let mut checker = self.checker_for_expression(key, luanalysis_engine::NativeCheckMode::empty(), queries);
            checker.native_require_pack(callee, args, expression.span);
            if !checker.incomplete_flow { self.merge_report(checker.report); }
        }
    }
    pub(crate) fn has_module_index(&self) -> bool {
        self.project_modules.is_some()
    }
    #[cfg(test)]
    pub(crate) fn module_key(&self) -> NativeModuleKey {
        NativeModuleKey {
            snapshot: self.snapshot,
            index: 0,
        }
    }
    /// The source's `require` calls, found once per revision.
    pub(crate) fn module_calls(&self) -> &[RequireCall] {
        self.derived_module_calls()
    }
    pub(crate) fn module_import(&self, span: Span) -> Option<(NativeLoaderPolicy, ImportRecipe)> {
        let modules = self.project_modules.as_ref()?;
        modules
            .imports
            .get(&(self.snapshot, span.start, span.end))
            .cloned()
            .map(|recipe| (modules.policy, recipe))
    }
    pub(crate) fn module_result_pack(&self, queries: &mut dyn TypeQueries) -> ValuePack {
        let graph = self.flow.graph_for_region(0, None);
        let source = &self.flow.program.graphs[graph.region as usize];
        let adapter = FlowAdapter {
            sources: &self.flow,
            values: self,
        };
        let mut returned = Vec::new();
        for return_id in &source.returns {
            let statement = self.flow.program.node(*return_id);
            let recipe = self
                .packs()
                .list_at(statement.span)
                .expect("original chunk return owns its pack recipe");
            assert_eq!(
                recipe.owner, None,
                "chunk return does not borrow a function identity"
            );
            let at = self.flow.flow_key(statement.antecedents[0], graph.context);
            if !queries.flow_reachable(&adapter, at) {
                continue;
            }
            let crate::flow::NodeKind::Return { values } = &statement.kind else {
                unreachable!()
            };
            let expressions: Vec<_> = values
                .iter()
                .map(|value| {
                    let evaluation = self.flow.program.evaluation(*value);
                    self.flow
                        .program
                        .expression(evaluation.expression)
                        .expression
                        .clone()
                })
                .collect();
            let mut checker = self.checker_at_flow(at, queries);
            returned.push(checker.return_expression_pack(&expressions, statement.span, None));
            if !checker.incomplete_flow {
                self.merge_flow_report(at, checker.check_mode, checker.report, checker.engine);
            }
        }
        let fallthrough =
            queries.flow_reachable(&adapter, self.flow.flow_key(source.exit, graph.context));
        queries.aggregate_result_packs(&returned, fallthrough, None)
    }
    pub(crate) fn module_query_error(&self, error: NativeProjectError) {
        // Invalid/unsupported requests are reported at their actual importer
        // expression. Guard refusals also identify the original chunk owner.
        if matches!(
            error,
            NativeProjectError::CircularQuery(_)
                | NativeProjectError::TooDeep(_)
                | NativeProjectError::UnresolvedQuery(_)
        ) {
            self.report.borrow_mut().diagnostics.push(Diagnostic {
                code: "LUA2036",
                span: self.file.span,
                message: module_error_message(error).into(),
            });
        }
    }
}

impl Checker<'_> {
    /// Which standard operation, if any, `callee` is at this point of its flow.
    pub(crate) fn native_standard_operation(&mut self, callee: &Expr) -> Option<luanalysis_engine::NativeStandardOperation> {
        let at = self.flow_at?;
        let native = self.native?;
        let evaluation = native.flow.evaluation_for_expression(callee, at.graph)?;
        Some(self.engine.flow_standard_operation(
            &FlowAdapter { sources: &native.flow, values: native },
            native.flow.evaluation_value(evaluation.id),
            native.flow.flow_key(evaluation.input, at.graph.context),
        ))
    }

    pub(crate) fn native_require_pack(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<ValuePack> {
        let (policy, recipe) = self.native?.module_import(span)?;
        let has_original_contract = self.native?.global_binding("require").is_some();
        if !has_original_contract && self.native?.flow.require_needs_declaration() {
            // The host's standard library leaves `require` out: this is an ordinary call.
            return None;
        }
        if has_original_contract && !self.native?.flow.has_original_require_capability() {
            // An unselected declaration supplies its ordinary callable type;
            // its spelling cannot activate a source module or borrow exports.
            return None;
        }
        if has_original_contract {
            let operation = self.native_standard_operation(callee);
            if matches!(operation, Some(luanalysis_engine::NativeStandardOperation::Pending)) {
                self.incomplete_flow = true;
                return Some(ValuePack::single(Type::Unknown));
            }
            let mut trusted = if let Some(operation) = operation {
                matches!(operation, luanalysis_engine::NativeStandardOperation::Standard(site) if site.kind == luanalysis_engine::NativeIntrinsicKind::Require)
            } else {
                let value = self.expression(callee);
                self.engine.function_intrinsic(&value).is_some_and(|site| site.kind == luanalysis_engine::NativeIntrinsicKind::Require)
            };
            let mut direct = callee;
            while let luanalysis_syntax::ExprKind::Parenthesized(inner) = &direct.kind { direct = inner; }
            let direct = matches!(&direct.kind, luanalysis_syntax::ExprKind::Name(name) if name.text == "require" && self.bound.binding_at(name.span.start).is_none());
            if !trusted && direct && !self.bound.has_modified_module_environment()
                && let Some(at) = self.flow_at
            {
                let native = self.native?;
                if !self.engine.flow_reachable(&FlowAdapter { sources: &native.flow, values: native }, at)
                    && let Some(key) = native.global_binding("require")
                    && let Ok(original) = self.engine.global_binding_type(key)
                {
                    trusted = self.engine.function_intrinsic(&original).is_some_and(|site| site.kind == luanalysis_engine::NativeIntrinsicKind::Require);
                }
            }
            if !trusted {
                // A replaced `require` is an ordinary call to its replacement. Strict standard
                // effects refuse to assume what an untraced effect left there.
                if !direct || !self.engine.strict_runtime_effects() { return None; }
                self.error("LUA2036", callee.span, "Standard require cannot be assumed for this runtime value after an environment write or custom loader replacement.");
                return Some(ValuePack::single(Type::Unknown));
            }
        }
        if let Some(at) = self.flow_at {
            let native = self.native?;
            // A statement call enters here without expression() updating the
            // checker's ambient flow position. Use its original invocation
            // epoch, after all operands and before executing this require.
            let expression = &native.expressions().get(native.expression_key_for_span(span)).expression;
            let at = native.flow.evaluation_for_expression(expression, at.graph)
                .and_then(|evaluation| match native.flow.node(native.flow.flow_key(evaluation.completed, at.graph.context)) {
                    Some(luanalysis_engine::NativeFlowNode::Call { before, .. }) => Some(before),
                    _ => None,
                }).unwrap_or(at);
            let reference = native.flow.loader_reference(at.graph);
            let result = self.engine.flow_standard_loader_state(&crate::native_flow::FlowAdapter { sources: &native.flow, values: native }, at, reference);
            self.incomplete_flow |= result.incomplete;
            if result.incomplete { return Some(ValuePack::single(Type::Unknown)); }
            if result.ty != Type::BooleanLiteral(true) {
                self.error("LUA2036", callee.span, "Standard require cannot be assumed after a loader mutation on this control-flow path; custom loader effects require an explicit module contract.");
                return Some(ValuePack::single(Type::Unknown));
            }
        }
        for (index, argument) in args.iter().enumerate() {
            let expected = (index == 0).then_some(Type::String);
            let value = self.expression_with_expected(argument, expected.as_ref());
            if index == 0 {
                self.compatible(&value, &Type::String, argument.span);
            }
        }
        if !self.incomplete_flow {
            self.report.imports.push(recipe.argument_span);
            if let Some(target) = recipe.target {
                self.native?.project_modules.as_ref()?.required_files.borrow_mut().insert(target);
            }
        }
        let result:String=match recipe.outcome {
            ImportOutcome::Module(module)=>match self.engine.require_pack(module,policy) {
                Ok(pack)=>return Some(self.merged_class_export(pack)),
                Err(error)=>module_error_message(error).into(),
            },
            // As TypeScript types an unresolved import, the value is `any` once reported.
            ImportOutcome::Missing(message)=>{
                self.error("TS2307",recipe.argument_span,message);
                return Some(ValuePack::single(Type::Any));
            },
            ImportOutcome::InvalidSource=>"Required module has syntax errors or is not a valid source in this project.".into(),
            // As TypeScript types `require(expression)`, a module named at runtime is `any`.
            ImportOutcome::Dynamic=>return Some(ValuePack::single(Type::Any)),
            ImportOutcome::NonUtf8=>"This project host requires UTF-8 module names.".into(),
            ImportOutcome::Arity(actual)=>{
                self.error("TS2554",recipe.argument_span,format!("Expected 1 argument, but got {actual}."));
                return Some(ValuePack::single(Type::Unknown));
            },
        };
        self.error("LUA2036", recipe.argument_span, result);
        Some(ValuePack::single(Type::Unknown))
    }
}

impl Checker<'_> {
    /// The definition table of a class declared in several places is, to its importers,
    /// the class, with the members every one of its definitions gives it (as LuaLS reads
    /// `require`). Other definitions keep their partial view, so an importer still sees
    /// which declared members a module assigns.
    fn merged_class_export(&mut self, pack: ValuePack) -> ValuePack {
        let Some(native) = self.native else { return pack };
        let value = self.engine.pack_slot(&pack, 0);
        let Some(declaration) = self.engine.named_object_declaration(&value) else { return pack };
        if !native.is_merged_declaration(declaration) { return pack }
        match self.engine.named_declared_type(declaration) {
            Ok(class) if class != value => ValuePack::single(class),
            _ => pack,
        }
    }
}

pub(crate) fn classify_unresolved(call: &RequireCall) -> Option<ImportOutcome> {
    match call.target {
        RequireTarget::Static(_) => None,
        RequireTarget::Dynamic => Some(ImportOutcome::Dynamic),
        RequireTarget::NonUtf8 => Some(ImportOutcome::NonUtf8),
        RequireTarget::Arity(actual) => Some(ImportOutcome::Arity(actual)),
    }
}

fn module_error_message(error: NativeProjectError) -> &'static str {
    match error {
        NativeProjectError::InvalidModule(_) => {
            "Required module is unavailable in this project snapshot."
        }
        NativeProjectError::MissingDeclarationContract(_) => {
            "Declaration modules require an explicit export contract."
        }
        NativeProjectError::UnsupportedLoader { .. } => {
            "Module exports require an unchanged standard Lua file loader."
        }
        NativeProjectError::CircularQuery(_) => {
            "Cannot infer module exports from a circular require."
        }
        NativeProjectError::TooDeep(_) => {
            "Module export inference exceeded the native query depth or stack limit."
        }
        NativeProjectError::UnresolvedQuery(_) => {
            "Module export inference depends on an unresolved query."
        }
    }
}
