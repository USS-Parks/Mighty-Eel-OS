// A CLI legitimately writes to stdout/stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]
//! `aogctl` binary — a formatting shell over [`aogctl::Client`] (kernel subset).
//!
//! Usage:
//!   aogctl apply -f <file>          create-or-update a resource from a JSON file
//!   aogctl get <Kind> [name]        fetch one, or list a kind
//!   aogctl describe <Kind> <name>   fetch one as pretty JSON
//!   aogctl delete <Kind> <name>     remove a resource
//!
//! Server + token come from `AOGCTL_SERVER` (default `http://127.0.0.1:8080`) and
//! `AOGCTL_TOKEN`. `--output json` selects JSON; the default is a compact table.

use std::process::ExitCode;

use aogctl::Client;
use serde_json::Value;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aogctl: {e}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let server =
        std::env::var("AOGCTL_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned());
    let token = std::env::var("AOGCTL_TOKEN").unwrap_or_default();
    let json_output = flag(&args, "--output")
        .or_else(|| flag(&args, "-o"))
        .as_deref()
        == Some("json");
    let positional = positional(&args);
    let client = Client::new(server, token);

    let cmd = positional.first().map(String::as_str).ok_or_else(usage)?;
    match cmd {
        "apply" => {
            let file = flag(&args, "-f")
                .or_else(|| flag(&args, "--file"))
                .ok_or("apply requires -f <file>")?;
            let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            let body: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let kind = body
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("the resource body has no `kind`")?;
            let out = client.apply(kind, &body).await.map_err(|e| e.to_string())?;
            emit(&out, json_output);
        }
        "get" => {
            let kind = positional.get(1).ok_or("get <Kind> [name]")?;
            let out = match positional.get(2) {
                Some(name) => client.get(kind, name).await,
                None => client.list(kind).await,
            }
            .map_err(|e| e.to_string())?;
            emit(&out, json_output);
        }
        "describe" => {
            let kind = positional.get(1).ok_or("describe <Kind> <name>")?;
            let name = positional.get(2).ok_or("describe <Kind> <name>")?;
            let out = client.get(kind, name).await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        }
        "delete" => {
            let kind = positional.get(1).ok_or("delete <Kind> <name>")?;
            let name = positional.get(2).ok_or("delete <Kind> <name>")?;
            client.delete(kind, name).await.map_err(|e| e.to_string())?;
            println!("{kind}/{name} deleted");
        }
        _ => return Err(usage()),
    }
    Ok(())
}

/// Positional (non-flag) arguments, with `-f/--file/-o/--output` values removed.
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if matches!(a.as_str(), "-f" | "--file" | "-o" | "--output") {
            it.next(); // consume the flag's value
        } else if !a.starts_with('-') {
            out.push(a.clone());
        }
    }
    out
}

/// The value following `name` (`--name value` or `--name=value`).
fn flag(args: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&prefix) {
            return Some(rest.to_owned());
        }
    }
    None
}

/// Print `value` as pretty JSON, or a compact `KIND NAME REV` table.
fn emit(value: &Value, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
        return;
    }
    println!("{:<18} {:<24} {:<8}", "KIND", "NAME", "REV");
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        for item in items {
            print_row(item);
        }
    } else if !value.is_null() {
        print_row(value);
    }
}

fn print_row(item: &Value) {
    let kind = item.get("kind").and_then(Value::as_str).unwrap_or("-");
    let meta = item.get("metadata");
    let name = meta
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let rev = meta
        .and_then(|m| m.get("resource_version"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("{kind:<18} {name:<24} {rev:<8}");
}

fn usage() -> String {
    "usage: aogctl <apply -f FILE | get KIND [NAME] | describe KIND NAME | delete KIND NAME> [--output json]"
        .to_owned()
}
