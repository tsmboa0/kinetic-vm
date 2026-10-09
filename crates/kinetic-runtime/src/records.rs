//! Local operation records.
//!
//! A builder lists the fields they want kept. `monad_attest` checks the
//! caller's JSON against that schema, writes one canonical line, and signs
//! those same bytes. Chat ids, wallets, and access codes are not added.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use kinetic_config::schema::{OperationFieldKind, RecordsConfig};
use serde_json::{Map, Value};

static APPEND_LOCK: Mutex<()> = Mutex::new(());

/// What `monad_attest` should sign.
#[derive(Debug, PartialEq, Eq)]
pub enum PreparedOperation {
    /// No record schema is configured. Sign the caller's parameters as given.
    Off,
    /// Canonical one-line JSON. Sign these bytes and append this line.
    Line(String),
}

/// Check `params` against the builder's schema.
///
/// An empty schema returns [`PreparedOperation::Off`]. A configured schema
/// returns the exact line that will be stored and signed. Key order in the
/// caller's JSON does not change that line.
pub fn prepare_operation(
    schema: &RecordsConfig,
    params: &str,
) -> Result<PreparedOperation, String> {
    if !schema.recording() {
        return Ok(PreparedOperation::Off);
    }
    schema.check()?;
    let value: Value = serde_json::from_str(params).map_err(|_| {
        "params must be a JSON object when record fields are configured".to_string()
    })?;
    let Some(object) = value.as_object() else {
        return Err("params must be a JSON object when record fields are configured".to_string());
    };
    for key in object.keys() {
        if !schema.fields.iter().any(|field| field.name == *key) {
            return Err(format!(
                "params includes {key}, which is not in the record schema"
            ));
        }
    }
    let mut canonical = Map::new();
    for field in &schema.fields {
        match object.get(&field.name) {
            None if field.required => {
                return Err(format!("params is missing {}", field.name));
            }
            None => {}
            Some(value) if !value_matches(value, field.kind) => {
                return Err(format!("{} must be a {}", field.name, field.kind.as_str()));
            }
            Some(value) => {
                canonical.insert(field.name.clone(), canonicalize(value));
            }
        }
    }
    let line = serde_json::to_string(&Value::Object(sorted_object(canonical)))
        .map_err(|_| "the operation record could not be encoded".to_string())?;
    if line.contains('\n') || line.contains('\r') {
        return Err("the operation record must be one line".to_string());
    }
    Ok(PreparedOperation::Line(line))
}

