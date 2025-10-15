use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::builder::OsStr;
use tracing::{debug, error, info};
use walkdir::WalkDir;

use crate::cli;
use crate::docker_file::{ContainerImage, Dockerfile};
use crate::registries::{DURATION_HOUR_AS_SECS, TAGS_CACHE};
use crate::version::{Tag, is_next_major, is_next_minor};

#[derive(Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Strategy {
    #[default]
    Latest,
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
            Self::NextMinor => write!(f, "next minor"),
            Self::LatestMinor => write!(f, "latest minor"),
            Self::NextMajor => write!(f, "next major"),
            Self::LatestMajor => write!(f, "latest major"),
            Self::Latest => write!(f, "latest"),
        }
    }
}

/// Finds a newer tag in a given list starting with `starting_tag`.
///
/// May return `None` if no candidate is found with the given strategy.
pub fn find_candidate_tag(starting_tag: &Tag, tag_list: &[Tag], strategy: &Strategy) -> Option<Tag> {
    let filtered_tags: Vec<&Tag> = tag_list
        .iter()
        .filter(|tag| {
            match strategy {
                Strategy::NextMinor | Strategy::LatestMinor => is_next_minor(starting_tag, tag),
                Strategy::NextMajor | Strategy::LatestMajor => is_next_major(starting_tag, tag),
                // for the latest, we first check the major versions, if we find one we take it, if
                // we do not we try minor
                Strategy::Latest => is_next_major(starting_tag, tag) || is_next_minor(starting_tag, tag),
            }
        })
        .collect();
    if filtered_tags.is_empty() {
        debug!("No matching tags found");
        return None;
    }

    let mut result_tags: Vec<Tag> = Vec::new();
    let variant_missing_count = filtered_tags.iter().any(|t| t.variant.is_none());
    if variant_missing_count {
        debug!("No variant, can't do additional variant filtering");
        result_tags = filtered_tags.iter().map(|tag| tag.to_owned().clone()).collect();
    } else {
        let mut tag_dedup_map = HashMap::<String, String>::new();
        for tag in filtered_tags {
            let (k, v) = tag.to_key_value_pair().expect("Key value pair was created.");
            let _ = tag_dedup_map.insert(k, v);
        }

        let mut deduped_tags = Vec::<String>::new();
        for (k, v) in tag_dedup_map {
            deduped_tags.push(format!("{k}{v}"));
        }
        deduped_tags.sort();

        for deduped_tag in deduped_tags {
            result_tags.push(deduped_tag.parse().expect("Tag could be parsed."));
        }
    }

    result_tags.sort();
    for result_tag in &result_tags {
        debug!("{result_tag}");
    }
    let result = match strategy {
        Strategy::NextMajor | Strategy::NextMinor => result_tags.first().expect("At least one element is in the result."),
        Strategy::LatestMajor | Strategy::LatestMinor | Strategy::Latest => result_tags.last().expect("At least one element is in the result."),
    };
    Some(result.clone())
}

type StageIndex = usize;
type ImageUpdate = (StageIndex, Tag);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerfileUpdate {
    pub dockerfile: Dockerfile,
    pub updates:    Vec<ImageUpdate>,
}

impl DockerfileUpdate {
    pub fn apply(&self) -> Dockerfile {
        let mut result = self.dockerfile.clone();
        for (stage_index, stage) in &mut result.get_stages_mut().iter_mut().enumerate() {
            for (update_index, updated_tag) in &self.updates {
                if *update_index == stage_index {
                    stage.update_image_tag(updated_tag);
                }
            }
        }
        result
    }
}

impl Display for DockerfileUpdate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "The following updates are available:")?;
        for (stage_idx, stage) in self.dockerfile.get_stages().iter().enumerate() {
            write!(f, "{}", stage.get_image().get_name())?;
            self.updates.iter().for_each(|update| {
                if update.0 == stage_idx {
                    let _ = write!(f, " {} -> {}", stage.get_image().get_tag(), update.1);
                }
            });
            writeln!(f)?;
        }
        write!(f, "")
    }
}

/// Handles data from standard input
pub fn handle_input(input_mode: &cli::InputArguments) {
    let docker_image: ContainerImage = input_mode.input.parse().expect("Image could be parsed.");
    let mut docker_image_tags = docker_image
        .get_remote_tags(input_mode.common.tag_search_limit, input_mode.common.arch.as_ref())
        .expect("Getting tags finishes sucessful.");
    docker_image_tags.tags.sort();
    if let Some(found_tag) = find_candidate_tag(docker_image.get_tag(), &docker_image_tags.tags, &input_mode.strat) {
        info!(
            "===> Candidate tag: {}:{found_tag} (from: {}:{})",
            docker_image.get_full_tagged_name(),
            docker_image.get_full_name(),
            docker_image.get_tag()
        );
        if input_mode.common.quiet {
            println!("{}:{}", docker_image.get_full_name(), found_tag.to_string().trim_end_matches('.'));
        }
    } else {
        info!("===> No candidate found.");
        if input_mode.common.quiet {
            println!();
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
                if multi_mode.dry_run {
                    info!("The following updates will be made:\n{possible_updates}");
                }
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::{fs, io};

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, fmt};

    use crate::cli::{CommonOptions, MultiFileArguments, SingleFileArguments};
    use crate::utils::{Strategy, handle_file, handle_multi};

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
    fn multi() {
        let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let custom_format = fmt::format()
            .with_target(false)
            .with_file(true)
            .with_level(true)
            .with_line_number(true)
            .compact();
        let fmt_layer = fmt::layer().event_format(custom_format);

        tracing_subscriber::registry().with(env_filter).with(fmt_layer).init();

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
            },
        };

        let mut f = SingleFileArguments {
            file:    "./tests/testfiles/DockerfileExample1".to_owned().into(),
            strat:   Strategy::Latest,
            dry_run: true,
            common:  CommonOptions {
                arch:             None,
                tag_search_limit: Some(1000),
                debug:            false,
                quiet:            false,
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
