use std::fs;
use std::path::{Path, PathBuf};

use crate::detect::{
    ArgFiles, ArgFilesLayout, FABRIC_LAUNCHER_PROPERTIES, RUN_SCRIPT, Snapshot, USER_JVM_ARGS,
};
use crate::error::{Error, io_error};
use crate::jar::{JarFacts, property, read_capped, read_jar_facts};
use crate::nofollow::{is_plain_relative, is_real_dir, open_regular};

const TEXT_LIMIT: u64 = 1024 * 1024;
const DEFAULT_FABRIC_GAME_JAR: &str = "server.jar";

pub(crate) fn scan(dir: &Path) -> Result<Snapshot, Error> {
    Ok(Snapshot {
        dir: dir.to_path_buf(),
        jars: root_jars(dir)?,
        arg_files: arg_files(dir)?,
        user_jvm_args: open_regular(dir, Path::new(USER_JVM_ARGS))?.is_some(),
        run_script: read_text(dir, Path::new(RUN_SCRIPT))?,
        fabric_game_jar: fabric_game_jar(dir)?,
    })
}

fn root_jars(dir: &Path) -> Result<Vec<JarFacts>, Error> {
    let mut jars = Vec::new();
    for name in child_names(dir)? {
        if name.to_ascii_lowercase().ends_with(".jar")
            && let Some(facts) = read_jar(dir, Path::new(&name))?
        {
            jars.push(facts);
        }
    }
    Ok(jars)
}

fn arg_files(dir: &Path) -> Result<Vec<ArgFiles>, Error> {
    let mut found = Vec::new();
    for layout in ArgFilesLayout::ALL {
        let libraries = Path::new(layout.libraries());
        if !is_real_dir(dir, libraries)? {
            continue;
        }
        for folder in child_names(&dir.join(libraries))? {
            let unix_args = libraries.join(&folder).join("unix_args.txt");
            if let Some(unix_args) = read_text(dir, &unix_args)? {
                found.push(ArgFiles {
                    layout,
                    folder,
                    unix_args,
                });
            }
        }
    }
    Ok(found)
}

fn fabric_game_jar(dir: &Path) -> Result<Option<JarFacts>, Error> {
    let configured = read_text(dir, Path::new(FABRIC_LAUNCHER_PROPERTIES))?
        .and_then(|text| property(&text, "serverJar"));
    let game_jar = PathBuf::from(configured.as_deref().unwrap_or(DEFAULT_FABRIC_GAME_JAR));
    if !is_plain_relative(&game_jar) {
        return Ok(None);
    }
    read_jar(dir, &game_jar)
}

/// Names of entries in `dir`, sorted so detection does not depend on directory order.
fn child_names(dir: &Path) -> Result<Vec<String>, Error> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error(dir))? {
        let entry = entry.map_err(io_error(dir))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn read_jar(dir: &Path, rel: &Path) -> Result<Option<JarFacts>, Error> {
    let Some(file) = open_regular(dir, rel)? else {
        return Ok(None);
    };
    let facts = read_jar_facts(file, &dir.join(rel))?;
    Ok(facts.map(|facts| JarFacts {
        path: rel.to_path_buf(),
        ..facts
    }))
}

fn read_text(dir: &Path, rel: &Path) -> Result<Option<String>, Error> {
    let Some(file) = open_regular(dir, rel)? else {
        return Ok(None);
    };
    read_capped(file, TEXT_LIMIT).map_err(io_error(dir.join(rel)))
}
