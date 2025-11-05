use std::fmt::{Display, Formatter};
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use std::{fs, vec};

use tracing::{debug, error, info};
use ureq::Agent;

use crate::registries::dockerhub::DockerHubResponse;
use crate::registries::mcr::McrResponseEntry;
use crate::registries::{self, RegistryResponse, TAG_RESULT_LIMIT, TAGS_CACHE};
use crate::utils::{DockerfileUpdate, Strategy, extract_cache_from_file, find_candidate_tag};
use crate::version::{Tag, VersionTags};

/// The dockerfile related errors, that may occur during parsing.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("No path was set for the given dockerfile.")]
    MissingPath,
    #[error("This seems to be an invalid dockerfile.")]
    EmptyFile,
    #[error("Could not find image: `{0}` in the docker hub.")]
    ImageNotFound(String),
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// Parsing related errors
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("Could not parse instruction: `{0}` on line: {1}.")]
    InvalidInstruction(String, usize),
    #[error("Image name is empty.")]
    EmptyImage,
    #[error("Could not parse tag: `{0}`.")]
    InvalidTag(String),
    #[error("Could arse dockerhub response.")]
    InvalidDockerhubResponse,
}

/// Each dockerfile can consist of one or more stages, therefore we structure
/// the data into stages.
///
/// Each stage consists of an image that is used, an optional name for the stage
/// and a set of instructions until a new stage comes, or the file ends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stage {
    image:        ContainerImage,
    name:         Option<String>,
    instructions: Vec<DockerInstruction>,
}

impl Stage {
    /// Sets the image of a given stage.
    pub fn set_image(&mut self, new_image: &ContainerImage) {
        new_image.clone_into(&mut self.image);
    }

    /// Updates the name of a stage.
    pub fn update_name(&mut self, new_name: &str) {
        if !new_name.is_empty() {
            self.name = Some(new_name.to_owned());
        }
    }

    /// Gets a shared reference to the image in the stage.
    pub const fn get_image(&self) -> &ContainerImage {
        &self.image
    }

    /// Updates the tag of a stage's image.
    pub fn update_image_tag(&mut self, new_tag: &Tag) {
        if self.instructions.iter().any(DockerInstruction::is_from_type) {
            for mut instruction in &mut self.instructions {
                if let DockerInstruction::From(image, _) = &mut instruction {
                    image.set_tag(new_tag);
                }
            }
            self.image.set_tag(new_tag);
        }
    }
}

impl Display for Stage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for instruction in &self.instructions {
            writeln!(f, "{instruction}")?;
        }
        write!(f, "")
    }
}

/// A dockerfile consists of a set of stages and an optional path, in case it
/// was ready from disk and not from standard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dockerfile {
    stages: Vec<Stage>,
    /// Original path of the file, in case it shall be written again.
    path:   Option<PathBuf>,
}

impl Dockerfile {
    /// # Returns
    ///
    /// * `Ok(Self)` - The parsed result, with its path set to the input path.
    /// * `Err(Box<dyn std::error::Error>)` - An error if reading or parsing
    ///   fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be read
    /// or if the contents cannot be parsed.
    pub(crate) fn read<P>(path: &P) -> Result<Self, Box<dyn std::error::Error>>
    where
        P: AsRef<Path>,
    {
        let content = fs::read_to_string(path)?;
        match Self::parse(&content) {
            Ok(mut result) => {
                result.set_path(path);
                Ok(result)
            }
            Err(e) => Err(e),
        }
    }

