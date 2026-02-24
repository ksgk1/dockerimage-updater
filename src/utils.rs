use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::builder::OsStr;
use serde::Deserialize;
use tracing::{debug, error, info};
use ureq::Agent;
use walkdir::WalkDir;

use crate::cli;
use crate::container_image::{ContainerImage, Dockerfile};
use crate::registries::{DURATION_HOUR_AS_SECS, TAGS_CACHE};
use crate::tag::Tag;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Strategy {
    #[default]
    Latest,
    NextPatch,
    LatestPatch,
    NextMinor,
    LatestMinor,
    NextMajor,
    LatestMajor,
}

// This needs to be OsStr since it is used by clap.
impl From<Strategy> for OsStr {
    fn from(value: Strategy) -> Self {
        match value {
            Strategy::Latest => Self::from("latest"),
            Strategy::NextPatch => Self::from("next-patch"),
            Strategy::LatestPatch => Self::from("latest-patch"),
            Strategy::NextMinor => Self::from("next-minor"),
            Strategy::LatestMinor => Self::from("latest-minor"),
            Strategy::NextMajor => Self::from("next-major"),
            Strategy::LatestMajor => Self::from("latest-major"),
        }
    }
}

impl Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NextPatch => write!(f, "next patch"),
            Self::LatestPatch => write!(f, "latest patch"),
            Self::NextMinor => write!(f, "next minor"),
            Self::LatestMinor => write!(f, "latest minor"),
            Self::NextMajor => write!(f, "next major"),
            Self::LatestMajor => write!(f, "latest major"),
            Self::Latest => write!(f, "latest"),
        }
    }
}

type StageIndex = usize;
type ImageUpdate = (StageIndex, Tag);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerfileUpdate {
    pub dockerfile: Dockerfile,
    pub updates:    Vec<ImageUpdate>,
}

impl DockerfileUpdate {
    pub(crate) fn apply(&self) -> Dockerfile {
        let mut result = self.dockerfile.clone();
        for (stage_index, image) in &mut result.get_base_images_mut().iter_mut().enumerate() {
            for (update_index, updated_tag) in &self.updates {
                if *update_index == stage_index {
                    image.update_image_tag(updated_tag);
                }
            }
        }
        result
    }
}

/// Handles data from standard input
pub fn handle_input(input_mode: &cli::InputArguments) {
    let docker_image: ContainerImage = input_mode.input.parse().expect("Image could be parsed.");
    let mut docker_image_tags = docker_image
        .get_remote_tags(input_mode.common.tag_search_limit, input_mode.common.arch.as_ref())
        .expect("Getting tags finishes sucessful.");
    docker_image_tags.sort();
    if let Some(found_tag) = docker_image.get_tag().find_candidate_tag(&docker_image_tags, &input_mode.strat) {
        info!(
            "===> Candidate tag: {}:{found_tag} (from: {})",
            docker_image.get_full_name(),
            docker_image.get_full_tagged_name(),
        );
        if input_mode.common.quiet {
            println!("{}:{}", docker_image.get_dockerimage_name(), found_tag.to_string().trim_end_matches('.'));
        }
    } else {
        info!("===> No candidate found.");
        if input_mode.common.quiet {
            println!();
        }
    }
}

/// Handles data from standard input
pub fn handle_overview(overview_mode: &cli::OverviewArguments) {
    let docker_image: ContainerImage = overview_mode.input.parse().expect("Image could be parsed.");
    let mut docker_image_tags = docker_image
        .get_remote_tags(overview_mode.common.tag_search_limit, overview_mode.common.arch.as_ref())
        .expect("Getting tags finishes sucessful.");
    docker_image_tags.sort();

    if overview_mode.common.quiet {
        println!("Results for:\t{}", docker_image.get_full_tagged_name());
    } else {
        info!("Results for:\t{}", docker_image.get_full_tagged_name());
    }
    // create one found tag for every Strat
    for strat in [
        Strategy::NextPatch,
        Strategy::LatestPatch,
        Strategy::NextMinor,
        Strategy::LatestMinor,
        Strategy::NextMajor,
        Strategy::LatestMajor,
    ] {
        if let Some(found_tag) = docker_image.get_tag().find_candidate_tag(&docker_image_tags, &strat) {
            if overview_mode.common.quiet {
                println!(
                    "{strat}:\t{}:{}",
                    docker_image.get_dockerimage_name(),
                    found_tag.to_string().trim_end_matches('.')
                );
            } else {
                info!("===> {strat}:\t{}:{found_tag}", docker_image.get_dockerimage_name(),);
            }
        } else if !overview_mode.common.quiet {
            info!("===> No candidate found for {strat}.");
        }
    }
}

