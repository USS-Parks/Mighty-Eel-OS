//! rule-tester.
//!
//! Given a rules-module TOML and a scenarios TOML, evaluate every scenario
//! and print which rules fired with the winning action. Designed for
//! regression testing — output is structured so it can be diffed.
//!
//! Usage:
//!     rule-tester <rules.toml> <scenarios.toml>
//!
//! Scenario file format:
//!     [[scenarios]]
//!     name = "phi_request_from_adult"
//!     classification = "regulated"   # public|internal|sensitive|regulated|critical
//!     role = "adult"
//!     profile_id = "alice"
//!     estimated_tokens = 256
//!     entities = ["medical"]         # any of: medical | tribal | export_controlled
//!     upstream_flags = []
//!     expect_action = "deny"         # optional: allow|deny|reroute|flag
//!     expect_rule = "hipaa_phi..."   # optional: substring match on rule name

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Deserialize;

use mai_router::classifier::Classification;
use mai_router::entities::EntityKind;
use mai_router::router::RouteRequest;
use mai_router::rules::{self, FactSet, PolicyModuleRegistry, evaluate, resolve};

#[derive(Debug, Deserialize)]
struct Scenarios {
    #[serde(default)]
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default = "default_classification")]
    classification: String,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default = "default_profile_id")]
    profile_id: String,
    #[serde(default = "default_tokens")]
    estimated_tokens: u32,
    #[serde(default)]
    entities: Vec<String>,
    #[serde(default)]
    upstream_flags: Vec<String>,
    /// Optional expected winning action kind: allow|deny|reroute|flag.
    #[serde(default)]
    expect_action: Option<String>,
    /// Optional substring match for the winning rule name.
    #[serde(default)]
    expect_rule: Option<String>,
}

fn default_classification() -> String {
    "public".to_string()
}
fn default_role() -> String {
    "adult".to_string()
}
fn default_profile_id() -> String {
    "tester".to_string()
}
fn default_tokens() -> u32 {
    100
}

fn parse_classification(s: &str) -> Result<Classification, String> {
    match s {
        "public" => Ok(Classification::Public),
        "internal" => Ok(Classification::Internal),
        "sensitive" => Ok(Classification::Sensitive),
        "regulated" => Ok(Classification::Regulated),
        "critical" => Ok(Classification::Critical),
        other => Err(format!("unknown classification '{other}'")),
    }
}

fn parse_entities(items: &[String]) -> Result<Vec<EntityKind>, String> {
    items
        .iter()
        .map(|s| match s.as_str() {
            "medical" => Ok(EntityKind::Medical),
            "tribal" => Ok(EntityKind::Tribal),
            "export_controlled" => Ok(EntityKind::ExportControlled),
            other => Err(format!("unknown entity '{other}'")),
        })
        .collect()
}

fn action_kind(action: &rules::Action) -> &'static str {
    match action {
        rules::Action::Allow => "allow",
        rules::Action::Deny { .. } => "deny",
        rules::Action::Reroute { .. } => "reroute",
        rules::Action::Flag { .. } => "flag",
    }
}

fn run(rules_path: PathBuf, scenarios_path: PathBuf) -> Result<u32, String> {
    let registry = PolicyModuleRegistry::new();
    registry
        .load_from_path("module", &rules_path)
        .map_err(|e| format!("rules load failed: {e}"))?;

    let scenarios_raw = std::fs::read_to_string(&scenarios_path)
        .map_err(|e| format!("scenarios read failed: {e}"))?;
    let scenarios: Scenarios =
        toml::from_str(&scenarios_raw).map_err(|e| format!("scenarios parse failed: {e}"))?;

    if scenarios.scenarios.is_empty() {
        println!("(no scenarios)");
        return Ok(0);
    }

    let rules = registry.enabled_rules();
    let mut mismatches = 0u32;

    for scenario in &scenarios.scenarios {
        let classification = parse_classification(&scenario.classification)?;
        let entity_kinds = parse_entities(&scenario.entities)?;
        let request = RouteRequest {
            query: scenario.name.clone(), // not classified again; facts are pre-supplied
            estimated_tokens: scenario.estimated_tokens,
            profile_id: scenario.profile_id.clone(),
            role: scenario.role.clone(),
            upstream_flags: scenario.upstream_flags.clone(),
        };
        let facts = FactSet::from_request(&request, classification, &entity_kinds);
        let hits = evaluate(&rules, &facts).map_err(|e| format!("evaluation error: {e}"))?;

        let winner = resolve(&hits);
        let action_label = winner.map(|h| action_kind(&h.action)).unwrap_or("(none)");
        let winner_name = winner.map(|h| h.name.as_str()).unwrap_or("(no rule fired)");

        println!("scenario: {}", scenario.name);
        println!("  classification:  {}", scenario.classification);
        println!("  role:            {}", scenario.role);
        println!("  entities:        {:?}", scenario.entities);
        println!("  rules_fired:     {}", hits.len());
        for hit in &hits {
            println!(
                "    - {} (priority={}, action={})",
                hit.name,
                hit.priority,
                action_kind(&hit.action),
            );
        }
        println!("  winning_rule:    {winner_name}");
        println!("  winning_action:  {action_label}");

        let mut scenario_ok = true;
        if let Some(expected_action) = scenario.expect_action.as_deref()
            && expected_action != action_label
        {
            println!("  MISMATCH expect_action={expected_action} actual={action_label}",);
            scenario_ok = false;
        }
        if let Some(expected_rule) = scenario.expect_rule.as_deref()
            && !winner_name.contains(expected_rule)
        {
            println!("  MISMATCH expect_rule={expected_rule} actual={winner_name}",);
            scenario_ok = false;
        }
        if scenario_ok {
            println!("  status:          OK");
        } else {
            mismatches += 1;
            println!("  status:          FAIL");
        }
        println!();
    }

    if mismatches == 0 {
        println!("all {} scenarios matched", scenarios.scenarios.len());
    } else {
        println!(
            "{} of {} scenarios failed",
            mismatches,
            scenarios.scenarios.len()
        );
    }
    Ok(mismatches)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: rule-tester <rules.toml> <scenarios.toml>");
        return ExitCode::from(2);
    }
    let rules = PathBuf::from(&args[1]);
    let scenarios = PathBuf::from(&args[2]);
    match run(rules, scenarios) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(2)
        }
    }
}