    /// # Returns
    ///
    /// This function will return an `Option<Pathbuf>`. This will contain the
    /// original path of the file, if it was read as file It will be `None`
    /// if the file was ready from regular input.
    pub const fn get_path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    #[allow(unused)]
    /// For testing purposes only
    fn get_path_str(&self) -> Option<String> {
        self.path.as_ref().and_then(|p| {
            let s = p.display().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
    }

    /// For testing purposes only
    fn set_path<P>(&mut self, path: P)
    where
        P: AsRef<Path>,
    {
        let pathbuf = PathBuf::from(path.as_ref());
        self.path = Option::from(pathbuf);
    }

    /// For testing purposes only
    #[allow(dead_code)]
    fn clear_path(&mut self) {
        self.path = None;
    }

    /// # Returns
    ///
    /// This function will return a copy of the stages in the given dockerfile.
    pub(crate) fn get_stages(&self) -> &[Stage] {
        &self.stages
    }

    /// # Returns
    ///
    /// This function will return a mutable reference to the stages in a given
    /// dockerfile.
    pub(crate) const fn get_stages_mut(&mut self) -> &mut Vec<Stage> {
        &mut self.stages
    }

    /// # Returns
    ///
    /// This function will return a mutable references to the images in a given
    /// dockerfile.
    pub fn get_base_images_mut(&mut self) -> Vec<&mut ContainerImage> {
        self.get_stages_mut()
            .iter_mut()
            .map(|stage| &mut stage.image)
            .collect::<Vec<&mut ContainerImage>>()
    }

    /// # Returns
    ///
    /// * `Ok(Self)` - The parsed result.
    /// * `Err(Box<dyn std::error::Error>)` - An error if reading or parsing
    ///   fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the contents cannot be parsed, for
    /// example, if the content is empty.
    pub(crate) fn parse(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let instructions = DockerInstruction::parse_str_to_vec(content)?;
        let stages = DockerInstruction::vec_to_stages(&instructions);
        if stages.is_empty() {
            return Err(Box::new(Error::EmptyFile));
        }

        Ok(Self { stages, path: None })
    }

    /// Writes the dockerfile to the disk, with the given path. It ignores the
    /// path set in the data. # Returns
    ///
    /// * `Ok()` - If the file can be successfully written.
    /// * `Err(Box<dyn std::error::Error>)` - An error if writing the file
    ///   fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be written.
    #[allow(unused)]
    pub fn write_to_path(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let content = format!("{self}"); // since display is implemented.
        match fs::write(path, content) {
            Ok(()) => {
                info!("Successfully written new dockerfile to: {path}");
                Ok(())
            }
            Err(e) => {
                error!("Could not write file: {path}, reason: {e}");
                Err(e.into())
            }
        }
    }

    /// Writes the dockerfile to the disk, with the given path. Will use the
    /// path given in the data. # Returns
    ///
    /// * `Ok()` - If the file can be successfully written.
    /// * `Err(Box<dyn std::error::Error>)` - An error if writing the file
    ///   fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be written or if
    /// no path was set.
    pub fn write(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.path.is_some() {
            let content = format!("{self}"); // since display is implemented.
            match fs::write(self.path.clone().expect("Path is set."), content) {
                Ok(()) => {
                    info!("Successfully written new dockerfile to: {}", self.path.clone().expect("Path is set").display());
                    return Ok(());
                }
                Err(e) => {
                    error!("Could not write file: {}, reason: {e}", self.path.clone().expect("Path is set").display());
                    return Err(e.into());
                }
            }
        }
        error!("Could not write dockerfile, since no path is set.");
        Err(Box::new(Error::MissingPath))
    }

    /// Updates the images in a the dockerfile with the given strategy. If the
    /// changes shall not be applied, it will print out a preview.
    pub(crate) fn update_images(&mut self, apply_to_file: bool, strategy: &Strategy, limit: Option<u16>, arch: Option<&String>) {
        dbg!(self.get_base_images_mut());
        for image in self.get_base_images_mut() {
            if image.is_empty() {
                // If this happens, we can not fetch any data. This can be cause by comments
                // above the first FROM instruction, since it is considered an empty stage with
                // an empty image
                continue;
            }
            let mut docker_image_tags = image.get_remote_tags(limit, arch).expect("Tags could be found.");
            docker_image_tags.tags.sort();
            if let Some(found_tag) = find_candidate_tag(image.get_tag(), &docker_image_tags.tags, strategy) {
                debug!("Found tag: {found_tag:?}");
                image.set_tag(&found_tag);
            }
        }

        if apply_to_file && self.get_path().is_some() {
            let _ = self.write();
        } else {
            info!("Resulting dockerfile:\n{}", self);
        }
    }

    /// Generates a list of updates that should be applied to a file, since we
    /// want to preview the changes differently for multi file updates.
    pub(crate) fn generate_image_updates(
        &self, strategy: &Strategy, limit: Option<u16>, arch: Option<&String>, ignore_versions: &[ContainerImage],
    ) -> DockerfileUpdate {
        let mut result = DockerfileUpdate {
            dockerfile: self.clone(),
            updates:    Vec::new(),
        };
        for (index, image) in result.dockerfile.get_base_images_mut().iter().enumerate() {
            let mut docker_image_tags = image.get_remote_tags(limit, arch).expect("Tags could be found.");
            docker_image_tags.tags.sort();
            if let Some(found_tag) = find_candidate_tag(image.get_tag(), &docker_image_tags.tags, strategy) {
                debug!("Found tag: {found_tag:?}");
                if !ignore_versions.contains(image) {
                    result.updates.push((index, found_tag));
                }
            }
        }
        result
    }
}

impl Display for Dockerfile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for stage in self.get_stages() {
            write!(f, "{stage}")?;
        }
        write!(f, "")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockerInstruction {
    Add(String),
    Arg(String),
    Cmd(String),
    Copy(String),
    Entrypoint(String),
    Env(String),
    Expose(String),
    From(Box<ContainerImage>, Option<String>),
    Healthcheck(String),
    Label(String),
    Maintainer(String),
    OnBuild(String),
    Run(String),
    Shell(String),
    StopSignal(String),
    User(String),
    Volume(String),
    Workdir(String),
    /// Not part of the Docker spec – we keep it for comments / blank lines. We
    /// also save the indentation
    Comment(String, usize),
    Empty(),
    #[allow(unused)]
    Unknown(String),
}

impl DockerInstruction {
    const fn is_from_type(&self) -> bool {
        matches!(self, Self::From(_, _))
    }

    fn get_argument(&self) -> String {
        match self {
            Self::Empty() => String::new(),
            Self::From(argument, _) => argument.to_string(),
            Self::Comment(argument, indentation) => {
                format!("{}{}", " ".repeat(*indentation), argument)
            }
            Self::Add(argument)
            | Self::Arg(argument)
            | Self::Cmd(argument)
            | Self::Copy(argument)
            | Self::Entrypoint(argument)
            | Self::Env(argument)
            | Self::Expose(argument)
            | Self::Healthcheck(argument)
            | Self::Label(argument)
            | Self::Maintainer(argument)
            | Self::OnBuild(argument)
            | Self::Run(argument)
            | Self::Shell(argument)
            | Self::StopSignal(argument)
            | Self::User(argument)
            | Self::Volume(argument)
            | Self::Workdir(argument)
            | Self::Unknown(argument) => argument.clone(),
        }
    }

    const fn get_from_name(&self) -> Option<&String> {
        match self {
            Self::From(_, name) => name.as_ref(),
            _ => None,
        }
    }

    /// Will return as much as possible of the valid file
    #[allow(clippy::unnecessary_wraps)]
    fn parse_str_to_vec(content: &str) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        if content.is_empty() {
            return Err(Box::new(Error::EmptyFile));
        }
        let mut collecting_multiline = false; // are we inside a `\`‑continued block?
        let mut buffer = String::new(); // buffer accumulator for the *logical* line

        let mut instructions = Vec::<Self>::new();

        for (line_number, raw_line) in content.lines().enumerate() {
            // keep original indentation
            let line = raw_line.trim_end();

            if line.trim_start().is_empty() {
                match Self::parse_instruction(line.trim_start(), line_number + 1) {
                    Ok(instr) => {
                        debug!("{instr}");
                        instructions.push(instr);
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }

                continue;
            }

            if collecting_multiline {
                // The previous line already ended with a back‑slash, so the
                // *virtual* newline (a real `\n`) was already inserted.
                // We only need to add the current line itself.
                buffer.push_str(line);

                if line.ends_with('\\') {
                    // keeping the backslash adding a newline that represents the escaped
                    // line‑break.
                    buffer.push('\n');

                    // stay in the multiline state – wait for the next line.
                    continue;
                }

                collecting_multiline = false;
                let logical = std::mem::take(&mut buffer);
                match Self::parse_instruction(logical.as_str().trim_start(), line_number + 1) {
                    Ok(instr) => {
                        debug!("{instr}");
                        instructions.push(instr);
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }
                continue;
            }

            if line.trim_start().starts_with('#') {
                match Self::parse_instruction(line /* .trim_start() */, line_number + 1) {
                    Ok(instr) => {
                        debug!("{instr}");
                        instructions.push(instr);
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }
                continue;
            }

            if line.ends_with('\\') {
                collecting_multiline = true;
                buffer.push_str(line);
                buffer.push('\n');
                continue;
            }

            // if its a regular one line instruction we just parse it
            match Self::parse_instruction(line.trim_start(), line_number + 1) {
                Ok(instr) => {
                    debug!("{instr}");
                    instructions.push(instr);
                }
                Err(e) => eprintln!("Error: {e}"),
            }
        }

        let _: () = if collecting_multiline && !buffer.is_empty() {
            // in case we have a trailing new line at the end of the file. Just to be safe.
            match Self::parse_instruction(buffer.trim_start(), content.lines().count()) {
                Ok(instr) => {
                    debug!("{instr}");
                    instructions.push(instr);
                }
                Err(e) => eprintln!("Error: {e}"),
            }
        };
        Ok(instructions)
    }

    /// Turns a vector of instructions into a vector of docker stages
    fn vec_to_stages(vec_instructions: &[Self]) -> Vec<Stage> {
        let mut stages = Vec::<Stage>::new();
        if !vec_instructions.iter().any(Self::is_from_type) {
            // We do not have any stages, if there are no from instructions and we return an
            // empty array.
            return Vec::new();
        }

        let mut current_stage = Stage::default();
        for instruction in vec_instructions {
            if instruction.is_from_type() {
                // if we found a new from instruction, it means we need to push the current
                // stage to the stages and reset the current stage and begin a new one.
                stages.push(current_stage.clone());
                current_stage = Stage::default();
                let image: ContainerImage = instruction.get_argument().parse().expect("From string is valid.");
                if instruction.get_from_name().is_some() {
                    current_stage.update_name(instruction.get_from_name().expect("Name exists"));
                }
                current_stage.set_image(&image);
            }
            // after setting the stage info, we add all instructions, including the from
            // line.
            current_stage.instructions.push(instruction.clone());
        }

        stages.push(current_stage);

        // It can happen that the first stage is all empty, e.g. if its just a comment.
        if stages
            .first()
            .expect("At least one stage exists, since we early return if there are none.")
            .instructions
            .is_empty()
        {
            stages.remove(0);
        }
        stages
    }

    /// Parsing the instruction with a helper, so we can show better errors,
    /// with the line number where it failed.
    pub fn parse_instruction(line: &str, line_no: usize) -> Result<Self, Error> {
        line.parse::<Self>()
            .map_or_else(|_| Err(Error::Parse(ParseError::InvalidInstruction(line.to_owned(), line_no))), Ok)
    }
}

impl Display for DockerInstruction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add(s) => write!(f, "ADD {s}"),
            Self::Arg(s) => write!(f, "ARG {s}"),
            Self::Cmd(s) => write!(f, "CMD {s}"),
            Self::Copy(s) => write!(f, "COPY {s}"),
            Self::Entrypoint(s) => write!(f, "ENTRYPOINT {s}"),
            Self::Env(s) => write!(f, "ENV {s}"),
            Self::Expose(s) => write!(f, "EXPOSE {s}"),
            Self::From(s, name) => {
                if name.is_some() {
                    write!(f, "FROM {s} AS {}", name.clone().expect("Value exists"))
                } else {
                    write!(f, "FROM {s}")
                }
            }
            Self::Healthcheck(s) => write!(f, "HEALTHCHECK {s}"),
            Self::Label(s) => write!(f, "LABEL {s}"),
            Self::Maintainer(s) => write!(f, "MAINTAINER {s}"),
            Self::OnBuild(s) => write!(f, "ONBUILD {s}"),
            Self::Run(s) => write!(f, "RUN {s}"),
            Self::Shell(s) => write!(f, "SHELL {s}"),
            Self::StopSignal(s) => write!(f, "STOPSIGNAL {s}"),
            Self::User(s) => write!(f, "USER {s}"),
            Self::Volume(s) => write!(f, "VOLUME {s}"),
            Self::Workdir(s) => write!(f, "WORKDIR {s}"),
            Self::Comment(s, indent) => write!(f, "{}# {s}", " ".repeat(*indent)),
            Self::Empty() => write!(f, ""),
            Self::Unknown(s) => write!(f, "{s}"),
        }
    }
}

fn parse_instruction(line: &str) -> Result<DockerInstruction, Error> {
    let initial_length = line.len();
    let mut indentation_size = 0;
    let trimmed = line.trim_start();

    if trimmed.is_empty() {
        return Ok(DockerInstruction::Empty());
    }

    if trimmed.starts_with('#') {
        // Preserve the comment text without the leading '#'
        if trimmed.len() != initial_length {
            indentation_size = initial_length.sub(trimmed.len());
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            return Ok(DockerInstruction::Comment(comment.trim_start().to_owned(), indentation_size));
        }
    }

    let (keyword, remainder) = trimmed.find(char::is_whitespace).map_or((trimmed, ""), |idx| {
        let (kw, rem) = trimmed.split_at(idx);
        (kw, rem.trim_start())
    });

    let keyword_uppercase = keyword.to_ascii_uppercase();

    Ok(match keyword_uppercase.as_str() {
        "ADD" => DockerInstruction::Add(remainder.to_owned()),
        "ARG" => DockerInstruction::Arg(remainder.to_owned()),
        "CMD" => DockerInstruction::Cmd(remainder.to_owned()),
        "COPY" => DockerInstruction::Copy(remainder.to_owned()),
        "ENTRYPOINT" => DockerInstruction::Entrypoint(remainder.to_owned()),
        "ENV" => DockerInstruction::Env(remainder.to_owned()),
        "EXPOSE" => DockerInstruction::Expose(remainder.to_owned()),
        "FROM" => {
            let (image, stage_name) = ContainerImage::parse_from_line(trimmed);
            DockerInstruction::From(Box::new(image), stage_name)
        }
        "HEALTHCHECK" => DockerInstruction::Healthcheck(remainder.to_owned()),
        "LABEL" => DockerInstruction::Label(remainder.to_owned()),
        "MAINTAINER" => DockerInstruction::Maintainer(remainder.to_owned()),
        "ONBUILD" => DockerInstruction::OnBuild(remainder.to_owned()),
        "RUN" => DockerInstruction::Run(remainder.to_owned()),
        "SHELL" => DockerInstruction::Shell(remainder.to_owned()),
        "STOPSIGNAL" => DockerInstruction::StopSignal(remainder.to_owned()),
        "USER" => DockerInstruction::User(remainder.to_owned()),
        "VOLUME" => DockerInstruction::Volume(remainder.to_owned()),
        "WORKDIR" => DockerInstruction::Workdir(remainder.to_owned()),
        // Anything else is not a recognised Docker instruction.
        _ => return Err(Error::Parse(ParseError::InvalidInstruction(keyword_uppercase, 0))),
    })
}

impl FromStr for DockerInstruction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_instruction(s)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageMetadata {
    group:  Option<String>,
    name:   String,
    tag:    Tag,
    latest: bool,
}

impl Display for ImageMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.group.is_some() {
            write!(f, "{}/", self.group.clone().expect("Group exists"))?;
        }
        write!(f, "{}:", self.name)?;
        if self.latest {
            write!(f, ":latest")?;
        } else {
            write!(f, ":{}", self.tag)?;
        }
        write!(f, "")
    }
}

