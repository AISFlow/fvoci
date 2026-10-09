//! Read-only replacement for the manual exact-SHA check/job/timeout table.
//!
//! JSON projection and pagination belong to `gh`, not a second JSON parser.
//! Source projection deliberately accepts only the selection registry and CI
//! metadata forms used here. Unsupported YAML/expressions produce MISSING; this
//! is not a general YAML parser or a replacement for the selection planner.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REPO: &str = "repos/AISFlow/fvoci";
const RUN_QUERY: &str = r#"("TOTAL\t" + (.total_count|tostring)), (.workflow_runs[] | [.id,.path,.head_sha,.run_attempt,.status,(.conclusion // ""),.check_suite_id,(.created_at|fromdateiso8601),.event] | @tsv)"#;
const CHECK_QUERY: &str = r#"("TOTAL\t" + (.total_count|tostring)), (.check_runs[] | [.id,.name,.head_sha,.app.id,.app.slug,.check_suite.id,.status,(.conclusion // "")] | @tsv)"#;
const JOB_QUERY: &str = r#"("TOTAL\t" + (.total_count|tostring)), (.jobs[] | [.id,.run_id,.run_attempt,.head_sha,.name,.status,(.conclusion // ""),(.started_at|if . == null then "" else fromdateiso8601 end),(.completed_at|if . == null then "" else fromdateiso8601 end),(.check_run_url|split("/")|last)] | @tsv)"#;

pub fn validate_sha(sha: &str) -> Result<(), String> {
    if sha.len() == 40
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("expected exactly 40 lowercase hexadecimal SHA characters".into())
    }
}

fn command(cwd: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|_| format!("{program} could not start"))?;
    if !output.status.success() {
        // Child stderr can contain credentials, host paths or account names.
        return Err(format!(
            "{program} failed (exit {:?})",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} returned non-UTF-8 data"))
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    command(cwd, "git", args)
}

fn api(cwd: &Path, endpoint: &str, query: &str, paginate: bool) -> Result<String, String> {
    let mut args = vec![
        "api",
        endpoint,
        "--method",
        "GET",
        "--header",
        "Accept: application/vnd.github+json",
        "--header",
        "X-GitHub-Api-Version: 2022-11-28",
        "--jq",
        query,
    ];
    if paginate {
        args.push("--paginate");
    }
    command(cwd, "gh", &args)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn quoted(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() < 2 && value.starts_with(['\'', '"']) {
        return Err("unsupported source scalar".into());
    }
    let inner = if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    };
    if inner.is_empty() || inner.contains(['\'', '"', '\\', '&', '*', '#', '\t']) {
        return Err("unsupported source scalar".into());
    }
    Ok(inner.to_string())
}

fn list(value: &str) -> Result<Vec<String>, String> {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or("unsupported source list")?;
    let value = value.trim().strip_suffix(',').unwrap_or(value);
    let values: Vec<_> = value.split(',').map(quoted).collect::<Result<_, _>>()?;
    if values.is_empty() || values.len() != values.iter().collect::<BTreeSet<_>>().len() {
        return Err("empty or duplicate source list".into());
    }
    Ok(values)
}

fn block<'a>(source: &'a str, prefix: &str) -> Result<Vec<&'a str>, String> {
    let lines: Vec<_> = source.lines().collect();
    let starts: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with(prefix))
        .map(|(i, _)| i)
        .collect();
    if starts.len() != 1 || !lines[starts[0]].ends_with(" = {") {
        return Err("ambiguous selection registry".into());
    }
    let tail = &lines[starts[0] + 1..];
    let end = tail
        .iter()
        .position(|l| *l == "}")
        .ok_or("unclosed selection registry")?;
    Ok(tail[..end].to_vec())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    id: String,
    name: String,
    timeout: Option<u64>,
    conditional: bool,
    problem: Option<String>,
}

#[derive(Debug, Clone)]
struct Workflow {
    path: String,
    gate: String,
    specs: Vec<Spec>,
}

// Read only job metadata at its precise indentation. Step bodies never supply
// job names, needs or timeouts. Unknown metadata is left unresolved, not guessed.
fn field(lines: &[&str], key: &str) -> Result<Option<String>, String> {
    let prefix = format!("    {key}: ");
    let values: Vec<_> = lines
        .iter()
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(str::to_string)
        .collect();
    if values.len() > 1 || lines.contains(&format!("    {key}:").as_str()) {
        return Err(format!("unsupported or duplicate {key}"));
    }
    Ok(values.into_iter().next())
}

fn needs(lines: &[&str]) -> Result<Option<Vec<String>>, String> {
    let starts: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("    needs:"))
        .map(|(index, _)| index)
        .collect();
    let Some(&start) = starts.first() else {
        return Ok(None);
    };
    if starts.len() != 1 {
        return Err("duplicate needs".into());
    }
    let inline = lines[start].strip_prefix("    needs:").unwrap();
    if !inline.is_empty() && !inline.starts_with(' ') {
        return Err("unsupported needs".into());
    }
    let tail = &lines[start + 1..];
    let end = tail
        .iter()
        .position(|line| line.starts_with("    ") && !line.starts_with("     "))
        .unwrap_or(tail.len());
    let continuation: Vec<_> = tail[..end]
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .collect();
    let values = if inline.trim().is_empty()
        && continuation.iter().all(|line| line.starts_with("      - "))
    {
        continuation
            .iter()
            .map(|line| quoted(line.strip_prefix("      - ").unwrap()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        if continuation.iter().any(|line| !line.starts_with("      ")) {
            return Err("unsupported needs indentation".into());
        }
        let joined = std::iter::once(inline.trim())
            .chain(continuation.iter().map(|line| line.trim()))
            .collect::<Vec<_>>()
            .join(" ");
        let joined = joined.trim();
        if joined.starts_with('[') {
            list(joined)?
        } else if continuation.is_empty() {
            vec![quoted(joined)?]
        } else {
            return Err("unsupported needs list".into());
        }
    };
    if values.is_empty()
        || values.iter().any(|value| !identifier(value))
        || values.len() != values.iter().collect::<BTreeSet<_>>().len()
    {
        return Err("empty, duplicate or unsupported needs".into());
    }
    Ok(Some(values))
}

fn matrices(lines: &[&str]) -> Result<Vec<BTreeMap<String, String>>, String> {
    let strategy = lines.iter().position(|line| *line == "    strategy:");
    let Some(start) = strategy else {
        return Ok(vec![BTreeMap::new()]);
    };
    let tail = &lines[start + 1..];
    let end = tail
        .iter()
        .position(|l| l.starts_with("    ") && !l.starts_with("     "))
        .unwrap_or(tail.len());
    let strategy = &tail[..end];
    if strategy.iter().filter(|l| **l == "      matrix:").count() != 1 {
        return Err("unsupported matrix source".into());
    }
    let start = strategy.iter().position(|l| *l == "      matrix:").unwrap();
    let matrix: Vec<_> = strategy[start + 1..]
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .collect();
    if matrix.first() == Some(&"        include:") {
        let mut result = Vec::new();
        let mut current = BTreeMap::new();
        for line in &matrix[1..] {
            let (entry, new) = if let Some(v) = line.strip_prefix("          - ") {
                (v, true)
            } else if let Some(v) = line.strip_prefix("            ") {
                (v, false)
            } else {
                return Err("unsupported matrix include".into());
            };
            if new && !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            if !new && current.is_empty() {
                return Err("matrix entry has no start".into());
            }
            let (key, value) = entry.split_once(": ").ok_or("unsupported matrix entry")?;
            if !identifier(key) || current.insert(key.into(), quoted(value)?).is_some() {
                return Err("duplicate or unsupported matrix key".into());
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
        if result.is_empty() {
            return Err("empty matrix include".into());
        }
        Ok(result)
    } else {
        let mut result = vec![BTreeMap::new()];
        for line in matrix {
            let (key, values) = line
                .strip_prefix("        ")
                .and_then(|l| l.split_once(": "))
                .ok_or("unsupported matrix axis")?;
            if !identifier(key) || result[0].contains_key(key) {
                return Err("ambiguous matrix axis".into());
            }
            let values = list(values)?;
            let mut expanded = Vec::new();
            for entry in &result {
                for value in &values {
                    let mut entry = entry.clone();
                    entry.insert(key.into(), value.clone());
                    expanded.push(entry);
                    if expanded.len() > 256 {
                        return Err("matrix expansion exceeds 256".into());
                    }
                }
            }
            result = expanded;
        }
        if result[0].is_empty() {
            return Err("empty matrix".into());
        }
        Ok(result)
    }
}

fn name(template: &str, matrix: &BTreeMap<String, String>) -> Result<String, String> {
    let mut value = template.to_string();
    for (key, replacement) in matrix {
        value = value.replace(&format!("${{{{ matrix.{key} }}}}"), replacement);
    }
    let value = quoted(&value)?;
    if !identifier(&value) {
        return Err("unresolved job name".into());
    }
    Ok(value)
}

fn check_name(
    id: &str,
    template: Option<&str>,
    matrix: &BTreeMap<String, String>,
) -> Result<String, String> {
    match template {
        Some(template) => name(template, matrix),
        None if matrix.is_empty() => Ok(id.into()),
        // GitHub's default name for the observed unnamed runner-only include
        // matrix. Other unnamed matrix shapes remain explicitly unresolved.
        None if matrix.len() == 1 && matrix.contains_key("runner") => {
            let runner = &matrix["runner"];
            if !runner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
            {
                return Err("unsupported default matrix name".into());
            }
            Ok(format!("{id} ({runner})"))
        }
        None => Err("unsupported default matrix name".into()),
    }
}

fn timeout_choice(expr: &str, matrix: &BTreeMap<String, String>, depth: u8) -> Option<u64> {
    if let Ok(n) = expr.parse::<u64>() {
        return (1..=360).contains(&n).then_some(n);
    }
    if depth > 2 {
        return None;
    }
    let (condition, alternatives) = expr.split_once(" && ")?;
    let (field, expected) = condition.split_once(" == ")?;
    let field = field.strip_prefix("matrix.")?;
    let expected = expected.strip_prefix('\'')?.strip_suffix('\'')?;
    if !identifier(field) || expected.contains(['\'', '(', ')']) {
        return None;
    }
    let (yes, no) = if let Some(group) = alternatives.strip_prefix('(') {
        let end = group.find(')')?;
        (&group[..end], group[end + 1..].strip_prefix(" || ")?)
    } else {
        alternatives.split_once(" || ")?
    };
    let yes = timeout_choice(yes, matrix, depth + 1)?;
    let no = timeout_choice(no, matrix, depth + 1)?;
    Some(if matrix.get(field)? == expected {
        yes
    } else {
        no
    })
}

fn timeout(expression: Option<&str>, matrix: &BTreeMap<String, String>) -> Option<u64> {
    let expression = expression?;
    let minutes = if let Ok(n) = expression.parse::<u64>() {
        n
    } else {
        // The existing bounded matrix timeout form; arbitrary expressions are
        // deliberately MISSING. No expression evaluation or default 360m guess.
        let expr = expression.strip_prefix("${{ ")?.strip_suffix(" }}")?;
        timeout_choice(expr, matrix, 0)?
    };
    if (1..=360).contains(&minutes) {
        Some(minutes * 60)
    } else {
        None
    }
}

fn workflow(path: &str, source: &str, registered: &[String]) -> Result<Workflow, String> {
    let mut jobs: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut in_jobs = false;
    let mut current = None;
    for line in source.lines() {
        if line == "jobs:" {
            if in_jobs {
                return Err("duplicate jobs section".into());
            }
            in_jobs = true;
            continue;
        }
        if !in_jobs || line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            return Err("unsupported workflow after jobs".into());
        }
        if line.starts_with("  ") && !line.starts_with("   ") {
            let id = line
                .trim()
                .strip_suffix(':')
                .ok_or("unsupported job declaration")?;
            if !identifier(id) || jobs.insert(id.into(), Vec::new()).is_some() {
                return Err("duplicate or unsupported job".into());
            }
            current = Some(id.to_string());
        } else {
            jobs.get_mut(current.as_ref().ok_or("metadata without job")?)
                .unwrap()
                .push(line);
        }
    }
    let gates: Vec<_> = jobs
        .keys()
        .filter(|id| id.ends_with("-ci-gate"))
        .cloned()
        .collect();
    if gates.len() != 1 {
        return Err("required gate source ambiguous".into());
    }
    let gate = &gates[0];
    let gate_lines = &jobs[gate];
    let gate_needs = needs(gate_lines)?.ok_or("gate has no needs")?;
    let mut expected: BTreeSet<_> = registered.iter().cloned().collect();
    expected.insert("ci-plan".into());
    if gate_needs.iter().cloned().collect::<BTreeSet<_>>() != expected
        || field(gate_lines, "if")?.as_deref() != Some("always()")
        || field(gate_lines, "name")?.as_deref() != Some(gate)
        || !gate_lines.contains(&"          TESTED_SHA: ${{ github.sha }}")
        || !gate_lines.contains(&"          NEEDS_JSON: ${{ toJSON(needs) }}")
    {
        return Err("gate contract does not match selection registry".into());
    }
    let workflow_id = path
        .strip_prefix(".github/workflows/")
        .and_then(|p| p.strip_suffix(".yml"))
        .ok_or("unsupported workflow path")?;
    let invocation = format!("          python3 scripts/ci_selection.py gate --workflow {workflow_id} --needs-json \"$NEEDS_JSON\" --tested-sha \"$TESTED_SHA\"");
    if !gate_lines.contains(&invocation.as_str()) {
        return Err("unrecognized selection gate invocation".into());
    }
    expected.insert(gate.clone());
    if jobs.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err("unregistered workflow job".into());
    }
    let mut dependencies = BTreeMap::new();
    for (id, lines) in &jobs {
        let values = needs(lines)?.unwrap_or_default();
        if values
            .iter()
            .any(|dependency| !jobs.contains_key(dependency) || dependency == id)
        {
            return Err("unknown or self-referential job dependency".into());
        }
        dependencies.insert(id.clone(), values);
    }
    let mut pending: BTreeSet<_> = jobs.keys().cloned().collect();
    while !pending.is_empty() {
        let ready: Vec<_> = pending
            .iter()
            .filter(|id| {
                dependencies[*id]
                    .iter()
                    .all(|dependency| !pending.contains(dependency))
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            return Err("cyclic job dependencies".into());
        }
        for id in ready {
            pending.remove(&id);
        }
    }
    let plan = &jobs["ci-plan"];
    let selectors: BTreeSet<_> = plan
        .iter()
        .filter_map(|line| line.strip_prefix("      select_"))
        .filter_map(|line| line.split_once(": "))
        .filter(|(key, value)| {
            identifier(key) && *value == format!("${{{{ steps.plan.outputs.select_{key} }}}}")
        })
        .map(|(key, _)| key.to_string())
        .collect();
    let mut specs = Vec::new();
    for (id, lines) in jobs {
        let conditional = registered.contains(&id);
        let condition = field(&lines, "if")?;
        let selector = condition
            .as_deref()
            .and_then(|value| value.strip_prefix("needs.ci-plan.outputs.select_"))
            .and_then(|value| value.strip_suffix(" == 'true'"));
        if conditional
            && (!selector.is_some_and(|selector| selectors.contains(selector))
                || !dependencies[&id]
                    .iter()
                    .any(|dependency| dependency == "ci-plan"))
            || id == "ci-plan" && condition.is_some()
        {
            return Err("unrecognized job selection condition".into());
        }
        let template = field(&lines, "name")?;
        let expression = field(&lines, "timeout-minutes")?;
        match matrices(&lines) {
            Ok(entries) => {
                for matrix in entries {
                    let check_name = check_name(&id, template.as_deref(), &matrix);
                    let limit = timeout(expression.as_deref(), &matrix);
                    let problem = check_name
                        .as_ref()
                        .err()
                        .cloned()
                        .or_else(|| limit.is_none().then(|| "unresolved timeout".into()));
                    specs.push(Spec {
                        id: id.clone(),
                        name: check_name.unwrap_or_else(|_| id.clone()),
                        timeout: limit,
                        conditional,
                        problem,
                    });
                }
            }
            Err(problem) => specs.push(Spec {
                id: id.clone(),
                name: id,
                timeout: None,
                conditional,
                problem: Some(problem),
            }),
        }
    }
    let names: BTreeSet<_> = specs.iter().map(|s| &s.name).collect();
    if names.len() != specs.len() {
        return Err("ambiguous source check names".into());
    }
    Ok(Workflow {
        path: path.into(),
        gate: gate.clone(),
        specs,
    })
}

fn sources(cwd: &Path, sha: &str) -> Result<Vec<Workflow>, String> {
    validate_sha(sha)?;
    let resolved = git(
        cwd,
        &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
    )?;
    if resolved.trim() != sha {
        return Err("source SHA mismatch".into());
    }
    let registry = git(cwd, &["show", &format!("{sha}:scripts/ci_selection.py")])?;
    let actual = git(
        cwd,
        &[
            "ls-tree",
            "--full-tree",
            "-r",
            "--name-only",
            sha,
            "--",
            ".github/workflows",
        ],
    )?;
    parse_sources(&registry, &actual, |path| {
        git(cwd, &["show", &format!("{sha}:{path}")])
    })
}

// The same bounded source parser is used by Git-backed production reads and
// in-module metadata snapshots. SHA resolution and gh transport stay outside
// this pure projection; fixtures do not bypass gate/registry validation.
fn parse_sources(
    registry: &str,
    actual: &str,
    mut load: impl FnMut(&str) -> Result<String, String>,
) -> Result<Vec<Workflow>, String> {
    let mut registered = BTreeMap::new();
    for line in block(registry, "WORKFLOW_JOBS:")? {
        let (key, value) = line
            .trim()
            .split_once(": ")
            .ok_or("unsupported job registry")?;
        let key = quoted(key)?;
        if !identifier(&key) {
            return Err("unsupported workflow ID".into());
        }
        let tuple = value
            .strip_prefix('(')
            .and_then(|v| v.strip_suffix("),"))
            .ok_or("unsupported registry tuple")?;
        let jobs: Vec<_> = tuple
            .trim_end_matches(',')
            .split(',')
            .map(quoted)
            .collect::<Result<_, _>>()?;
        if jobs.iter().any(|j| !identifier(j))
            || jobs.len() != jobs.iter().collect::<BTreeSet<_>>().len()
            || registered.insert(key, jobs).is_some()
        {
            return Err("ambiguous registered jobs".into());
        }
    }
    let mut workflows = Vec::new();
    let mut paths = BTreeSet::new();
    let mut workflow_ids = BTreeSet::new();
    for line in block(registry, "WORKFLOW_YAML:")? {
        let (key, file) = line
            .trim()
            .split_once(": ")
            .ok_or("unsupported workflow registry")?;
        let key = quoted(key)?;
        let file = quoted(
            file.strip_suffix(',')
                .ok_or("unsupported workflow registry entry")?,
        )?;
        if file != format!("{key}.yml") || !workflow_ids.insert(key.clone()) {
            return Err("ambiguous workflow registry".into());
        }
        let path = format!(".github/workflows/{file}");
        paths.insert(path.clone());
        let source = load(&path)?;
        workflows.push(workflow(
            &path,
            &source,
            registered.get(&key).ok_or("workflow jobs missing")?,
        )?);
    }
    if workflow_ids != registered.keys().cloned().collect() || workflows.is_empty() {
        return Err("incomplete workflow registry".into());
    }
    // The registry explicitly separates release/base-image publication. Do not
    // guess that a newly added, unregistered workflow is optional.
    for key in [
        "RELEASE_WORKFLOW_FILE",
        "CI_BASE_WORKFLOW_FILE",
        "TURSO_MANUAL_WORKFLOW_FILE",
    ] {
        let prefix = format!("{key} = ");
        let values: Vec<_> = registry
            .lines()
            .filter_map(|l| l.strip_prefix(&prefix))
            .collect();
        if values.is_empty() && key == "TURSO_MANUAL_WORKFLOW_FILE" {
            continue;
        }
        if values.len() != 1 {
            return Err("publication workflow source ambiguous".into());
        }
        let file = quoted(values[0])?;
        if key == "TURSO_MANUAL_WORKFLOW_FILE" && file != "turso-test.yml" {
            return Err("unrecognized manual Turso workflow".into());
        }
        if !identifier(
            file.strip_suffix(".yml")
                .ok_or("unsupported workflow filename")?,
        ) {
            return Err("unsupported workflow filename".into());
        }
        paths.insert(format!(".github/workflows/{file}"));
    }
    let actual: BTreeSet<_> = actual.lines().map(str::to_string).collect();
    if actual != paths {
        return Err("unregistered or missing workflow source".into());
    }
    Ok(workflows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    id: u64,
    path: String,
    sha: String,
    attempt: u64,
    status: String,
    conclusion: String,
    suite: u64,
    created: u64,
    event: String,
}

#[derive(Debug, Clone)]
struct Check {
    id: u64,
    name: String,
    sha: String,
    app: u64,
    slug: String,
    suite: u64,
    status: String,
    conclusion: String,
}

#[derive(Debug, Clone)]
struct Job {
    id: u64,
    run: u64,
    attempt: u64,
    sha: String,
    name: String,
    status: String,
    conclusion: String,
    start: Option<u64>,
    end: Option<u64>,
    check: u64,
}

fn number(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| "invalid numeric API field".into())
}
fn optional_number(value: &str) -> Result<Option<u64>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        number(value).map(Some)
    }
}

