//! The API only grows (docs/integrating.md, "Things that will not change under
//! you"). `api-v1.json` lists every route and every field the server has ever
//! described under `/api/v1`. A release may add to them; it may not take one
//! away, rename it or change its type, because somebody's program is built on
//! it. A change that cannot be made by adding goes into `/api/v2`, beside v1.
//!
//! Adding a route or a field means adding it to `api-v1.json` too, so that it is
//! a promise somebody made on purpose: run the tests once with `WRITE_API_V1=1`,
//! which writes the list as the server describes it now, but only after the
//! check below has passed.

use std::collections::BTreeMap;

use serde_json::{json, Value};

const PROMISED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/api-v1.json");

// Every route as "METHOD /path", and every field of every schema with its type.
fn promises(document: &Value) -> (Vec<String>, BTreeMap<String, String>) {
    let mut routes = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for method in item.as_object().expect("a path's methods").keys() {
            if ["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                routes.push(format!("{} {path}", method.to_uppercase()));
            }
        }
    }
    routes.sort();
    let mut fields = BTreeMap::new();
    for (schema, definition) in document["components"]["schemas"].as_object().expect("schemas") {
        if let Some(properties) = definition["properties"].as_object() {
            for (field, def) in properties {
                fields.insert(format!("{schema}.{field}"), kind(def));
            }
        }
    }
    (routes, fields)
}

fn kind(def: &Value) -> String {
    let named = |d: &Value| match (d["type"].as_str(), d["$ref"].as_str()) {
        (Some(t), _) => t.to_string(),
        (None, Some(r)) => r.rsplit('/').next().unwrap_or(r).to_string(),
        _ => "any".to_string(),
    };
    if def["type"] == "array" {
        format!("array of {}", named(&def["items"]))
    } else {
        named(def)
    }
}

#[test]
fn nothing_the_api_has_promised_is_taken_away_renamed_or_changed() {
    let (routes, fields) = promises(&hyperview_server::openapi::document("Example Steel"));
    let promised: Value = serde_json::from_str(&std::fs::read_to_string(PROMISED).expect("api-v1.json"))
        .expect("api-v1.json is JSON");

    let mut broken = Vec::new();
    for route in promised["routes"].as_array().expect("routes") {
        let route = route.as_str().expect("a route");
        if !routes.iter().any(|r| r == route) {
            broken.push(format!("route taken away: {route}"));
        }
    }
    for (field, was) in promised["fields"].as_object().expect("fields") {
        let was = was.as_str().expect("a type");
        match fields.get(field) {
            None => broken.push(format!("field taken away or renamed: {field}")),
            Some(now) if now != was => broken.push(format!("field changed: {field} was {was}, is now {now}")),
            Some(_) => {}
        }
    }
    assert!(
        broken.is_empty(),
        "A release must not break a program built on /api/v1. Add instead, or put the change in /api/v2:\n  {}",
        broken.join("\n  ")
    );

    let now = json!({ "routes": routes, "fields": fields });
    if std::env::var_os("WRITE_API_V1").is_some() {
        std::fs::write(PROMISED, serde_json::to_string_pretty(&now).expect("JSON") + "\n").expect("write api-v1.json");
        return;
    }
    let added: Vec<String> = routes
        .iter()
        .filter(|r| !promised["routes"].as_array().expect("routes").iter().any(|p| p == *r))
        .cloned()
        .chain(fields.keys().filter(|f| promised["fields"].get(f.as_str()).is_none()).cloned())
        .collect();
    assert!(
        added.is_empty(),
        "New in the API and not yet promised in api-v1.json (run the tests once with WRITE_API_V1=1):\n  {}",
        added.join("\n  ")
    );
}