impl FromStr for ImageMetadata {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let cleaned_slice = if s.ends_with(':') {
            s.strip_suffix(':').expect("We just checked if the slice ends with a colon")
        } else {
            s
        };
        if cleaned_slice.trim().is_empty() {
            return Err(Error::Parse(ParseError::EmptyImage));
        }
        if let Some((group, name)) = cleaned_slice.split_once('/') {
            if let Some((name, tag)) = name.split_once(':') {
                return Ok(Self {
                    group:  Some(group.to_owned()),
                    name:   name.to_owned(),
                    tag:    tag.parse()?,
                    latest: tag.eq_ignore_ascii_case("latest"),
                });
            }
        } else if let Some((name, tag)) = cleaned_slice.split_once(':') {
            return Ok(Self {
                group:  None,
                name:   name.to_owned(),
                tag:    tag.parse()?,
                latest: tag == "latest",
            });
        }
        //This happens if we reference another image
        Ok(Self {
            group:  None,
            name:   cleaned_slice.to_owned(),
            tag:    Tag {
                major:           None,
                minor:           None,
                patch:           None,
                variant:         None,
                prefix:          None,
                allowed_missing: true,
            },
            latest: false,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerImage {
    Dockerhub(ImageMetadata),
    Mcr(ImageMetadata),
    Gcr(ImageMetadata),
}

impl Default for ContainerImage {
    fn default() -> Self {
        Self::Dockerhub(ImageMetadata::default())
    }
}

#[allow(unused)]
impl ContainerImage {
    const fn get_group(&self) -> Option<&String> {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => metadata.group.as_ref(),
        }
    }

    fn get_group_string(&self) -> String {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => metadata.group.clone().unwrap_or_default(),
        }
    }

    pub const fn get_name(&self) -> &String {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => &metadata.name,
        }
    }