pub fn handle_file(file_mode: &cli::SingleFileArguments) {
    let file = file_mode.file.to_string_lossy().into_owned();
    let path = Path::new(&file);
    info!("Processing dockerfile: {}", path.canonicalize().expect("Path can be canonicalised.").display());
    let mut dockerfile = Dockerfile::read(&file_mode.file).expect("File is readable and a valid dockerfile");
    dockerfile.update_images(
        !file_mode.dry_run,
        &file_mode.strat,
        file_mode.common.tag_search_limit,
        file_mode.common.arch.as_ref(),
    );
}

/// Handling function that will handle multiple files at once, with a given
/// ignore for single files or specific images.
pub fn handle_multi(multi_mode: &cli::MultiFileArguments) {
    let folder = multi_mode.folder.to_str().unwrap_or_default().to_owned();
    let path = Path::new(&folder);
    info!("Processing folder: {}", path.canonicalize().expect("Path can be canonicalised.").display());
    let mut dockerfiles_to_process = Vec::<String>::new();
    for entry in WalkDir::new(path).into_iter().filter_map(std::result::Result::ok) {
        if entry.file_name().to_string_lossy().to_ascii_lowercase().starts_with("dockerfile") {
            dockerfiles_to_process.push(entry.path().display().to_string());
        }
    }
    if !multi_mode.exclude_file.is_empty() {
        info!("Ignoring files: {:?}", &multi_mode.exclude_file);
        for excluded in &multi_mode.exclude_file {
            dockerfiles_to_process.retain(|f| !f.ends_with(excluded));
        }
    }
    info!("Found files: {dockerfiles_to_process:?}");
    for dockerfile_to_process in &dockerfiles_to_process {
        match Dockerfile::read(&PathBuf::from(dockerfile_to_process)) {
            Ok(dockerfile) => {
                let ignored_images: Vec<ContainerImage> = multi_mode
                    .ignore_versions
                    .iter()
                    .map(|image| image.parse().expect("Image could be parsed."))
                    .collect();
                if !ignored_images.is_empty() {
                    debug!("Skipping image updates:");
                    for image in &ignored_images {
                        debug!("\t\t{}", image.get_name());
                    }
                }
                let possible_updates = dockerfile.generate_image_updates(
                    &multi_mode.strat,
                    multi_mode.common.tag_search_limit,
                    multi_mode.common.arch.as_ref(),
                    &ignored_images,
                );
                let dockerfile_updated = possible_updates.apply();
                if multi_mode.dry_run {
                    info!(
                        "Updated dockerfile `{}` would look like:\n{dockerfile_updated}",
                        dockerfile.get_path().expect("Path is not empty.").display()
                    );
                } else {
                    let _ = dockerfile_updated.write();
                }
            }
            Err(e) => {
                error!("Could not read dockerfile: `{dockerfile_to_process}` with error: {e}");
            }
        }
    }
}

