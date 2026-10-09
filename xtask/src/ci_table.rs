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
    let mut registered = BTreeMap::new();
    for line in block(&registry, "WORKFLOW_JOBS:")? {
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
    for line in block(&registry, "WORKFLOW_YAML:")? {
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
        let source = git(cwd, &["show", &format!("{sha}:{path}")])?;
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
            &["show", &format!("{SOURCE}:.github/workflows/web.yml")],
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
        let cwd = repository_root();
        for sha in [REVIEWED_PR_SOURCE, REVIEWED_MAIN_SOURCE] {
            let workflows = sources(&cwd, sha).unwrap();
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

    // This is the task's read-only real-input acceptance check, separate from
    // the pure metadata regressions above. It does not dispatch CI or treat an
    // incomplete/failed observed run as successful CI evidence.
    // n1: requires network access and an already authenticated gh CLI.
    #[test]
    fn real_read_only_tables_for_fixed_pr_and_main_have_zero_parsing_errors() {
        println!("n1: network/gh required; no ignored or skipped tests");
        for sha in [REVIEWED_PR_SOURCE, REVIEWED_MAIN_SOURCE] {
            let (accepted, errors) = run_report(sha).expect("read-only table query failed");
            assert!(
                errors.is_empty(),
                "source/API parsing errors for {sha}: {errors:?}"
            );
            println!("fixed_input={sha} parsing_errors=0 observed_acceptance={accepted}");
        }
    }

    // n1: read-only real GitHub metadata, network and authenticated gh required.
    // A fixture success never substitutes for this run's observed failure.
    #[test]
    fn real_read_only_cancelled_web_run_37881810443_is_fail() {
        let cwd = repository_root();
        let query = r#""TOTAL\t1", ([.id,.path,.head_sha,.run_attempt,.status,(.conclusion // ""),.check_suite_id,(.created_at|fromdateiso8601),.event] | @tsv)"#;
        let endpoint = format!("{REPO}/actions/runs/37881810443");
        let run = runs(&api(&cwd, &endpoint, query, false).unwrap())
            .unwrap()
            .remove(0);
        assert_eq!(run.sha, REVIEWED_PR_SOURCE);
        assert_eq!(run.path, ".github/workflows/web.yml");
        assert_eq!(run.event, "pull_request");
        assert_eq!(run.conclusion, "cancelled");
        let history = runs(
            &api(
                &cwd,
                &format!("{REPO}/actions/runs?head_sha={}&per_page=100", run.sha),
                RUN_QUERY,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let workflows = sources(&cwd, &run.sha).unwrap();
        let workflow = workflows
            .iter()
            .find(|workflow| workflow.path == run.path)
            .unwrap();
        let jobs = jobs(
            &api(
                &cwd,
                &format!(
                    "{REPO}/actions/runs/{}/attempts/{}/jobs?per_page=100",
                    run.id, run.attempt
                ),
                JOB_QUERY,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let checks = checks(
            &api(
                &cwd,
                &format!(
                    "{REPO}/commits/{}/check-runs?filter=all&per_page=100",
                    run.sha
                ),
                CHECK_QUERY,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let current = runs(&api(&cwd, &endpoint, query, false).unwrap()).unwrap();
        assert_eq!(
            current.as_slice(),
            std::slice::from_ref(&run),
            "run snapshot changed"
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (rows, accepted) = table(workflow, &run, &history, &jobs, &checks, now);
        let state = workflow_state(&run, &rows, accepted);
        println!("read_only_run_table_begin=37881810443");
        println!("n1: network/gh required; read-only GET");
        println!(
            "tested_sha={}\nxtask_commit={}\nqueried_at_utc={}",
            run.sha,
            git(&cwd, &["rev-parse", "HEAD"]).unwrap().trim(),
            command(&cwd, "date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])
                .unwrap()
                .trim()
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
            print_row(&workflow.path, spec, Some(&run), row);
        }
        println!("read_only_run_table_end=37881810443");
        assert!(!accepted);
        assert_eq!(state, State::Fail);
        assert!(rows.iter().any(|row| row.state == State::Fail));
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
    fn real_fixed_git_source_matches_its_gate_registry_and_matrix_timeouts() {
        let cwd = repository_root();
        let workflows = sources(&cwd, SOURCE).unwrap();
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

    #[test]
    fn real_git_fixture_rejects_gate_registry_and_new_workflow_mutations_at_their_exact_shas() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let cwd = repository_root();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = GitFixture(cwd.join("target/ci-table-1-evidence").join(format!(
            "git-fixture-{}-{unique}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(&fixture.0).unwrap();
        git(&fixture.0, &["-c", "init.templateDir=", "init", "-q"]).unwrap();
        let paths = git(
            &cwd,
            &[
                "ls-tree",
                "--full-tree",
                "-r",
                "--name-only",
                SOURCE,
                "--",
                ".github/workflows",
            ],
        )
        .unwrap();
        for path in paths
            .lines()
            .chain(std::iter::once("scripts/ci_selection.py"))
        {
            let contents = git(&cwd, &["show", &format!("{SOURCE}:{path}")]).unwrap();
            let file = fixture.0.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
        }
        let positive = fixture_commit(&fixture.0);
        let actual = sources(&fixture.0, &positive).unwrap();
        let expected = sources(&cwd, SOURCE).unwrap();
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
        fs::write(
            &file,
            original.replace("needs: [ci-plan, web-static,", "needs: [ci-plan,"),
        )
        .unwrap();
        let negative = fixture_commit(&fixture.0);
        assert!(sources(&fixture.0, &negative)
            .unwrap_err()
            .contains("gate contract"));
        // The positive SHA remains valid even after the working tree changes.
        assert!(sources(&fixture.0, &positive).is_ok());
        fs::write(&file, original).unwrap();
        let registry_file = fixture.0.join("scripts/ci_selection.py");
        let registry = fs::read_to_string(&registry_file).unwrap();
        let manual_registration = "\nTURSO_MANUAL_WORKFLOW_FILE = \"turso-test.yml\"\n";
        fs::write(&registry_file, format!("{registry}{manual_registration}")).unwrap();
        let turso = git(
            &cwd,
            &[
                "show",
                &format!("{REVIEWED_PR_SOURCE}:.github/workflows/turso-test.yml"),
            ],
        )
        .unwrap();
        fs::write(fixture.0.join(".github/workflows/turso-test.yml"), turso).unwrap();
        let manual = fixture_commit(&fixture.0);
        // Registered manual-only Turso does not become a stable selection gate.
        assert_eq!(sources(&fixture.0, &manual).unwrap().len(), 5);
        fs::write(
            &registry_file,
            format!("{registry}{manual_registration}{manual_registration}"),
        )
        .unwrap();
        let duplicate_manual = fixture_commit(&fixture.0);
        assert!(sources(&fixture.0, &duplicate_manual)
            .unwrap_err()
            .contains("ambiguous"));
        fs::write(
            &registry_file,
            format!("{registry}\nTURSO_MANUAL_WORKFLOW_FILE = \"unknown.yml\"\n"),
        )
        .unwrap();
        let unknown_manual = fixture_commit(&fixture.0);
        assert!(sources(&fixture.0, &unknown_manual)
            .unwrap_err()
            .contains("unrecognized manual"));
        fs::write(&registry_file, format!("{registry}{manual_registration}")).unwrap();
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
}