    pub fn get_full_name(&self) -> String {
        match self {
            Self::Dockerhub(metadata) => {
                if metadata.tag.allowed_missing {
                    self.get_name().clone()
                } else if self.get_group().is_some() {
                    format!("{}/{}", self.get_group().expect("Group was set."), self.get_name())
                } else {
                    format!("library/{}", self.get_name())
                }
            }
            Self::Mcr(metadata) | Self::Gcr(metadata) => {
                if self.get_group().is_some() {
                    format!("{}/{}", self.get_group().expect("Group was set"), self.get_name())
                } else {
                    String::from(self.get_name())
                }
            }
        }
    }

    pub fn get_full_tagged_name(&self) -> String {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => {
                format!("{}/{}:{}", self.get_group_string(), self.get_name(), self.get_tag())
            }
        }
    }

    pub fn get_tagged_name(&self) -> String {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => {
                format!("{}:{}", self.get_name(), self.get_tag())
            }
        }
    }

    fn get_full_query_name(&self) -> String {
        match self {
            Self::Dockerhub(metadata) => {
                format!(
                    "{}/{}",
                    if self.get_group().is_some() {
                        self.get_group().expect("Group was set")
                    } else {
                        "library"
                    },
                    self.get_name()
                )
            }
            Self::Mcr(metadata) | Self::Gcr(metadata) => {
                format!("{}/{}", self.get_group().expect("Group was set"), self.get_name())
            }
        }
    }

    pub const fn get_tag(&self) -> &Tag {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => &metadata.tag,
        }
    }

    fn set_tag(&mut self, tag: &Tag) {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => metadata.tag = tag.clone(),
        }
    }

    const fn is_latest(&self) -> bool {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => metadata.latest,
        }
    }

    const fn is_mcr(&self) -> bool {
        match self {
            Self::Dockerhub(_) | Self::Gcr(_) => false,
            Self::Mcr(_) => true,
        }
    }

    const fn is_gcr(&self) -> bool {
        match self {
            Self::Dockerhub(_) | Self::Mcr(_) => false,
            Self::Gcr(_) => true,
        }
    }

    const fn is_dockerhub(&self) -> bool {
        match self {
            Self::Dockerhub(_) => true,
            Self::Mcr(_) | Self::Gcr(_) => false,
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Dockerhub(image_metadata) | Self::Mcr(image_metadata) | Self::Gcr(image_metadata) => *image_metadata == ImageMetadata::default(),
        }
    }

    fn get_query_url(&self) -> String {
        match self {
            Self::Dockerhub(_) => {
                let full_name = self.get_full_query_name();
                format!("https://hub.docker.com/v2/repositories/{full_name}/tags?page_size=100")
            }
            Self::Mcr(_) => {
                let full_name = self.get_full_query_name();
                format!("https://mcr.microsoft.com/api/v1/catalog/{full_name}/tags?reg=mar")
            }
            Self::Gcr(_) => {
                let name = self.get_name();
                let group = self.get_group().expect("Group was set");
                format!("https://artifactregistry.clients6.google.com/v1/projects/{group}/locations/us/repositories/gcr.io/packages/{name}/versions")
            }
        }
    }

    /// Handles the data fetching for dockerhub, since dockerhub only returns a
    /// limited amount of versions, but will return the next query link.
    fn request_dockerhub(&self, limit: Option<u16>) -> Result<DockerHubResponse, Box<dyn std::error::Error>> {
        // build agent with global timeout
        let config = Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build();
        let agent: Agent = config.into();

        let mut request_url = Some(self.get_query_url());
        let mut parsed_response = DockerHubResponse::default();

        while let Some(ref inner_url) = request_url {
            let mut response = match agent.get(inner_url).call() {
                Ok(resp) => {
                    debug!("Received response: {:?}", resp);
                    resp
                }
                Err(e) => {
                    error!("Failed to send request to DockerHub: {e}");
                    return Err(Box::new(Error::ImageNotFound(self.get_full_query_name())));
                }
            };

            let json: DockerHubResponse = match response.body_mut().read_json() {
                Ok(json) => {
                    debug!("Parsed JSON response successfully.");
                    json
                }
                Err(e) => {
                    error!("Failed to parse JSON response: {e}. Exiting tag retrieval.");
                    if parsed_response.results.is_empty() {
                        // If the error happens on the first iteration
                        return Err(Box::new(Error::Parse(ParseError::InvalidDockerhubResponse)));
                    }
                    break;
                }
            };

            request_url.clone_from(&json.next);
            let mut results = json.results.clone();
            if results.is_empty() {
                info!("Fetching tags done!");
                break;
            }

            parsed_response.results.append(&mut results);
            debug!("Parsed results length: {}", parsed_response.results.len());

            let limit = limit.unwrap_or_else(|| u16::try_from(TAG_RESULT_LIMIT).expect("Tag result limit is <= 65535"));
            info!("Fetched {}/{}.", parsed_response.results.len(), limit);

            if parsed_response.results.len() >= usize::from(limit) {
                info!("Fetching tags done!");
                break;
            }
        }
        {
            let names: Vec<&String> = parsed_response.results.iter().map(|r| &r.name).collect();
            debug!("Found raw tags: {names:?}");
        }

        Ok(parsed_response)
    }

    fn request_mcr(&self) -> Result<Vec<McrResponseEntry>, Box<dyn std::error::Error>> {
        // build agent with global timeout
        let config = Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build();
        let agent: Agent = config.into();

        let url = self.get_query_url();
        let mut response = match agent.get(&url).call() {
            Ok(resp) => {
                debug!("Received response: {:?}", resp);
                resp
            }
            Err(e) => {
                error!("Failed to send request to DockerHub: {e}");
                return Err(Box::new(Error::ImageNotFound(self.get_full_query_name())));
            }
        };

        match response.body_mut().read_json::<Vec<McrResponseEntry>>() {
            Ok(json) => Ok(json),
            Err(e) => {
                error!("Failed to parse JSON response: {e}");
                Err(Box::new(Error::ImageNotFound(self.get_full_query_name())))
            }
        }
    }

    pub fn get_remote_tags(&self, limit: Option<u16>, arch: Option<&String>) -> Result<VersionTags, Box<dyn std::error::Error>> {
        if self.get_tag().clone().allowed_missing {
            // This happens if we reference a previous stage, so we just return
            return Ok(VersionTags { tags: vec![] });
        }
        let full_name = &self.get_full_name();
        let mut tags = Vec::<Tag>::new();
        if full_name == "library/" {
            dbg!(&self);
        }
        if full_name.is_empty() || full_name == "/" {
            return Ok(VersionTags { tags });
        }
        let mut cache_file_name = full_name.replace('/', "-");
        cache_file_name.push_str(".json");
        extract_cache_from_file(full_name, &mut tags, &cache_file_name)?;

        debug!("Searching for all tags for image: {full_name}");
        let cache = TAGS_CACHE.read().expect("Tags cache can be read.");
        if cache.contains_key(full_name) {
            debug!("Found tags in application cache.");
            tags.clone_from(cache.get(full_name).expect("Version exists in cache."));
            Ok(VersionTags { tags })
        } else {
            drop(cache); // explicit drop, since the cache would still be locked for reading otherwise.

            let registry_response: RegistryResponse = match &self {
                Self::Dockerhub(image_metadata) => registries::RegistryResponse::DockerHub(self.request_dockerhub(limit)?),
                Self::Mcr(image_metadata) => registries::RegistryResponse::MicrosoftContainerRegistry(self.request_mcr()?),
                // TODO: GCR image fetching and result parsing
                Self::Gcr(image_metadata) => todo!(),
            };

            let tags = arch.map_or_else(|| registry_response.get_tags(), |arch| registry_response.get_tags_for_arch(arch));

            // Inserting found tags into cache
            let mut cache = TAGS_CACHE.write().expect("Cache can be written.");
            if cache.insert(full_name.clone(), tags.clone()).is_none() {
                debug!(
                    "Inserted tags into cache successfully. Cache contains {} tags for {full_name}",
                    cache.get(full_name).expect("Version exists in cache.").len()
                );
            }
            drop(cache); // drop since we no longer need to keep the lock after the insertion
            {
                let tags_content = serde_json::to_string_pretty(&tags);
                let _ = fs::write(cache_file_name, tags_content.expect("Tags can be turned into json string."));
            }
            Ok(VersionTags { tags })
        }
    }

    pub fn parse_from_line(line: &str) -> (Self, Option<String>) {
        let trimmed = line.trim_start().replace("  ", " ");
        let without_from = trimmed.strip_prefix("FROM").or_else(|| trimmed.strip_prefix("from")).unwrap_or(&trimmed).trim();

        without_from.to_ascii_lowercase().find(" as").map_or_else(
            || (without_from.trim().parse().expect("Could parse string."), None),
            |i| {
                let (image, alias) = without_from.split_at(i);
                let alias = alias[3..].trim(); // skip " a "
                (image.trim().parse().expect("Could parse string."), Some(alias.to_owned()))
            },
        )
    }
}

