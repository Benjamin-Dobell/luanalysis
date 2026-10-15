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
use std::collections::{HashMap, HashSet};

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
    pub(crate) fn module_key(&self) -> NativeModuleKey {
        NativeModuleKey {
            snapshot: self.snapshot,
            index: 0,
        }
    }
    pub(crate) fn module_calls(&self) -> Vec<RequireCall> {
        crate::module_requests::require_calls(&self.file, &self.bound)
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
                self.merge_flow_report(at, checker.check_mode, checker.report);
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
    pub(crate) fn native_require_pack(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<ValuePack> {
        let (policy, recipe) = self.native?.module_import(span)?;
        let has_original_contract = self.native?.global_binding("require").is_some();
        if has_original_contract {
            let value = self.expression(callee);
            let mut trusted = self.engine.function_intrinsic(&value)
                .is_some_and(|site| site.kind == luanalysis_engine::NativeIntrinsicKind::Require);
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
                if !direct { return None; }
                self.error("LUA2036", callee.span, "Standard require cannot be assumed for this runtime value after an environment write or custom loader replacement.");
                return Some(ValuePack::single(Type::Unknown));
            }
        }
        if let Some(at) = self.flow_at {
            let native = self.native?;
            let reference = native.flow.loader_reference(at.graph);
            let result = self.engine.flow_type(&crate::native_flow::FlowAdapter { sources: &native.flow, values: native }, at, reference, &Type::BooleanLiteral(false), &Type::BooleanLiteral(true));
            self.incomplete_flow |= result.incomplete;
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
        if !self.incomplete_flow && let Some(target) = recipe.target {
            self.native?.project_modules.as_ref()?.required_files.borrow_mut().insert(target);
        }
        let result:String=match recipe.outcome {
            ImportOutcome::Module(module)=>match self.engine.require_pack(module,policy) {
                Ok(pack)=>return Some(pack),
                Err(error)=>module_error_message(error).into(),
            },
            ImportOutcome::Missing(message)=>{
                self.error("TS2307",recipe.argument_span,message);
                return Some(ValuePack::single(Type::Unknown));
            },
            ImportOutcome::InvalidSource=>"Required module has syntax errors or is not a valid source in this project.".into(),
            ImportOutcome::Dynamic=>"Dynamic require needs an explicit external-module or loader contract.".into(),
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
