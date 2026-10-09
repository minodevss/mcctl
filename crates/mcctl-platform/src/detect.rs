use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::jar::{JarFacts, property};
use crate::minecraft::{java_major, neoforge_minecraft, parse_release};
use crate::scan::scan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Vanilla,
    Fabric,
    Paper,
    Forge,
    NeoForge,
    ForgeLegacy,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Paper => "paper",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::ForgeLegacy => "forge-legacy",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub platform: Platform,
    pub minecraft: String,
    pub java: u8,
    pub entry: Entry,
}

/// How to start the server; paths are relative to the server folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Jar(PathBuf),
    ArgFiles {
        user: Option<PathBuf>,
        unix: PathBuf,
    },
}

/// Finds the server software in `dir`. Symlinks inside `dir` are treated as absent.
pub fn detect(dir: &Path) -> Result<Detected, Error> {
    classify(&scan(dir)?)
}

pub(crate) const USER_JVM_ARGS: &str = "user_jvm_args.txt";
pub(crate) const RUN_SCRIPT: &str = "run.sh";
pub(crate) const FABRIC_LAUNCHER_PROPERTIES: &str = "fabric-server-launcher.properties";

const FABRIC_INSTALLER_LAUNCHER: &str = "net.fabricmc.installer.ServerLauncher";
const FABRIC_LOADER_LAUNCHERS: [&str; 2] = [
    "net.fabricmc.loader.launch.server.FabricServerLauncher",
    "net.fabricmc.loader.impl.launch.server.FabricServerLauncher",
];
const PAPERCLIP_PREFIX: &str = "io.papermc.paperclip.";
const FORGE_PREFIX: &str = "net.minecraftforge.";
const FORGE_INSTALLER_PREFIX: &str = "net.minecraftforge.installer.";
const VANILLA_MAIN_CLASSES: [&str; 3] = [
    "net.minecraft.bundler.Main",
    "net.minecraft.server.Main",
    "net.minecraft.server.MinecraftServer",
];

/// Everything detection reads from a server folder, gathered up front so classification stays pure.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) dir: PathBuf,
    pub(crate) jars: Vec<JarFacts>,
    pub(crate) arg_files: Vec<ArgFiles>,
    pub(crate) user_jvm_args: bool,
    pub(crate) run_script: Option<String>,
    pub(crate) fabric_game_jar: Option<JarFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArgFiles {
    pub(crate) layout: ArgFilesLayout,
    pub(crate) folder: String,
    pub(crate) unix_args: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgFilesLayout {
    NeoForge,
    NeoForgeLegacy,
    Forge,
}

impl ArgFilesLayout {
    pub(crate) const ALL: [Self; 3] = [Self::NeoForge, Self::NeoForgeLegacy, Self::Forge];

    pub(crate) fn libraries(self) -> &'static str {
        match self {
            Self::NeoForge => "libraries/net/neoforged/neoforge",
            Self::NeoForgeLegacy => "libraries/net/neoforged/forge",
            Self::Forge => "libraries/net/minecraftforge/forge",
        }
    }

    fn platform(self) -> Platform {
        match self {
            Self::NeoForge | Self::NeoForgeLegacy => Platform::NeoForge,
            Self::Forge => Platform::Forge,
        }
    }

    fn minecraft_from_folder(self, folder: &str) -> Option<String> {
        match self {
            Self::NeoForge => neoforge_minecraft(folder),
            Self::NeoForgeLegacy | Self::Forge => {
                let (minecraft, _) = folder.split_once('-')?;
                parse_release(minecraft).map(|_| minecraft.to_owned())
            }
        }
    }
}

impl ArgFiles {
    pub(crate) fn unix_args_path(&self) -> PathBuf {
        Path::new(self.layout.libraries())
            .join(&self.folder)
            .join("unix_args.txt")
    }
}

struct Candidate {
    platform: Platform,
    origin: PathBuf,
    minecraft: Option<String>,
    java_from_jar: Option<u8>,
    entry: Entry,
}

pub(crate) fn classify(snapshot: &Snapshot) -> Result<Detected, Error> {
    let levels: [fn(&Snapshot) -> Vec<Candidate>; 5] = [
        arg_file_servers,
        fabric_jars,
        paper_jars,
        forge_legacy_jars,
        vanilla_jars,
    ];
    for level in levels {
        let mut candidates = level(snapshot);
        match candidates.len() {
            0 => {}
            1 => return into_detected(snapshot, candidates.remove(0)),
            _ => {
                return Err(Error::Ambiguous {
                    dir: snapshot.dir.clone(),
                    candidates: candidates
                        .iter()
                        .map(|candidate| {
                            format!("{} ({})", candidate.origin.display(), candidate.platform)
                        })
                        .collect(),
                });
            }
        }
    }
    Err(Error::NotAServer {
        dir: snapshot.dir.clone(),
    })
}