impl FromStr for ContainerImage {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(if s.to_ascii_lowercase().starts_with("mcr.microsoft.com/") {
            Self::Mcr(s.strip_prefix("mcr.microsoft.com/").expect("Prefix exists.").parse()?)
        } else if s.to_ascii_lowercase().starts_with("gcr.io/") {
            Self::Gcr(s.strip_prefix("gcr.io/").expect("Prefix exists.").parse()?)
        } else {
            Self::Dockerhub(s.parse()?)
        })
    }
}

impl Display for ContainerImage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dockerhub(metadata) | Self::Mcr(metadata) | Self::Gcr(metadata) => {
                if self.is_gcr() {
                    write!(f, "gcr.io/")?;
                }
                if self.is_mcr() {
                    write!(f, "mcr.microsoft.com/")?;
                }
                if metadata.group.is_some() {
                    write!(f, "{}/{}", metadata.group.clone().expect("Group was set"), metadata.name)?;
                } else {
                    write!(f, "{}", metadata.name)?;
                }
                if metadata.latest {
                    write!(f, ":latest")?;
                } else if metadata.tag.allowed_missing {
                    write!(f, "{}", metadata.tag)?;
                } else {
                    write!(f, ":{}", metadata.tag)?;
                }
                write!(f, "")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::fs::{File, remove_file};
    use std::io::Write;

