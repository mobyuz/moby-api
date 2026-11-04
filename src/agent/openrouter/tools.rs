//! Tool definitions come from the protocol's JSON Schema, so they cannot drift
//! from `ToolRequest`. A model's call is decoded by the same strict type.

use std::sync::OnceLock;

use moby_protocol::{AgentTurnRequest, ResourceKind, ToolName, ToolRequest};
use serde_json::{Map, Value, json};

const STABLE_ID: &str = "#/$defs/StableId";

struct Variant {
    name: ToolName,
    properties: Map<String, Value>,
    required: Vec<String>,
    // Parameters that hold registry IDs and the resource kind each one names.
    ids: Vec<(String, ResourceKind)>,
}

/// OpenAI-style function definitions for the tools the desktop advertised.
/// ID parameters accept only IDs from `resources`. A tool that needs an ID
/// the desktop did not register is withheld.
pub(super) fn definitions(request: &AgentTurnRequest) -> Vec<Value> {
    request
        .available_tools
        .iter()
        .filter_map(|name| {
            let variant = variants().iter().find(|variant| variant.name == *name)?;
            let mut properties = variant.properties.clone();
            let required = &variant.required;
            for (property, kind) in &variant.ids {
                let ids: Vec<&str> = request
                    .resources
                    .iter()
                    .filter(|resource| resource.kind == *kind)
                    .map(|resource| resource.id.as_str())
                    .collect();
                if ids.is_empty() {
                    if required.contains(property) {
                        return None;
                    }
                    properties.remove(property);
                } else {
                    properties.insert(property.clone(), json!({"type": "string", "enum": ids}));
                }
            }
            Some(json!({
                "type": "function",
                "function": {
                    "name": serde_json::to_value(name).ok()?,
                    "description": description(*name),
                    "parameters": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                        "additionalProperties": false,
                    },
                },
            }))
        })
        .collect()
}

/// Decodes a model's function call through `ToolRequest`, which rejects
/// unknown tools, extra fields and out-of-range values. The caller still
/// checks the result against the advertised resources.
pub(super) fn parse(
    request: &AgentTurnRequest,
    name: &str,
    arguments: &str,
) -> Result<ToolRequest, &'static str> {
    let tool: ToolName =
        serde_json::from_value(Value::String(name.into())).map_err(|_| "unknown_tool")?;
    if !request.available_tools.contains(&tool) {
        return Err("unadvertised_tool");
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(arguments) else {
        return Err("arguments_not_object");
    };
    if fields.contains_key("tool") {
        return Err("arguments_name_tool");
    }
    fields.insert("tool".into(), Value::String(name.into()));
    serde_json::from_value(Value::Object(fields)).map_err(|_| "arguments_invalid")
}

fn description(name: ToolName) -> &'static str {
    match name {
        ToolName::GitStatus => "Show the git status of a project registered on the desktop.",
        ToolName::OpenApp => "Open an application registered on the desktop.",
        ToolName::OpenProject => {
            "Open a project registered on the desktop, optionally in a registered application."
        }
        ToolName::SearchFiles => {
            "Search file names for a literal substring inside a registered search root."
        }
        ToolName::CreateReminder => {
            "Create a reminder. due_at is an RFC 3339 time with a UTC offset."
        }
    }
}

// Which resources an ID parameter can name. A StableId parameter without an
// entry here withholds its tool instead of accepting arbitrary text.
fn id_kind(property: &str) -> Option<ResourceKind> {
    match property {
        "project_id" => Some(ResourceKind::Project),
        "app_id" => Some(ResourceKind::App),
        "root_id" => Some(ResourceKind::SearchRoot),
        _ => None,
    }
}

fn variants() -> &'static [Variant] {
    static VARIANTS: OnceLock<Vec<Variant>> = OnceLock::new();
    VARIANTS.get_or_init(|| {
        let root = serde_json::to_value(schemars::schema_for!(ToolRequest)).unwrap_or_default();
        let defs = root["$defs"].as_object().cloned().unwrap_or_default();
        root["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|schema| variant(schema, &defs))
            .collect()
    })
}

