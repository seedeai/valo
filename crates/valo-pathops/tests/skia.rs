//! Checks the port against Skia: every case in tests/fixtures, recorded from Skia's own PathOps
//! tests and from Flutter's `Path.combine` calls by tests/reference/build.sh, must give Skia's
//! result bit for bit: the same success or failure, then the same fill type, verbs, points and
//! conic weights.
//!
//! Each fixture file is one test. `PATHOPS_CASE=<name> cargo test -p valo-pathops --test skia`
//! runs only the cases of that name and prints their paths in full.

#[path = "skia/fixture.rs"]
mod fixture;

use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use fixture::{Case, Kind, RawPath};

/// Cases the port is known to answer differently from Skia, as (fixture file, case name), each
/// with a comment saying why. A difference not listed here fails its fixture's test.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

/// How many differing cases a fixture's summary names.
const NAMED_DIFFERENCES: usize = 10;

macro_rules! fixture_tests {
    ($($test:ident: $file:literal,)*) => {
        $(
            #[test]
            fn $test() {
                check_fixture($file);
            }
        )*
    };
}

fixture_tests! {
    path_ops_op_test: "PathOpsOpTest",
    path_ops_battles: "PathOpsBattles",
    path_ops_fuzz763_test: "PathOpsFuzz763Test",
    path_ops_issue3651: "PathOpsIssue3651",
    path_ops_skp_test: "PathOpsSkpTest",
    path_ops_inverse_test: "PathOpsInverseTest",
    path_ops_simplify_test: "PathOpsSimplifyTest",
    path_ops_simplify_fail_test: "PathOpsSimplifyFailTest",
    path_ops_tiger_test: "PathOpsTigerTest",
    path_ops_chalkboard_test: "PathOpsChalkboardTest",
    path_ops_builder_conic_test: "PathOpsBuilderConicTest",
    flutter: "flutter",
}

/// `Outcome` is how the port's answer to a case compares with Skia's.
enum Outcome {
    Matched,
    Mismatched,
    FailedWhereSkiaSucceeded,
    SucceededWhereSkiaFailed,
    Panicked(String),
}

/// `PortAnswer` is what the port returned for a case, or the panic it raised.
enum PortAnswer {
    Path(valo_pathops::SkPath),
    Failed,
    Panicked(String),
}

fn check_fixture(file: &str) {
    let cases = read_fixture(file);
    match std::env::var("PATHOPS_CASE") {
        Ok(name) => check_named_case(file, &cases, &name),
        Err(_) => check_all_cases(file, &cases),
    }
}