    use pretty_assertions::assert_eq;
    use rand::Rng;

    use crate::docker_file::{
        ContainerImage, DockerInstruction, Dockerfile, Error, {self},
    };
    use crate::version::Tag;

    const CONTENT: &str = r#"# Comment 1
# Comment 2
# Comment 3
# comment 3.1
FROM alpine:3.0 AS base
FROM base AS something
COPY /app /app
ADD src dest
CMD ["/command"]
ENTRYPOINT ["/entrypoint.sh"]
HEALTHCHECK /bin/true
LABEL multi.label1="value1" \
      multi.label2="value2" \
      other="value3"
MAINTAINER info@example.com
WORKDIR /tmp

FROM node:8.0-alpine AS build
RUN apk install \
        python \
        make \
        g++
        # comment in the middle
COPY --from=base /app /app
RUN npm install

FROM node:12.0-alpine AS release
COPY /app /app

FROM python:3.12.3-alpine

FROM nginx:1.26.1-alpine3.19

FROM guacamole/guacamole:1.3.0

# comment 4
FROM mcr.microsoft.com/dotnet/aspnet:9.0.0 AS Final
# comment 5
ARG ARG1=ARG1
ENV ENV1=ENV1 \
    ENV2=ENV2
USER ${USERNAME}:${GROUPNAME}
EXPOSE 1337
SHELL /bin/bash
VOLUME /data
ONBUILD echo "hello world"
STOPSIGNAL SIGTERM
"#;

