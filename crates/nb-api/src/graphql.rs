//! `POST /graphql` with the server's error keys mapped to exit-code classes.

use reqwest::Method;
use serde_json::{Value, json};

use crate::config::ENDPOINTS;
use crate::error::{NbError, Result};
use crate::http::{Http, Req};

pub const AUTH_KEYS: [&str; 4] = [
    "notAuthenticatedError",
    "invalidTokenError",
    "unauthenticated",
    "notAuthenticated",
];
pub const QUOTA_KEYS: [&str; 3] = ["userNotSubscribedError", "aiDisabled", "learnQuotaExceeded"];

/// Map a GraphQL response body to `data` or an error.
pub fn interpret(body: &Value) -> Result<Value> {
    if let Some(e0) = body
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        let key = e0
            .get("key")
            .and_then(Value::as_str)
            .or_else(|| e0.pointer("/extensions/code").and_then(Value::as_str))
            .map(str::to_string);
        let msg = e0
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("GraphQL error")
            .to_string();
        let k = key.as_deref().unwrap_or_default();
        if e0
            .get("forceLogOut")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || AUTH_KEYS.contains(&k)
        {
            return Err(NbError::auth(msg).with_key(key));
        }
        if QUOTA_KEYS.contains(&k) {
            return Err(NbError::quota(msg).with_key(key));
        }
        if k.contains("notFound") {
            return Err(NbError::not_found(msg).with_key(key));
        }
        if body.get("data").is_none_or(Value::is_null) {
            return Err(NbError::generic(msg).with_key(key));
        }
    }
    Ok(body
        .get("data")
        .filter(|d| !d.is_null())
        .cloned()
        .unwrap_or_else(|| json!({})))
}

impl Http {
    pub async fn graphql(
        &self,
        query: &str,
        variables: Value,
        operation: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({"query": query, "variables": variables});
        if let Some(op) = operation {
            body["operationName"] = json!(op);
        }
        let r = self
            .send(Req::new(Method::POST, &ENDPOINTS.graphql).json(body))
            .await?;
        let data = serde_json::from_slice::<Value>(&r.body).map_err(|_| {
            NbError::server(format!("GraphQL returned non-JSON (HTTP {})", r.status))
        })?;
        interpret(&data)
    }

    /// Run a query and take one top-level field of `data`.
    pub async fn gql_field(&self, query: &str, variables: Value, field: &str) -> Result<Value> {
        let op = query
            .split_whitespace()
            .nth(1)
            .map(|s| s.split(['(', '{']).next().unwrap_or(s).to_string());
        let mut data = self.graphql(query, variables, op.as_deref()).await?;
        Ok(data.get_mut(field).map(Value::take).unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Kind;

    #[test]
    fn error_keys_map_to_kinds() {
        let k = |b: Value| interpret(&b).unwrap_err().kind;
        assert_eq!(
            k(json!({"errors": [{"message": "x", "key": "invalidTokenError"}]})),
            Kind::Auth
        );
        assert_eq!(
            k(json!({"errors": [{"message": "x", "forceLogOut": true}], "data": {}})),
            Kind::Auth
        );
        assert_eq!(
            k(json!({"errors": [{"message": "x", "extensions": {"code": "aiDisabled"}}]})),
            Kind::Quota
        );
        assert_eq!(
            k(json!({"errors": [{"message": "x", "key": "studyMaterialsnotFound"}]})),
            Kind::NotFound
        );
        assert_eq!(
            k(json!({"errors": [{"message": "boom"}], "data": null})),
            Kind::Generic
        );
        assert_eq!(
            interpret(&json!({"errors": [{"message": "partial"}], "data": {"a": 1}})).unwrap(),
            json!({"a": 1})
        );
        assert_eq!(interpret(&json!({"data": null})).unwrap(), json!({}));
    }

    #[test]
    fn sample_graphql_error_maps() {
        let cap = include_str!("../tests/fixtures/graphql-error.json");
        let v: Value = serde_json::from_str(cap).expect("fixture");
        assert!(interpret(&v).is_err());
    }
}
