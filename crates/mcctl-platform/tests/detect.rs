use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use mcctl_platform::{Detected, Entry, Error, Platform, detect};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

struct ServerDir(TempDir);

#[expect(
    clippy::unwrap_used,
    reason = "fixture setup failing should fail the test"
)]
impl ServerDir {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn jar(&self, name: &str, main_class: &str, extra: &[(&str, &str)]) -> &Self {
        let manifest = format!("Manifest-Version: 1.0\r\nMain-Class: {main_class}\r\n\r\n");
        let mut entries = vec![("META-INF/MANIFEST.MF", manifest.as_str())];
        entries.extend_from_slice(extra);
        write_jar(&self.path().join(name), &entries);
        self
    }

    fn file(&self, rel: &str, contents: &str) -> &Self {
        let path = self.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn detect(&self) -> Result<Detected, Error> {
        detect(self.path())
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "fixture setup failing should fail the test"
)]
fn write_jar(path: &Path, entries: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, contents) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn jar_entry(path: &str) -> Entry {
    Entry::Jar(PathBuf::from(path))
}

#[test]
fn vanilla_version_from_bundler_json() {
    let server = ServerDir::new();
    server.jar(
        "server.jar",
        "net.minecraft.bundler.Main",
        &[(
            "version.json",
            r#"{"id": "26.3", "java_version": 25, "stable": true}"#,
        )],
    );
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::Vanilla,
            minecraft: "26.3".to_owned(),
            java: 25,
            entry: jar_entry("server.jar"),
        }
    );
}

#[test]
fn old_vanilla_version_from_file_name() {
    let server = ServerDir::new();
    server.jar(
        "minecraft_server.1.12.2.jar",
        "net.minecraft.server.MinecraftServer",
        &[],
    );
    let detected = server.detect().unwrap();
    assert_eq!((detected.minecraft.as_str(), detected.java), ("1.12.2", 8));
}

#[test]
fn fabric_version_from_install_properties() {
    let server = ServerDir::new();
    server
        .jar(
            "fabric-server-launch.jar",
            "net.fabricmc.installer.ServerLauncher",
            &[(
                "install.properties",
                "fabric-loader-version=0.19.5\ngame-version=1.21.1\n",
            )],
        )
        .file(".fabric/server/1.21.1-server.jar", "");
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::Fabric,
            minecraft: "1.21.1".to_owned(),
            java: 21,
            entry: jar_entry("fabric-server-launch.jar"),
        }
    );
}