// gh --paginate projects every page. Page counts must agree and the number of
// distinct IDs must equal total_count; API caps, partial pages and races fail.
fn pages<const N: usize>(text: &str) -> Result<Vec<[&str; N]>, String> {
    let mut total = None;
    let mut rows = Vec::new();
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        if let Some(count) = line.strip_prefix("TOTAL\t") {
            let count = number(count)?;
            if total.is_some_and(|n| n != count) {
                return Err("API pagination total changed".into());
            }
            total = Some(count);
        } else {
            let fields: [&str; N] = line
                .split('\t')
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|_| "invalid API projection")?;
            if fields.iter().any(|v| v.contains(['\\', '\r', '\x1b']))
                || !ids.insert(number(fields[0])?)
            {
                return Err("duplicate or unsafe API row".into());
            }
            rows.push(fields);
        }
    }
    if total != Some(rows.len() as u64) {
        return Err("incomplete API pagination".into());
    }
    Ok(rows)
}

fn runs(text: &str) -> Result<Vec<Run>, String> {
    pages::<9>(text)?
        .into_iter()
        .map(|v| {
            Ok(Run {
                id: number(v[0])?,
                path: v[1].into(),
                sha: v[2].into(),
                attempt: number(v[3])?,
                status: v[4].into(),
                conclusion: v[5].into(),
                suite: number(v[6])?,
                created: number(v[7])?,
                event: v[8].into(),
            })
        })
        .collect()
}
fn checks(text: &str) -> Result<Vec<Check>, String> {
    pages::<8>(text)?
        .into_iter()
        .map(|v| {
            Ok(Check {
                id: number(v[0])?,
                name: v[1].into(),
                sha: v[2].into(),
                app: number(v[3])?,
                slug: v[4].into(),
                suite: number(v[5])?,
                status: v[6].into(),
                conclusion: v[7].into(),
            })
        })
        .collect()
}
fn jobs(text: &str) -> Result<Vec<Job>, String> {
    pages::<10>(text)?
        .into_iter()
        .map(|v| {
            Ok(Job {
                id: number(v[0])?,
                run: number(v[1])?,
                attempt: number(v[2])?,
                sha: v[3].into(),
                name: v[4].into(),
                status: v[5].into(),
                conclusion: v[6].into(),
                start: optional_number(v[7])?,
                end: optional_number(v[8])?,
                check: number(v[9])?,
            })
        })
        .collect()
}

fn latest<'a>(
    runs: &'a [Run],
    path: &str,
    sha: &str,
    event: &str,
) -> Result<Option<&'a Run>, String> {
    if runs
        .iter()
        .any(|r| r.sha != sha || r.attempt == 0 || !identifier(&r.event))
        || !identifier(event)
    {
        return Err("API run source SHA/attempt/event mismatch".into());
    }
    Ok(runs
        .iter()
        .filter(|r| r.path == path && r.event == event)
        .max_by_key(|r| (r.created, r.id, r.attempt)))
}

fn replaced(run: &Run, history: &[Run]) -> Result<bool, String> {
    if !history.contains(run) {
        return Err("selected run missing from exact-SHA history".into());
    }
    Ok(latest(history, &run.path, &run.sha, &run.event)?
        .is_some_and(|new| new.id != run.id && (new.created, new.id) > (run.created, run.id)))
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Pass,
    Fail,
    NotRun,
    Missing,
    Cancelled,
}
impl State {
    fn label(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotRun => "NOTRUN",
            Self::Missing => "MISSING",
            Self::Cancelled => "CANCELLED(대체됨)",
        }
    }
}
#[derive(Debug)]
struct Row {
    state: State,
    note: String,
    job: Option<u64>,
    check: Option<u64>,
    app: Option<u64>,
    elapsed: Option<u64>,
    remaining: Option<i128>,
    warning: bool,
}
fn missing(note: &str) -> Row {
    Row {
        state: State::Missing,
        note: note.into(),
        job: None,
        check: None,
        app: None,
        elapsed: None,
        remaining: None,
        warning: false,
    }
}

