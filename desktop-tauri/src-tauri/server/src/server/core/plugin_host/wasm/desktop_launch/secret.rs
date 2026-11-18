use std::ops::{Deref, DerefMut};

use serde_json::Value;

pub struct Secret(pub Value);

impl Deref for Secret {
    type Target = Value;
    fn deref(&self) -> &Value { &self.0 }
}

impl DerefMut for Secret {
    fn deref_mut(&mut self) -> &mut Value { &mut self.0 }
}

impl Drop for Secret {
    fn drop(&mut self) { super::wipe(&mut self.0); }
}