fn read_fixture(file: &str) -> Vec<Case> {
    let path = format!("{}/tests/fixtures/{file}.txt", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    fixture::parse(&text).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn check_all_cases(file: &str, cases: &[Case]) {
    let outcomes: Vec<(&Case, Outcome)> = cases
        .iter()
        .map(|case| (case, outcome(case, &run_port(case))))
        .collect();
    print_summary(file, &outcomes);
    let unexpected: Vec<&str> = outcomes
        .iter()
        .filter(|(case, outcome)| {
            !matches!(outcome, Outcome::Matched) && !is_known_difference(file, &case.name)
        })
        .map(|(case, _)| case.name.as_str())
        .collect();
    assert!(
        unexpected.is_empty(),
        "{file}: {} of {} cases differ from Skia",
        unexpected.len(),
        cases.len()
    );
}

fn check_named_case(file: &str, cases: &[Case], name: &str) {
    for case in cases.iter().filter(|case| case.name == name) {
        let answer = run_port(case);
        print_case(file, case, &answer);
        assert!(
            matches!(outcome(case, &answer), Outcome::Matched),
            "{file}: {name} differs from Skia"
        );
    }
}

fn is_known_difference(file: &str, name: &str) -> bool {
    KNOWN_DIFFERENCES
        .iter()
        .any(|&(known_file, known_name)| known_file == file && known_name == name)
}

fn run_port(case: &Case) -> PortAnswer {
    let inputs: Vec<valo_pathops::SkPath> = case.inputs.iter().map(RawPath::build).collect();
    let answer = catch_panic(|| match case.kind {
        Kind::Op(op) => valo_pathops::op(&inputs[0], &inputs[1], op),
        Kind::Simplify => valo_pathops::simplify(&inputs[0]),
    });
    match answer {
        Ok(Ok(path)) => PortAnswer::Path(path),
        Ok(Err(_)) => PortAnswer::Failed,
        Err(message) => PortAnswer::Panicked(message),
    }
}

fn outcome(case: &Case, answer: &PortAnswer) -> Outcome {
    match (answer, &case.result) {
        (PortAnswer::Panicked(message), _) => Outcome::Panicked(message.clone()),
        (PortAnswer::Path(path), Some(expected)) if expected.matches(path) => Outcome::Matched,
        (PortAnswer::Path(_), Some(_)) => Outcome::Mismatched,
        (PortAnswer::Path(_), None) => Outcome::SucceededWhereSkiaFailed,
        (PortAnswer::Failed, Some(_)) => Outcome::FailedWhereSkiaSucceeded,
        (PortAnswer::Failed, None) => Outcome::Matched,
    }
}

fn print_summary(file: &str, outcomes: &[(&Case, Outcome)]) {
    let count = |wanted: fn(&Outcome) -> bool| {
        outcomes
            .iter()
            .filter(|(_, outcome)| wanted(outcome))
            .count()
    };
    println!(
        "{file}: {} cases, {} matched, {} mismatched, {} failed where Skia succeeded, {} succeeded where Skia failed, {} panicked",
        outcomes.len(),
        count(|outcome| matches!(outcome, Outcome::Matched)),
        count(|outcome| matches!(outcome, Outcome::Mismatched)),
        count(|outcome| matches!(outcome, Outcome::FailedWhereSkiaSucceeded)),
        count(|outcome| matches!(outcome, Outcome::SucceededWhereSkiaFailed)),
        count(|outcome| matches!(outcome, Outcome::Panicked(_))),
    );
    let differing = outcomes
        .iter()
        .filter(|(_, outcome)| !matches!(outcome, Outcome::Matched));
    for (case, outcome) in differing.take(NAMED_DIFFERENCES) {
        let known = if is_known_difference(file, &case.name) {
            " (known)"
        } else {
            ""
        };
        println!("  {}{known}: {}", case.name, describe_outcome(outcome));
    }
}

fn describe_outcome(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Matched => "matched".to_string(),
        Outcome::Mismatched => "a different path".to_string(),
        Outcome::FailedWhereSkiaSucceeded => "failed where Skia succeeded".to_string(),
        Outcome::SucceededWhereSkiaFailed => "succeeded where Skia failed".to_string(),
        Outcome::Panicked(message) => format!("panicked: {}", message.lines().last().unwrap_or("")),
    }
}

fn print_case(file: &str, case: &Case, answer: &PortAnswer) {
    let kind = match case.kind {
        Kind::Op(op) => format!("op {op:?}"),
        Kind::Simplify => "simplify".to_string(),
    };
    println!(
        "{file}: {} ({kind}, helper {}, expect {}, skia runs it: {})",
        case.name, case.helper, case.expect, case.skia_runs
    );
    for (index, input) in case.inputs.iter().enumerate() {
        println!("input {}: {}", index + 1, input.describe());
    }
    match &case.result {
        Some(result) => println!("Skia: {}", result.describe()),
        None => println!("Skia: failed\n"),
    }
    match answer {
        PortAnswer::Path(path) => println!("port: {}", RawPath::of(path).describe()),
        PortAnswer::Failed => println!("port: failed\n"),
        PortAnswer::Panicked(message) => println!("port: panicked at {message}\n"),
    }
    println!("{}", describe_outcome(&outcome(case, answer)));
}

thread_local! {
    static CATCHING_PANIC: Cell<bool> = const { Cell::new(false) };
    static CAUGHT_PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

/// `catch_panic` runs `body`, turning a panic into its message, which the panic hook keeps
/// instead of printing; panics outside it print as usual.
fn catch_panic<T>(body: impl FnOnce() -> T) -> Result<T, String> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if CATCHING_PANIC.get() {
                CAUGHT_PANIC.with_borrow_mut(|message| *message = info.to_string());
            } else {
                default_hook(info);
            }
        }));
    });
    CATCHING_PANIC.set(true);
    let result = panic::catch_unwind(AssertUnwindSafe(body));
    CATCHING_PANIC.set(false);
    result.map_err(|_| CAUGHT_PANIC.take())
}