fn variant(schema: &Value, defs: &Map<String, Value>) -> Option<Variant> {
    let name = serde_json::from_value(schema["properties"]["tool"]["const"].clone()).ok()?;
    let mut properties = Map::new();
    let mut ids = Vec::new();
    let mut required = Vec::new();
    for (property, value) in schema["properties"].as_object()? {
        if property == "tool" {
            continue;
        }
        if let Some(inner) = stable_id(value) {
            ids.push((property.clone(), id_kind(property)?));
            properties.insert(property.clone(), inline(inner, defs, 0));
        } else {
            properties.insert(property.clone(), inline(value, defs, 0));
        }
    }
    for property in schema["required"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
    {
        if property != "tool" {
            required.push(property.to_owned());
        }
    }
    Some(Variant {
        name,
        properties,
        required,
        ids,
    })
}

// An ID parameter is a StableId reference, optionally allowing null.
fn stable_id(schema: &Value) -> Option<&Value> {
    let is_id = |value: &Value| value["$ref"] == STABLE_ID;
    if is_id(schema) {
        return Some(schema);
    }
    schema["anyOf"]
        .as_array()?
        .iter()
        .find(|option| is_id(option))
}

// Replaces `$ref`s with their definitions. Regex patterns use look-ahead that
// providers may reject, so they stay out. Strict decoding enforces them anyway.
fn inline(schema: &Value, defs: &Map<String, Value>, depth: u8) -> Value {
    match schema {
        Value::Object(map) => {
            if let Some(definition) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .and_then(|name| defs.get(name))
                && depth < 8
            {
                return inline(definition, defs, depth + 1);
            }
            map.iter()
                .filter(|(key, value)| {
                    !(key.as_str() == "pattern" && value.is_string())
                        && !(key.as_str() == "format" && *value != "date-time")
                })
                .map(|(key, value)| (key.clone(), inline(value, defs, depth)))
                .collect::<Map<_, _>>()
                .into()
        }
        Value::Array(items) => items.iter().map(|item| inline(item, defs, depth)).collect(),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(tools: &[&str]) -> AgentTurnRequest {
        serde_json::from_value(json!({
            "messages": [{"kind": "user", "text": "hi"}],
            "available_tools": tools,
            "resources": [
                {"kind": "project", "id": "alpha", "display_name": "Alpha"},
                {"kind": "project", "id": "beta", "display_name": "Beta"},
                {"kind": "app", "id": "editor", "display_name": "Editor"},
                {"kind": "search_root", "id": "docs", "display_name": "Docs"}
            ]
        }))
        .unwrap()
    }

    fn all() -> AgentTurnRequest {
        request(&[
            "git_status",
            "open_app",
            "open_project",
            "search_files",
            "create_reminder",
        ])
    }

    #[test]
    fn every_protocol_tool_has_a_derived_definition() {
        assert_eq!(variants().len(), 5);
        let definitions = definitions(&all());
        assert_eq!(definitions.len(), 5);
        let text = serde_json::to_string(&definitions).unwrap();
        for forbidden in ["$ref", "$defs", "pattern", "uint8", "\"tool\":{"] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
        let search = &definitions[3]["function"];
        assert_eq!(search["name"], "search_files");
        assert_eq!(
            search["parameters"]["required"],
            json!(["root_id", "query", "max_results"])
        );
        assert_eq!(
            search["parameters"]["properties"]["root_id"]["enum"],
            json!(["docs"])
        );
        assert_eq!(
            search["parameters"]["properties"]["max_results"]["maximum"],
            50
        );
        assert_eq!(
            search["parameters"]["properties"]["query"]["maxLength"],
            100
        );
        assert_eq!(search["parameters"]["additionalProperties"], false);
        let reminder = &definitions[4]["function"]["parameters"]["properties"];
        assert_eq!(reminder["due_at"]["format"], "date-time");
    }

    #[test]
    fn ids_are_restricted_to_registered_resources() {
        let definitions = definitions(&all());
        let open_project = &definitions[2]["function"]["parameters"];
        assert_eq!(
            open_project["properties"]["project_id"]["enum"],
            json!(["alpha", "beta"])
        );
        assert_eq!(
            open_project["properties"]["app_id"]["enum"],
            json!(["editor"])
        );
        assert_eq!(open_project["required"], json!(["project_id"]));
    }

    #[test]
    fn tools_without_registered_ids_are_withheld() {
        let mut request = request(&["git_status", "open_project", "create_reminder"]);
        request
            .resources
            .retain(|resource| resource.kind != ResourceKind::Project);
        let definitions = definitions(&request);
        let names: Vec<&Value> = definitions.iter().map(|d| &d["function"]["name"]).collect();
        assert_eq!(names, ["create_reminder"]);

        let mut request = self::request(&["open_project"]);
        request
            .resources
            .retain(|resource| resource.kind != ResourceKind::App);
        let definitions = super::definitions(&request);
        let properties = &definitions[0]["function"]["parameters"]["properties"];
        assert!(properties.get("app_id").is_none(), "optional ID is dropped");
        assert!(super::definitions(&self::request(&[])).is_empty());
    }

    #[test]
    fn parse_uses_strict_protocol_decoding() {
        let request = all();
        let ok = parse(&request, "git_status", r#"{"project_id":"alpha"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(ok).unwrap(),
            json!({"tool": "git_status", "project_id": "alpha"})
        );
        assert!(
            parse(
                &request,
                "search_files",
                r#"{"root_id":"docs","query":"a","max_results":3}"#
            )
            .is_ok()
        );
        for (name, arguments, reason) in [
            ("shell", "{}", "unknown_tool"),
            ("git_status", "{", "arguments_not_object"),
            ("git_status", "[]", "arguments_not_object"),
            ("git_status", "", "arguments_not_object"),
            (
                "git_status",
                r#"{"project_id":"alpha","tool":"open_app"}"#,
                "arguments_name_tool",
            ),
            (
                "git_status",
                r#"{"project_id":"alpha","force":true}"#,
                "arguments_invalid",
            ),
            (
                "git_status",
                r#"{"project_id":"Alpha!"}"#,
                "arguments_invalid",
            ),
            ("git_status", r#"{}"#, "arguments_invalid"),
            (
                "search_files",
                r#"{"root_id":"docs","query":"a/b","max_results":3}"#,
                "arguments_invalid",
            ),
            (
                "search_files",
                r#"{"root_id":"docs","query":"a","max_results":"3"}"#,
                "arguments_invalid",
            ),
            (
                "search_files",
                r#"{"root_id":"docs","query":"a","max_results":51}"#,
                "arguments_invalid",
            ),
            (
                "create_reminder",
                r#"{"text":"x","due_at":"tomorrow"}"#,
                "arguments_invalid",
            ),
        ] {
            assert_eq!(
                parse(&request, name, arguments),
                Err(reason),
                "{name} {arguments}"
            );
        }
        assert_eq!(
            parse(
                &self::request(&["open_app"]),
                "git_status",
                r#"{"project_id":"alpha"}"#
            ),
            Err("unadvertised_tool")
        );
    }
}