fn observe(
    spec: &Spec,
    run: &Run,
    history: &[Run],
    all_jobs: &[Job],
    all_checks: &[Check],
    now: u64,
) -> Row {
    if let Some(problem) = &spec.problem {
        return missing(problem);
    }
    if all_jobs
        .iter()
        .any(|j| j.sha != run.sha || j.run != run.id || j.attempt != run.attempt)
        || all_checks.iter().any(|c| c.sha != run.sha)
    {
        return missing("job/check SHA or latest attempt mismatch");
    }
    let replaced = match replaced(run, history) {
        Ok(value) => value,
        Err(error) => return missing(&error),
    };
    let mut candidates: Vec<_> = all_jobs.iter().filter(|j| j.name == spec.name).collect();
    if candidates.is_empty() && spec.conditional {
        candidates = all_jobs
            .iter()
            .filter(|j| j.name == spec.id && j.conclusion == "skipped")
            .collect();
    }
    if candidates.is_empty() && matches!(run.conclusion.as_str(), "cancelled" | "timed_out") {
        let mut row = missing("cancelled run has no job; excluded from acceptance");
        row.state = if run.conclusion == "timed_out" {
            row.note = "run timed_out; job missing".into();
            State::Fail
        } else if replaced {
            State::Cancelled
        } else {
            row.note =
                "cancelled run without newer same-SHA/workflow/event run; job missing".into();
            State::Fail
        };
        return row;
    }
    if candidates.len() != 1 {
        return missing("latest job missing or ambiguous");
    }
    let job = candidates[0];
    let Some(check) = all_checks.iter().find(|c| c.id == job.check) else {
        return missing("matching check ID missing");
    };
    // A third-party app with the same name cannot satisfy an Actions job. Nor
    // can an older check from another suite or an earlier rerun of this job.
    if check.slug != "github-actions"
        || check.app == 0
        || check.suite != run.suite
        || check.name != job.name
        || check.status != job.status
        || check.conclusion != job.conclusion
        || all_checks.iter().any(|c| {
            c.app == check.app && c.suite == check.suite && c.name == check.name && c.id > check.id
        })
    {
        return missing("check identity/result is not the latest matching Actions job");
    }
    let mut row = missing("");
    row.job = Some(job.id);
    row.check = Some(check.id);
    row.app = Some(check.app);
    let running = matches!(
        job.status.as_str(),
        "in_progress" | "queued" | "waiting" | "pending" | "requested"
    );
    // A cancelled run can still contain a failing gate or timed-out product
    // job. Preserve that failure even when a genuinely newer run exists.
    if job.status == "completed"
        && matches!(
            job.conclusion.as_str(),
            "failure" | "timed_out" | "action_required" | "startup_failure" | "stale"
        )
    {
        row.state = State::Fail;
        row.note = job.conclusion.clone();
    } else if run.conclusion == "timed_out" {
        row.state = State::Fail;
        row.note = "run timed_out".into();
    } else if job.status == "completed" && job.conclusion == "cancelled"
        || run.conclusion == "cancelled"
    {
        row.state = if replaced {
            row.note = "newer same-SHA/workflow/event run; excluded from acceptance".into();
            State::Cancelled
        } else {
            row.note = "cancelled without newer same-SHA/workflow/event run".into();
            State::Fail
        };
    } else if running && job.conclusion.is_empty() && job.end.is_none() {
        row.state = State::NotRun;
        row.note = "incomplete latest attempt".into();
    } else if job.status == "completed" {
        match job.conclusion.as_str() {
            "success" => {
                row.state = State::Pass;
            }
            "skipped" => {
                row.state = State::NotRun;
                row.note = "skipped; selection gate required".into();
            }
            _ => {
                row.note = "unsupported conclusion".into();
            }
        }
    } else {
        row.note = "inconsistent status/conclusion".into();
    }
    let elapsed = match (job.start, job.end) {
        (Some(start), Some(end)) if end <= now => end.checked_sub(start),
        (Some(start), None) if running => now.checked_sub(start),
        _ => None,
    };
    row.elapsed = elapsed;
    // Skipped/queued jobs legitimately have no execution timestamps. A passed
    // check without a completion time must never become successful evidence.
    if elapsed.is_none() {
        if row.state == State::Pass {
            row.state = State::Missing;
            row.note = "completed job timestamps missing or invalid".into();
        } else if row.state == State::Fail {
            row.note.push_str("; elapsed MISSING");
        }
    }
    if let (Some(limit), Some(elapsed)) = (spec.timeout, elapsed) {
        let remaining = i128::from(limit) - i128::from(elapsed);
        row.remaining = Some(remaining);
        row.warning = remaining * 10 < i128::from(limit);
    }
    row
}

fn table(
    workflow: &Workflow,
    run: &Run,
    history: &[Run],
    jobs: &[Job],
    checks: &[Check],
    now: u64,
) -> (Vec<Row>, bool) {
    let mut rows: Vec<_> = workflow
        .specs
        .iter()
        .map(|s| observe(s, run, history, jobs, checks, now))
        .collect();
    let gate_ok = workflow
        .specs
        .iter()
        .zip(&rows)
        .any(|(s, r)| s.id == workflow.gate && r.state == State::Pass);
    let plan_ok = workflow
        .specs
        .iter()
        .zip(&rows)
        .any(|(s, r)| s.id == "ci-plan" && r.state == State::Pass);
    let mut accepted = run.status == "completed" && run.conclusion == "success";
    for (spec, row) in workflow.specs.iter().zip(&mut rows) {
        if row.state == State::NotRun
            && row.note == "skipped; selection gate required"
            && spec.conditional
            && gate_ok
            && plan_ok
        {
            row.note = "skipped; matching ci-plan + selection gate PASS".into();
        } else if row.state != State::Pass {
            accepted = false;
        }
    }
    (rows, accepted)
}

fn workflow_state(run: &Run, rows: &[Row], accepted: bool) -> State {
    if rows.iter().any(|row| row.state == State::Fail)
        || matches!(run.conclusion.as_str(), "failure" | "timed_out")
    {
        State::Fail
    } else if accepted {
        State::Pass
    } else if rows.iter().any(|row| row.state == State::Missing) {
        State::Missing
    } else if rows.iter().any(|row| row.state == State::Cancelled) {
        State::Cancelled
    } else {
        State::NotRun
    }
}

fn show_number(value: Option<u64>) -> String {
    value.map_or_else(|| "MISSING".into(), |n| n.to_string())
}
fn print_row(path: &str, spec: &Spec, run: Option<&Run>, row: &Row) {
    let run_id = run.map(|r| r.id);
    let attempt = run.map(|r| r.attempt);
    let remaining = row
        .remaining
        .map_or_else(|| "MISSING".into(), |n| format!("{n}s"));
    println!(
        "| {path} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}{} |",
        run.map_or("MISSING", |run| run.event.as_str()),
        spec.name,
        row.state.label(),
        show_number(run_id),
        show_number(attempt),
        show_number(row.job),
        show_number(row.check),
        show_number(row.app),
        show_number(row.elapsed),
        show_number(spec.timeout),
        remaining,
        row.note,
        if row.warning {
            "; WARNING remaining <10%"
        } else {
            ""
        }
    );
}

/// Returns true only when the exact-SHA snapshot is complete and its selection
/// gates validate any skips. This result is a self-check, not review/approval.
pub fn run(sha: &str) -> Result<bool, String> {
    run_report(sha).map(|(accepted, _)| accepted)
}