#[test]
fn old_fabric_reads_configured_game_jar() {
    let server = ServerDir::new();
    server
        .jar(
            "fabric-server-launch.jar",
            "net.fabricmc.loader.impl.launch.server.FabricServerLauncher",
            &[],
        )
        .file(
            "fabric-server-launcher.properties",
            "#Fabric\nserverJar=vanilla-1.20.1.jar\n",
        )
        .jar(
            "vanilla-1.20.1.jar",
            "net.minecraft.bundler.Main",
            &[("version.json", r#"{"id": "1.20.1", "java_version": 17}"#)],
        );
    let detected = server.detect().unwrap();
    assert_eq!(detected.platform, Platform::Fabric);
    assert_eq!((detected.minecraft.as_str(), detected.java), ("1.20.1", 17));
    assert_eq!(detected.entry, jar_entry("fabric-server-launch.jar"));
}

#[test]
fn paper_version_from_version_json() {
    let server = ServerDir::new();
    server.jar(
        "paper-26.2-132.jar",
        "io.papermc.paperclip.Main",
        &[("version.json", r#"{"id": "26.2", "java_version": 25}"#)],
    );
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::Paper,
            minecraft: "26.2".to_owned(),
            java: 25,
            entry: jar_entry("paper-26.2-132.jar"),
        }
    );
}

#[test]
fn old_paper_falls_back_to_patch_properties() {
    let server = ServerDir::new();
    server.jar(
        "paper.jar",
        "io.papermc.paperclip.Paperclip",
        &[(
            "patch.properties",
            "patch=paperMC.patch\nsourceUrl=https\\://launcher.mojang.com/server.jar\nversion=1.16.5\n",
        )],
    );
    let detected = server.detect().unwrap();
    assert_eq!(detected.platform, Platform::Paper);
    assert_eq!((detected.minecraft.as_str(), detected.java), ("1.16.5", 8));
}

#[test]
fn paper_version_from_file_name_as_last_resort() {
    let server = ServerDir::new();
    server.jar("paper-1.20.4-499.jar", "io.papermc.paperclip.Main", &[]);
    let detected = server.detect().unwrap();
    assert_eq!((detected.minecraft.as_str(), detected.java), ("1.20.4", 17));
}

#[test]
fn neoforge_version_from_fml_args() {
    let server = ServerDir::new();
    server
        .file(
            "libraries/net/neoforged/neoforge/21.1.256/unix_args.txt",
            "-p libraries/a.jar\ncpw.mods.bootstraplauncher.BootstrapLauncher\n--launchTarget forgeserver\n--fml.neoForgeVersion 21.1.256\n--fml.mcVersion 1.21.1\n",
        )
        .file("user_jvm_args.txt", "# -Xmx4G\n");
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::NeoForge,
            minecraft: "1.21.1".to_owned(),
            java: 21,
            entry: Entry::ArgFiles {
                user: Some(PathBuf::from("user_jvm_args.txt")),
                unix: PathBuf::from("libraries/net/neoforged/neoforge/21.1.256/unix_args.txt"),
            },
        }
    );
}

#[test]
fn neoforge_version_from_folder_name() {
    let cases = [
        ("21.1.233", "1.21.1", 21),
        ("21.0.167", "1.21", 21),
        ("20.4.251", "1.20.4", 17),
        ("26.3.0.58-beta", "26.3", 25),
    ];
    for (folder, minecraft, java) in cases {
        let server = ServerDir::new();
        server.file(
            &format!("libraries/net/neoforged/neoforge/{folder}/unix_args.txt"),
            "-p libraries/a.jar\nnet.neoforged.fml.startup.Server\n",
        );
        let detected = server.detect().unwrap();
        assert_eq!(detected.platform, Platform::NeoForge, "{folder}");
        assert_eq!(
            (detected.minecraft.as_str(), detected.java),
            (minecraft, java),
            "{folder}"
        );
    }
}

#[test]
fn neoforge_for_1_20_1_uses_forge_style_folder() {
    let server = ServerDir::new();
    server.file(
        "libraries/net/neoforged/forge/1.20.1-47.1.106/unix_args.txt",
        "cpw.mods.bootstraplauncher.BootstrapLauncher\n",
    );
    let detected = server.detect().unwrap();
    assert_eq!(detected.platform, Platform::NeoForge);
    assert_eq!(detected.minecraft, "1.20.1");
}

#[test]
fn forge_shim_layout_detected() {
    let server = ServerDir::new();
    server
        .file(
            "libraries/net/minecraftforge/forge/26.3-66.0.9/unix_args.txt",
            "-Djava.net.preferIPv6Addresses=system -XX:+UseCompactObjectHeaders -XX:StackShadowPages=32 -jar forge-26.3-66.0.9-shim.jar\n",
        )
        .file("user_jvm_args.txt", "")
        .jar(
            "forge-26.3-66.0.9-shim.jar",
            "net.minecraftforge.bootstrap.shim.Main",
            &[],
        )
        .jar(
            "forge-26.3-66.0.9-installer.jar",
            "net.minecraftforge.installer.SimpleInstaller",
            &[],
        );
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::Forge,
            minecraft: "26.3".to_owned(),
            java: 25,
            entry: Entry::ArgFiles {
                user: Some(PathBuf::from("user_jvm_args.txt")),
                unix: PathBuf::from("libraries/net/minecraftforge/forge/26.3-66.0.9/unix_args.txt"),
            },
        }
    );
}