    // rand will be a dev dependency
    fn random_string(length: usize) -> String {
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::rng();

        (0..length)
            .map(|_| {
                let idx = rng.random_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn parse_tests_valid_checks() {
        let dockerfile = Dockerfile::parse(CONTENT);
        assert!(dockerfile.is_ok());
        let dockerfile = dockerfile.unwrap();
        assert_eq!(dockerfile.get_path(), None);
        // The first stage is just the comments, which is an anonymous stage
        assert_eq!(
            dockerfile.get_stages().first().unwrap().instructions.first().unwrap(),
            &(DockerInstruction::Comment(String::from("Comment 1"), 0))
        );
        assert_eq!(
            dockerfile.get_stages().first().unwrap().instructions.last().unwrap(),
            &(DockerInstruction::Comment(String::from("comment 3.1"), 0))
        );
        // The first stage after the comments actually has data
        assert_eq!(dockerfile.get_stages().get(1).unwrap().image.get_tagged_name(), "alpine:3.0");
        assert_eq!(dockerfile.get_stages().get(3).unwrap().image.get_tagged_name(), "node:8.0-alpine");
        assert_eq!(dockerfile.get_stages().get(3).unwrap().image.get_full_query_name(), "library/node");
        assert_eq!(dockerfile.get_stages().get(3).unwrap().image.get_name(), "node");
        assert_eq!(
            dockerfile.get_stages().get(3).unwrap().instructions.get(2).unwrap(),
            &(DockerInstruction::Comment(String::from("comment in the middle"), 8))
        );
        assert_eq!(dockerfile.get_stages().last().unwrap().image.get_full_tagged_name(), "dotnet/aspnet:9.0.0");
        assert_eq!(
            dockerfile.get_stages().last().unwrap().image.to_string(),
            "mcr.microsoft.com/dotnet/aspnet:9.0.0"
        );
        assert_eq!(dockerfile.get_stages().last().unwrap().image.get_full_query_name(), "dotnet/aspnet");
        assert_eq!(dockerfile.get_stages().last().unwrap().name, Some("Final".to_owned()));
        assert_eq!(
            dockerfile.get_stages().last().unwrap().instructions.get(1).unwrap(),
            &(DockerInstruction::Comment(String::from("comment 5"), 0))
        );
        assert_eq!(
            dockerfile.get_stages().last().unwrap().instructions.get(3).unwrap(),
            &(DockerInstruction::Env(String::from("ENV1=ENV1 \\\n    ENV2=ENV2")))
        );
        assert_eq!(
            dockerfile.get_stages().last().unwrap().instructions.get(4).unwrap(),
            &(DockerInstruction::User(String::from(r"${USERNAME}:${GROUPNAME}")))
        );
        assert_eq!(
            dockerfile.get_stages().last().unwrap().instructions.last().unwrap(),
            &(DockerInstruction::StopSignal(String::from("SIGTERM")))
        );
        assert_eq!(dockerfile.get_stages().len(), 9); // 8 images + 1 comment stage above the first, they count as individual stages
        let mut dockerfile = dockerfile;
        dockerfile.clear_path();
        assert_eq!(dockerfile.get_path(), None);
        let concrete_err = dockerfile
            .write()
            .expect_err("Dockerfile error, due to missing path.")
            .downcast::<docker_file::Error>()
            .expect("Error was not dockerfile::Error");
        assert_eq!(concrete_err, Box::new(docker_file::Error::MissingPath));
        assert_eq!(CONTENT, dockerfile.to_string());

        let dockerfile = Dockerfile::parse(CONTENT).unwrap();
        let stages = dockerfile.get_stages();
        let mut stage = stages.get(1).unwrap().to_owned();
        stage.update_image_tag(&"3.22.1".parse().unwrap());
    }

    #[test]
    fn parse_tests_invalid_checks() {
        let content = r"FROM alpine:3.0 as base
    EXPOSED 8080";
        let dockerfile = Dockerfile::parse(content);
        assert!(dockerfile.is_ok());
        let dockerfile = dockerfile.expect("Parsing successful");
        assert_eq!(dockerfile.get_stages().len(), 1); // first line was parsed, second line was invalid and therefore discarded.

        let content = r"FROM alpine:3.0 AS base
EXPOSED 8080
FROM alpine:3.22 AS
EXPOSE 1337";
        let dockerfile = Dockerfile::parse(content);
        assert!(dockerfile.is_ok());
        let dockerfile = dockerfile.expect("Parsing successful");
        assert_eq!(dockerfile.get_stages().len(), 2); // first line was parsed, second line was invalid and therefore discarded.
    }

    #[test]
    fn file_handling() {
        #[cfg(target_os = "linux")]
        let filename = format!("/tmp/{}", random_string(15));
        #[cfg(target_os = "windows")]
        let filename = format!("C:\\Windows\\Temp\\{}", random_string(15));

        let mut file = File::create(&filename).expect("File can be created.");
        assert!(file.write_all(CONTENT.as_bytes()).is_ok());
        let d = Dockerfile::read(&filename).expect("Reading succeeds.");
        let p = d.get_path_str();
        assert_eq!(Some(filename.clone()), p);
        println!("{d}");
        assert!(remove_file(&filename).is_ok());

        assert!(d.write_to_path(&filename).is_ok());
        assert!(remove_file(&filename).is_ok());

        // empty filec, only a comment.

        let mut file = File::create(&filename).expect("File can be created.");
        assert!(file.write_all(b"#just a comment").is_ok());
        let e = Dockerfile::read(&filename)
            .expect_err("No stages found, just a comment.")
            .downcast::<docker_file::Error>()
            .expect("Error was not dockerfile::Error");
        assert_eq!(e, Box::new(Error::EmptyFile));
        // empty file

        let mut file = File::create(&filename).expect("File can be created.");
        assert!(file.write_all(b"").is_ok());
        let e = Dockerfile::read(&filename)
            .expect_err("No stages found, just a comment.")
            .downcast::<docker_file::Error>()
            .expect("Error was not dockerfile::Error");
        assert_eq!(e, Box::new(Error::EmptyFile));
    }

    #[test]
    fn parse_registry_image_dockerhub() {
        // parsing library dockerhub image
        let image = "node:8.0.0-alpine3.10";
        let registry_image: ContainerImage = image.parse().unwrap();
        assert!(!registry_image.is_latest());
        assert!(registry_image.is_dockerhub());
        assert!(registry_image.get_group().is_none());
        assert_eq!(registry_image.get_tag(), "8.0.0-alpine3.10".parse::<Tag>().unwrap().as_ref());
        assert_eq!(registry_image.get_name(), "node");
        let tags = registry_image.get_remote_tags(None, None);
        assert!(tags.is_ok());
        assert!(!tags.unwrap().tags.is_empty());

        let image = "node:8.0-alpine";
        let registry_image: ContainerImage = image.parse().unwrap();
        assert!(!registry_image.is_latest());
        assert!(registry_image.is_dockerhub());
        assert!(registry_image.get_group().is_none());
        assert_eq!(registry_image.get_tag(), "8.0-alpine".parse::<Tag>().unwrap().as_ref());
        assert_eq!(registry_image.get_name(), "node");

        // parsing non-library dockerhub image
        let image = "guacamole/guacamole:latest";
        let registry_image: ContainerImage = image.parse().unwrap();
        assert!(registry_image.is_latest());
        assert!(registry_image.is_dockerhub());
        assert_eq!(registry_image.get_group(), Some(&String::from("guacamole")));
        assert_eq!(registry_image.get_name(), "guacamole");
        assert_eq!(image, &registry_image.to_string());
        let tags = registry_image.get_remote_tags(None, Some(&String::from("amd64")));
        assert!(tags.is_ok());
        assert!(!tags.unwrap().tags.is_empty());
    }

    #[test]
    fn parse_registry_image_mcr() {
        let image = "mcr.microsoft.com/dotnet/aspnet:9.0.0";
        let registry_image: ContainerImage = image.parse().unwrap();
        assert!(!registry_image.is_latest());
        assert!(registry_image.is_mcr());
        assert!(registry_image.get_group().is_some());
        assert_eq!(registry_image.get_group(), Some(&String::from("dotnet")));
        assert_eq!(registry_image.get_tag(), "9.0.0".parse::<Tag>().unwrap().as_ref());
        assert_eq!(registry_image.get_name(), "aspnet");
        assert_eq!(image, &registry_image.to_string());
        let tags = registry_image.get_remote_tags(None, None);
        assert!(tags.is_ok());
        assert!(!tags.unwrap().tags.is_empty());
    }
}
