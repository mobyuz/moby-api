use std::collections::BTreeMap;

use schemars::{
    JsonSchema,
    generate::SchemaSettings,
    transform::{ReplaceConstValue, Transform},
};
use serde_json::Value;
use utoipa::openapi::{Content, Ref, RefOr, Response, ResponseBuilder, Schema};

pub(crate) fn json_response(description: &str, schema: &str) -> Response {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name(schema))),
        )
        .build()
}

pub(crate) fn add_schema<T: JsonSchema>(components: &mut BTreeMap<String, RefOr<Schema>>) {
    let schema = SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.definitions_path = "/components/schemas".into();
            settings.meta_schema = None;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let mut root = serde_json::to_value(schema).expect("serializable JSON schema");
    if let Some(definitions) = root
        .as_object_mut()
        .expect("root schema object")
        .remove("components")
    {
        for (name, schema) in definitions["schemas"]
            .as_object()
            .expect("component schemas")
        {
            components
                .entry(name.clone())
                .or_insert_with(|| utoipa_schema(schema.clone()));
        }
    }
    components.insert(T::schema_name().into_owned(), utoipa_schema(root));
}

fn utoipa_schema(value: Value) -> RefOr<Schema> {
    let mut schema = schemars::Schema::try_from(value).expect("valid JSON schema");
    // Utoipa represents a constant as a single-value enum.
    ReplaceConstValue::default().transform(&mut schema);
    serde_json::from_value(schema.into()).expect("OpenAPI-compatible JSON schema")
}