#[test]
fn forge_legacy_version_from_file_name() {
    let server = ServerDir::new();
    server
        .jar(
            "forge-1.12.2-14.23.5.2860.jar",
            "net.minecraftforge.fml.relauncher.ServerLaunchWrapper",
            &[],
        )
        .jar(
            "minecraft_server.1.12.2.jar",
            "net.minecraft.server.MinecraftServer",
            &[],
        )
        .jar(
            "forge-1.12.2-14.23.5.2860-installer.jar",
            "net.minecraftforge.installer.SimpleInstaller",
            &[],
        );
    assert_eq!(
        server.detect().unwrap(),
        Detected {
            platform: Platform::ForgeLegacy,
            minecraft: "1.12.2".to_owned(),
            java: 8,
            entry: jar_entry("forge-1.12.2-14.23.5.2860.jar"),
        }
    );
}

#[test]
fn two_loader_jars_are_ambiguous() {
    let server = ServerDir::new();
    let version = [("version.json", r#"{"id": "26.2"}"#)];
    server
        .jar("paper-26.2-131.jar", "io.papermc.paperclip.Main", &version)
        .jar("paper-26.2-132.jar", "io.papermc.paperclip.Main", &version);
    match server.detect() {
        Err(Error::Ambiguous { candidates, .. }) => assert_eq!(
            candidates,
            ["paper-26.2-131.jar (paper)", "paper-26.2-132.jar (paper)"]
        ),
        other => panic!("expected ambiguous, got {other:?}"),
    }
}

#[test]
fn empty_folder_is_not_a_server() {
    let server = ServerDir::new();
    assert!(matches!(server.detect(), Err(Error::NotAServer { .. })));
}

#[test]
fn files_that_are_not_zips_are_ignored() {
    let server = ServerDir::new();
    server.file("server.jar", "not a zip");
    assert!(matches!(server.detect(), Err(Error::NotAServer { .. })));
}

#[test]
fn symlinked_jar_is_ignored() {
    let outside = ServerDir::new();
    outside.jar(
        "server.jar",
        "net.minecraft.bundler.Main",
        &[("version.json", r#"{"id": "26.3"}"#)],
    );
    let server = ServerDir::new();
    symlink(
        outside.path().join("server.jar"),
        server.path().join("server.jar"),
    )
    .unwrap();
    assert!(matches!(server.detect(), Err(Error::NotAServer { .. })));
}

#[test]
fn symlinked_libraries_folder_is_ignored() {
    let outside = ServerDir::new();
    outside.file(
        "net/neoforged/neoforge/21.1.256/unix_args.txt",
        "--fml.mcVersion 1.21.1\n",
    );
    let server = ServerDir::new();
    symlink(outside.path(), server.path().join("libraries")).unwrap();
    assert!(matches!(server.detect(), Err(Error::NotAServer { .. })));
}

#[test]
fn symlinked_user_jvm_args_is_left_out() {
    let outside = ServerDir::new();
    outside.file("args.txt", "-Xmx1G\n");
    let server = ServerDir::new();
    server.file(
        "libraries/net/neoforged/neoforge/21.1.256/unix_args.txt",
        "--fml.mcVersion 1.21.1\n",
    );
    symlink(
        outside.path().join("args.txt"),
        server.path().join("user_jvm_args.txt"),
    )
    .unwrap();
    let detected = server.detect().unwrap();
    assert!(matches!(detected.entry, Entry::ArgFiles { user: None, .. }));
}

#[test]
fn jar_java_version_beats_table() {
    let server = ServerDir::new();
    server.jar(
        "server.jar",
        "net.minecraft.server.Main",
        &[("version.json", r#"{"id": "1.20.4", "java_version": 21}"#)],
    );
    let detected = server.detect().unwrap();
    assert_eq!((detected.minecraft.as_str(), detected.java), ("1.20.4", 21));
}
