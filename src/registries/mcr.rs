use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// The inner response from Microsoft Container Registry when requesting a list
/// of tags for a given image.
pub struct McrResponseEntry {
    pub name:            String,
    digest:              String,
    layer_zero_digest:   Option<String>,
    layer_zero_size:     Option<usize>,
    repository:          String,
    reg_hash:            String,
    operating_system:    Option<String>,
    pub architecture:    Option<String>,
    last_modified_date:  String,
    created_date:        String,
    manifest_type:       String,
    artifact_type:       String,
    size:                Option<usize>,
    annotations:         Option<String>,
    sbom_summary_digest: Option<String>,
}

pub type McrResponse = Vec<McrResponseEntry>;
