//! Secret portion of administrator-owned shared acquisition profiles.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcquisitionSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}