fn into_detected(snapshot: &Snapshot, candidate: Candidate) -> Result<Detected, Error> {
    let Some(minecraft) = candidate
        .minecraft
        .filter(|minecraft| is_printable_id(minecraft))
    else {
        return Err(Error::UnknownVersion {
            path: snapshot.dir.join(candidate.origin),
        });
    };
    Ok(Detected {
        platform: candidate.platform,
        java: java_major(&minecraft, candidate.java_from_jar),
        minecraft,
        entry: candidate.entry,
    })
}

/// Version text comes from files the server user controls; keep it short and free of control characters.
fn is_printable_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.chars().any(char::is_control)
}

fn arg_file_servers(snapshot: &Snapshot) -> Vec<Candidate> {
    let all: Vec<&ArgFiles> = snapshot.arg_files.iter().collect();
    let chosen = match (all.len(), snapshot.run_script.as_deref()) {
        (2.., Some(script)) => {
            let named: Vec<&ArgFiles> = all
                .iter()
                .copied()
                .filter(|args| named_in_script(script, args))
                .collect();
            if named.len() == 1 { named } else { all }
        }
        _ => all,
    };
    chosen
        .into_iter()
        .map(|args| arg_file_candidate(snapshot, args))
        .collect()
}

fn named_in_script(script: &str, args: &ArgFiles) -> bool {
    let path = args.unix_args_path();
    path.to_str()
        .is_some_and(|path| script.contains(&format!("@{path}")))
}

fn arg_file_candidate(snapshot: &Snapshot, args: &ArgFiles) -> Candidate {
    let unix = args.unix_args_path();
    Candidate {
        platform: args.layout.platform(),
        minecraft: fml_minecraft_version(&args.unix_args)
            .or_else(|| args.layout.minecraft_from_folder(&args.folder)),
        java_from_jar: None,
        entry: Entry::ArgFiles {
            user: snapshot.user_jvm_args.then(|| PathBuf::from(USER_JVM_ARGS)),
            unix: unix.clone(),
        },
        origin: unix,
    }
}

fn fml_minecraft_version(unix_args: &str) -> Option<String> {
    let mut tokens = unix_args.split_whitespace();
    tokens.find(|token| *token == "--fml.mcVersion")?;
    tokens.next().map(str::to_owned)
}

fn jar_candidate(
    jar: &JarFacts,
    platform: Platform,
    minecraft: Option<String>,
    java_from_jar: Option<u8>,
) -> Candidate {
    Candidate {
        platform,
        origin: jar.path.clone(),
        minecraft,
        java_from_jar,
        entry: Entry::Jar(jar.path.clone()),
    }
}

fn fabric_jars(snapshot: &Snapshot) -> Vec<Candidate> {
    snapshot
        .jars
        .iter()
        .filter_map(|jar| {
            let main_class = jar.main_class.as_deref()?;
            if main_class == FABRIC_INSTALLER_LAUNCHER {
                let minecraft = jar
                    .install_properties
                    .as_deref()
                    .and_then(|text| property(text, "game-version"));
                return Some(jar_candidate(jar, Platform::Fabric, minecraft, None));
            }
            if FABRIC_LOADER_LAUNCHERS.contains(&main_class) {
                let game = snapshot.fabric_game_jar.as_ref();
                let minecraft =
                    game.and_then(|game| game.version_id().or_else(|| vanilla_file_version(game)));
                let java = game.and_then(JarFacts::java_version);
                return Some(jar_candidate(jar, Platform::Fabric, minecraft, java));
            }
            None
        })
        .collect()
}

fn paper_jars(snapshot: &Snapshot) -> Vec<Candidate> {
    snapshot
        .jars
        .iter()
        .filter(|jar| has_main_class_prefix(jar, PAPERCLIP_PREFIX))
        .map(|jar| {
            let minecraft = jar
                .version_id()
                .or_else(|| {
                    jar.patch_properties
                        .as_deref()
                        .and_then(|text| property(text, "version"))
                })
                .or_else(|| paper_file_version(jar.file_name()));
            jar_candidate(jar, Platform::Paper, minecraft, jar.java_version())
        })
        .collect()
}

fn forge_legacy_jars(snapshot: &Snapshot) -> Vec<Candidate> {
    snapshot
        .jars
        .iter()
        .filter(|jar| {
            has_main_class_prefix(jar, FORGE_PREFIX)
                && !has_main_class_prefix(jar, FORGE_INSTALLER_PREFIX)
        })
        .map(|jar| {
            let minecraft = forge_file_version(jar.file_name());
            jar_candidate(jar, Platform::ForgeLegacy, minecraft, None)
        })
        .collect()
}

fn vanilla_jars(snapshot: &Snapshot) -> Vec<Candidate> {
    snapshot
        .jars
        .iter()
        .filter(|jar| {
            jar.main_class
                .as_deref()
                .is_some_and(|main_class| VANILLA_MAIN_CLASSES.contains(&main_class))
        })
        .map(|jar| {
            let minecraft = jar.version_id().or_else(|| vanilla_file_version(jar));
            jar_candidate(jar, Platform::Vanilla, minecraft, jar.java_version())
        })
        .collect()
}

fn has_main_class_prefix(jar: &JarFacts, prefix: &str) -> bool {
    jar.main_class
        .as_deref()
        .is_some_and(|main_class| main_class.starts_with(prefix))
}