/// Short field list for the attest tool's description.
#[must_use]
pub fn field_summary(schema: &RecordsConfig) -> String {
    schema
        .fields
        .iter()
        .map(|field| {
            let presence = if field.required {
                "required"
            } else {
                "optional"
            };
            format!("{} ({}, {presence})", field.name, field.kind.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Append one canonical line to the operations file.
///
/// The directory is mode `0700` and the file is mode `0600` on Unix.
pub fn append_operation_line(path: &Path, line: &str) -> Result<(), String> {
    if line.is_empty() || line.contains('\n') || line.contains('\r') {
        return Err("the operation record must be one line".to_string());
    }
    let _guard = APPEND_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create the records directory: {err}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|err| format!("could not protect the records directory: {err}"))?;
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("could not open the operations file: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("could not protect the operations file: {err}"))?;
    }
    writeln!(file, "{line}")
        .map_err(|err| format!("could not write the operation record: {err}"))?;
    file.sync_all()
        .map_err(|err| format!("could not store the operation record: {err}"))?;
    Ok(())
}

fn value_matches(value: &Value, kind: OperationFieldKind) -> bool {
    match kind {
        OperationFieldKind::String => value.is_string(),
        OperationFieldKind::Number => value.is_number(),
        OperationFieldKind::Boolean => value.is_boolean(),
        OperationFieldKind::Object => value.is_object(),
        OperationFieldKind::Array => value.is_array(),
    }
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(sorted_object(
            object
                .iter()
                .map(|(key, child)| (key.clone(), canonicalize(child)))
                .collect(),
        )),
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

fn sorted_object(object: Map<String, Value>) -> Map<String, Value> {
    let mut keys: Vec<String> = object.keys().cloned().collect();
    keys.sort();
    let mut sorted = Map::new();
    for key in keys {
        if let Some(value) = object.get(&key) {
            sorted.insert(key, value.clone());
        }
    }
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinetic_config::schema::OperationRecordField;

    fn schema(fields: Vec<OperationRecordField>) -> RecordsConfig {
        RecordsConfig { fields }
    }

    fn field(name: &str, kind: OperationFieldKind, required: bool) -> OperationRecordField {
        OperationRecordField {
            name: name.to_string(),
            kind,
            required,
        }
    }

    #[test]
    fn an_empty_schema_leaves_recording_off() {
        let prepared = prepare_operation(&RecordsConfig::default(), "slot-1").expect("prepared");
        assert_eq!(prepared, PreparedOperation::Off);
    }

    #[test]
    fn the_canonical_line_sorts_keys_and_drops_nothing_required() {
        let records = schema(vec![
            field("subject", OperationFieldKind::String, true),
            field("count", OperationFieldKind::Number, true),
            field("note", OperationFieldKind::String, false),
            field("detail", OperationFieldKind::Object, false),
        ]);
        let first = prepare_operation(
            &records,
            r#"{"count":2,"subject":"soil","detail":{"z":1,"a":true}}"#,
        )
        .expect("first");
        let second = prepare_operation(
            &records,
            "{\n  \"subject\" : \"soil\",\n  \"count\" : 2,\n  \"detail\" : { \"a\" : true, \"z\" : 1 }\n}",
        )
        .expect("second");
        assert_eq!(
            first,
            PreparedOperation::Line(
                r#"{"count":2,"detail":{"a":true,"z":1},"subject":"soil"}"#.to_string()
            )
        );
        assert_eq!(first, second);
    }

    #[test]
    fn a_record_outside_the_schema_is_refused() {
        let records = schema(vec![field("subject", OperationFieldKind::String, true)]);
        let missing = prepare_operation(&records, r#"{}"#).expect_err("missing");
        assert!(missing.contains("subject"));
        let extra =
            prepare_operation(&records, r#"{"subject":"a","chat_id":"1"}"#).expect_err("extra");
        assert!(extra.contains("chat_id"));
        let wrong = prepare_operation(&records, r#"{"subject":1}"#).expect_err("type");
        assert!(wrong.contains("string"));
        let not_object = prepare_operation(&records, r#""soil""#).expect_err("object");
        assert!(not_object.contains("JSON object"));
    }

    #[test]
    fn a_bad_schema_is_refused_before_a_line_is_made() {
        let records = schema(vec![
            field("subject", OperationFieldKind::String, true),
            field("subject", OperationFieldKind::String, true),
        ]);
        let error = prepare_operation(&records, r#"{"subject":"a"}"#).expect_err("duplicate");
        assert!(error.contains("more than once"));
        assert!(
            schema(vec![field("1slot", OperationFieldKind::String, true)])
                .check()
                .is_err()
        );
    }

    #[test]
    fn lines_append_in_order_and_the_file_stays_private() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("records").join("operations.jsonl");
        append_operation_line(&path, r#"{"subject":"a"}"#).expect("first");
        append_operation_line(&path, r#"{"subject":"b"}"#).expect("second");
        let stored = std::fs::read_to_string(&path).expect("read");
        assert_eq!(stored, "{\"subject\":\"a\"}\n{\"subject\":\"b\"}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&path).expect("file").permissions().mode() & 0o777;
            let dir_mode = std::fs::metadata(path.parent().expect("parent"))
                .expect("dir")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            assert_eq!(dir_mode, 0o700);
        }
    }
}
