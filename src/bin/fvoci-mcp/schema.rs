//! Input validation against the tools' JSON Schemas (`tools.json`, generated
//! from the source zod shapes by the MCP SDK). Covers the draft-07 keywords
//! those schemas use and returns the normalized value: missing properties with
//! a `default` are filled in (zod `.default`), and properties the tool's top
//! level does not declare are dropped (zod non-strict object). Rules zod
//! enforces but JSON Schema cannot express (`\0` refinements in view query
//! text) stay with the API server, which validates every request again.

use serde_json::{Map, Value};

pub struct Validator<'a> {
    root: &'a Value,
}

impl<'a> Validator<'a> {
    pub fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// Validates the tool arguments (top-level object schema).
    pub fn validate_arguments(&self, args: &Value) -> Result<Value, String> {
        let Some(obj) = args.as_object() else {
            return Err("expected an object".into());
        };
        let props = self.root.get("properties").and_then(Value::as_object);
        let known: Map<String, Value> = obj
            .iter()
            .filter(|(k, _)| props.is_some_and(|p| p.contains_key(k.as_str())))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.validate(self.root, &Value::Object(known), "")
    }

    fn resolve(&self, reference: &str) -> Result<&'a Value, String> {
        let name = reference
            .strip_prefix("#/definitions/")
            .ok_or_else(|| format!("unsupported $ref {reference}"))?;
        self.root
            .get("definitions")
            .and_then(|d| d.get(name))
            .ok_or_else(|| format!("unknown $ref {reference}"))
    }

    fn validate(&self, schema: &'a Value, value: &Value, path: &str) -> Result<Value, String> {
        let Some(schema) = schema.as_object() else {
            return Ok(value.clone());
        };
        let at = |msg: String| {
            if path.is_empty() {
                msg
            } else {
                format!("{path}: {msg}")
            }
        };
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            return self.validate(self.resolve(reference)?, value, path);
        }
        let mut value = value.clone();
        if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
            for sub in all {
                value = self.validate(sub, &value, path)?;
            }
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(branches) = schema.get(key).and_then(Value::as_array) {
                let mut matched = None;
                let mut count = 0;
                for sub in branches {
                    if let Ok(v) = self.validate(sub, &value, path) {
                        count += 1;
                        matched.get_or_insert(v);
                    }
                }
                match matched {
                    None => return Err(at("does not match any allowed form".into())),
                    Some(_) if key == "oneOf" && count > 1 => {
                        return Err(at("matches more than one allowed form".into()))
                    }
                    Some(v) => value = v,
                }
            }
        }
        if let Some(types) = schema.get("type") {
            let allowed: Vec<&str> = match types {
                Value::String(t) => vec![t.as_str()],
                Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if !allowed.iter().any(|t| type_matches(t, &value)) {
                return Err(at(format!("expected {}", allowed.join(" or "))));
            }
        }
        if let Some(options) = schema.get("enum").and_then(Value::as_array) {
            if !options.contains(&value) {
                return Err(at(format!(
                    "expected one of {}",
                    Value::Array(options.clone())
                )));
            }
        }
        if let Some(expected) = schema.get("const") {
            if expected != &value {
                return Err(at(format!("expected {expected}")));
            }
        }
        if let Value::String(s) = &value {
            // zod (JS) string lengths count UTF-16 code units.
            let len = s.encode_utf16().count() as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
                if len < min {
                    return Err(at(format!("must be at least {min} characters")));
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                if len > max {
                    return Err(at(format!("must be at most {max} characters")));
                }
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
                // JS `\d`/`\w` are ASCII-only; so is a non-Unicode Rust regex.
                let re = regex::RegexBuilder::new(pattern)
                    .unicode(false)
                    .build()
                    .map_err(|_| "unsupported pattern".to_string())?;
                if !re.is_match(s) {
                    return Err(at("has an invalid format".into()));
                }
            }
        }
        if let Value::Number(n) = &value {
            let n = n.as_f64().unwrap_or(f64::NAN);
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
                if n < min {
                    return Err(at(format!("must be >= {min}")));
                }
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
                if n > max {
                    return Err(at(format!("must be <= {max}")));
                }
            }
        }
        if let Value::Array(items) = &value {
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
                if items.len() as u64 > max {
                    return Err(at(format!("must have at most {max} items")));
                }
            }
            if let Some(item_schema) = schema.get("items") {
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    out.push(self.validate(item_schema, item, &format!("{path}[{i}]"))?);
                }
                value = Value::Array(out);
            }
        }
        if let Value::Object(obj) = &value {
            value = Value::Object(self.validate_object(schema, obj, path)?);
        }
        Ok(value)
    }

    fn validate_object(
        &self,
        schema: &'a Map<String, Value>,
        obj: &Map<String, Value>,
        path: &str,
    ) -> Result<Map<String, Value>, String> {
        let join = |key: &str| {
            if path.is_empty() {
                key.to_string()
            } else {
                format!("{path}.{key}")
            }
        };
        let props = schema.get("properties").and_then(Value::as_object);
        let mut out = Map::new();
        if let Some(props) = props {
            for (key, sub) in props {
                match obj.get(key) {
                    Some(v) => {
                        out.insert(key.clone(), self.validate(sub, v, &join(key))?);
                    }
                    None => {
                        if let Some(default) = sub.get("default") {
                            out.insert(key.clone(), self.validate(sub, default, &join(key))?);
                        }
                    }
                }
            }
        }
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(key) {
                    return Err(format!("{}: required", join(key)));
                }
            }
        }
        for (key, v) in obj {
            if props.is_some_and(|p| p.contains_key(key)) {
                continue;
            }
            if let Some(names) = schema.get("propertyNames") {
                self.validate(names, &Value::String(key.clone()), &join(key))?;
            }
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    return Err(format!("{}: unrecognized key", join(key)));
                }
                Some(extra @ Value::Object(_)) => {
                    out.insert(key.clone(), self.validate(extra, v, &join(key))?);
                }
                _ => {
                    out.insert(key.clone(), v.clone());
                }
            }
        }
        Ok(out)
    }
}

fn type_matches(t: &str, value: &Value) -> bool {
    match t {
        "string" => value.is_string(),
        "number" => value.as_f64().is_some_and(f64::is_finite),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value
                    .as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
        }
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}
