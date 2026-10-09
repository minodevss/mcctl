use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::{Error, io_error};

const ENTRY_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JarFacts {
    pub(crate) path: PathBuf,
    pub(crate) main_class: Option<String>,
    pub(crate) version_json: Option<String>,
    pub(crate) install_properties: Option<String>,
    pub(crate) patch_properties: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VersionJson {
    id: String,
    java_version: Option<u8>,
}

impl JarFacts {
    pub(crate) fn file_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    pub(crate) fn version_id(&self) -> Option<String> {
        self.parsed_version_json().map(|json| json.id)
    }

    pub(crate) fn java_version(&self) -> Option<u8> {
        self.parsed_version_json()
            .and_then(|json| json.java_version)
    }

    fn parsed_version_json(&self) -> Option<VersionJson> {
        serde_json::from_str(self.version_json.as_deref()?).ok()
    }
}

/// Reads the few small entries detection needs; a file that is not a zip gives `None`.
pub(crate) fn read_jar_facts(file: File, path: &Path) -> Result<Option<JarFacts>, Error> {
    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(ZipError::Io(source)) => return Err(io_error(path)(source)),
        Err(_) => return Ok(None),
    };
    let manifest = read_entry(&mut archive, "META-INF/MANIFEST.MF", path)?;
    Ok(Some(JarFacts {
        path: path.to_path_buf(),
        main_class: manifest.as_deref().and_then(main_class),
        version_json: read_entry(&mut archive, "version.json", path)?,
        install_properties: read_entry(&mut archive, "install.properties", path)?,
        patch_properties: read_entry(&mut archive, "patch.properties", path)?,
    }))
}

fn read_entry(
    archive: &mut ZipArchive<File>,
    name: &str,
    path: &Path,
) -> Result<Option<String>, Error> {
    let entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(ZipError::Io(source)) => return Err(io_error(path)(source)),
        Err(_) => return Ok(None),
    };
    read_capped(entry, ENTRY_LIMIT).map_err(io_error(path))
}

/// Reads at most `limit` bytes as lossy UTF-8; anything longer is not one of our small files.
pub(crate) fn read_capped(reader: impl Read, limit: u64) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn main_class(manifest: &str) -> Option<String> {
    let mut main_section = Vec::<String>::new();
    for line in manifest.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        match (line.strip_prefix(' '), main_section.last_mut()) {
            (Some(continuation), Some(previous)) => previous.push_str(continuation),
            _ => main_section.push(line.to_owned()),
        }
    }
    main_section.into_iter().find_map(|header| {
        let (name, value) = header.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("Main-Class")
            .then(|| value.trim().to_owned())
    })
}

/// Looks up `key` in Java `.properties` text, honouring `=`, `:` and backslash escapes.
pub(crate) fn property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim_start();
        if line.starts_with(['#', '!']) {
            return None;
        }
        let (name, value) = split_property(line);
        (unescape(name) == key).then(|| unescape(value.trim_start()).trim_end().to_owned())
    })
}

fn split_property(line: &str) -> (&str, &str) {
    let mut escaped = false;
    for (index, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '=' | ':' => return (&line[..index], &line[index + 1..]),
            c if c.is_whitespace() => {
                let rest = line[index..].trim_start();
                let rest = rest.strip_prefix(['=', ':']).map_or(rest, str::trim_start);
                return (&line[..index], rest);
            }
            _ => {}
        }
    }
    (line, "")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_main_class_from_manifest() {
        let cases = [
            (
                "Manifest-Version: 1.0\nMain-Class: net.minecraft.bundler.Main\n",
                Some("net.minecraft.bundler.Main"),
            ),
            (
                "Manifest-Version: 1.0\r\nMain-Class: io.papermc.paperclip.Main\r\n\r\n",
                Some("io.papermc.paperclip.Main"),
            ),
            ("main-class:  a.B  \n", Some("a.B")),
            (
                "Main-Class: net.minecraftforge.bootstrap.sh\n im.Main\n",
                Some("net.minecraftforge.bootstrap.shim.Main"),
            ),
            (
                "Manifest-Version: 1.0\n\nName: x\nMain-Class: not.Main\n",
                None,
            ),
            ("Class-Path: libraries/a.jar\n libraries/b.jar\n", None),
        ];
        for (manifest, expected) in cases {
            assert_eq!(main_class(manifest).as_deref(), expected, "{manifest:?}");
        }
    }

    #[test]
    fn reads_java_properties() {
        let text = "#comment\nfabric-loader-version=0.19.5\ngame-version = 1.21.1\nsourceUrl=https\\://example.com/a\nserverJar: server.jar\nspaced value\n";
        let cases = [
            ("game-version", Some("1.21.1")),
            ("fabric-loader-version", Some("0.19.5")),
            ("sourceUrl", Some("https://example.com/a")),
            ("serverJar", Some("server.jar")),
            ("spaced", Some("value")),
            ("version", None),
        ];
        for (key, expected) in cases {
            assert_eq!(property(text, key).as_deref(), expected, "{key}");
        }
    }
}
