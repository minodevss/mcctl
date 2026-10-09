use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::detect::{Detected, Entry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub java: PathBuf,
    pub memory_mib: u32,
    pub port: u16,
}

/// Full argv for the server, `argv[0]` being the java binary; run it from the server folder.
pub fn command_line(detected: &Detected, launch: &Launch) -> Vec<OsString> {
    let mut argv = vec![launch.java.clone().into_os_string()];
    match &detected.entry {
        Entry::Jar(jar) => {
            argv.extend(memory_flags(launch.memory_mib));
            argv.push("-jar".into());
            argv.push(jar.clone().into_os_string());
        }
        Entry::ArgFiles { user, unix } => {
            // After the user's file so our heap flags win; before unix_args.txt because it names the main class.
            argv.extend(user.as_deref().map(arg_file));
            argv.extend(memory_flags(launch.memory_mib));
            argv.push(arg_file(unix));
        }
    }
    argv.extend([
        "--nogui".into(),
        "--port".into(),
        launch.port.to_string().into(),
    ]);
    argv
}

fn memory_flags(memory_mib: u32) -> [OsString; 2] {
    [
        format!("-Xms{memory_mib}M").into(),
        format!("-Xmx{memory_mib}M").into(),
    ]
}

fn arg_file(path: &Path) -> OsString {
    let mut arg = OsString::from("@");
    arg.push(path);
    arg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Platform;

    fn launch() -> Launch {
        Launch {
            java: PathBuf::from("/var/lib/mcctl/java/21/current/bin/java"),
            memory_mib: 4096,
            port: 25601,
        }
    }

    fn strings(argv: &[OsString]) -> Vec<&str> {
        argv.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn launch_uses_port_flag() {
        let detected = Detected {
            platform: Platform::Paper,
            minecraft: "1.21.1".to_owned(),
            java: 21,
            entry: Entry::Jar(PathBuf::from("paper.jar")),
        };
        assert_eq!(
            strings(&command_line(&detected, &launch())),
            [
                "/var/lib/mcctl/java/21/current/bin/java",
                "-Xms4096M",
                "-Xmx4096M",
                "-jar",
                "paper.jar",
                "--nogui",
                "--port",
                "25601",
            ]
        );
    }

    #[test]
    fn xmx_follows_user_jvm_args() {
        let detected = Detected {
            platform: Platform::NeoForge,
            minecraft: "1.21.1".to_owned(),
            java: 21,
            entry: Entry::ArgFiles {
                user: Some(PathBuf::from("user_jvm_args.txt")),
                unix: PathBuf::from("libraries/net/neoforged/neoforge/21.1.256/unix_args.txt"),
            },
        };
        assert_eq!(
            strings(&command_line(&detected, &launch())),
            [
                "/var/lib/mcctl/java/21/current/bin/java",
                "@user_jvm_args.txt",
                "-Xms4096M",
                "-Xmx4096M",
                "@libraries/net/neoforged/neoforge/21.1.256/unix_args.txt",
                "--nogui",
                "--port",
                "25601",
            ]
        );
    }

    #[test]
    fn arg_files_without_user_file_skip_it() {
        let detected = Detected {
            platform: Platform::Forge,
            minecraft: "26.3".to_owned(),
            java: 25,
            entry: Entry::ArgFiles {
                user: None,
                unix: PathBuf::from("libraries/net/minecraftforge/forge/26.3-66.0.9/unix_args.txt"),
            },
        };
        let argv = command_line(&detected, &launch());
        assert_eq!(strings(&argv)[1], "-Xms4096M");
    }
}
