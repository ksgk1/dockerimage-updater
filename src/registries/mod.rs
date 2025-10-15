use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use dockerhub::DockerHubResponse;
use mcr::McrResponse;

use crate::Tag;

pub mod dockerhub;
pub mod mcr;

pub const TAG_RESULT_LIMIT: usize = 2000;
pub const DURATION_HOUR_AS_SECS: u64 = 60 * 60;
pub static TAGS_CACHE: LazyLock<RwLock<HashMap<String, Vec<Tag>>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug)]
pub enum RegistryResponse {
    DockerHub(DockerHubResponse),
    MicrosoftContainerRegistry(McrResponse),
}

impl RegistryResponse {
    pub fn get_tags(&self) -> Vec<Tag> {
        match self {
            Self::DockerHub(docker_hub_response) => docker_hub_response.get_tags(),
            Self::MicrosoftContainerRegistry(mcr_response) => mcr_response.get_tags(),
        }
    }

    pub fn get_tags_for_arch(&self, arch: &str) -> Vec<Tag> {
        match self {
            Self::DockerHub(docker_hub_response) => docker_hub_response.get_tags_for_arch(arch),
            Self::MicrosoftContainerRegistry(mcr_response) => mcr_response.get_tags_for_arch(arch),
        }
    }
}

trait ResponseTagList {
    fn get_tags(&self) -> Vec<Tag>;
    fn get_tags_for_arch(&self, arch: &str) -> Vec<Tag>;
}

impl ResponseTagList for DockerHubResponse {
    fn get_tags(&self) -> Vec<Tag> {
        self.results
            .iter()
            .map(|entry| entry.name.parse().expect("Tag could be parsed."))
            .filter(|tag: &Tag| tag.major.is_some() || tag.variant.is_some())
            .collect()
    }

    fn get_tags_for_arch(&self, arch: &str) -> Vec<Tag> {
        self.results
            .iter()
            .filter(|entry| entry.images.iter().any(|image| image.architecture == arch))
            .map(|entry| entry.name.parse().expect("Tag could be parsed."))
            .filter(|tag: &Tag| tag.major.is_some() || tag.variant.is_some())
            .collect()
    }
}

impl ResponseTagList for McrResponse {
    fn get_tags(&self) -> Vec<Tag> {
        self.iter()
            .map(|entry| entry.name.parse().expect("Tag could be parsed."))
            .filter(|tag: &Tag| tag.major.is_some() || tag.variant.is_some())
            .collect()
    }

    fn get_tags_for_arch(&self, arch: &str) -> Vec<Tag> {
        self.iter()
            .filter(|entry| entry.architecture.as_ref().is_some_and(|a| a == arch))
            .map(|entry| entry.name.parse().expect("Tag could be parsed."))
            .filter(|tag: &Tag| tag.major.is_some() || tag.variant.is_some())
            .collect()
    }
}