/// Reads already fetched data into the program's memory (global variable).
///
/// Cache invalidates after `DURATION_HOUR_AS_SECS` seconds, to ensure the data
/// is up to date.
pub fn extract_cache_from_file(full_name: &str, tags: &mut Vec<Tag>, cache_file_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if fs::exists(cache_file_name)? {
        debug!("Cache file `{cache_file_name}`exists.");
        let file_metadata = fs::metadata(cache_file_name).expect("Cache file exists");
        if let Ok(time) = file_metadata.modified() {
            if time.elapsed().expect("No error with systime occured.") < Duration::new(DURATION_HOUR_AS_SECS, 0) {
                let cache_file_content = fs::read_to_string(cache_file_name).expect("File exists for reading.");
                if let Ok(read_tags) = &serde_json::from_str(&cache_file_content) {
                    tags.clone_from(read_tags);
                    let mut cache = TAGS_CACHE.write().expect("Cache can be written.");
                    if cache.insert(full_name.to_string(), tags.clone()).is_none() {
                        debug!("Populated cache successfully.");
                    }
                } else {
                    error!("Could not read tags from file");
                }
            } else {
                info!("Cache file is older than {DURATION_HOUR_AS_SECS} seconds. Fetching new data instead.");
            }
        }
    } else {
        info!("No cache file exists under `{cache_file_name}`, fetching info from docker hub.");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagRefListResponse {
    refs:       Vec<String>,
    _cache_key: String,
}

pub fn check_update() {
    let agent = Agent::new_with_defaults();

    if let Ok(mut response) = agent
        .get("https://github.com/ksgk1/dockerimage-updater/refs?type=tag")
        .header("Accept", "application/json")
        .call()
    {
        let current_tag: Tag = VERSION.parse().expect("We used a valid semver version for our own project.");
        let response_body = &response.body_mut().read_to_string().expect("Well-formed response");
        let parsed_response: TagRefListResponse = serde_json::from_str(response_body).expect("Well-formed json with expected fields");
        let ref_tags: Vec<Tag> = parsed_response
            .refs
            .iter()
            .filter_map(|tag| tag.strip_prefix("v").unwrap_or(tag).parse().ok())
            .collect();
        if ref_tags.iter().any(|t| t > &current_tag) {
            println!(
                "A newer version is available: v{}\nPlease check: https://github.com/ksgk1/dockerimage-updater/releases",
                ref_tags.iter().max().expect("We have a max version")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::{fs, io};

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, fmt};

    use crate::cli::{CommonOptions, InputArguments, MultiFileArguments, SingleFileArguments};
    use crate::utils::{Strategy, handle_file, handle_input, handle_multi};

    fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
        fs::create_dir_all(&dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
            } else {
                fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
            }
        }
        Ok(())
    }

    #[test]
    fn input_single_multi() {
        let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let custom_format = fmt::format()
            .with_target(false)
            .with_file(true)
            .with_level(true)
            .with_line_number(true)
            .compact();
        let fmt_layer = fmt::layer().event_format(custom_format);
        tracing_subscriber::registry().with(env_filter).with(fmt_layer).init();

        let mut i = InputArguments {
            input:  "clamav/clamav:1.5.1-11_base".into(),
            strat:  Strategy::Latest,
            common: CommonOptions {
                arch:             None,
                tag_search_limit: Some(1000),
                debug:            false,
                quiet:            false,
                color:            false,
            },
        };
        handle_input(&i);
        i.common.quiet = true;
        handle_input(&i);
        i.input = "clamav/clamav:1.5.1-99_base".into();
        handle_input(&i);

        let mut f = SingleFileArguments {
            file:    "./tests/testfiles/DockerfileExample1".to_owned().into(),
            strat:   Strategy::Latest,
            dry_run: true,
            common:  CommonOptions {
                arch:             None,
                tag_search_limit: Some(1000),
                debug:            false,
                quiet:            false,
                color:            false,
            },
        };

        let mut m = MultiFileArguments {
            folder:          "./tests/testfiles".into(),
            strat:           Strategy::Latest,
            dry_run:         true,
            exclude_file:    vec!["./tests/testfiles/DockerfileExample1".to_owned()],
            ignore_versions: vec!["node:8.0-alpine".to_owned()],
            common:          CommonOptions {
                arch:             None,
                tag_search_limit: Some(1000),
                debug:            false,
                quiet:            false,
                color:            false,
            },
        };

        handle_multi(&m);
        handle_file(&f);

        // copy testfiles folder
        assert!(copy_dir_all("./tests/testfiles", "./tests/testfiles.backup").is_ok());
        m.dry_run = false;
        f.dry_run = false;
        handle_multi(&m);
        handle_file(&f);
        m.common.arch = Some("amd64".to_owned());
        handle_multi(&m);
        handle_file(&f);
        f.common.arch = Some("amd64".to_owned());
        // restore testfiles folder
        let _ = fs::remove_dir_all("./tests/testfiles");
        let _ = fs::rename("./tests/testfiles.backup", "./tests/testfiles").is_ok();
    }
}
