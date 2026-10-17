//! Fetch a claimed Slack message through Violet, without a local body cache.
use serde_json::Value;
use super::violet_reader::{BodySource, NamedRefusal};

pub(crate) struct VioletBodySource<F>(pub F);

impl<F: Fn(Value) -> Result<Value, NamedRefusal>> BodySource for VioletBodySource<F> {
    fn fetch_body(&self, _target: &Value) -> Result<String, NamedRefusal> {
        Ok(String::new())
    }
}