fn release_or_none(minecraft: &str) -> Option<String> {
    parse_release(minecraft).map(|_| minecraft.to_owned())
}

fn paper_file_version(file_name: &str) -> Option<String> {
    let stem = file_name.strip_prefix("paper-")?.strip_suffix(".jar")?;
    let (minecraft, build) = stem.rsplit_once('-')?;
    if build.is_empty() || !build.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    release_or_none(minecraft)
}

fn forge_file_version(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix("forge-")?;
    let (minecraft, _) = rest.split_once('-')?;
    release_or_none(minecraft)
}

fn vanilla_file_version(jar: &JarFacts) -> Option<String> {
    let minecraft = jar
        .file_name()
        .strip_prefix("minecraft_server.")?
        .strip_suffix(".jar")?;
    release_or_none(minecraft)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar(path: &str, main_class: &str) -> JarFacts {
        JarFacts {
            path: PathBuf::from(path),
            main_class: Some(main_class.to_owned()),
            ..JarFacts::default()
        }
    }

    fn snapshot(jars: Vec<JarFacts>) -> Snapshot {
        Snapshot {
            dir: PathBuf::from("/srv/test"),
            jars,
            ..Snapshot::default()
        }
    }

    fn neoforge_args(folder: &str, unix_args: &str) -> ArgFiles {
        ArgFiles {
            layout: ArgFilesLayout::NeoForge,
            folder: folder.to_owned(),
            unix_args: unix_args.to_owned(),
        }
    }

    #[test]
    fn reads_versions_from_file_names() {
        assert_eq!(
            paper_file_version("paper-1.21.1-133.jar").as_deref(),
            Some("1.21.1")
        );
        assert_eq!(
            paper_file_version("paper-26.2-132.jar").as_deref(),
            Some("26.2")
        );
        assert_eq!(paper_file_version("paper.jar"), None);
        assert_eq!(
            forge_file_version("forge-1.12.2-14.23.5.2860.jar").as_deref(),
            Some("1.12.2")
        );
        assert_eq!(
            forge_file_version("forge-1.7.10-10.13.4.1614-1.7.10-universal.jar").as_deref(),
            Some("1.7.10")
        );
        assert_eq!(forge_file_version("forge.jar"), None);
    }

    #[test]
    fn fml_args_beat_folder_name() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.arg_files = vec![neoforge_args(
            "21.1.256",
            "-p x net.neoforged.fml.startup.Server\n--fml.neoForgeVersion 21.1.256\n--fml.mcVersion 1.21.2\n",
        )];
        assert_eq!(classify(&snapshot).unwrap().minecraft, "1.21.2");
    }

    #[test]
    fn run_script_breaks_tie_between_installed_versions() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.arg_files = vec![
            neoforge_args("21.1.200", "--fml.mcVersion 1.21.1"),
            neoforge_args("21.1.256", "--fml.mcVersion 1.21.1"),
        ];
        snapshot.run_script = Some(
            "java @user_jvm_args.txt @libraries/net/neoforged/neoforge/21.1.256/unix_args.txt \"$@\"\n"
                .to_owned(),
        );
        let detected = classify(&snapshot).unwrap();
        assert_eq!(
            detected.entry,
            Entry::ArgFiles {
                user: None,
                unix: PathBuf::from("libraries/net/neoforged/neoforge/21.1.256/unix_args.txt"),
            }
        );
    }

    #[test]
    fn installer_jars_are_not_servers() {
        let installers = vec![
            jar(
                "forge-1.20.1-47.4.10-installer.jar",
                "net.minecraftforge.installer.SimpleInstaller",
            ),
            jar("fabric-installer-1.1.2.jar", "net.fabricmc.installer.Main"),
        ];
        assert!(matches!(
            classify(&snapshot(installers)),
            Err(Error::NotAServer { .. })
        ));
    }

    #[test]
    fn loader_jar_beats_leftover_vanilla_jar() {
        let mut paper = jar("paper.jar", "io.papermc.paperclip.Main");
        paper.version_json = Some(r#"{"id":"26.2","java_version":25}"#.to_owned());
        let mut vanilla = jar("server.jar", "net.minecraft.bundler.Main");
        vanilla.version_json = Some(r#"{"id":"26.2","java_version":25}"#.to_owned());
        let detected = classify(&snapshot(vec![vanilla, paper])).unwrap();
        assert_eq!(detected.platform, Platform::Paper);
    }

    #[test]
    fn control_characters_in_version_are_rejected() {
        let mut vanilla = jar("server.jar", "net.minecraft.server.Main");
        vanilla.version_json = Some(r#"{"id":"1.21\u001b[2J"}"#.to_owned());
        assert!(matches!(
            classify(&snapshot(vec![vanilla])),
            Err(Error::UnknownVersion { .. })
        ));
    }

    #[test]
    fn missing_version_is_reported() {
        let old = jar("server.jar", "net.minecraft.server.MinecraftServer");
        assert!(matches!(
            classify(&snapshot(vec![old])),
            Err(Error::UnknownVersion { .. })
        ));
    }
}