fn run_report(sha: &str) -> Result<(bool, Vec<String>), String> {
    validate_sha(sha)?;
    let current = std::env::current_dir().map_err(|_| "working directory unavailable")?;
    let root = git(&current, &["rev-parse", "--show-toplevel"])?;
    let cwd = Path::new(root.trim_end_matches('\n'));
    let xtask_commit = git(
        cwd,
        &[
            "log",
            "-1",
            "--format=%H",
            "--",
            "xtask",
            ".cargo/config.toml",
        ],
    )?;
    validate_sha(xtask_commit.trim())?;
    let dirty = git(
        cwd,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "xtask",
            ".cargo/config.toml",
        ],
    )?;
    if !dirty.is_empty() {
        return Err(
            "xtask source is uncommitted; commit identity cannot attest this executable".into(),
        );
    }
    let query_time = command(cwd, "date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?;
    println!(
        "tested_sha={sha}\nxtask_commit={}\nqueried_at_utc={}",
        xtask_commit.trim(),
        query_time.trim()
    );
    println!(
        "source=exact-SHA workflow registry + ci-gate needs; self-check != independent acceptance"
    );
    println!("| workflow | event | check | state | run ID | attempt | job ID | check ID | app ID | elapsed s | timeout s | remaining | note |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let workflows = match sources(cwd, sha) {
        Ok(value) => value,
        Err(error) => {
            println!(
                "| source | - | required-list | MISSING | - | - | - | - | - | - | - | - | {error} |"
            );
            return Ok((false, vec![error]));
        }
    };
    let mut parsing_errors: Vec<_> = workflows
        .iter()
        .flat_map(|workflow| {
            workflow.specs.iter().filter_map(|spec| {
                spec.problem
                    .as_ref()
                    .map(|problem| format!("{} {}: {problem}", workflow.path, spec.name))
            })
        })
        .collect();
    let run_data = api(
        cwd,
        &format!("{REPO}/actions/runs?head_sha={sha}&per_page=100"),
        RUN_QUERY,
        true,
    )
    .and_then(|v| runs(&v));
    let check_data = api(
        cwd,
        &format!("{REPO}/commits/{sha}/check-runs?filter=all&per_page=100"),
        CHECK_QUERY,
        true,
    )
    .and_then(|v| checks(&v));
    for error in [run_data.as_ref().err(), check_data.as_ref().err()]
        .into_iter()
        .flatten()
    {
        parsing_errors.push(error.clone());
    }
    let mut accepted = true;
    for workflow in workflows {
        let selected = run_data.as_ref().map_err(Clone::clone).and_then(|runs| {
            // Different events have independent latest results. A newer
            // push run cannot conceal a cancelled pull_request run.
            let events: BTreeSet<_> = runs
                .iter()
                .filter(|run| run.path == workflow.path)
                .map(|run| run.event.as_str())
                .collect();
            events
                .into_iter()
                .map(|event| {
                    latest(runs, &workflow.path, sha, event)?
                        .ok_or_else(|| "no exact-SHA workflow/event run".into())
                })
                .collect::<Result<Vec<_>, String>>()
        });
        let selected = match selected {
            Ok(value) if !value.is_empty() => value,
            other => {
                let note = other
                    .err()
                    .unwrap_or_else(|| "no exact-SHA workflow run".into());
                for spec in &workflow.specs {
                    print_row(&workflow.path, spec, None, &missing(&note));
                }
                accepted = false;
                continue;
            }
        };
        for selected in selected {
            let job_data = api(
                cwd,
                &format!(
                    "{REPO}/actions/runs/{}/attempts/{}/jobs?per_page=100",
                    selected.id, selected.attempt
                ),
                JOB_QUERY,
                true,
            )
            .and_then(|v| jobs(&v));
            if let Err(error) = &job_data {
                parsing_errors.push(format!("{}: {error}", workflow.path));
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "clock before UNIX epoch")?
                .as_secs();
            let snapshot = match (&job_data, &check_data) {
                (Ok(jobs), Ok(checks)) => {
                    // A final read is a snapshot consistency check, never a retry.
                    let query = r#"[.head_sha,.run_attempt,.status,(.conclusion // ""),.check_suite_id,.event,.path]|@tsv"#;
                    let current = api(
                        cwd,
                        &format!("{REPO}/actions/runs/{}", selected.id),
                        query,
                        false,
                    );
                    let expected = format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        sha,
                        selected.attempt,
                        selected.status,
                        selected.conclusion,
                        selected.suite,
                        selected.event,
                        selected.path
                    );
                    if current
                        .as_ref()
                        .is_ok_and(|v| v.trim_end_matches('\n') == expected)
                    {
                        Ok(table(
                            &workflow,
                            selected,
                            run_data.as_ref().map_err(Clone::clone)?,
                            jobs,
                            checks,
                            now,
                        ))
                    } else {
                        Err("latest run changed or consistency read failed".to_string())
                    }
                }
                (Err(error), _) | (_, Err(error)) => Err(error.clone()),
            };
            match snapshot {
                Ok((rows, pass)) => {
                    accepted &= pass;
                    println!(
                        "workflow_result={} event={} run={} state={}",
                        workflow.path,
                        selected.event,
                        selected.id,
                        workflow_state(selected, &rows, pass).label()
                    );
                    for (spec, row) in workflow.specs.iter().zip(rows) {
                        print_row(&workflow.path, spec, Some(selected), &row);
                    }
                }
                Err(note) => {
                    accepted = false;
                    for spec in &workflow.specs {
                        print_row(&workflow.path, spec, Some(selected), &missing(&note));
                    }
                }
            }
        }
    }
    println!(
        "acceptance={} (observed CI only; review/approval separate)",
        if accepted { "PASS" } else { "INCOMPLETE" }
    );
    println!("source_api_parsing_errors={}", parsing_errors.len());
    Ok((accepted, parsing_errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const SOURCE: &str = "430a260a05d440dffe812f1d19993d7d5009c58f";
    const OTHER: &str = "93534e2831aea473523674fc458015dfbd375cb1";
    const REVIEWED_PR_SOURCE: &str = "1b977c16465c72b2095b3ba44d2aa1d545c4825c";
    const REVIEWED_MAIN_SOURCE: &str = "68529a276579217703a525a1d191a7d45652c9d9";

    // Offline metadata projections from the exact source SHAs below. Keep job
    // names, needs, selectors, matrices, timeouts and complete gate steps; omit
    // product execution steps that the bounded parser does not read. Optional
    // publication/manual files contribute paths only, as in production.
    const PR_SOURCES: &[(&str, &str)] = &[
        (
            "scripts/ci_selection.py",
            r###"WORKFLOW_JOBS: dict[str, tuple[str, ...]] = {
    "web": ("web-static", "web-checks", "web-native-checks", "workspace-browser-build", "workspace-browser-shard", "collaboration-build", "collaboration-flow"),
    "rust": ("fast", "native-arm64", "postgres-build", "postgres", "collaboration"),
    "documents": ("native-extraction",),
    "collab-engine": ("native-collab-engine",),
    "install": ("install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"),
}
WORKFLOW_YAML: dict[str, str] = {
    "web": "web.yml",
    "rust": "rust.yml",
    "documents": "documents.yml",
    "collab-engine": "collab-engine.yml",
    "install": "install.yml",
}
RELEASE_WORKFLOW_FILE = "release.yml"
TURSO_MANUAL_WORKFLOW_FILE = "turso-test.yml"
CI_BASE_WORKFLOW_FILE = "ci-base-image.yml"
"###,
        ),
        (".github/workflows/ci-base-image.yml", r###""###),
        (
            ".github/workflows/collab-engine.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-26.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_native_collab_engine: ${{ steps.plan.outputs.select_native_collab_engine }}
  native-collab-engine:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_native_collab_engine == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
            check: native-collab-engine
          - runner: ubuntu-26.04-arm
            check: native-collab-engine-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 15
  collab-engine-ci-gate:
    name: collab-engine-ci-gate
    needs: [ci-plan, native-collab-engine]
    if: always()
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow collab-engine --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (
            ".github/workflows/documents.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-26.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_native_extraction: ${{ steps.plan.outputs.select_native_extraction }}
  native-extraction:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_native_extraction == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
            check: native-extraction
          - runner: ubuntu-26.04-arm
            check: native-extraction-arm64
    runs-on: ${{ matrix.runner }}
    # x64 113019003045 cancelled in apt after the extract crate had passed.
    # 30m is the 914s wall, 183s to finish the last 28.8MB at the observed
    # rate, 44s sqlite, 460s cross-feature compile analogue, 120s unmeasured
    # test bodies, and 60s. ARM has no shortage evidence and stays 15m.
    timeout-minutes: ${{ matrix.check == 'native-extraction' && 30 || 15 }}
  documents-ci-gate:
    name: documents-ci-gate
    needs: [ci-plan, native-extraction]
    if: always()
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow documents --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (
            ".github/workflows/install.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-26.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_install_smoke: ${{ steps.plan.outputs.select_install_smoke }}
      select_backup_restore_smoke: ${{ steps.plan.outputs.select_backup_restore_smoke }}
      select_upgrade_smoke_arm64: ${{ steps.plan.outputs.select_upgrade_smoke_arm64 }}
  install-smoke:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_install_smoke == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
            check: install-smoke
          - runner: ubuntu-26.04-arm
            check: install-smoke-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 45
  backup-restore-smoke:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_backup_restore_smoke == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
            check: backup-restore-smoke
          - runner: ubuntu-26.04-arm
            check: backup-restore-smoke-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 60
  upgrade-smoke-arm64:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'
    runs-on: ubuntu-26.04-arm
    timeout-minutes: 60
  install-ci-gate:
    name: install-ci-gate
    needs: [ci-plan, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]
    if: always()
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow install --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (".github/workflows/release.yml", r###""###),
        (
            ".github/workflows/rust.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-26.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_fast: ${{ steps.plan.outputs.select_fast }}
      select_postgres: ${{ steps.plan.outputs.select_postgres }}
      select_native_arm64: ${{ steps.plan.outputs.select_native_arm64 }}
      select_collaboration: ${{ steps.plan.outputs.select_collaboration }}
  fast:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_fast == 'true'
    runs-on: ubuntu-26.04
    # Cold target miss 113019589871 filled 911s through the 54 SQLite filters.
    # Seven later cargo test commands were not started. 19m is that wall,
    # plus one measured 165s profile compile for the unrun tail, plus 60s.
    timeout-minutes: 19
  native-arm64:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_native_arm64 == 'true'
    runs-on: ubuntu-26.04-arm
    timeout-minutes: 15
  postgres-build:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_postgres == 'true'
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
          - runner: ubuntu-26.04-arm
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 25
  postgres:
    needs: [ci-plan, postgres-build]
    if: needs.ci-plan.outputs.select_postgres == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        # A/C split only task/comment from measured PG16 A: 160.19s moved,
        # 404.03s retained (cold compile/setup are separate). Both keep 15m.
        # B retains every target/helper/install/S3: x64 20m, ARM64 25m.
        # Measured ARM setup/PG 17m59.916s + install 3m30s + S3/post 34s
        # = 22m03.916s; 25m leaves 2m56.084s for hosted variation/cleanup.
        # Each major/architecture pair runs every suite exactly once across
        # A/B/C. PG18 covers both architectures; PG16/17 cover x64.
        # Keep image pins in sync with scripts/start-test-postgres.sh.
        include:
          - runner: ubuntu-26.04
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres
            shard: a
            tests: --test db_integration --test collab_integration --test invitation_integration --test search_index --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-26.04
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-c
            shard: c
            tests: --test task_integration --test comment_integration
          - runner: ubuntu-26.04
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-26.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64
            shard: a
            tests: --test db_integration --test collab_integration --test invitation_integration --test search_index --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-26.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64-c
            shard: c
            tests: --test task_integration --test comment_integration
          - runner: ubuntu-26.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-26.04
            pg_major: "16"
            postgres_image: postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54
            check: postgres-pg16
            shard: a
            tests: --test db_integration --test collab_integration --test invitation_integration --test search_index --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-26.04
            pg_major: "16"
            postgres_image: postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54
            check: postgres-pg16-c
            shard: c
            tests: --test task_integration --test comment_integration
          - runner: ubuntu-26.04
            pg_major: "16"
            postgres_image: postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54
            check: postgres-pg16-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-26.04
            pg_major: "17"
            postgres_image: postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f
            check: postgres-pg17
            shard: a
            tests: --test db_integration --test collab_integration --test invitation_integration --test search_index --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-26.04
            pg_major: "17"
            postgres_image: postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f
            check: postgres-pg17-c
            shard: c
            tests: --test task_integration --test comment_integration
          - runner: ubuntu-26.04
            pg_major: "17"
            postgres_image: postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f
            check: postgres-pg17-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
    runs-on: ${{ matrix.runner }}
    # Main PG16 B measured ~818s PostgreSQL + ~48s S3, exceeding 15m with setup.
    # A/C stay at 15m and x64 B at 20m; ARM64 B uses the measured allowance above.
    timeout-minutes: ${{ matrix.shard == 'b' && (matrix.runner == 'ubuntu-26.04-arm' && 25 || 20) || 15 }}
  collaboration:
    needs: [ci-plan, postgres-build]
    if: needs.ci-plan.outputs.select_collaboration == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-26.04
            check: collaboration
          - runner: ubuntu-26.04-arm
            check: collaboration-arm64
    runs-on: ${{ matrix.runner }}
    # x64 cold miss 113019589982 filled 913s at 13/75 of the serial binary.
    # 62 tests at the observed 20s/13 pace is 95s; that pace is doubled, plus 60s.
    # ARM is not in that cancelled log and keeps 15m.
    timeout-minutes: ${{ matrix.check == 'collaboration' && 20 || 15 }}
  rust-ci-gate:
    name: rust-ci-gate
    needs: [ci-plan, fast, native-arm64, postgres-build, postgres, collaboration]
    if: always()
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow rust --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (".github/workflows/turso-test.yml", r###""###),
        (
            ".github/workflows/web.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-26.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}
      select_web_static: ${{ steps.plan.outputs.select_web_static }}
      select_workspace_browser_shard: ${{ steps.plan.outputs.select_workspace_browser_shard }}
      select_collaboration_build: ${{ steps.plan.outputs.select_collaboration_build }}
      select_collaboration_flow: ${{ steps.plan.outputs.select_collaboration_flow }}
  web-static:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_web_static == 'true'
    runs-on: ubuntu-26.04
    timeout-minutes: 15
  web-checks:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_web_checks == 'true'
    runs-on: ubuntu-26.04
    # Cold target miss 113019000421 filled 913s, including 133s of an unfinished
    # test-profile compile. 23m is 780s before that compile, a measured 269s
    # test-profile envelope, a 112s larger-suite body analogue, 120s for the
    # unmeasured bun/plan steps, and 60s.
    timeout-minutes: 23
  web-native-checks:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_web_checks == 'true'
    runs-on: ubuntu-26.04
    timeout-minutes: 15
  workspace-browser-build:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_workspace_browser_shard == 'true'
    runs-on: ubuntu-26.04
    timeout-minutes: 20
    outputs:
      artifact_id: ${{ steps.publish.outputs.artifact-id }}
      handoff_sha256: ${{ steps.prepare.outputs.handoff_sha256 }}
  workspace-browser-shard:
    needs: [ci-plan, workspace-browser-build]
    if: needs.ci-plan.outputs.select_workspace_browser_shard == 'true'
    name: web-browser-shard-${{ matrix.shard }}
    strategy:
      fail-fast: false
      matrix:
        shard: [0, 1, 2, 3, 4, 5, 6, 7]
    runs-on: ubuntu-26.04
    # Dependencies and groups retain their budgets; compilation is producer-only.
    timeout-minutes: 20
  collaboration-build:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_collaboration_build == 'true'
    runs-on: ubuntu-26.04
    timeout-minutes: 15
    outputs:
      artifact_id: ${{ steps.publish.outputs.artifact-id }}
      handoff_sha256: ${{ steps.prepare.outputs.handoff_sha256 }}
  collaboration-flow:
    needs: [ci-plan, collaboration-build]
    if: needs.ci-plan.outputs.select_collaboration_flow == 'true'
    runs-on: ubuntu-26.04
    timeout-minutes: 15
  web-ci-gate:
    name: web-ci-gate
    needs:
      [
        ci-plan,
        web-static,
        web-checks,
        web-native-checks,
        workspace-browser-build,
        workspace-browser-shard,
        collaboration-build,
        collaboration-flow,
      ]
    if: always()
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow web --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
    ];
    const MAIN_SOURCES: &[(&str, &str)] = &[
        (
            "scripts/ci_selection.py",
            r###"WORKFLOW_JOBS: dict[str, tuple[str, ...]] = {
    "web": ("web-static", "web-checks", "workspace-browser-shard", "collaboration-flow"),
    "rust": ("fast", "postgres", "collaboration"),
    "documents": ("native-extraction",),
    "collab-engine": ("native-collab-engine",),
    "install": ("install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"),
}
WORKFLOW_YAML: dict[str, str] = {
    "web": "web.yml",
    "rust": "rust.yml",
    "documents": "documents.yml",
    "collab-engine": "collab-engine.yml",
    "install": "install.yml",
}
RELEASE_WORKFLOW_FILE = "release.yml"
CI_BASE_WORKFLOW_FILE = "ci-base-image.yml"
"###,
        ),
        (".github/workflows/ci-base-image.yml", r###""###),
        (
            ".github/workflows/collab-engine.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_native_collab_engine: ${{ steps.plan.outputs.select_native_collab_engine }}
  native-collab-engine:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_native_collab_engine == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-24.04
            check: native-collab-engine
          - runner: ubuntu-24.04-arm
            check: native-collab-engine-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 15
  collab-engine-ci-gate:
    name: collab-engine-ci-gate
    needs: [ci-plan, native-collab-engine]
    if: always()
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow collab-engine --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (
            ".github/workflows/documents.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_native_extraction: ${{ steps.plan.outputs.select_native_extraction }}
  native-extraction:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_native_extraction == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-24.04
            check: native-extraction
          - runner: ubuntu-24.04-arm
            check: native-extraction-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 15
  documents-ci-gate:
    name: documents-ci-gate
    needs: [ci-plan, native-extraction]
    if: always()
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow documents --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (
            ".github/workflows/install.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_install_smoke: ${{ steps.plan.outputs.select_install_smoke }}
      select_backup_restore_smoke: ${{ steps.plan.outputs.select_backup_restore_smoke }}
      select_upgrade_smoke_arm64: ${{ steps.plan.outputs.select_upgrade_smoke_arm64 }}
  install-smoke:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_install_smoke == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-24.04
            check: install-smoke
          - runner: ubuntu-24.04-arm
            check: install-smoke-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 45
  backup-restore-smoke:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_backup_restore_smoke == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-24.04
            check: backup-restore-smoke
          - runner: ubuntu-24.04-arm
            check: backup-restore-smoke-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 60
  upgrade-smoke-arm64:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'
    runs-on: ubuntu-24.04-arm
    timeout-minutes: 60
  install-ci-gate:
    name: install-ci-gate
    needs: [ci-plan, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]
    if: always()
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow install --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (".github/workflows/release.yml", r###""###),
        (
            ".github/workflows/rust.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_fast: ${{ steps.plan.outputs.select_fast }}
      select_postgres: ${{ steps.plan.outputs.select_postgres }}
      select_collaboration: ${{ steps.plan.outputs.select_collaboration }}
  fast:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_fast == 'true'
    runs-on: ubuntu-24.04
    timeout-minutes: 15
  postgres:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_postgres == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        # Two DB shards on x64; ARM64 also has a small C shard for the native
        # default-feature build and policy tests. The cold native checks took
        # 483s before A's DB build, exhausting its 15m budget on main 6fb29931.
        # Each PostgreSQL suite runs in exactly one shard per major/architecture.
        # Keep the lists in sync when adding a suite. PG18 runs on both; the
        # other source-supported majors (16, 17) run both shards on x64 only.
        # Keep image pins in sync with scripts/start-test-postgres.sh.
        include:
          - runner: ubuntu-24.04
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres
            shard: a
            tests: --test db_integration --test task_integration --test collab_integration --test invitation_integration --test search_index --test comment_integration --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-24.04
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-24.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64
            shard: a
            tests: --test db_integration --test task_integration --test collab_integration --test invitation_integration --test search_index --test comment_integration --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration
          - runner: ubuntu-24.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64-c
            shard: c
            tests: --test schedule_ics_integration --test pool_release_integration
          - runner: ubuntu-24.04-arm
            pg_major: "18"
            postgres_image: postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db
            check: postgres-arm64-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-24.04
            pg_major: "16"
            postgres_image: postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54
            check: postgres-pg16
            shard: a
            tests: --test db_integration --test task_integration --test collab_integration --test invitation_integration --test search_index --test comment_integration --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-24.04
            pg_major: "16"
            postgres_image: postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54
            check: postgres-pg16-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
          - runner: ubuntu-24.04
            pg_major: "17"
            postgres_image: postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f
            check: postgres-pg17
            shard: a
            tests: --test db_integration --test task_integration --test collab_integration --test invitation_integration --test search_index --test comment_integration --test api_token_integration --test mail_integration --test task_labels_integration --test workspace_lifecycle --test search_query --test notification_integration --test push_integration --test schedule_ics_integration --test search_meili --test secret_maintenance_integration --test outbox_reset_integration --test pool_release_integration
          - runner: ubuntu-24.04
            pg_major: "17"
            postgres_image: postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f
            check: postgres-pg17-b
            shard: b
            tests: --test project_integration --test document_integration --test attachment_integration --test group_integration --test task_activity_integration --test background_jobs --test task_milestones_integration --test outbox_integration --test attachment_extract_integration --test share_stars_integration --test account_lifecycle --test admin_integration --test integrations_integration --test collections_integration --test identity_integration --test attachment_parents_integration --test attachment_preview_integration --test project_lifecycle_integration --test task_ops_integration --test document_export_docx_integration --test document_export_pdf_integration --test search_semantic --test mcp_integration --test doctor_integration --test templates_integration --test unfurl_integration
    runs-on: ${{ matrix.runner }}
    # Main PG16 B measured ~818s PostgreSQL + ~48s S3, exceeding 15m with setup.
    # Keep A/C at 15m; B gets a finite 20m allowance for this observed cold path.
    timeout-minutes: ${{ matrix.shard == 'b' && 20 || 15 }}
  collaboration:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_collaboration == 'true'
    name: ${{ matrix.check }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-24.04
            check: collaboration
          - runner: ubuntu-24.04-arm
            check: collaboration-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 15
  rust-ci-gate:
    name: rust-ci-gate
    needs: [ci-plan, fast, postgres, collaboration]
    if: always()
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow rust --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
        (
            ".github/workflows/web.yml",
            r###"jobs:
  ci-plan:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      mode: ${{ steps.plan.outputs.mode }}
      reason_code: ${{ steps.plan.outputs.reason_code }}
      plan_ok: ${{ steps.plan.outputs.plan_ok }}
      plan_json: ${{ steps.plan.outputs.plan_json }}
      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}
      select_web_static: ${{ steps.plan.outputs.select_web_static }}
      select_workspace_browser_shard: ${{ steps.plan.outputs.select_workspace_browser_shard }}
      select_collaboration_flow: ${{ steps.plan.outputs.select_collaboration_flow }}
  web-static:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_web_static == 'true'
    runs-on: ubuntu-24.04
    timeout-minutes: 15
  web-checks:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_web_checks == 'true'
    runs-on: ubuntu-24.04
    timeout-minutes: 15
  workspace-browser-shard:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_workspace_browser_shard == 'true'
    name: web-browser-shard-${{ matrix.shard }}
    strategy:
      fail-fast: false
      matrix:
        shard: [0, 1, 2, 3, 4, 5, 6, 7]
    runs-on: ubuntu-24.04
    timeout-minutes: 15
  collaboration-flow:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_collaboration_flow == 'true'
    runs-on: ubuntu-24.04
    timeout-minutes: 15
  web-ci-gate:
    name: web-ci-gate
    needs: [ci-plan, web-static, web-checks, workspace-browser-shard, collaboration-flow]
    if: always()
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Fail closed on selection results
        env:
          NEEDS_JSON: ${{ toJSON(needs) }}
          TESTED_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          python3 scripts/ci_selection.py gate --workflow web --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"
"###,
        ),
    ];
    // GET projections captured from run 37881810443 attempt 1 (16 jobs and
    // checks, Actions app 15368, suite 102636832927) and real newer run
    // 37886450340. No auth data, URLs or live requests enter offline tests.
    const OBSERVED_RUNS: &str = r###"TOTAL	2
37881810443	.github/workflows/web.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	cancelled	102636832927	1791518392	pull_request
37886450340	.github/workflows/web.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	failure	102649070866	1791521977	pull_request
"###;
    const OBSERVED_JOBS: &str = r###"TOTAL	16
113662960801	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791518491	1791518502	113662960801
113663412779	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration-build	completed	cancelled	1791518632	1791519536	113663412779
113663412813	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-native-checks	completed	success	1791518832	1791519263	113663412813
113663412826	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-static	completed	success	1791519221	1791519843	113663412826
113663412862	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	workspace-browser-build	completed	success	1791518755	1791519238	113663412862
113663412963	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-checks	completed	success	1791519691	1791520148	113663412963
113666376624	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-4	completed	success	1791519706	1791520146	113666376624
113666376661	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-6	completed	success	1791520096	1791520503	113666376661
113666376684	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-1	completed	success	1791520295	1791520774	113666376684
113666376693	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-3	completed	success	1791520102	1791520497	113666376693
113666376696	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-2	completed	success	1791520511	1791520891	113666376696
113666376735	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-0	completed	success	1791520603	1791520986	113666376735
113666376754	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-5	completed	success	1791520569	1791521006	113666376754
113666376772	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-7	completed	success	1791520660	1791521089	113666376772
113667597720	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration-flow	completed	skipped	1791519537	1791519536	113667597720
113673894736	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-ci-gate	completed	failure	1791521092	1791521098	113673894736
"###;
    const OBSERVED_CHECKS: &str = r###"TOTAL	16
113673894736	web-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	failure
113667597720	collaboration-flow	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	skipped
113666376772	web-browser-shard-7	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376754	web-browser-shard-5	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376735	web-browser-shard-0	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376696	web-browser-shard-2	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376693	web-browser-shard-3	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376684	web-browser-shard-1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376661	web-browser-shard-6	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376624	web-browser-shard-4	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412963	web-checks	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412862	workspace-browser-build	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412826	web-static	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412813	web-native-checks	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412779	collaboration-build	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	cancelled
113662960801	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
"###;
    const OBSERVED_AT: u64 = 1791522812;
    // Read-only GET snapshots captured from the reviewed PR/main heads.
    // Tuple fields: provenance, exact run, all attempt jobs, all suite checks.
    // Source metadata above retains the gate steps; unrelated execution steps
    // are omitted because the bounded production parser never consumes them.
    // These snapshots reproduce observed results, not current CI acceptance.
    const OBSERVED_TABLES: &[(&str, &str, &str, &str)] = &[
        (
            "PR",
            r####"37881810443	.github/workflows/web.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	cancelled	102636832927	1791518392	pull_request
"####,
            r####"TOTAL	16
113662960801	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791518491	1791518502	113662960801
113663412779	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration-build	completed	cancelled	1791518632	1791519536	113663412779
113663412813	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-native-checks	completed	success	1791518832	1791519263	113663412813
113663412826	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-static	completed	success	1791519221	1791519843	113663412826
113663412862	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	workspace-browser-build	completed	success	1791518755	1791519238	113663412862
113663412963	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-checks	completed	success	1791519691	1791520148	113663412963
113666376624	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-4	completed	success	1791519706	1791520146	113666376624
113666376661	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-6	completed	success	1791520096	1791520503	113666376661
113666376684	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-1	completed	success	1791520295	1791520774	113666376684
113666376693	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-3	completed	success	1791520102	1791520497	113666376693
113666376696	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-2	completed	success	1791520511	1791520891	113666376696
113666376735	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-0	completed	success	1791520603	1791520986	113666376735
113666376754	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-5	completed	success	1791520569	1791521006	113666376754
113666376772	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-browser-shard-7	completed	success	1791520660	1791521089	113666376772
113667597720	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration-flow	completed	skipped	1791519537	1791519536	113667597720
113673894736	37881810443	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	web-ci-gate	completed	failure	1791521092	1791521098	113673894736
"####,
            r####"TOTAL	16
113673894736	web-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	failure
113667597720	collaboration-flow	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	skipped
113666376772	web-browser-shard-7	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376754	web-browser-shard-5	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376735	web-browser-shard-0	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376696	web-browser-shard-2	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376693	web-browser-shard-3	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376684	web-browser-shard-1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376661	web-browser-shard-6	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113666376624	web-browser-shard-4	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412963	web-checks	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412862	workspace-browser-build	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412826	web-static	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412813	web-native-checks	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
113663412779	collaboration-build	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	cancelled
113662960801	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832927	completed	success
"####,
        ),
        (
            "PR",
            r####"37881810418	.github/workflows/rust.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	success	102636832845	1791518392	pull_request
"####,
            r####"TOTAL	20
113662961047	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791519404	1791519527	113662961047
113667560041	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-build (ubuntu-26.04-arm)	completed	success	1791519540	1791520395	113667560041
113667560047	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	native-arm64	completed	success	1791519699	1791520325	113667560047
113667560098	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	fast	completed	success	1791519943	1791520658	113667560098
113667560164	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-build (ubuntu-26.04)	completed	success	1791520175	1791520817	113667560164
113672787613	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration-arm64	completed	success	1791520822	1791521671	113672787613
113672787668	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collaboration	completed	success	1791520819	1791521369	113672787668
113672787810	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg16	completed	success	1791520819	1791521115	113672787810
113672787820	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg17-b	completed	success	1791520820	1791521216	113672787820
113672787822	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg17	completed	success	1791520819	1791521094	113672787822
113672787848	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-arm64-c	completed	success	1791520866	1791521023	113672787848
113672787855	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg16-b	completed	success	1791520819	1791521228	113672787855
113672787874	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-b	completed	success	1791520819	1791521250	113672787874
113672787879	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-arm64-b	completed	success	1791520899	1791521537	113672787879
113672787885	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-arm64	completed	success	1791520922	1791521192	113672787885
113672787898	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg16-c	completed	success	1791520820	1791521019	113672787898
113672787907	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-c	completed	success	1791520822	1791521007	113672787907
113672787922	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres	completed	success	1791520931	1791521207	113672787922
113672788141	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	postgres-pg17-c	completed	success	1791520856	1791521011	113672788141
113676209447	37881810418	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	rust-ci-gate	completed	success	1791521751	1791521757	113676209447
"####,
            r####"TOTAL	20
113676209447	rust-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672788141	postgres-pg17-c	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787922	postgres	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787907	postgres-c	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787898	postgres-pg16-c	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787885	postgres-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787879	postgres-arm64-b	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787874	postgres-b	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787855	postgres-pg16-b	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787848	postgres-arm64-c	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787822	postgres-pg17	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787820	postgres-pg17-b	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787810	postgres-pg16	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787668	collaboration	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113672787613	collaboration-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113667560164	postgres-build (ubuntu-26.04)	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113667560098	fast	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113667560047	native-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113667560041	postgres-build (ubuntu-26.04-arm)	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
113662961047	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832845	completed	success
"####,
        ),
        (
            "PR",
            r####"37881810439	.github/workflows/documents.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	success	102636832912	1791518392	pull_request
"####,
            r####"TOTAL	4
113662960994	37881810439	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791518931	1791518939	113662960994
113665175831	37881810439	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	native-extraction-arm64	completed	success	1791519138	1791519920	113665175831
113665175839	37881810439	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	native-extraction	completed	success	1791519529	1791520130	113665175839
113670012329	37881810439	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	documents-ci-gate	completed	success	1791520328	1791520335	113670012329
"####,
            r####"TOTAL	4
113670012329	documents-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832912	completed	success
113665175839	native-extraction	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832912	completed	success
113665175831	native-extraction-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832912	completed	success
113662960994	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832912	completed	success
"####,
        ),
        (
            "PR",
            r####"37881810438	.github/workflows/collab-engine.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	success	102636832915	1791518392	pull_request
"####,
            r####"TOTAL	4
113662960765	37881810438	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791518478	1791518489	113662960765
113663362620	37881810438	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	native-collab-engine-arm64	completed	success	1791518627	1791518703	113663362620
113663362636	37881810438	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	native-collab-engine	completed	success	1791518860	1791518930	113663362636
113665138893	37881810438	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	collab-engine-ci-gate	completed	success	1791519511	1791519516	113665138893
"####,
            r####"TOTAL	4
113665138893	collab-engine-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832915	completed	success
113663362636	native-collab-engine	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832915	completed	success
113663362620	native-collab-engine-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832915	completed	success
113662960765	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832915	completed	success
"####,
        ),
        (
            "PR",
            r####"37881810420	.github/workflows/install.yml	1b977c16465c72b2095b3ba44d2aa1d545c4825c	1	completed	success	102636832847	1791518392	pull_request
"####,
            r####"TOTAL	7
113662960973	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	ci-plan	completed	success	1791519139	1791519147	113662960973
113666009039	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	install-smoke-arm64	completed	success	1791519242	1791520494	113666009039
113666009167	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	install-smoke	completed	success	1791519646	1791520687	113666009167
113666009197	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	backup-restore-smoke	completed	success	1791519921	1791520862	113666009197
113666009268	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	backup-restore-smoke-arm64	completed	success	1791519494	1791520807	113666009268
113666010072	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	upgrade-smoke-arm64	completed	skipped	1791519147	1791519147	113666010072
113672975967	37881810420	1	1b977c16465c72b2095b3ba44d2aa1d545c4825c	install-ci-gate	completed	success	1791520945	1791520950	113672975967
"####,
            r####"TOTAL	7
113672975967	install-ci-gate	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
113666010072	upgrade-smoke-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	skipped
113666009268	backup-restore-smoke-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
113666009197	backup-restore-smoke	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
113666009167	install-smoke	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
113666009039	install-smoke-arm64	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
113662960973	ci-plan	1b977c16465c72b2095b3ba44d2aa1d545c4825c	15368	github-actions	102636832847	completed	success
"####,
        ),
        (
            "MAIN",
            r####"37881795532	.github/workflows/web.yml	68529a276579217703a525a1d191a7d45652c9d9	1	completed	success	102636789222	1791518379	push
"####,
            r####"TOTAL	13
113662914132	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	ci-plan	completed	success	1791518684	1791518693	113662914132
113664184971	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-checks	completed	success	1791519100	1791519679	113664184971
113664184988	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-static	completed	success	1791519430	1791520031	113664184988
113664185011	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	collaboration-flow	completed	success	1791520149	1791520789	113664185011
113664185068	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-4	completed	success	1791519553	1791520281	113664185068
113664185107	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-2	completed	success	1791520444	1791520930	113664185107
113664185119	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-5	completed	success	1791520131	1791520772	113664185119
113664185126	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-1	completed	success	1791520337	1791520809	113664185126
113664185130	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-3	completed	success	1791520492	1791521121	113664185130
113664185145	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-0	completed	success	1791520496	1791521140	113664185145
113664185150	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-7	completed	success	1791520283	1791520942	113664185150
113664185155	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-browser-shard-6	completed	success	1791520397	1791520918	113664185155
113674102727	37881795532	1	68529a276579217703a525a1d191a7d45652c9d9	web-ci-gate	completed	success	1791521143	1791521152	113674102727
"####,
            r####"TOTAL	13
113674102727	web-ci-gate	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185155	web-browser-shard-6	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185150	web-browser-shard-7	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185145	web-browser-shard-0	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185130	web-browser-shard-3	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185126	web-browser-shard-1	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185119	web-browser-shard-5	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185107	web-browser-shard-2	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185068	web-browser-shard-4	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664185011	collaboration-flow	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664184988	web-static	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113664184971	web-checks	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
113662914132	ci-plan	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789222	completed	success
"####,
        ),
        (
            "MAIN",
            r####"37881795561	.github/workflows/rust.yml	68529a276579217703a525a1d191a7d45652c9d9	1	completed	success	102636789281	1791518379	push
"####,
            r####"TOTAL	14
113662914124	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	ci-plan	completed	success	1791518677	1791518700	113662914124
113664213851	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	collaboration-arm64	completed	success	1791518934	1791519695	113664213851
113664213914	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	collaboration	completed	success	1791519728	1791520283	113664213914
113664213916	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-pg16-b	completed	success	1791519010	1791519643	113664213916
113664213918	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	fast	completed	success	1791520507	1791520776	113664213918
113664213921	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-pg16	completed	success	1791519148	1791519663	113664213921
113664213939	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-pg17	completed	success	1791519238	1791519726	113664213939
113664213959	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-pg17-b	completed	success	1791519665	1791520186	113664213959
113664213966	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-b	completed	success	1791519551	1791520173	113664213966
113664213977	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-arm64	completed	success	1791518985	1791519551	113664213977
113664213978	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-arm64-c	completed	success	1791518969	1791519549	113664213978
113664213984	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres-arm64-b	completed	success	1791519028	1791519705	113664213984
113664214031	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	postgres	completed	success	1791520155	1791520455	113664214031
113672622248	37881795561	1	68529a276579217703a525a1d191a7d45652c9d9	rust-ci-gate	completed	success	1791520778	1791520785	113672622248
"####,
            r####"TOTAL	14
113672622248	rust-ci-gate	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664214031	postgres	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213984	postgres-arm64-b	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213978	postgres-arm64-c	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213977	postgres-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213966	postgres-b	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213959	postgres-pg17-b	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213939	postgres-pg17	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213921	postgres-pg16	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213918	fast	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213916	postgres-pg16-b	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213914	collaboration	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113664213851	collaboration-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
113662914124	ci-plan	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789281	completed	success
"####,
        ),
        (
            "MAIN",
            r####"37881795588	.github/workflows/documents.yml	68529a276579217703a525a1d191a7d45652c9d9	1	completed	success	102636789326	1791518379	push
"####,
            r####"TOTAL	4
113662914248	37881795588	1	68529a276579217703a525a1d191a7d45652c9d9	ci-plan	completed	success	1791518704	1791518712	113662914248
113664266827	37881795588	1	68529a276579217703a525a1d191a7d45652c9d9	native-extraction	completed	success	1791518980	1791519439	113664266827
113664266909	37881795588	1	68529a276579217703a525a1d191a7d45652c9d9	native-extraction-arm64	completed	success	1791518820	1791519401	113664266909
113667201406	37881795588	1	68529a276579217703a525a1d191a7d45652c9d9	documents-ci-gate	completed	success	1791520089	1791520094	113667201406
"####,
            r####"TOTAL	4
113667201406	documents-ci-gate	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789326	completed	success
113664266909	native-extraction-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789326	completed	success
113664266827	native-extraction	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789326	completed	success
113662914248	ci-plan	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789326	completed	success
"####,
        ),
        (
            "MAIN",
            r####"37881795548	.github/workflows/collab-engine.yml	68529a276579217703a525a1d191a7d45652c9d9	1	completed	success	102636789250	1791518379	push
"####,
            r####"TOTAL	4
113662914016	37881795548	1	68529a276579217703a525a1d191a7d45652c9d9	ci-plan	completed	success	1791518444	1791518454	113662914016
113663219672	37881795548	1	68529a276579217703a525a1d191a7d45652c9d9	native-collab-engine	completed	success	1791518758	1791518816	113663219672
113663219717	37881795548	1	68529a276579217703a525a1d191a7d45652c9d9	native-collab-engine-arm64	completed	success	1791518544	1791518616	113663219717
113664684440	37881795548	1	68529a276579217703a525a1d191a7d45652c9d9	collab-engine-ci-gate	completed	success	1791519090	1791519098	113664684440
"####,
            r####"TOTAL	4
113664684440	collab-engine-ci-gate	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789250	completed	success
113663219717	native-collab-engine-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789250	completed	success
113663219672	native-collab-engine	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789250	completed	success
113662914016	ci-plan	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789250	completed	success
"####,
        ),
        (
            "MAIN",
            r####"37881795595	.github/workflows/install.yml	68529a276579217703a525a1d191a7d45652c9d9	1	completed	success	102636789351	1791518379	push
"####,
            r####"TOTAL	7
113662914279	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	ci-plan	completed	success	1791518504	1791518512	113662914279
113663449933	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	backup-restore-smoke	completed	success	1791519273	1791520079	113663449933
113663450013	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	install-smoke	completed	success	1791519440	1791520077	113663450013
113663450054	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	install-smoke-arm64	completed	success	1791518598	1791519236	113663450054
113663450194	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	backup-restore-smoke-arm64	completed	success	1791518678	1791519305	113663450194
113663450913	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	upgrade-smoke-arm64	completed	skipped	1791518512	1791518512	113663450913
113669809055	37881795595	1	68529a276579217703a525a1d191a7d45652c9d9	install-ci-gate	completed	success	1791520153	1791520158	113669809055
"####,
            r####"TOTAL	7
113669809055	install-ci-gate	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
113663450913	upgrade-smoke-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	skipped
113663450194	backup-restore-smoke-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
113663450054	install-smoke-arm64	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
113663450013	install-smoke	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
113663449933	backup-restore-smoke	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
113662914279	ci-plan	68529a276579217703a525a1d191a7d45652c9d9	15368	github-actions	102636789351	completed	success
"####,
        ),
    ];

    fn spec(name: &str) -> Spec {
        Spec {
            id: name.into(),
            name: name.into(),
            timeout: Some(900),
            conditional: false,
            problem: None,
        }
    }
    fn sample_run() -> Run {
        Run {
            id: 100,
            path: ".github/workflows/web.yml".into(),
            sha: SOURCE.into(),
            attempt: 2,
            status: "completed".into(),
            conclusion: "success".into(),
            suite: 800,
            created: 10,
            event: "pull_request".into(),
        }
    }
    fn sample_job(name: &str, id: u64) -> Job {
        Job {
            id,
            run: 100,
            attempt: 2,
            sha: SOURCE.into(),
            name: name.into(),
            status: "completed".into(),
            conclusion: "success".into(),
            start: Some(1000),
            end: Some(1810),
            check: id,
        }
    }
    fn sample_check(job: &Job) -> Check {
        Check {
            id: job.check,
            name: job.name.clone(),
            sha: job.sha.clone(),
            app: 15368,
            slug: "github-actions".into(),
            suite: 800,
            status: job.status.clone(),
            conclusion: job.conclusion.clone(),
        }
    }

    fn repository_root() -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"]).unwrap().trim())
    }

    fn head(cwd: &Path) -> String {
        git(cwd, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    fn fixture_sources(files: &[(&str, &str)]) -> Result<Vec<Workflow>, String> {
        let registry = files
            .iter()
            .find(|(path, _)| *path == "scripts/ci_selection.py")
            .ok_or("fixture registry missing")?
            .1;
        let paths = files
            .iter()
            .filter(|(path, _)| path.starts_with(".github/workflows/"))
            .map(|(path, _)| *path)
            .collect::<Vec<_>>()
            .join("\n");
        parse_sources(registry, &paths, |path| {
            files
                .iter()
                .find(|(key, _)| *key == path)
                .map(|(_, value)| (*value).to_string())
                .ok_or_else(|| "fixture workflow missing".to_string())
        })
    }

    #[test]
    fn needs_accepts_scalar_inline_block_and_multiline_flow_without_guessing() {
        for lines in [
            vec!["    needs: ci-plan"],
            vec!["    needs: [ci-plan, build]"],
            vec!["    needs:", "      - ci-plan", "      - build"],
            vec![
                "    needs:",
                "      [",
                "        ci-plan,",
                "        build,",
                "      ]",
                "    if: always()",
            ],
        ] {
            let expected = if lines.len() == 1 && lines[0] == "    needs: ci-plan" {
                vec!["ci-plan".to_string()]
            } else {
                vec!["ci-plan".to_string(), "build".to_string()]
            };
            assert_eq!(needs(&lines).unwrap(), Some(expected));
        }
        for lines in [
            vec!["    needs: [ci-plan, ci-plan]"],
            vec!["    needs: [ci-plan]", "    needs: build"],
            vec!["    needs: *alias"],
            vec!["    needs: ${{ fromJSON(needs.plan.outputs.jobs) }}"],
            vec!["    needs:", "      unknown: build"],
            vec!["    needs:", "      [", "        ci-plan,"],
            vec!["    needs:ci-plan"],
            vec!["    needs: []"],
        ] {
            assert!(needs(&lines).is_err(), "unexpected needs acceptance");
        }
    }

    #[test]
    fn registered_build_dependencies_and_shared_selectors_are_supported_but_unknowns_are_rejected()
    {
        let cwd = repository_root();
        let original = git(
            &cwd,
            &["show", &format!("{}:.github/workflows/web.yml", head(&cwd))],
        )
        .unwrap();
        let registered: Vec<_> = [
            "web-static",
            "web-checks",
            "workspace-browser-shard",
            "collaboration-flow",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let source = original.replace(
            "  workspace-browser-shard:\n    needs: ci-plan",
            "  workspace-browser-shard:\n    needs: [ci-plan, web-checks]",
        );
        assert!(workflow(".github/workflows/web.yml", &source, &registered).is_ok());
        assert!(workflow(
            ".github/workflows/web.yml",
            &source.replace("[ci-plan, web-checks]", "[ci-plan, unknown]"),
            &registered
        )
        .is_err());
        let cyclic = source.replace(
            "  web-checks:\n    needs: ci-plan",
            "  web-checks:\n    needs: [ci-plan, workspace-browser-shard]",
        );
        assert!(workflow(".github/workflows/web.yml", &cyclic, &registered)
            .unwrap_err()
            .contains("cyclic"));
        let unknown = source.replace(
            "select_workspace_browser_shard == 'true'",
            "select_unknown == 'true'",
        );
        assert!(workflow(".github/workflows/web.yml", &unknown, &registered).is_err());
        let shared = source.replace(
            "select_workspace_browser_shard == 'true'",
            "select_web_checks == 'true'",
        );
        assert!(workflow(".github/workflows/web.yml", &shared, &registered).is_ok());
    }

    #[test]
    fn nested_existing_matrix_timeout_and_runner_default_name_are_exact() {
        let expression =
            "${{ matrix.shard == 'b' && (matrix.runner == 'ubuntu-26.04-arm' && 25 || 20) || 15 }}";
        for (shard, runner, seconds) in [
            ("b", "ubuntu-26.04-arm", 1500),
            ("b", "ubuntu-26.04", 1200),
            ("a", "ubuntu-26.04-arm", 900),
        ] {
            let matrix = BTreeMap::from([
                ("shard".into(), shard.into()),
                ("runner".into(), runner.into()),
            ]);
            assert_eq!(timeout(Some(expression), &matrix), Some(seconds));
        }
        let matrix = BTreeMap::from([("runner".into(), "ubuntu-26.04-arm".into())]);
        assert_eq!(
            check_name("postgres-build", None, &matrix).unwrap(),
            "postgres-build (ubuntu-26.04-arm)"
        );
        assert!(check_name(
            "unknown",
            None,
            &BTreeMap::from([("unsupported".into(), "value".into())])
        )
        .is_err());
        assert_eq!(
            timeout(Some("${{ matrix.runner != 'x' && 25 || 20 }}"), &matrix),
            None
        );
    }

    #[test]
    fn fixed_modern_workflow_sources_have_complete_names_timeouts_and_gate_contracts() {
        for (sha, files) in [
            (REVIEWED_PR_SOURCE, PR_SOURCES),
            (REVIEWED_MAIN_SOURCE, MAIN_SOURCES),
        ] {
            let workflows = fixture_sources(files).unwrap();
            assert_eq!(workflows.len(), 5);
            assert!(
                workflows
                    .iter()
                    .flat_map(|workflow| &workflow.specs)
                    .all(|spec| spec.problem.is_none()),
                "unresolved source metadata"
            );
            if sha == REVIEWED_PR_SOURCE {
                let rust = workflows
                    .iter()
                    .find(|workflow| workflow.path.ends_with("/rust.yml"))
                    .unwrap();
                assert_eq!(
                    rust.specs
                        .iter()
                        .find(|spec| spec.name == "postgres-arm64-b")
                        .unwrap()
                        .timeout,
                    Some(1500)
                );
                assert_eq!(
                    rust.specs
                        .iter()
                        .find(|spec| spec.name == "postgres-build (ubuntu-26.04)")
                        .unwrap()
                        .timeout,
                    Some(1500)
                );
            }
        }
    }

    // Offline replay of real read-only source/run/job/check projections.
    // No gh process or network is involved; these are historical snapshots.
    #[test]
    fn offline_tables_for_fixed_pr_and_main_have_zero_parsing_errors() {
        for (label, sha, files) in [
            ("PR", REVIEWED_PR_SOURCE, PR_SOURCES),
            ("MAIN", REVIEWED_MAIN_SOURCE, MAIN_SOURCES),
        ] {
            let workflows = fixture_sources(files).unwrap();
            let mut history: Vec<_> = OBSERVED_TABLES
                .iter()
                .filter(|(source, _, _, _)| *source == label)
                .flat_map(|(_, run, _, _)| runs(&format!("TOTAL\t1\n{run}")).unwrap())
                .collect();
            if label == "PR" {
                history.extend(
                    runs(OBSERVED_RUNS)
                        .unwrap()
                        .into_iter()
                        .filter(|run| run.id != 37881810443),
                );
            }
            assert_eq!(workflows.len(), 5);
            let mut seen = BTreeSet::new();
            for (_, run, job_data, check_data) in OBSERVED_TABLES
                .iter()
                .filter(|(source, _, _, _)| *source == label)
            {
                let run = runs(&format!("TOTAL\t1\n{run}")).unwrap().remove(0);
                assert_eq!(run.sha, sha);
                assert!(seen.insert(run.path.clone()));
                let workflow = workflows
                    .iter()
                    .find(|workflow| workflow.path == run.path)
                    .unwrap();
                let jobs = jobs(job_data).unwrap();
                let checks = checks(check_data).unwrap();
                assert!(!jobs.is_empty());
                assert!(jobs
                    .iter()
                    .all(|job| job.run == run.id && job.attempt == run.attempt && job.sha == sha));
                assert!(checks.iter().all(|check| check.sha == sha
                    && check.suite == run.suite
                    && check.app == 15368
                    && check.slug == "github-actions"));
                assert_eq!(jobs.len(), checks.len());
                let (rows, accepted) = table(workflow, &run, &history, &jobs, &checks, OBSERVED_AT);
                assert_eq!(rows.len(), workflow.specs.len());
                assert!(workflow.specs.iter().all(|spec| spec.problem.is_none()));
                assert!(
                    rows.iter().all(|row| row.state != State::Missing),
                    "unexpected source/API identity gap"
                );
                println!("offline_snapshot={label} sha={sha} workflow={} run={} rows={} state={} acceptance={accepted} parsing_errors=0", run.path, run.id, rows.len(), workflow_state(&run, &rows, accepted).label());
            }
            assert_eq!(seen.len(), 5);
        }
    }

    #[test]
    fn offline_cancelled_web_run_37881810443_is_fail() {
        let history = runs(OBSERVED_RUNS).unwrap();
        let run = history.iter().find(|run| run.id == 37881810443).unwrap();
        assert_eq!(run.sha, REVIEWED_PR_SOURCE);
        assert_eq!(run.path, ".github/workflows/web.yml");
        assert_eq!(run.event, "pull_request");
        assert_eq!(run.conclusion, "cancelled");
        let workflows = fixture_sources(PR_SOURCES).unwrap();
        let workflow = workflows
            .iter()
            .find(|workflow| workflow.path == run.path)
            .unwrap();
        let jobs = jobs(OBSERVED_JOBS).unwrap();
        let checks = checks(OBSERVED_CHECKS).unwrap();
        assert_eq!(jobs.len(), 16);
        assert_eq!(checks.len(), 16);
        let (rows, accepted) = table(workflow, run, &history, &jobs, &checks, OBSERVED_AT);
        let state = workflow_state(run, &rows, accepted);
        println!("offline_run_table_begin=37881810443");
        println!(
            "historical_snapshot_utc=2026-10-09T05:13:32Z sha={}",
            run.sha
        );
        println!(
            "Web={} event={} run={} acceptance={accepted}",
            state.label(),
            run.event,
            run.id
        );
        println!("| workflow | event | check | state | run ID | attempt | job ID | check ID | app ID | elapsed s | timeout s | remaining | note |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for (spec, row) in workflow.specs.iter().zip(&rows) {
            print_row(&workflow.path, spec, Some(run), row);
        }
        println!("offline_run_table_end=37881810443");
        assert!(!accepted);
        assert_eq!(state, State::Fail);
        assert_eq!(
            rows.iter().filter(|row| row.state == State::Fail).count(),
            1
        );
        assert!(workflow.specs.iter().zip(&rows).any(|(spec, row)| {
            spec.id == workflow.gate && row.state == State::Fail && row.note == "failure"
        }));
    }

    #[test]
    fn only_exact_lowercase_sha_is_accepted() {
        assert!(validate_sha(SOURCE).is_ok());
        for value in [
            "",
            "430a260a",
            "HEAD",
            &SOURCE.to_uppercase(),
            &format!("{SOURCE}0"),
            "z30a260a05d440dffe812f1d19993d7d5009c58f",
            &format!("{SOURCE}\n"),
        ] {
            assert!(validate_sha(value).is_err());
            assert!(run(value).is_err());
        }
    }

    #[test]
    fn pagination_retains_every_page_and_rejects_partial_duplicate_and_changed_counts() {
        let value = format!("TOTAL\t2\n1\t.github/workflows/web.yml\t{SOURCE}\t1\tcompleted\tsuccess\t8\t10\tpull_request\nTOTAL\t2\n2\t.github/workflows/web.yml\t{SOURCE}\t2\tin_progress\t\t9\t20\tpull_request\n");
        let parsed = runs(&value).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            latest(&parsed, ".github/workflows/web.yml", SOURCE, "pull_request")
                .unwrap()
                .unwrap()
                .id,
            2
        );
        assert!(runs(&value.replace("TOTAL\t2\n2", "TOTAL\t3\n2")).is_err());
        assert!(runs(&value.replace("\n2\t", "\n1\t")).is_err());
        assert!(runs(&value.replace("TOTAL\t2", "TOTAL\t3")).is_err());
        assert!(runs("").is_err());
        assert!(runs(&format!(
            "TOTAL\t1\n1\tunsafe\\tname\t{SOURCE}\t1\tcompleted\tsuccess\t8\t10\tpull_request\n"
        ))
        .is_err());
        assert!(runs("TOTAL\t0\n").unwrap().is_empty());
    }

    #[test]
    fn latest_run_replaces_old_success_and_never_uses_another_sha() {
        let old = sample_run();
        let mut new = old.clone();
        new.id += 1;
        new.created += 1;
        new.conclusion = "cancelled".into();
        let values = [old.clone(), new.clone()];
        assert_eq!(
            latest(&values, &new.path, SOURCE, &new.event).unwrap(),
            Some(&new)
        );
        assert!(latest(&values, &new.path, OTHER, &new.event).is_err());
        new.sha = OTHER.into();
        assert!(latest(
            &[old, new],
            ".github/workflows/web.yml",
            SOURCE,
            "pull_request"
        )
        .is_err());
        assert!(
            latest(&[], ".github/workflows/web.yml", SOURCE, "pull_request")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn latest_attempt_and_checks_are_scoped_by_job_id_suite_app_and_sha() {
        let run = sample_run();
        let job = sample_job("fast", 7);
        let check = sample_check(&job);
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                std::slice::from_ref(&check),
                2000
            )
            .state,
            State::Pass
        );
        let mut third_party = check.clone();
        third_party.id = 999;
        third_party.app = 12;
        third_party.slug = "external".into();
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                &[check.clone(), third_party.clone()],
                2000
            )
            .state,
            State::Pass
        );
        third_party.id = 7;
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                &[third_party],
                2000
            )
            .state,
            State::Missing
        );
        let mut stale_job = job.clone();
        stale_job.attempt = 1;
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                &[stale_job],
                std::slice::from_ref(&check),
                2000
            )
            .state,
            State::Missing
        );
        let mut wrong_suite = check.clone();
        wrong_suite.suite = 799;
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                &[wrong_suite],
                2000
            )
            .state,
            State::Missing
        );
        let mut replacement = check.clone();
        replacement.id = 8;
        replacement.conclusion = "failure".into();
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                &[check.clone(), replacement],
                2000
            )
            .state,
            State::Missing
        );
        let mut wrong_sha = check.clone();
        wrong_sha.sha = OTHER.into();
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&job),
                &[wrong_sha],
                2000
            )
            .state,
            State::Missing
        );
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                &[job.clone(), job],
                &[check],
                2000
            )
            .state,
            State::Missing
        );
    }

    #[test]
    fn check_and_job_projection_preserves_empty_end_and_app_identity() {
        let text = format!("TOTAL\t1\n7\t100\t2\t{SOURCE}\tfast\tin_progress\t\t1000\t\t7\n");
        let job = jobs(&text).unwrap().remove(0);
        assert_eq!(job.end, None);
        let text =
            format!("TOTAL\t1\n7\tfast\t{SOURCE}\t15368\tgithub-actions\t800\tin_progress\t\n");
        let check = checks(&text).unwrap().remove(0);
        assert_eq!(check.app, 15368);
        let row = observe(
            &spec("fast"),
            &sample_run(),
            &[sample_run()],
            &[job],
            &[check],
            1831,
        );
        assert_eq!(row.state, State::NotRun);
        assert_eq!(row.elapsed, Some(831));
        assert_eq!(row.remaining, Some(69));
        assert!(row.warning);
    }

    #[test]
    fn completed_checks_need_real_end_times_and_nonnegative_elapsed() {
        let mut job = sample_job("fast", 7);
        for end in [None, Some(999), Some(2100)] {
            job.end = end;
            let check = sample_check(&job);
            let row = observe(
                &spec("fast"),
                &sample_run(),
                &[sample_run()],
                std::slice::from_ref(&job),
                &[check],
                2000,
            );
            assert_eq!(row.state, State::Missing);
            assert_eq!(row.elapsed, None);
        }
    }

    #[test]
    fn warning_threshold_is_strict_and_timeout_overrun_is_visible() {
        let mut job = sample_job("fast", 7);
        let check = sample_check(&job);
        for (elapsed, warning, remaining) in [(810, false, 90), (811, true, 89), (910, true, -10)] {
            job.end = Some(1000 + elapsed);
            let row = observe(
                &spec("fast"),
                &sample_run(),
                &[sample_run()],
                std::slice::from_ref(&job),
                std::slice::from_ref(&check),
                2000,
            );
            assert_eq!(row.warning, warning);
            assert_eq!(row.remaining, Some(remaining));
        }
    }

    #[test]
    fn cancellation_replacement_neutral_and_failure_never_become_pass() {
        let mut run = sample_run();
        let mut job = sample_job("fast", 7);
        for (conclusion, state) in [
            ("cancelled", State::Fail),
            ("failure", State::Fail),
            ("timed_out", State::Fail),
            ("neutral", State::Missing),
            ("", State::Missing),
        ] {
            job.conclusion = conclusion.into();
            let check = sample_check(&job);
            assert_eq!(
                observe(
                    &spec("fast"),
                    &run,
                    std::slice::from_ref(&run),
                    std::slice::from_ref(&job),
                    &[check],
                    2000
                )
                .state,
                state
            );
        }
        run.conclusion = "cancelled".into();
        job.conclusion = "success".into();
        let check = sample_check(&job);
        assert_eq!(
            observe(
                &spec("fast"),
                &run,
                std::slice::from_ref(&run),
                &[job],
                &[check],
                2000
            )
            .state,
            State::Fail
        );
    }

    #[test]
    fn sole_cancelled_run_with_timeout_and_failure_gate_is_fail() {
        let mut run = sample_run();
        run.conclusion = "cancelled".into();
        let mut timeout = sample_job("build", 7);
        timeout.conclusion = "timed_out".into();
        let mut gate = sample_job("web-ci-gate", 8);
        gate.conclusion = "failure".into();
        let workflow = Workflow {
            path: run.path.clone(),
            gate: "web-ci-gate".into(),
            specs: vec![spec("build"), spec("web-ci-gate")],
        };
        let jobs = [timeout, gate];
        let checks: Vec<_> = jobs.iter().map(sample_check).collect();
        let (rows, accepted) = table(
            &workflow,
            &run,
            std::slice::from_ref(&run),
            &jobs,
            &checks,
            2000,
        );
        assert!(!accepted);
        assert!(rows.iter().all(|row| row.state == State::Fail));
        assert_eq!(workflow_state(&run, &rows, accepted), State::Fail);
        assert_eq!(
            observe(
                &spec("absent"),
                &run,
                std::slice::from_ref(&run),
                &jobs,
                &checks,
                2000
            )
            .state,
            State::Fail
        );
        run.conclusion = "timed_out".into();
        assert_eq!(
            observe(
                &spec("absent"),
                &run,
                std::slice::from_ref(&run),
                &jobs,
                &checks,
                2000
            )
            .state,
            State::Fail
        );
        let passed = sample_job("passed", 9);
        assert_eq!(
            observe(
                &spec("passed"),
                &run,
                std::slice::from_ref(&run),
                std::slice::from_ref(&passed),
                &[sample_check(&passed)],
                2000
            )
            .state,
            State::Fail
        );
    }

    #[test]
    fn cancelled_is_replaced_only_by_newer_same_sha_workflow_and_event_run() {
        let mut old = sample_run();
        old.conclusion = "cancelled".into();
        let mut new = old.clone();
        new.id += 1;
        new.created += 1;
        new.conclusion = "success".into();
        let mut job = sample_job("build", 7);
        job.conclusion = "cancelled".into();
        let check = sample_check(&job);
        let history = [old.clone(), new.clone()];
        assert!(replaced(&old, &history).unwrap());
        assert_eq!(
            latest(&history, &old.path, SOURCE, &old.event).unwrap(),
            Some(&new)
        );
        assert_eq!(
            observe(
                &spec("build"),
                &old,
                &history,
                std::slice::from_ref(&job),
                std::slice::from_ref(&check),
                2000
            )
            .state,
            State::Cancelled
        );
        assert_eq!(
            observe(&spec("absent"), &old, &history, &[], &[], 2000).state,
            State::Cancelled
        );
        let workflow = Workflow {
            path: old.path.clone(),
            gate: "build".into(),
            specs: vec![spec("build")],
        };
        let (rows, accepted) = table(
            &workflow,
            &old,
            &history,
            std::slice::from_ref(&job),
            std::slice::from_ref(&check),
            2000,
        );
        assert!(!accepted);
        assert_eq!(workflow_state(&old, &rows, accepted), State::Cancelled);
        let mut older = new.clone();
        older.created = old.created - 1;
        let mut other_workflow = new.clone();
        other_workflow.path = ".github/workflows/rust.yml".into();
        let mut another_attempt = old.clone();
        // Another attempt of the same run is not a newer run.
        another_attempt.attempt += 1;
        for other in [older, other_workflow, another_attempt] {
            assert!(!replaced(&old, &[old.clone(), other]).unwrap());
        }
        new.sha = OTHER.into();
        assert!(replaced(&old, &[old.clone(), new]).is_err());
        assert!(replaced(&old, &[]).is_err());
    }

    #[test]
    fn newer_different_event_does_not_replace_cancelled_run_or_hide_its_latest() {
        let mut old = sample_run();
        old.conclusion = "cancelled".into();
        let mut push = old.clone();
        push.id += 1;
        push.created += 1;
        push.event = "push".into();
        push.conclusion = "success".into();
        let history = [old.clone(), push.clone()];
        assert_eq!(
            latest(&history, &old.path, SOURCE, "pull_request").unwrap(),
            Some(&old)
        );
        assert_eq!(
            latest(&history, &old.path, SOURCE, "push").unwrap(),
            Some(&push)
        );
        assert!(!replaced(&old, &history).unwrap());
        let mut job = sample_job("build", 7);
        job.conclusion = "cancelled".into();
        assert_eq!(
            observe(
                &spec("build"),
                &old,
                &history,
                std::slice::from_ref(&job),
                &[sample_check(&job)],
                2000
            )
            .state,
            State::Fail
        );
        assert_eq!(
            observe(&spec("absent"), &old, &history, &[], &[], 2000).state,
            State::Fail
        );
        push.event.clear();
        assert!(latest(
            &[old, push],
            ".github/workflows/web.yml",
            SOURCE,
            "pull_request"
        )
        .is_err());
    }

    #[test]
    fn cancelled_run_failure_job_remains_fail_even_with_a_real_replacement() {
        let mut old = sample_run();
        old.conclusion = "cancelled".into();
        let mut new = old.clone();
        new.id += 1;
        new.created += 1;
        let history = [old.clone(), new];
        let mut job = sample_job("gate", 7);
        for conclusion in ["failure", "timed_out"] {
            job.conclusion = conclusion.into();
            for history in [std::slice::from_ref(&old), history.as_slice()] {
                let row = observe(
                    &spec("gate"),
                    &old,
                    history,
                    std::slice::from_ref(&job),
                    &[sample_check(&job)],
                    2000,
                );
                assert_eq!(row.state, State::Fail);
                assert_eq!(row.note, conclusion);
            }
        }
    }

    fn selection_fixture() -> (Workflow, Vec<Job>, Vec<Check>) {
        let mut product = spec("web-browser-shard-0");
        product.id = "workspace-browser-shard".into();
        product.conditional = true;
        let mut skipped = sample_job("workspace-browser-shard", 7);
        skipped.conclusion = "skipped".into();
        skipped.start = None;
        skipped.end = None;
        let jobs = vec![
            skipped,
            sample_job("ci-plan", 8),
            sample_job("web-ci-gate", 9),
        ];
        let checks = jobs.iter().map(sample_check).collect();
        (
            Workflow {
                path: ".github/workflows/web.yml".into(),
                gate: "web-ci-gate".into(),
                specs: vec![product, spec("ci-plan"), spec("web-ci-gate")],
            },
            jobs,
            checks,
        )
    }

    #[test]
    fn skipped_matrix_is_notrun_and_only_matching_selection_gate_validates_it() {
        let (workflow, mut jobs, mut checks) = selection_fixture();
        let (rows, pass) = table(
            &workflow,
            &sample_run(),
            &[sample_run()],
            &jobs,
            &checks,
            2000,
        );
        assert!(pass);
        assert_eq!(rows[0].state, State::NotRun);
        assert!(rows[0].note.contains("selection gate PASS"));
        jobs[2].conclusion = "failure".into();
        checks[2] = sample_check(&jobs[2]);
        assert!(
            !table(
                &workflow,
                &sample_run(),
                &[sample_run()],
                &jobs,
                &checks,
                2000
            )
            .1
        );
        checks[2].conclusion = "success".into();
        checks[2].suite = 799;
        assert!(
            !table(
                &workflow,
                &sample_run(),
                &[sample_run()],
                &jobs,
                &checks,
                2000
            )
            .1
        );
        jobs[2].conclusion = "success".into();
        checks[2] = sample_check(&jobs[2]);
        jobs[1].conclusion = "skipped".into();
        checks[1] = sample_check(&jobs[1]);
        assert!(
            !table(
                &workflow,
                &sample_run(),
                &[sample_run()],
                &jobs,
                &checks,
                2000
            )
            .1
        );
    }

    #[test]
    fn missing_or_queued_job_is_not_a_selection_skip() {
        let (workflow, mut jobs, mut checks) = selection_fixture();
        jobs.remove(0);
        checks.remove(0);
        assert!(
            !table(
                &workflow,
                &sample_run(),
                &[sample_run()],
                &jobs,
                &checks,
                2000
            )
            .1
        );
        let mut queued = sample_job("web-browser-shard-0", 7);
        queued.status = "queued".into();
        queued.conclusion.clear();
        queued.start = None;
        queued.end = None;
        checks.push(sample_check(&queued));
        jobs.push(queued);
        let (rows, pass) = table(
            &workflow,
            &sample_run(),
            &[sample_run()],
            &jobs,
            &checks,
            2000,
        );
        assert!(!pass);
        assert_eq!(rows[0].state, State::NotRun);
        assert_eq!(rows[0].elapsed, None);
    }

    #[test]
    fn dynamic_timeout_uses_each_matrix_value_and_unknowns_stay_missing() {
        let mut matrix = BTreeMap::from([("shard".into(), "b".into())]);
        let expression = "${{ matrix.shard == 'b' && 20 || 15 }}";
        assert_eq!(timeout(Some(expression), &matrix), Some(1200));
        matrix.insert("shard".into(), "a".into());
        assert_eq!(timeout(Some(expression), &matrix), Some(900));
        for expression in [
            None,
            Some("0"),
            Some("361"),
            Some("${{ fromJSON(needs.plan.outputs.timeout) }}"),
            Some("${{ matrix.shard == 'b' && 0 || 15 }}"),
        ] {
            assert_eq!(timeout(expression, &matrix), None);
        }
        assert_eq!(timeout(Some("15"), &BTreeMap::new()), Some(900));
        assert_eq!(timeout(Some(expression), &BTreeMap::new()), None);
    }

    #[test]
    fn matrix_axes_include_and_unknown_forms_are_explicit() {
        let axes = ["    strategy:", "      matrix:", "        shard: [0, 1]"];
        let values = matrices(&axes).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(
            name("web-browser-shard-${{ matrix.shard }}", &values[1]).unwrap(),
            "web-browser-shard-1"
        );
        let include = [
            "    strategy:",
            "      matrix:",
            "        include:",
            "          - check: postgres",
            "            shard: a",
            "          - check: postgres-b",
            "            shard: b",
        ];
        let values = matrices(&include).unwrap();
        assert_eq!(
            name("${{ matrix.check }}", &values[1]).unwrap(),
            "postgres-b"
        );
        for lines in [
            vec![
                "    strategy:",
                "      matrix: ${{ fromJSON(needs.plan.outputs.matrix) }}",
            ],
            vec!["    strategy:", "      matrix:", "        shard: [0, 0]"],
            vec![
                "    strategy:",
                "      matrix:",
                "        shard: [0, 1]",
                "        exclude:",
            ],
        ] {
            assert!(matrices(&lines).is_err());
        }
        assert!(name("${{ matrix.unknown }}", &values[0]).is_err());
        assert!(quoted("'").is_err());
    }

    #[test]
    fn real_head_git_source_matches_its_gate_registry_and_matrix_timeouts() {
        let cwd = repository_root();
        let workflows = sources(&cwd, &head(&cwd)).unwrap();
        assert_eq!(workflows.len(), 5);
        let rust = workflows
            .iter()
            .find(|w| w.path.ends_with("/rust.yml"))
            .unwrap();
        assert_eq!(rust.specs.len(), 14);
        for (name, seconds) in [
            ("postgres", 900),
            ("postgres-b", 1200),
            ("postgres-arm64-b", 1200),
            ("postgres-pg16-b", 1200),
            ("postgres-arm64-c", 900),
        ] {
            assert_eq!(
                rust.specs.iter().find(|s| s.name == name).unwrap().timeout,
                Some(seconds)
            );
        }
        let web = workflows
            .iter()
            .find(|w| w.path.ends_with("/web.yml"))
            .unwrap();
        assert_eq!(
            web.specs
                .iter()
                .filter(|s| s.id == "workspace-browser-shard")
                .count(),
            8
        );
        assert!(workflows
            .iter()
            .flat_map(|w| &w.specs)
            .all(|s| s.problem.is_none()));
        assert!(sources(&cwd, &"0".repeat(40)).is_err());
    }

    struct GitFixture(PathBuf);
    impl Drop for GitFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("fixture cleanup failed");
        }
    }
    fn fixture_commit(cwd: &Path) -> String {
        git(cwd, &["add", "--", ".github", "scripts"]).unwrap();
        git(
            cwd,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
        git(cwd, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    fn new_fixture() -> GitFixture {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = GitFixture(repository_root().join("target/ci-table-5-evidence").join(
            format!(
                "git-fixture-{}-{unique}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        ));
        fs::create_dir_all(&fixture.0).unwrap();
        fixture
    }

    fn head_fixture() -> GitFixture {
        let cwd = repository_root();
        let fixture = new_fixture();
        git(&fixture.0, &["-c", "init.templateDir=", "init", "-q"]).unwrap();
        git(
            &fixture.0,
            &[
                "fetch",
                "-q",
                "--depth=1",
                cwd.to_str().unwrap(),
                &head(&cwd),
            ],
        )
        .unwrap();
        git(&fixture.0, &["checkout", "-q", "--detach", "FETCH_HEAD"]).unwrap();
        assert_eq!(head(&fixture.0), head(&cwd));
        assert_eq!(
            git(&fixture.0, &["rev-parse", "--is-shallow-repository"])
                .unwrap()
                .trim(),
            "true"
        );
        fixture
    }

    // Actual Git positive/negative controls use only HEAD, available even in a
    // depth-one checkout. Registry mutations stay in memory: no generated
    // Python files or Python interpreter is needed by these fixtures.
    #[test]
    fn real_git_fixture_rejects_gate_registry_and_new_workflow_mutations_at_their_exact_shas() {
        let cwd = repository_root();
        let fixture = head_fixture();
        let positive = head(&fixture.0);
        let actual = sources(&fixture.0, &positive).unwrap();
        let expected = sources(&cwd, &head(&cwd)).unwrap();
        assert_eq!(
            actual
                .iter()
                .flat_map(|w| w.specs.clone())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .flat_map(|w| w.specs.clone())
                .collect::<Vec<_>>()
        );
        let file = fixture.0.join(".github/workflows/web.yml");
        let original = fs::read_to_string(&file).unwrap();
        let mutation = original.replace("needs: [ci-plan, web-static,", "needs: [ci-plan,");
        assert_ne!(original, mutation, "gate mutation must change source");
        fs::write(&file, mutation).unwrap();
        let negative = fixture_commit(&fixture.0);
        assert!(sources(&fixture.0, &negative)
            .unwrap_err()
            .contains("gate contract"));
        assert!(sources(&fixture.0, &positive).is_ok());
        fs::write(&file, original).unwrap();

        let registry = git(
            &fixture.0,
            &["show", &format!("{positive}:scripts/ci_selection.py")],
        )
        .unwrap();
        let paths = git(
            &fixture.0,
            &[
                "ls-tree",
                "--full-tree",
                "-r",
                "--name-only",
                &positive,
                "--",
                ".github/workflows",
            ],
        )
        .unwrap();
        let paths = format!("{paths}.github/workflows/turso-test.yml\n");
        let manual_registration = "\nTURSO_MANUAL_WORKFLOW_FILE = \"turso-test.yml\"\n";
        let parse = |registry: &str| {
            parse_sources(registry, &paths, |path| {
                git(&fixture.0, &["show", &format!("{positive}:{path}")])
            })
        };
        assert_eq!(
            parse(&format!("{registry}{manual_registration}"))
                .unwrap()
                .len(),
            5
        );
        assert!(parse(&format!(
            "{registry}{manual_registration}{manual_registration}"
        ))
        .unwrap_err()
        .contains("ambiguous"));
        assert!(parse(&format!(
            "{registry}\nTURSO_MANUAL_WORKFLOW_FILE = \"unknown.yml\"\n"
        ))
        .unwrap_err()
        .contains("unrecognized manual"));

        fs::write(
            fixture.0.join(".github/workflows/unregistered.yml"),
            "name: undeclared\n",
        )
        .unwrap();
        let negative = fixture_commit(&fixture.0);
        assert!(sources(&fixture.0, &negative)
            .unwrap_err()
            .contains("unregistered"));
    }

    #[test]
    fn unknown_source_reports_missing_without_gh() {
        const MARKER: &str = "FVOCI_CI_TABLE_UNKNOWN_SOURCE_CHILD";
        if std::env::var_os(MARKER).is_some() {
            let sha = "0".repeat(40);
            let (accepted, errors) = run_report(&sha).unwrap();
            assert!(!accepted);
            assert_eq!(errors.len(), 1);
            assert!(errors[0].starts_with("git failed"));
            assert!(!run(&sha).unwrap());
            return;
        }
        let fixture = head_fixture();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!(
                    "{}::unknown_source_reports_missing_without_gh",
                    module_path!().split_once("::").unwrap().1
                ),
                "--nocapture",
            ])
            .current_dir(&fixture.0)
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "unknown-source child: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains("| source | - | required-list | MISSING |"));
    }

    #[test]
    fn command_outside_repository_fails_closed_before_gh() {
        const MARKER: &str = "FVOCI_CI_TABLE_OUTSIDE_REPO_CHILD";
        if std::env::var_os(MARKER).is_some() {
            assert!(run(SOURCE).unwrap_err().starts_with("git failed"));
            return;
        }
        let fixture = new_fixture();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!(
                    "{}::command_outside_repository_fails_closed_before_gh",
                    module_path!().split_once("::").unwrap().1
                ),
            ])
            .current_dir(&fixture.0)
            .env(MARKER, "1")
            .env("GIT_CEILING_DIRECTORIES", fixture.0.parent().unwrap())
            .output()
            .unwrap();
        assert!(output.status.success(), "outside-repo child failed");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
