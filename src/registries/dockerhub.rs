use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
/// The inner response from Dockerhub when requesting a list of tags for a given
/// image.
pub struct DockerHubResult {
    creator:               u32,
    id:                    u32,
    pub images:            Vec<HubImage>,
    last_updated:          Option<String>,
    last_updater:          u32,
    last_updater_username: Option<String>,
    pub name:              String,
    repository:            u32,
    full_size:             u64,
    v2:                    bool,
    tag_status:            Option<String>,
    tag_last_pulled:       Option<String>,
    tag_last_pushed:       Option<String>,
    media_type:            Option<String>,
    content_type:          Option<String>,
    digest:                Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
/// The image metadata for a dockerhub image.
pub struct HubImage {
    pub architecture: String,
    features:         Option<String>,
    variant:          Option<String>,
    digest:           Option<String>,
    os:               Option<String>,
    os_features:      Option<String>,
    os_version:       Option<String>,
    size:             u64,
    status:           Option<String>,
    last_pulled:      Option<String>,
    last_pushed:      Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Default, Clone, Deserialize)]
/// The outer response from Dockerhub when requesting a list of tags for a given
/// image.
pub struct DockerHubResponse {
    count:       Option<u32>,
    pub next:    Option<String>,
    previous:    Option<String>,
    pub results: Vec<DockerHubResult>,
}
