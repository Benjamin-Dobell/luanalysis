use luanalysis::Diagnostic;
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::{StandardEnvironmentProfile, StandardLibraryAllowList};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "luanalysis language versions {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.lua"), source).unwrap();
        Self(root)
    }

    fn project(&self) -> Project {
        Project::load_plan(
            &Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap(),
            ProjectLoadPlan::new([self.0.join("main.lua")]),
            static_require_requests,
        )
    }

    fn check(&self, profile: StandardEnvironmentProfile) -> ProjectCheckReport {
        check_project_with_options(self.project(), options(profile))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn options(profile: StandardEnvironmentProfile) -> ProjectCheckOptions {
    ProjectCheckOptions {
        environment: profile,
        standard_library: Some(StandardLibraryAllowList::default()),
        jobs: 1,
        ..Default::default()
    }
}

fn diagnostics(report: &ProjectCheckReport) -> &[Diagnostic] {
    &report
        .file(report.project.entries[0])
        .unwrap()
        .report
        .diagnostics
}

fn tokens<'a>(source: &'a str, report: &ProjectCheckReport) -> Vec<&'a str> {
    diagnostics(report)
        .iter()
        .map(|diagnostic| &source[diagnostic.span.start..diagnostic.span.end])
        .collect()
}

const LUA53_OPERATORS: &str = "local a = 1\na = a & a\na = a | a\na = a ~ a\na = a >> a\na = a << a\na = ~a\na = a // 2\nreturn a\n";

#[test]
fn selected_older_lua_versions_reject_bitwise_and_floor_division_at_the_operator() {
    let fixture = Fixture::new(LUA53_OPERATORS);
    for profile in [
        StandardEnvironmentProfile::Lua50,
        StandardEnvironmentProfile::Lua51,
        StandardEnvironmentProfile::Lua52,
    ] {
        let report = fixture.check(profile);
        assert_eq!(
            tokens(LUA53_OPERATORS, &report),
            ["&", "|", "~", ">>", "<<", "~", "//"],
            "{profile:?}"
        );
        assert!(diagnostics(&report).iter().all(|diagnostic| {
            diagnostic.code == "LUA1001" && diagnostic.message.contains("Lua 5.3 and above")
        }));
        assert_eq!(
            diagnostics(&report)[5].message,
            "The unary operator '~' only available in Lua 5.3 and above"
        );
        assert!(report.has_errors());
    }
}

#[test]
fn lua53_and_lua54_allow_the_original_bitwise_and_floor_division_operations() {
    let fixture = Fixture::new(LUA53_OPERATORS);
    for profile in [
        StandardEnvironmentProfile::Lua53,
        StandardEnvironmentProfile::Lua54,
    ] {
        let report = fixture.check(profile);
        assert!(
            diagnostics(&report).is_empty(),
            "{profile:?}: {:?}",
            diagnostics(&report)
        );
    }
}

#[test]
fn selected_older_lua_versions_reject_local_attributes_including_their_delimiters() {
    let source = "local constant <const> = 1\nlocal resource < --[[ attribute > < ]] close > = nil\nreturn constant, resource\n";
    let fixture = Fixture::new(source);
    for profile in [
        StandardEnvironmentProfile::Lua50,
        StandardEnvironmentProfile::Lua51,
        StandardEnvironmentProfile::Lua52,
        StandardEnvironmentProfile::Lua53,
    ] {
        let report = fixture.check(profile);
        assert_eq!(
            tokens(source, &report),
            ["<const>", "< --[[ attribute > < ]] close >"],
            "{profile:?}"
        );
        assert!(diagnostics(&report).iter().all(|diagnostic| {
            diagnostic.code == "LUA1001" && diagnostic.message.contains("Lua 5.4 and above")
        }));
    }
    let report = fixture.check(StandardEnvironmentProfile::Lua54);
    assert!(
        diagnostics(&report).is_empty(),
        "{:?}",
        diagnostics(&report)
    );
}

#[test]
fn version_validation_visits_nested_functions_statement_bodies_and_table_keys() {
    let source = "local callback = function()\n local a <const> = 1\n return ~a\nend\nlocal records = { [1 << 1] = function() local resource <close> = nil; return 7 // 2 end }\ndo\n local a = 1 | 2\n if (1 & 2) == 0 then local b = 1 >> 1 else local b = 1 ~ 2 end\n while (1 // 2) > 0 do break end\n repeat local c = 1 & 1 until (1 | 2) > 0\n for i = 1 // 1, 2 << 1, 1 | 0 do local d <const> = i end\n for value in function() return 1 ~ 1 end do break end\nend\nreturn records, callback\n";
    let report = Fixture::new(source).check(StandardEnvironmentProfile::Lua52);
    assert_eq!(
        tokens(source, &report),
        [
            "<const>", "~", "<<", "<close>", "//", "|", "&", ">>", "~", "//", "&", "|", "//", "<<",
            "|", "<const>", "~",
        ]
    );
}

#[test]
fn comments_strings_and_annotation_type_operators_are_not_runtime_features() {
    let source = "---@type string|nil\nlocal value = '& | ~ >> << // <const> <close>'\n-- 1 & 2; local x <const> = 1\n--[[ 1 // 2; local x <close> = nil ]]\nreturn value\n";
    let report = Fixture::new(source).check(StandardEnvironmentProfile::Lua52);
    assert!(
        diagnostics(&report).is_empty(),
        "{:?}",
        diagnostics(&report)
    );
}

#[test]
fn unversioned_checks_preserve_the_existing_newest_grammar() {
    let source = "local constant <const> = 1\nlocal resource <close> = nil\nreturn constant // 1, ~constant, resource\n";
    let fixture = Fixture::new(source);
    for profile in [
        StandardEnvironmentProfile::LegacyMinimal,
        StandardEnvironmentProfile::None,
    ] {
        let report = fixture.check(profile);
        assert!(
            diagnostics(&report).is_empty(),
            "{profile:?}: {:?}",
            diagnostics(&report)
        );
    }
    assert!(luanalysis::check(source).diagnostics.is_empty());
}

#[test]
fn version_diagnostics_survive_parser_recovery_and_suppress_semantic_noise() {
    let source = "local value = 'wrong operand' // 2\nlocal broken = )\n";
    let report = Fixture::new(source).check(StandardEnvironmentProfile::Lua52);
    let diagnostics = diagnostics(&report);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Lua 5.3 and above"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| !diagnostic.message.contains("Lua 5.3 and above"))
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "LUA1001"),
        "{diagnostics:?}"
    );
}

#[test]
fn changing_the_selected_version_revalidates_reused_syntax_without_a_standard_library() {
    let source = "local constant <const> = 1\nreturn constant\n";
    let fixture = Fixture::new(source);
    let mut workspace = ProjectWorkspace::new();
    let older = workspace.check_project(
        fixture.project(),
        options(StandardEnvironmentProfile::Lua53),
    );
    assert_eq!(tokens(source, &older), ["<const>"]);
    let newer = workspace.check_project(
        fixture.project(),
        options(StandardEnvironmentProfile::Lua54),
    );
    assert!(diagnostics(&newer).is_empty(), "{:?}", diagnostics(&newer));
    // The selected versions have different bundled declaration sources; the
    // unchanged application syntax is reused while those sources are replaced.
    assert!(workspace.last_preparation_stats().parse_reused >= 1);
    let older_again = workspace.check_project(
        fixture.project(),
        options(StandardEnvironmentProfile::Lua53),
    );
    assert_eq!(tokens(source, &older_again), ["<const>"]);
    assert!(workspace.last_preparation_stats().parse_reused >= 1);
}
