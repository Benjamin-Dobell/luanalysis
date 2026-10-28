//! Generic parameter defaults, `T = Default`, as TypeScript gives them: the type an omitted type
//! argument, or a type argument nothing infers, takes.
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Checked {
    diagnostics: Vec<String>,
    bindings: Vec<(String, String)>,
}

impl Checked {
    fn ty(&self, name: &str) -> &str {
        self.bindings.iter().find(|(binding, _)| binding == name).map(|(_, ty)| ty.as_str()).unwrap_or_else(|| panic!("{name}: {:#?}", self.bindings))
    }
}

/// `code text | message` for each diagnostic, and each binding's type.
fn check(source: &str) -> Checked {
    let root = std::env::temp_dir().join(format!("luanalysis generic defaults {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        describe_types: true,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    Checked {
        diagnostics: main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect(),
        bindings: main.report.bindings.iter().map(|binding| (binding.name.clone(), binding.ty.clone())).collect(),
    }
}

#[test]
fn an_omitted_type_argument_is_its_default() {
    let checked = check(r#"---@shape Box<T = string>
---@field value T

---@alias Pair<A, B = A> { first: A, second: B }

---@type Box
local plain = { value = "text" }
---@type Box<number>
local numeric = { value = 1 }
---@type Pair<string>
local pair = { first = "a", second = 2 }
---@type number
local wrong = plain.value
"#);
    assert_eq!(checked.ty("plain"), "Box<string>");
    assert_eq!(checked.ty("numeric"), "Box<number>");
    assert_eq!(checked.ty("pair"), "Pair<string, string>");
    assert_eq!(checked.diagnostics, [
        "TS2322 { first = \"a\", second = 2 } | Type '{ first: \"a\", second: 2 }' is not assignable to type 'Pair<string, string>'.",
        "TS2322 plain.value | Type 'string' is not assignable to type 'number'.",
    ]);
}

#[test]
fn a_type_argument_nothing_infers_is_its_default() {
    let checked = check(r#"---@generic T = number
---@param value? T
---@return T
local function make(value) return --[[---@type T]] value end

local inferred = make("word")
local defaulted = make()

---@type fun<T = boolean>(value?: T): T
local typed
local flag = typed()
"#);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    assert_eq!(checked.ty("inferred"), "string");
    assert_eq!(checked.ty("defaulted"), "number");
    assert_eq!(checked.ty("flag"), "boolean");
}

#[test]
fn defaults_follow_required_parameters_name_earlier_ones_and_satisfy_constraints() {
    let checked = check(r#"---@shape Later<T = U, U = string>

---@shape Constrained<T: number = string>

---@shape Earlier<T, U = T[]>

---@generic T = string, U
---@param u U
local function required(u) end
"#);
    assert_eq!(checked.diagnostics, [
        "TS2744 U | Type parameter defaults can only reference previously declared type parameters.",
        "TS2344 string | Type 'string' does not satisfy the constraint 'number'.",
        "TS2706 U | Required type parameters may not follow optional type parameters.",
    ]);
}
